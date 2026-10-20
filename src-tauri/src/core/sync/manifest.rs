use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::HashSet;

pub const SCHEMA_VERSION: u32 = 1;
pub const MANIFEST_MAX_BYTES: u64 = 1024 * 1024;
pub const PAYLOAD_MAX_BYTES: u64 = 100 * 1024 * 1024;
pub const UNCOMPRESSED_MAX_BYTES: u64 = 200 * 1024 * 1024;
pub const MAX_ARCHIVE_ENTRIES: usize = 2_000;
pub const MAX_OVERRIDE_FILES: usize = 100;
pub const MAX_SUBSCRIPTIONS: usize = 500;
pub const MAX_OVERRIDE_BYTES: u64 = 5 * 1024 * 1024;
pub const HISTORY_KEEP: usize = 10;
pub const CONFLICTS_KEEP: usize = 20;
pub const MIN_INTERVAL_MINUTES: u64 = 15;
pub const DEFAULT_INTERVAL_MINUTES: u64 = 360;

pub const SUBSCRIPTIONS_FILE: &str = "subscriptions.yaml";
pub const CUSTOM_RULES_FILE: &str = "custom-rules.yaml";
pub const SETTINGS_FILE: &str = "settings.yaml";

/// Ensures the payload cannot address anything outside the staging directory.
pub fn validate_payload_path(path: &str) -> Result<()> {
    if path.is_empty() || path.len() > 256 {
        bail!("payload path is empty or too long");
    }
    if path.contains('\\') || path.contains('\0') || path.starts_with('/') || path.contains(':') {
        bail!("payload path is not a relative portable path: {path}");
    }

    let segments: Vec<&str> = path.split('/').collect();
    if segments
        .iter()
        .any(|segment| segment.is_empty() || *segment == "." || *segment == "..")
    {
        bail!("payload path contains an empty or traversing segment: {path}");
    }

    match path {
        SUBSCRIPTIONS_FILE | CUSTOM_RULES_FILE | SETTINGS_FILE => Ok(()),
        _ => {
            if segments.len() != 3 || segments[0] != "overrides" {
                bail!("unsupported payload path: {path}");
            }
            let scope = segments[1];
            let file = segments[2];
            if scope != "global" && !is_safe_identifier(scope) {
                bail!("invalid override scope in payload path: {path}");
            }
            match file {
                "script.js" | "merge.yaml" => Ok(()),
                "proxies.yaml" | "groups.yaml" if scope != "global" => Ok(()),
                "proxies.yaml" | "groups.yaml" => {
                    bail!("global payload overrides support script and merge only: {path}")
                }
                _ => bail!("unsupported override file in payload path: {path}"),
            }
        }
    }
}

fn is_safe_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Per-file bounds keep a malicious or corrupted manifest from staging a huge single file.
pub fn max_payload_file_size(path: &str) -> u64 {
    if path.starts_with("overrides/") {
        MAX_OVERRIDE_BYTES
    } else if path == CUSTOM_RULES_FILE {
        2 * 1024 * 1024
    } else {
        10 * 1024 * 1024
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ManifestFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    /// Local edit time in milliseconds; drives file-level last-modified merges.
    pub modified_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptionInfo {
    pub algo: String,
    pub kdf: String,
    pub iterations: u32,
    pub salt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SyncManifest {
    pub schema: u32,
    pub revision: u64,
    pub parent_revision: u64,
    pub device_id: String,
    pub device_name: String,
    pub created_at: u64,
    pub scopes: Vec<String>,
    /// SHA-256 of the stored payload (ciphertext when encrypted).
    pub payload_sha256: String,
    pub payload_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encryption: Option<EncryptionInfo>,
    pub files: Vec<ManifestFile>,
    pub deleted: Vec<String>,
}

impl SyncManifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema != SCHEMA_VERSION {
            bail!(
                "unsupported sync manifest schema {} (this app supports schema {SCHEMA_VERSION})",
                self.schema
            );
        }
        if self.revision == 0 {
            bail!("sync manifest revision must be positive");
        }
        if self.parent_revision >= self.revision {
            bail!("sync manifest parent revision is not older than its revision");
        }
        if self.device_id.is_empty()
            || self.device_id.len() > 64
            || !self
                .device_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            bail!("sync manifest device identity is invalid");
        }
        if self.device_name.len() > 128 {
            bail!("sync manifest device name is too long");
        }
        if !is_sha256_hex(&self.payload_sha256) {
            bail!("sync manifest payload hash is invalid");
        }
        if self.payload_size > PAYLOAD_MAX_BYTES {
            bail!(
                "sync payload is {} bytes, above the {} byte limit",
                self.payload_size,
                PAYLOAD_MAX_BYTES
            );
        }
        for scope in &self.scopes {
            if !matches!(
                scope.as_str(),
                "subscriptions" | "overrides" | "custom_rules" | "settings"
            ) {
                bail!("sync manifest contains an unknown scope: {scope}");
            }
        }
        if let Some(encryption) = &self.encryption {
            if encryption.algo != "aes-256-gcm" || encryption.kdf != "pbkdf2-sha256" {
                bail!("sync manifest uses unsupported payload encryption");
            }
            if encryption.iterations == 0 || encryption.iterations > 1_000_000 {
                bail!("sync manifest key derivation iteration count is invalid");
            }
            let salt = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encryption.salt.as_bytes())
                .context("sync manifest encryption salt is not valid base64")?;
            if !(8..=64).contains(&salt.len()) {
                bail!("sync manifest encryption salt has an invalid length");
            }
        }
        if self.files.len() > MAX_ARCHIVE_ENTRIES {
            bail!("sync manifest lists too many files");
        }
        let mut seen = HashSet::new();
        let mut total = 0_u64;
        for file in &self.files {
            validate_payload_path(&file.path)?;
            if !is_sha256_hex(&file.sha256) {
                bail!("sync manifest hash is invalid for {}", file.path);
            }
            if file.size > max_payload_file_size(&file.path) {
                bail!("sync manifest file is too large: {}", file.path);
            }
            if !seen.insert(file.path.as_str()) {
                bail!("sync manifest lists a duplicate path: {}", file.path);
            }
            total = total.saturating_add(file.size);
        }
        if total > UNCOMPRESSED_MAX_BYTES {
            bail!("sync manifest declares more than {UNCOMPRESSED_MAX_BYTES} uncompressed bytes");
        }
        if self.deleted.len() > MAX_ARCHIVE_ENTRIES {
            bail!("sync manifest lists too many deletions");
        }
        for path in &self.deleted {
            validate_payload_path(path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> SyncManifest {
        SyncManifest {
            schema: SCHEMA_VERSION,
            revision: 2,
            parent_revision: 1,
            device_id: "Dabc".into(),
            device_name: "laptop".into(),
            created_at: 1,
            scopes: vec!["subscriptions".into()],
            payload_sha256: sha256_hex(b"payload"),
            payload_size: 7,
            encryption: None,
            files: vec![ManifestFile {
                path: SUBSCRIPTIONS_FILE.into(),
                sha256: sha256_hex(b"subs"),
                size: 4,
                modified_at: 1,
            }],
            deleted: vec![],
        }
    }

    #[test]
    fn validates_a_well_formed_manifest() {
        assert!(manifest().validate().is_ok());
    }

    #[test]
    fn rejects_a_newer_schema_instead_of_partially_applying_it() {
        let mut newer = manifest();
        newer.schema = SCHEMA_VERSION + 1;
        let error = newer.validate().unwrap_err().to_string();
        assert!(error.contains("unsupported sync manifest schema"));
    }

    #[test]
    fn rejects_traversal_and_unknown_paths() {
        for path in [
            "../evil.yaml",
            "overrides/../evil.yaml",
            "overrides/global/../merge.yaml",
            "/absolute.yaml",
            "C:/absolute.yaml",
            "overrides/global/other.yaml",
            "unknown.yaml",
            "overrides/global/proxies.yaml",
        ] {
            assert!(validate_payload_path(path).is_err(), "{path} should be rejected");
        }
        assert!(validate_payload_path("overrides/Sabc/script.js").is_ok());
        assert!(validate_payload_path("overrides/global/merge.yaml").is_ok());
    }

    #[test]
    fn rejects_duplicate_and_mismatched_hashes() {
        let mut duplicated = manifest();
        duplicated.files.push(duplicated.files[0].clone());
        assert!(duplicated.validate().is_err());

        let mut bad_hash = manifest();
        bad_hash.payload_sha256 = "not-a-hash".into();
        assert!(bad_hash.validate().is_err());
    }

    #[test]
    fn rejects_encryption_with_invalid_parameters() {
        let mut encrypted = manifest();
        encrypted.encryption = Some(EncryptionInfo {
            algo: "aes-256-gcm".into(),
            kdf: "pbkdf2-sha256".into(),
            iterations: 0,
            salt: "AAAA".into(),
        });
        assert!(encrypted.validate().is_err());
    }
}
