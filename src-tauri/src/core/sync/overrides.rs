use super::{
    ConflictPolicy,
    manifest::{MAX_OVERRIDE_BYTES, MAX_OVERRIDE_FILES, sha256_hex},
    state::SyncState,
};
use crate::{
    config::{IProfiles, PrfItem},
    utils::dirs,
};
use anyhow::{Context as _, Result, bail};
use clash_verge_logging::{Type, logging};
use std::collections::{BTreeMap, HashSet};
use tokio::fs;

#[derive(Clone, Debug)]
pub struct OverrideFile {
    pub path: std::string::String,
    pub content: Vec<u8>,
    pub modified_at: u64,
}

impl OverrideFile {
    pub fn sha256(&self) -> String {
        sha256_hex(&self.content)
    }
}

pub struct OverrideCollection {
    pub files: BTreeMap<std::string::String, OverrideFile>,
    pub tombstones: BTreeMap<std::string::String, u64>,
}

const GLOBAL_SCRIPT_UID: &str = "Script";
const GLOBAL_MERGE_UID: &str = "Merge";

fn role_file(itype: &str) -> Option<(&'static str, &'static str)> {
    match itype {
        "script" => Some(("script", "script.js")),
        "merge" => Some(("merge", "merge.yaml")),
        "proxies" => Some(("proxies", "proxies.yaml")),
        "groups" => Some(("groups", "groups.yaml")),
        _ => None,
    }
}

fn override_ref(item: &PrfItem, role: &str) -> Option<std::string::String> {
    let option = item.option.as_ref()?;
    let value = match role {
        "script" => option.script.as_ref(),
        "merge" => option.merge.as_ref(),
        "proxies" => option.proxies.as_ref(),
        "groups" => option.groups.as_ref(),
        _ => None,
    };
    value.map(ToString::to_string)
}

pub fn linked_role_items<'a>(profiles: &'a IProfiles, item: &PrfItem) -> Vec<(&'static str, &'a PrfItem)> {
    let mut linked = Vec::new();
    for role in ["script", "merge", "proxies", "groups"] {
        if let Some(uid) = override_ref(item, role)
            && let Ok(target) = profiles.get_item(uid.as_str())
            && target.itype.as_deref() == Some(role)
        {
            linked.push((role, target));
        }
    }
    linked
}

async fn file_for_item(item: &PrfItem, role: &str, payload_path: &str) -> Result<Option<OverrideFile>> {
    let Some(file) = item.file.as_ref() else {
        return Ok(None);
    };
    let Ok(dir) = dirs::app_profiles_dir() else {
        return Ok(None);
    };
    let path = dir.join(file.as_str());
    let metadata = match fs::metadata(&path).await {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect override file {}", path.display()));
        }
    };
    let modified_at = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_millis() as u64);
    if metadata
        .as_ref()
        .is_some_and(|metadata| metadata.len() > MAX_OVERRIDE_BYTES)
    {
        bail!("override file {role} exceeds the {MAX_OVERRIDE_BYTES} byte sync limit");
    }
    let content = fs::read(&path)
        .await
        .with_context(|| format!("failed to read override file {}", path.display()))?;
    if content.len() as u64 > MAX_OVERRIDE_BYTES {
        bail!("override file {role} exceeds the {MAX_OVERRIDE_BYTES} byte sync limit");
    }
    let fallback = item.updated.map_or(0, |seconds| seconds as u64 * 1_000);
    Ok(Some(OverrideFile {
        path: payload_path.into(),
        content,
        modified_at: modified_at.max(fallback),
    }))
}

fn is_neutral_global(path: &str, content: &[u8]) -> bool {
    (path == "overrides/global/script.js" && content == crate::utils::tmpl::ITEM_SCRIPT.as_bytes())
        || (path == "overrides/global/merge.yaml" && content == crate::utils::tmpl::ITEM_MERGE_EMPTY.as_bytes())
}

pub async fn collect_local_overrides(profiles: &IProfiles, state: &SyncState, now: u64) -> Result<OverrideCollection> {
    let mut files = BTreeMap::new();
    let mut tombstones = state.override_tombstones.clone();

    if let Ok(script) = profiles.get_item(GLOBAL_SCRIPT_UID)
        && script.itype.as_deref() == Some("script")
        && let Some(file) = file_for_item(script, "script", "overrides/global/script.js").await?
    {
        if !(state.override_tombstones.contains_key(file.path.as_str()) && is_neutral_global(&file.path, &file.content))
        {
            files.insert(file.path.clone(), file);
        }
    }
    if let Ok(merge) = profiles.get_item(GLOBAL_MERGE_UID)
        && merge.itype.as_deref() == Some("merge")
        && let Some(file) = file_for_item(merge, "merge", "overrides/global/merge.yaml").await?
    {
        if !(state.override_tombstones.contains_key(file.path.as_str()) && is_neutral_global(&file.path, &file.content))
        {
            files.insert(file.path.clone(), file);
        }
    }

    for item in profiles.items.iter().flatten() {
        if item.itype.as_deref() != Some("remote") {
            continue;
        }
        let Some(sync_id) = item.sync_id.as_deref() else {
            continue;
        };
        for (role, target) in linked_role_items(profiles, item) {
            if files.len() >= MAX_OVERRIDE_FILES {
                anyhow::bail!(
                    "too many override files are linked for sync (max {MAX_OVERRIDE_FILES}); reduce linked overrides before syncing"
                );
            }
            let Some(name) = role_file_name(role) else {
                continue;
            };
            let payload_path = format!("overrides/{sync_id}/{name}");
            if files.contains_key(payload_path.as_str()) {
                continue;
            }
            if let Some(file) = file_for_item(target, role, &payload_path).await? {
                files.insert(file.path.clone(), file);
            }
        }
    }

    for path in state.overrides.keys() {
        if !files.contains_key(path.as_str()) {
            tombstones.entry(path.clone()).or_insert(now);
        }
    }
    for (path, file) in &files {
        tombstones.remove(path.as_str());
        let _ = file;
    }

    Ok(OverrideCollection { files, tombstones })
}

pub struct MergedOverrides {
    pub files: BTreeMap<std::string::String, Vec<u8>>,
    pub deleted: Vec<std::string::String>,
    pub conflicts: Vec<(std::string::String, Vec<u8>, &'static str)>,
}

const fn choose(policy: ConflictPolicy, local_modified: u64, remote_modified: u64, remote_revision_wins: bool) -> bool {
    match policy {
        ConflictPolicy::Local => false,
        ConflictPolicy::Remote => true,
        ConflictPolicy::LastModified | ConflictPolicy::Ask => {
            if remote_modified > local_modified {
                true
            } else if local_modified > remote_modified {
                false
            } else {
                remote_revision_wins
            }
        }
    }
}

/// File-level last-modified merge. `remote_present` holds verified payload contents.
pub fn merge_overrides(
    local: &BTreeMap<std::string::String, OverrideFile>,
    local_tombstones: &BTreeMap<std::string::String, u64>,
    remote: &BTreeMap<std::string::String, OverrideFile>,
    remote_deleted: &[std::string::String],
    remote_created_at: u64,
    policy: ConflictPolicy,
    remote_revision_wins: bool,
) -> MergedOverrides {
    let mut files = BTreeMap::new();
    let mut deleted = Vec::new();
    let mut conflicts = Vec::new();
    let remote_deleted: HashSet<&str> = remote_deleted.iter().map(String::as_str).collect();
    let mut paths: BTreeMap<&str, ()> = BTreeMap::new();
    for path in local.keys() {
        paths.insert(path.as_str(), ());
    }
    for path in remote.keys() {
        paths.insert(path.as_str(), ());
    }
    for path in remote_deleted.iter() {
        paths.insert(*path, ());
    }
    for path in local_tombstones.keys() {
        paths.insert(path.as_str(), ());
    }

    for path in paths.keys() {
        let path_string: std::string::String = (*path).into();
        let local_file = local.get(*path);
        let remote_file = remote.get(*path).filter(|_| !remote_deleted.contains(path));
        let local_deleted_at = local_tombstones.get(*path).copied();

        match (local_file, remote_file) {
            (Some(local_file), Some(remote_file)) => {
                let same = local_file.content == remote_file.content;
                if same {
                    files.insert(path_string, remote_file.content.clone());
                    continue;
                }
                if choose(
                    policy,
                    local_file.modified_at,
                    remote_file.modified_at,
                    remote_revision_wins,
                ) {
                    files.insert(path_string.clone(), remote_file.content.clone());
                    conflicts.push((path_string, local_file.content.clone(), "local"));
                } else {
                    files.insert(path_string.clone(), local_file.content.clone());
                    conflicts.push((path_string, remote_file.content.clone(), "remote"));
                }
            }
            (None, Some(remote_file)) => {
                if let Some(deleted_at) = local_deleted_at {
                    if choose(policy, deleted_at, remote_file.modified_at, remote_revision_wins) {
                        files.insert(path_string, remote_file.content.clone());
                    } else {
                        deleted.push(path_string.clone());
                        conflicts.push((path_string, remote_file.content.clone(), "remote"));
                    }
                } else {
                    files.insert(path_string, remote_file.content.clone());
                }
            }
            (Some(local_file), None) => {
                if remote_deleted.contains(path) {
                    // The remote deleted the file at its manifest commit time.
                    if local_file.modified_at > remote_created_at {
                        files.insert(path_string, local_file.content.clone());
                    } else {
                        deleted.push(path_string.clone());
                        conflicts.push((path_string, local_file.content.clone(), "local"));
                    }
                } else {
                    // Present locally, absent remotely, not tombstoned: a local addition.
                    files.insert(path_string, local_file.content.clone());
                }
            }
            (None, None) => {
                deleted.push(path_string);
            }
        }
    }

    deleted.sort();
    deleted.dedup();
    MergedOverrides {
        files,
        deleted,
        conflicts,
    }
}

/// Removes stale sync staging before every extraction.
pub async fn clean_staging() -> Result<()> {
    let dir = dirs::sync_staging_dir()?;
    match fs::remove_dir_all(&dir).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("failed to clean sync staging"),
    }
    fs::create_dir_all(&dir).await?;
    Ok(())
}

pub async fn write_conflict_copy(copy_dir: &std::path::Path, path: &str, content: &[u8]) -> Result<()> {
    let target = copy_dir.join(path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).await?;
    }
    super::state::write_bytes_atomic(&target, content).await
}

/// Keeps the newest conflict snapshots; each sync writes at most one directory.
pub async fn prune_conflicts(keep: usize) -> Result<()> {
    let dir = dirs::sync_conflicts_dir()?;
    let mut entries = match fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("failed to read sync conflicts"),
    };
    let mut names = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    for name in names.into_iter().rev().skip(keep) {
        let path = dir.join(name);
        if let Err(error) = fs::remove_dir_all(&path).await {
            logging!(warn, Type::Backup, "Failed to prune sync conflicts: {error:#}");
        }
    }
    Ok(())
}

pub fn conflict_copy_name(device_name: &str, now: u64) -> String {
    let sanitized: String = device_name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    let sanitized = if sanitized.is_empty() {
        "device".to_owned()
    } else {
        sanitized
    };
    format!("{now}-{sanitized}")
}

/// Splits a payload override path into its profile scope and role.
pub fn parse_override_path(path: &str) -> Option<(std::string::String, &'static str)> {
    let rest = path.strip_prefix("overrides/")?;
    let (scope, file) = rest.split_once('/')?;
    let role = match file {
        "script.js" => "script",
        "merge.yaml" => "merge",
        "proxies.yaml" => "proxies",
        "groups.yaml" => "groups",
        _ => return None,
    };
    Some((scope.into(), role))
}

pub fn role_file_name(role: &str) -> Option<&'static str> {
    role_file(role).map(|(_, file)| file)
}

pub fn role_item_type(role: &str) -> Option<&'static str> {
    role_file(role).map(|(role, _)| role)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, content: &str, modified_at: u64) -> OverrideFile {
        OverrideFile {
            path: path.into(),
            content: content.as_bytes().to_vec(),
            modified_at,
        }
    }

    #[test]
    fn newer_remote_wins_and_keeps_the_local_copy() {
        let mut local = BTreeMap::new();
        local.insert(
            "overrides/global/script.js".into(),
            file("overrides/global/script.js", "local", 10),
        );
        let mut remote = BTreeMap::new();
        remote.insert(
            "overrides/global/script.js".into(),
            file("overrides/global/script.js", "remote", 20),
        );

        let merged = merge_overrides(
            &local,
            &BTreeMap::new(),
            &remote,
            &[],
            30,
            ConflictPolicy::LastModified,
            true,
        );

        assert_eq!(merged.files["overrides/global/script.js"], b"remote");
        assert_eq!(merged.conflicts[0].2, "local");
    }

    #[test]
    fn equal_file_timestamps_defer_to_revision_then_local() {
        let mut local = BTreeMap::new();
        local.insert(
            "overrides/global/merge.yaml".into(),
            file("overrides/global/merge.yaml", "local", 10),
        );
        let mut remote = BTreeMap::new();
        remote.insert(
            "overrides/global/merge.yaml".into(),
            file("overrides/global/merge.yaml", "remote", 10),
        );

        let remote_wins = merge_overrides(
            &local,
            &BTreeMap::new(),
            &remote,
            &[],
            20,
            ConflictPolicy::LastModified,
            true,
        );
        assert_eq!(remote_wins.files["overrides/global/merge.yaml"], b"remote");

        let tied = merge_overrides(
            &local,
            &BTreeMap::new(),
            &remote,
            &[],
            20,
            ConflictPolicy::LastModified,
            false,
        );
        assert_eq!(tied.files["overrides/global/merge.yaml"], b"local");
        assert_eq!(tied.conflicts[0].2, "remote");
    }

    #[test]
    fn remote_deletion_does_not_remove_a_newer_local_edit() {
        let mut local = BTreeMap::new();
        local.insert(
            "overrides/global/script.js".into(),
            file("overrides/global/script.js", "edited", 50),
        );

        let merged = merge_overrides(
            &local,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &["overrides/global/script.js".into()],
            40,
            ConflictPolicy::LastModified,
            true,
        );

        assert!(merged.files.contains_key("overrides/global/script.js"));
        assert!(merged.deleted.is_empty());
    }

    #[test]
    fn local_deletion_removes_an_older_remote_file_and_keeps_a_copy() {
        let mut remote = BTreeMap::new();
        remote.insert(
            "overrides/S1/merge.yaml".into(),
            file("overrides/S1/merge.yaml", "remote", 10),
        );
        let mut tombstones = BTreeMap::new();
        tombstones.insert("overrides/S1/merge.yaml".into(), 20);

        let merged = merge_overrides(
            &BTreeMap::new(),
            &tombstones,
            &remote,
            &[],
            30,
            ConflictPolicy::LastModified,
            true,
        );

        assert!(merged.files.is_empty());
        assert_eq!(merged.deleted, vec!["overrides/S1/merge.yaml"]);
        assert_eq!(merged.conflicts[0].2, "remote");
    }

    #[test]
    fn a_newer_remote_file_resurrects_a_local_deletion() {
        let mut remote = BTreeMap::new();
        remote.insert(
            "overrides/S1/merge.yaml".into(),
            file("overrides/S1/merge.yaml", "remote", 30),
        );
        let mut tombstones = BTreeMap::new();
        tombstones.insert("overrides/S1/merge.yaml".into(), 20);

        let merged = merge_overrides(
            &BTreeMap::new(),
            &tombstones,
            &remote,
            &[],
            40,
            ConflictPolicy::LastModified,
            true,
        );

        assert!(merged.files.contains_key("overrides/S1/merge.yaml"));
        assert!(merged.deleted.is_empty());
    }
}
