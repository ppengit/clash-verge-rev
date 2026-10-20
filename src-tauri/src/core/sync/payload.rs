use super::{
    manifest::{
        CUSTOM_RULES_FILE, MAX_ARCHIVE_ENTRIES, MAX_OVERRIDE_BYTES, MAX_OVERRIDE_FILES, MAX_SUBSCRIPTIONS,
        SETTINGS_FILE, SUBSCRIPTIONS_FILE, SyncManifest, UNCOMPRESSED_MAX_BYTES, sha256_hex, validate_payload_path,
    },
    overrides::OverrideCollection,
    records::{self, SubscriptionRecord},
    rules, settings,
    state::{LocalRecordState, LocalSettingState, SyncState},
};
use crate::{
    config::{IProfiles, IVerge},
    enhance::custom_rules::CustomRule,
};
use anyhow::{Context as _, Result, bail};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{Cursor, Read as _, Write as _},
};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

#[derive(Clone, Copy, Debug)]
pub struct SyncScopes {
    pub subscriptions: bool,
    pub overrides: bool,
    pub custom_rules: bool,
    pub settings: bool,
}

impl SyncScopes {
    pub fn from_names(names: &[std::string::String]) -> Self {
        // Defaults are applied by the caller when the setting is absent.
        Self {
            subscriptions: names.iter().any(|name| name == "subscriptions"),
            overrides: names.iter().any(|name| name == "overrides"),
            custom_rules: names.iter().any(|name| name == "custom_rules"),
            settings: names.iter().any(|name| name == "settings"),
        }
    }

    pub fn names(&self) -> Vec<std::string::String> {
        let mut names = Vec::new();
        if self.subscriptions {
            names.push("subscriptions".into());
        }
        if self.overrides {
            names.push("overrides".into());
        }
        if self.custom_rules {
            names.push("custom_rules".into());
        }
        if self.settings {
            names.push("settings".into());
        }
        names
    }
}

impl Default for SyncScopes {
    fn default() -> Self {
        Self {
            subscriptions: true,
            overrides: true,
            custom_rules: true,
            settings: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PayloadFile {
    pub path: std::string::String,
    pub bytes: Vec<u8>,
    pub modified_at: u64,
}

pub struct BuiltPayload {
    pub files: Vec<PayloadFile>,
    pub scopes: SyncScopes,
    pub zip: Vec<u8>,
    /// SHA-256 over the plaintext archive; compared with `last_local_hash`.
    pub sha256: String,
    pub subscriptions: Vec<SubscriptionRecord>,
    pub record_state: BTreeMap<std::string::String, LocalRecordState>,
    pub record_tombstones: BTreeMap<std::string::String, u64>,
    pub overrides: BTreeMap<std::string::String, super::overrides::OverrideFile>,
    pub override_state: BTreeMap<std::string::String, super::state::LocalFileState>,
    pub override_tombstones: BTreeMap<std::string::String, u64>,
    pub rules: Vec<CustomRule>,
    pub settings_values: BTreeMap<std::string::String, serde_json::Value>,
    pub settings_state: BTreeMap<std::string::String, LocalSettingState>,
    pub deleted: Vec<std::string::String>,
}

impl BuiltPayload {
    pub fn manifest_files(&self) -> Vec<super::manifest::ManifestFile> {
        self.files
            .iter()
            .map(|file| super::manifest::ManifestFile {
                path: file.path.clone(),
                sha256: sha256_hex(&file.bytes),
                size: file.bytes.len() as u64,
                modified_at: file.modified_at,
            })
            .collect()
    }
}

pub struct BuildInput<'a> {
    pub profiles: &'a IProfiles,
    pub verge: &'a IVerge,
    pub state: &'a SyncState,
    pub scopes: SyncScopes,
    pub first_sync: bool,
    pub now: u64,
}

pub async fn build(input: BuildInput<'_>) -> Result<BuiltPayload> {
    let mut files: Vec<PayloadFile> = Vec::new();
    let mut deleted = Vec::new();
    let mut subscriptions = Vec::new();
    let mut record_state = BTreeMap::new();
    let mut record_tombstones = BTreeMap::new();
    let mut overrides = BTreeMap::new();
    let mut override_state = BTreeMap::new();
    let mut override_tombstones = BTreeMap::new();
    let mut rules = Vec::new();
    let mut settings_values = BTreeMap::new();
    let mut settings_state = BTreeMap::new();

    if input.scopes.subscriptions {
        let collected = records::collect_local_records(input.profiles, input.state, input.first_sync, input.now)?;
        if collected.records.len() > MAX_SUBSCRIPTIONS {
            bail!(
                "cannot sync {} subscriptions (limit {MAX_SUBSCRIPTIONS})",
                collected.records.len()
            );
        }
        let document = records::render_document(&collected.records)?;
        files.push(PayloadFile {
            path: SUBSCRIPTIONS_FILE.into(),
            bytes: document,
            modified_at: input.now,
        });
        subscriptions = collected.records;
        record_state = collected.next_state;
        record_tombstones = collected.tombstones;
    }

    if input.scopes.overrides {
        let OverrideCollection {
            files: local_files,
            tombstones,
        } = super::overrides::collect_local_overrides(input.profiles, input.state, input.now).await?;
        for (path, file) in local_files {
            override_state.insert(
                path.clone(),
                super::state::LocalFileState {
                    sha256: file.sha256(),
                    modified_at: file.modified_at,
                },
            );
            files.push(PayloadFile {
                path,
                bytes: file.content.clone(),
                modified_at: file.modified_at,
            });
            overrides.insert(file.path.clone(), file);
        }
        for path in tombstones.keys() {
            deleted.push(path.clone());
        }
        override_tombstones = tombstones;
    }

    if input.scopes.custom_rules {
        let local_rules = rules::load_local_rules().await?;
        let document = rules::render_document(&local_rules)?;
        if document.len() as u64 > crate::enhance::custom_rules::MAX_FILE_BYTES {
            bail!("custom-rules.yaml would exceed the sync size limit");
        }
        files.push(PayloadFile {
            path: CUSTOM_RULES_FILE.into(),
            bytes: document,
            modified_at: input.now,
        });
        rules = local_rules;
    }

    if input.scopes.settings {
        settings_values = settings::capture(input.verge);
        let (document, state) = settings::collect_document(&settings_values, input.state, input.first_sync, input.now);
        files.push(PayloadFile {
            path: SETTINGS_FILE.into(),
            bytes: settings::render_document(&document)?,
            modified_at: input.now,
        });
        settings_state = state;
    }

    files.sort_by(|left, right| left.path.cmp(&right.path));
    deleted.sort();
    deleted.dedup();

    if input.scopes.overrides {
        let override_count = files.iter().filter(|file| file.path.starts_with("overrides/")).count();
        if override_count > MAX_OVERRIDE_FILES {
            bail!("cannot sync {override_count} override files (limit {MAX_OVERRIDE_FILES})");
        }
        for file in files.iter().filter(|file| file.path.starts_with("overrides/")) {
            if file.bytes.len() as u64 > MAX_OVERRIDE_BYTES {
                bail!("override file {} exceeds {MAX_OVERRIDE_BYTES} bytes", file.path);
            }
        }
    }

    let zip = zip_payload(&files)?;
    let sha256 = sha256_hex(&zip);
    Ok(BuiltPayload {
        files,
        scopes: input.scopes,
        zip,
        sha256,
        subscriptions,
        record_state,
        record_tombstones,
        overrides,
        override_state,
        override_tombstones,
        rules,
        settings_values,
        settings_state,
        deleted,
    })
}

fn zip_payload(files: &[PayloadFile]) -> Result<Vec<u8>> {
    if files.len() > MAX_ARCHIVE_ENTRIES {
        bail!("payload has too many entries");
    }
    let mut total = 0_u64;
    for file in files {
        validate_payload_path(&file.path)?;
        if file.bytes.len() as u64 > super::manifest::max_payload_file_size(&file.path) {
            bail!("payload entry is too large: {}", file.path);
        }
        total = total.saturating_add(file.bytes.len() as u64);
    }
    if total > UNCOMPRESSED_MAX_BYTES {
        bail!("payload exceeds {UNCOMPRESSED_MAX_BYTES} uncompressed bytes");
    }

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    // Stored keeps payload hashes deterministic and avoids zip-bomb ambiguity on re-upload.
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for file in files {
        writer.start_file(file.path.as_str(), options)?;
        writer.write_all(&file.bytes)?;
    }
    let cursor = writer.finish()?;
    let bytes = cursor.into_inner();
    if bytes.len() as u64 > super::manifest::PAYLOAD_MAX_BYTES {
        bail!("payload exceeds the {} byte limit", super::manifest::PAYLOAD_MAX_BYTES);
    }
    Ok(bytes)
}

pub struct ExtractedPayload {
    pub files: BTreeMap<std::string::String, PayloadFile>,
    pub subscriptions: Vec<SubscriptionRecord>,
    pub rules: Vec<CustomRule>,
    pub settings: BTreeMap<std::string::String, settings::SettingEntry>,
}

/// Verifies the outer payload hash before any decryption or extraction.
pub fn verify_payload_hash(stored: &[u8], manifest: &SyncManifest) -> Result<()> {
    if sha256_hex(stored) != manifest.payload_sha256 {
        bail!("sync payload hash does not match its manifest");
    }
    Ok(())
}

/// Verifies the archive against the manifest and stages every entry below `staging`.
pub fn extract_and_validate(
    zip_bytes: &[u8],
    manifest: &SyncManifest,
    staging: &std::path::Path,
) -> Result<ExtractedPayload> {
    if zip_bytes.len() as u64 > super::manifest::PAYLOAD_MAX_BYTES {
        bail!("sync payload exceeds the download limit");
    }

    let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).context("sync payload is not a valid zip archive")?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        bail!("sync payload contains too many entries");
    }

    let declared: HashMap<&str, &super::manifest::ManifestFile> =
        manifest.files.iter().map(|file| (file.path.as_str(), file)).collect();
    let mut extracted = BTreeMap::new();
    let mut total = 0_u64;
    let mut seen = HashSet::new();

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        if entry.encrypted() {
            bail!("sync payload uses unsupported per-entry zip encryption");
        }
        let name = entry.name()?.into_owned();
        validate_payload_path(&name)?;
        let Some(manifest_file) = declared.get(name.as_str()) else {
            bail!("sync payload contains an unlisted file: {name}");
        };
        if !seen.insert(name.clone()) {
            bail!("sync payload contains a duplicate entry: {name}");
        }
        if entry.size() != manifest_file.size {
            bail!("sync payload entry size does not match the manifest: {name}");
        }
        if entry.size() > super::manifest::max_payload_file_size(&name) {
            bail!("sync payload entry is too large: {name}");
        }
        if entry.size() > 64 * 1024 && entry.compressed_size() == 0 {
            bail!("sync payload entry has an invalid compressed size: {name}");
        }
        if entry.compressed_size() > 0 && entry.size() / entry.compressed_size().max(1) > 1_000 {
            bail!("sync payload entry exceeds the compression ratio limit: {name}");
        }
        total = total.saturating_add(entry.size());
        if total > UNCOMPRESSED_MAX_BYTES {
            bail!("sync payload exceeds {UNCOMPRESSED_MAX_BYTES} uncompressed bytes");
        }

        let limit = manifest_file.size.saturating_add(1);
        let mut bytes = Vec::with_capacity(usize::try_from(manifest_file.size).unwrap_or(0));
        entry
            .by_ref()
            .take(limit)
            .read_to_end(&mut bytes)
            .with_context(|| format!("failed to read sync payload entry {name}"))?;
        if bytes.len() as u64 != manifest_file.size {
            bail!("sync payload entry was truncated: {name}");
        }
        if sha256_hex(&bytes) != manifest_file.sha256 {
            bail!("sync payload entry hash does not match the manifest: {name}");
        }

        let target = staging.join(&name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create staging directory for {name}"))?;
        }
        std::fs::write(&target, &bytes).with_context(|| format!("failed to stage sync payload entry {name}"))?;
        extracted.insert(
            name.clone(),
            PayloadFile {
                path: name,
                bytes,
                modified_at: manifest_file.modified_at,
            },
        );
    }

    if extracted.len() != manifest.files.len() {
        bail!("sync payload is missing files listed in its manifest");
    }

    let subscriptions = match extracted.get(SUBSCRIPTIONS_FILE) {
        Some(file) => records::parse_document(&file.bytes)?,
        None => Vec::new(),
    };
    let rules = match extracted.get(CUSTOM_RULES_FILE) {
        Some(file) => rules::parse_document(&file.bytes)?,
        None => Vec::new(),
    };
    let settings = match extracted.get(SETTINGS_FILE) {
        Some(file) => settings::parse_document(&file.bytes)?,
        None => BTreeMap::new(),
    };

    for file in extracted.values() {
        validate_override_syntax(file)?;
    }

    Ok(ExtractedPayload {
        files: extracted,
        subscriptions,
        rules,
        settings,
    })
}

fn validate_override_syntax(file: &PayloadFile) -> Result<()> {
    let Some(rest) = file.path.strip_prefix("overrides/") else {
        return Ok(());
    };
    let is_script = rest.ends_with("script.js");
    if is_script {
        let text = std::str::from_utf8(&file.bytes)
            .with_context(|| format!("override script is not valid UTF-8: {}", file.path))?;
        let mut context = boa_engine::Context::default();
        boa_engine::Script::parse(boa_engine::Source::from_bytes(text), None, &mut context)
            .map_err(|error| anyhow::anyhow!("script syntax error in {}: {error}", file.path))?;
    } else {
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_slice(&file.bytes)
            .with_context(|| format!("override file is not valid YAML: {}", file.path))?;
        if !value.is_mapping() && !value.is_null() {
            bail!("override file is not a YAML mapping: {}", file.path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload_file(path: &str, content: &str) -> PayloadFile {
        PayloadFile {
            path: path.into(),
            bytes: content.as_bytes().to_vec(),
            modified_at: 0,
        }
    }

    #[test]
    fn zip_round_trip_is_deterministic_and_stored() {
        let files = vec![
            payload_file(SUBSCRIPTIONS_FILE, "version: 1\nsubscriptions: []\n"),
            payload_file("overrides/global/merge.yaml", "profile: {}\n"),
        ];
        let first = zip_payload(&files).unwrap();
        let second = zip_payload(&files).unwrap();
        assert_eq!(first, second, "the same payload must produce the same bytes");

        let mut archive = ZipArchive::new(Cursor::new(first)).unwrap();
        let entry = archive.by_index(0).unwrap();
        assert_eq!(entry.compression(), zip::CompressionMethod::Stored);
    }

    #[test]
    fn extraction_rejects_traversal_even_with_a_matching_manifest() {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file("../escape.yaml", options).unwrap();
        writer.write_all(b"evil").unwrap();
        let zip = writer.finish().unwrap().into_inner();

        let manifest = SyncManifest {
            schema: super::super::manifest::SCHEMA_VERSION,
            revision: 1,
            parent_revision: 0,
            device_id: "D1".into(),
            device_name: "test".into(),
            created_at: 1,
            scopes: vec!["overrides".into()],
            payload_sha256: sha256_hex(&zip),
            payload_size: zip.len() as u64,
            encryption: None,
            files: vec![super::super::manifest::ManifestFile {
                path: "../escape.yaml".into(),
                sha256: sha256_hex(b"evil"),
                size: 4,
                modified_at: 0,
            }],
            deleted: vec![],
        };

        let staging = std::env::temp_dir().join(format!("verge-sync-test-{}", std::process::id()));
        let result = extract_and_validate(&zip, &manifest, &staging);
        let _ = std::fs::remove_dir_all(&staging);
        assert!(result.is_err());
    }

    #[test]
    fn extraction_rejects_unlisted_entries_and_hash_mismatches() {
        let files = vec![payload_file(SUBSCRIPTIONS_FILE, "version: 1\nsubscriptions: []\n")];
        let zip = zip_payload(&files).unwrap();
        let mut manifest = SyncManifest {
            schema: super::super::manifest::SCHEMA_VERSION,
            revision: 1,
            parent_revision: 0,
            device_id: "D1".into(),
            device_name: "test".into(),
            created_at: 1,
            scopes: vec!["subscriptions".into()],
            payload_sha256: sha256_hex(&zip),
            payload_size: zip.len() as u64,
            encryption: None,
            files: vec![],
            deleted: vec![],
        };
        let staging = std::env::temp_dir().join(format!("verge-sync-test-unlisted-{}", std::process::id()));
        assert!(extract_and_validate(&zip, &manifest, &staging).is_err());

        manifest.files = vec![super::super::manifest::ManifestFile {
            path: SUBSCRIPTIONS_FILE.into(),
            sha256: super::super::manifest::sha256_hex(b"different"),
            size: 30,
            modified_at: 0,
        }];
        assert!(extract_and_validate(&zip, &manifest, &staging).is_err());
        let _ = std::fs::remove_dir_all(staging);
    }

    #[test]
    fn fetched_profile_content_is_never_selected_for_the_payload() {
        // Only metadata files and override paths are addressable, so a fetched `R<uid>.yaml`
        // cannot be added to a payload even if one exists on disk.
        assert!(validate_payload_path("Rabc.yaml").is_err());
        assert!(validate_payload_path("profiles/Rabc.yaml").is_err());
        assert!(validate_payload_path("custom-rules.yaml").is_ok());
    }
}
