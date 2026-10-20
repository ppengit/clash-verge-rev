use crate::utils::{dirs, help};
use anyhow::{Context as _, Result};
use clash_verge_logging::{Type, logging};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::fs;

/// Tombstones only exist to make deletions syncable; prune them after 90 days.
pub const TOMBSTONE_TTL_MS: u64 = 90 * 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalRecordState {
    pub fingerprint: String,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalFileState {
    pub sha256: String,
    pub modified_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalSettingState {
    pub fingerprint: String,
    pub updated_at: u64,
}

/// `sync/sync-state.json`; local only and never uploaded.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncState {
    pub device_id: String,
    pub device_name: String,
    pub last_revision: u64,
    pub last_synced_at: Option<String>,
    pub last_local_hash: Option<String>,
    pub last_error: Option<String>,
    /// Scopes enabled on this device when the last sync completed.
    pub last_scopes: Vec<String>,
    /// Last synced metadata per subscription `sync_id`.
    pub local_records: BTreeMap<String, LocalRecordState>,
    /// Deleting a subscription leaves a tombstone until the remote acknowledges it.
    pub record_tombstones: BTreeMap<String, u64>,
    /// Last synced hash and modification time per override payload path.
    pub overrides: BTreeMap<String, LocalFileState>,
    /// Deleting an override file leaves a tombstone keyed by payload path.
    pub override_tombstones: BTreeMap<String, u64>,
    /// Last synced whitelisted setting per key.
    pub settings: BTreeMap<String, LocalSettingState>,
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

fn default_device_name() -> String {
    let hostname = gethostname::gethostname();
    let name = hostname.to_string_lossy();
    let trimmed = name.trim();
    if trimmed.is_empty() {
        "device".into()
    } else {
        trimmed.chars().take(32).collect()
    }
}

impl SyncState {
    pub async fn load_or_create() -> Result<Self> {
        let path = dirs::sync_state_path()?;
        match fs::read(&path).await {
            Ok(bytes) => match serde_json::from_slice::<Self>(&bytes) {
                Ok(mut state) => {
                    if state.ensure_identity()
                        && let Err(error) = state.save().await
                    {
                        logging!(warn, Type::Backup, "Failed to persist repaired sync state: {error:#}");
                    }
                    Ok(state)
                }
                Err(error) => {
                    logging!(
                        warn,
                        Type::Backup,
                        "sync-state.json is corrupt ({error}); starting from an empty sync state"
                    );
                    let state = Self::fresh();
                    state.save().await?;
                    Ok(state)
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let state = Self::fresh();
                state.save().await?;
                Ok(state)
            }
            Err(error) => Err(error).context("failed to read sync state"),
        }
    }

    fn fresh() -> Self {
        Self {
            device_id: help::get_uid("D"),
            device_name: default_device_name(),
            ..Self::default()
        }
    }

    fn ensure_identity(&mut self) -> bool {
        let id_missing = !is_safe_device_id(&self.device_id);
        let name_missing = self.device_name.trim().is_empty();
        if id_missing {
            self.device_id = help::get_uid("D");
        }
        if name_missing {
            self.device_name = default_device_name();
        }
        id_missing || name_missing
    }

    pub async fn save(&self) -> Result<()> {
        write_json_atomic(&dirs::sync_state_path()?, self).await
    }

    pub fn display_device_name(&self) -> String {
        if self.device_name.trim().is_empty() {
            default_device_name()
        } else {
            self.device_name.clone()
        }
    }

    /// Drops tombstones old enough that every synced device has seen them.
    pub fn prune_tombstones(&mut self, now: u64) {
        self.record_tombstones
            .retain(|_, deleted_at| now.saturating_sub(*deleted_at) <= TOMBSTONE_TTL_MS);
        self.override_tombstones
            .retain(|_, deleted_at| now.saturating_sub(*deleted_at) <= TOMBSTONE_TTL_MS);
    }
}

fn is_safe_device_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub async fn write_json_atomic<T: Serialize + Sync>(path: &Path, value: &T) -> Result<()> {
    let data = serde_json::to_vec_pretty(value)?;
    write_bytes_atomic(path, &data).await
}

/// Writes beside the target and replaces it so a crash cannot leave a partial file.
pub async fn write_bytes_atomic(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let temporary = path.with_extension(format!("tmp-{}-{}", std::process::id(), help::get_uid("t")));
    fs::write(&temporary, data)
        .await
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    if let Err(error) = fs::rename(&temporary, path).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(error).with_context(|| format!("failed to replace {}", path.display()));
    }
    Ok(())
}

pub fn now_ms() -> u64 {
    now_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tombstone_pruning_keeps_recent_deletions_only() {
        let now = 10 * TOMBSTONE_TTL_MS;
        let mut state = SyncState::default();
        state.record_tombstones.insert("old".into(), now - TOMBSTONE_TTL_MS - 1);
        state.record_tombstones.insert("new".into(), now - 1);
        state.override_tombstones.insert("old".into(), 0);

        state.prune_tombstones(now);

        assert_eq!(state.record_tombstones.keys().collect::<Vec<_>>(), vec!["new"]);
        assert!(state.override_tombstones.is_empty());
    }
}
