use super::ConflictPolicy;
use crate::{
    config::{IProfiles, PrfOption},
    utils::help,
};
use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const DOCUMENT_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubscriptionOption {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_interval: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_auto_update: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<std::string::String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub with_proxy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_proxy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub danger_accept_invalid_certs: Option<bool>,
}

impl SubscriptionOption {
    pub fn from_prf(option: Option<&PrfOption>) -> Self {
        let Some(option) = option else {
            return Self::default();
        };
        Self {
            update_interval: option.update_interval,
            allow_auto_update: option.allow_auto_update,
            user_agent: option.user_agent.as_ref().map(ToString::to_string),
            timeout_seconds: option.timeout_seconds,
            with_proxy: option.with_proxy,
            self_proxy: option.self_proxy,
            danger_accept_invalid_certs: option.danger_accept_invalid_certs,
        }
    }

    /// Overwrites only synced fields; local override references and unrelated options survive.
    pub fn apply_to_prf(&self, option: &mut PrfOption) {
        option.update_interval = self.update_interval;
        option.allow_auto_update = self.allow_auto_update;
        option.user_agent = self.user_agent.as_ref().map(|value| value.as_str().into());
        option.timeout_seconds = self.timeout_seconds;
        option.with_proxy = self.with_proxy;
        option.self_proxy = self.self_proxy;
        option.danger_accept_invalid_certs = self.danger_accept_invalid_certs;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubscriptionRecord {
    pub sync_id: std::string::String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<std::string::String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<std::string::String>,
    #[serde(rename = "type")]
    pub itype: std::string::String,
    #[serde(skip_serializing_if = "is_empty_option")]
    pub option: SubscriptionOption,
    pub order: u32,
    /// Metadata edit time in milliseconds; never the subscription fetch time.
    pub updated_at: u64,
    pub deleted: bool,
}

fn is_empty_option(option: &SubscriptionOption) -> bool {
    *option == SubscriptionOption::default()
}

impl SubscriptionRecord {
    pub fn fingerprint(&self) -> String {
        let mut fingerprint = self.clone();
        fingerprint.updated_at = 0;
        let data = serde_json::to_vec(&fingerprint).unwrap_or_default();
        super::manifest::sha256_hex(&data)
    }

    fn is_remote(&self) -> bool {
        self.itype.is_empty() || self.itype == "remote"
    }

    fn normalized_url(&self) -> Option<String> {
        self.url.as_deref().and_then(normalize_url)
    }

    fn same_content(&self, other: &Self) -> bool {
        let mut left = self.clone();
        let mut right = other.clone();
        left.updated_at = 0;
        right.updated_at = 0;
        left.order = 0;
        right.order = 0;
        left == right
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SubscriptionDocument {
    pub version: u32,
    pub subscriptions: Vec<SubscriptionRecord>,
}

fn is_safe_sync_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn parse_document(bytes: &[u8]) -> Result<Vec<SubscriptionRecord>> {
    let document: SubscriptionDocument =
        serde_yaml_ng::from_slice(bytes).context("subscriptions.yaml is not valid YAML")?;
    if document.version != DOCUMENT_VERSION {
        bail!(
            "unsupported subscriptions.yaml schema {} (expected {DOCUMENT_VERSION})",
            document.version
        );
    }
    if document.subscriptions.len() > super::manifest::MAX_SUBSCRIPTIONS {
        bail!("subscriptions.yaml contains too many subscriptions");
    }

    let mut ids = HashSet::new();
    for record in &document.subscriptions {
        let sync_id = record.sync_id.trim();
        if !is_safe_sync_id(sync_id) {
            bail!("subscriptions.yaml contains an invalid sync id");
        }
        if !ids.insert(sync_id.to_owned()) {
            bail!("subscriptions.yaml contains a duplicate sync id");
        }
        if !record.deleted && !record.is_remote() {
            bail!("subscriptions.yaml contains a non-remote subscription");
        }
        if let Some(url) = &record.url
            && !url.is_empty()
            && normalize_url(url).is_none()
        {
            bail!("subscriptions.yaml contains an invalid subscription URL");
        }
    }
    Ok(document.subscriptions)
}

pub fn normalize_url(raw: &str) -> Option<String> {
    let url = reqwest_dav::re_exports::url::Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    Some(url.to_string())
}

pub struct LocalRecordCollection {
    pub records: Vec<SubscriptionRecord>,
    pub next_state: BTreeMap<std::string::String, super::state::LocalRecordState>,
    pub tombstones: BTreeMap<std::string::String, u64>,
}

/// Reads subscription metadata only; fetched profile content and counters never leave the device.
pub fn collect_local_records(
    profiles: &IProfiles,
    state: &super::state::SyncState,
    first_sync: bool,
    now: u64,
) -> Result<LocalRecordCollection> {
    let mut records = Vec::new();
    let mut next_state = BTreeMap::new();
    let mut tombstones = state.record_tombstones.clone();
    let mut seen = HashSet::new();
    let mut remote_order = 0_u32;

    for item in profiles.items.iter().flatten() {
        if item.itype.as_deref() != Some("remote") {
            continue;
        }
        let Some(sync_id) = item
            .sync_id
            .as_ref()
            .map(ToString::to_string)
            .filter(|sync_id| is_safe_sync_id(sync_id))
        else {
            continue;
        };
        if !seen.insert(sync_id.clone()) {
            continue;
        }
        let record = SubscriptionRecord {
            sync_id: sync_id.clone(),
            name: item.name.as_ref().map(ToString::to_string),
            url: item.url.as_ref().map(ToString::to_string),
            itype: "remote".into(),
            option: SubscriptionOption::from_prf(item.option.as_ref()),
            order: remote_order,
            updated_at: 0,
            deleted: false,
        };
        remote_order = remote_order.saturating_add(1);
        let fingerprint = record.fingerprint();
        let updated_at = if first_sync {
            0
        } else {
            state
                .local_records
                .get(sync_id.as_str())
                .filter(|previous| previous.fingerprint == fingerprint)
                .map_or(now, |previous| previous.updated_at)
        };
        let mut record = record;
        record.updated_at = updated_at;
        next_state.insert(
            sync_id.clone(),
            super::state::LocalRecordState {
                fingerprint,
                updated_at,
            },
        );
        tombstones.remove(sync_id.as_str());
        records.push(record);
    }

    // Restoring an old backup (or manually replacing profiles.yaml) can give every profile a
    // new sync_id. Treating that as a mass deletion would tombstone the cloud copy on the next
    // upload, so refuse instead and ask the user to reconcile.
    let old_record_count = state.local_records.len();
    let matched_old_count = state
        .local_records
        .keys()
        .filter(|sync_id| seen.contains(sync_id.as_str()))
        .count();
    if old_record_count > 0 {
        if next_state.is_empty() {
            bail!(
                "all remote profiles disappeared locally; refusing to delete cloud subscriptions automatically. Remove them explicitly or reset the sync state."
            );
        }
        if matched_old_count * 2 < old_record_count {
            bail!(
                "most local profile identities changed (for example after restoring an old backup); refusing to generate deletion tombstones. Re-import the profiles or reset the sync state."
            );
        }
    }

    // A profile that disappeared locally keeps a tombstone until the remote acknowledges it.
    for sync_id in state.local_records.keys() {
        if !seen.contains(sync_id.as_str()) {
            tombstones.entry(sync_id.clone()).or_insert(now);
        }
    }
    for (sync_id, deleted_at) in &tombstones {
        if seen.contains(sync_id.as_str()) {
            continue;
        }
        records.push(SubscriptionRecord {
            sync_id: sync_id.clone(),
            itype: "remote".into(),
            updated_at: *deleted_at,
            deleted: true,
            ..SubscriptionRecord::default()
        });
    }

    records.sort_by(|left, right| {
        (left.deleted, left.order, &left.sync_id).cmp(&(right.deleted, right.order, &right.sync_id))
    });
    Ok(LocalRecordCollection {
        records,
        next_state,
        tombstones,
    })
}

#[derive(Debug, Clone)]
pub struct RecordConflict {
    pub side: &'static str,
    pub record: SubscriptionRecord,
}

pub struct MergedSubscriptions {
    pub records: Vec<SubscriptionRecord>,
    pub conflicts: Vec<RecordConflict>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Local,
    Remote,
}

const fn choose_side(
    local: &SubscriptionRecord,
    remote: &SubscriptionRecord,
    policy: ConflictPolicy,
    local_base_revision: u64,
    remote_revision: u64,
) -> Side {
    match policy {
        ConflictPolicy::Local => Side::Local,
        ConflictPolicy::Remote => Side::Remote,
        ConflictPolicy::LastModified | ConflictPolicy::Ask => {
            if remote.updated_at > local.updated_at {
                Side::Remote
            } else if local.updated_at > remote.updated_at {
                Side::Local
            } else if remote_revision > local_base_revision {
                Side::Remote
            } else {
                // Final tie goes to the local record; the remote copy is retained.
                Side::Local
            }
        }
    }
}

pub fn merge_subscriptions(
    local: &[SubscriptionRecord],
    remote: &[SubscriptionRecord],
    policy: ConflictPolicy,
    local_base_revision: u64,
    remote_revision: u64,
) -> MergedSubscriptions {
    let mut by_id: HashMap<&str, &SubscriptionRecord> = HashMap::new();
    for record in local {
        by_id.insert(record.sync_id.as_str(), record);
    }
    let mut by_url: HashMap<(String, &str), &SubscriptionRecord> = HashMap::new();
    for record in local {
        if let Some(url) = record.normalized_url() {
            by_url.insert((url, record.itype.as_str()), record);
        }
    }

    let mut matched_local: HashSet<std::string::String> = HashSet::new();
    let mut merged = Vec::new();
    let mut conflicts = Vec::new();

    for remote_record in remote {
        let matched = by_id.get(remote_record.sync_id.as_str()).copied().or_else(|| {
            remote_record.normalized_url().and_then(|url| {
                by_url
                    .get(&(url, remote_record.itype.as_str()))
                    .copied()
                    .filter(|record| !matched_local.contains(record.sync_id.as_str()))
            })
        });

        let Some(local_record) = matched else {
            merged.push(remote_record.clone());
            continue;
        };
        matched_local.insert(local_record.sync_id.as_str().to_owned());

        if local_record.same_content(remote_record) {
            let mut winner = local_record.clone();
            winner.updated_at = local_record.updated_at.max(remote_record.updated_at);
            // The remote list order is canonical; adopting it avoids cross-device order churn.
            winner.order = remote_record.order;
            merged.push(winner);
            continue;
        }

        match choose_side(
            local_record,
            remote_record,
            policy,
            local_base_revision,
            remote_revision,
        ) {
            Side::Local => {
                merged.push(local_record.clone());
                if !remote_record.deleted {
                    conflicts.push(RecordConflict {
                        side: "remote",
                        record: remote_record.clone(),
                    });
                }
            }
            Side::Remote => {
                merged.push(remote_record.clone());
                if !local_record.deleted {
                    conflicts.push(RecordConflict {
                        side: "local",
                        record: local_record.clone(),
                    });
                }
            }
        }
    }

    for local_record in local {
        if !matched_local.contains(local_record.sync_id.as_str()) {
            merged.push(local_record.clone());
        }
    }

    merged.sort_by(|left, right| {
        (left.deleted, left.order, &left.sync_id).cmp(&(right.deleted, right.order, &right.sync_id))
    });
    // Canonical 0..n order so both the local profile positions and the seeded
    // metadata fingerprints agree after apply; stray remote order values would
    // otherwise look like a fresh local edit.
    for (position, record) in merged.iter_mut().filter(|record| !record.deleted).enumerate() {
        record.order = u32::try_from(position).unwrap_or(u32::MAX);
    }

    MergedSubscriptions {
        records: merged,
        conflicts,
    }
}

/// Deterministic YAML used in the payload; the same records always produce the same bytes.
pub fn render_document(records: &[SubscriptionRecord]) -> Result<Vec<u8>> {
    let document = SubscriptionDocument {
        version: DOCUMENT_VERSION,
        subscriptions: records.to_vec(),
    };
    let body = serde_yaml_ng::to_string(&document)?;
    Ok(format!("# Clash Verge subscription metadata\n\n{body}").into_bytes())
}

/// Seeds local timestamps after applying a merged document so the next build is stable.
pub fn seed_record_state(state: &mut super::state::SyncState, records: &[SubscriptionRecord], now: u64) {
    state.local_records.clear();
    for record in records.iter().filter(|record| !record.deleted) {
        let mut seeded = record.clone();
        if seeded.updated_at == 0 {
            seeded.updated_at = now;
        }
        let fingerprint = record.fingerprint();
        state.local_records.insert(
            record.sync_id.clone(),
            super::state::LocalRecordState {
                fingerprint,
                updated_at: seeded.updated_at,
            },
        );
    }
    for record in records.iter().filter(|record| record.deleted) {
        state
            .record_tombstones
            .insert(record.sync_id.clone(), record.updated_at);
    }
    for record in records.iter().filter(|record| !record.deleted) {
        state.record_tombstones.remove(record.sync_id.as_str());
    }
}

/// A fresh local identity for a subscription created from remote metadata.
pub fn new_profile_uid() -> std::string::String {
    help::get_uid("R").as_str().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(sync_id: &str, name: &str, updated_at: u64) -> SubscriptionRecord {
        SubscriptionRecord {
            sync_id: sync_id.into(),
            name: Some(name.into()),
            url: Some(format!("https://example.com/{sync_id}?token=secret").into()),
            itype: "remote".into(),
            updated_at,
            ..SubscriptionRecord::default()
        }
    }

    #[test]
    fn tombstones_only_delete_older_local_metadata() {
        let mut local = record("S1", "local", 1_000);
        local.deleted = true;
        local.updated_at = 5_000;
        let remote = record("S1", "remote", 1_000);

        let merged = merge_subscriptions(
            std::slice::from_ref(&local),
            std::slice::from_ref(&remote),
            ConflictPolicy::LastModified,
            1,
            2,
        );

        assert_eq!(merged.records.len(), 1);
        assert!(merged.records[0].deleted, "the newer tombstone wins");
        assert_eq!(merged.conflicts.len(), 1);
        assert_eq!(
            merged.conflicts[0].side, "remote",
            "the resurrected remote copy is retained"
        );
    }

    #[test]
    fn a_local_edit_after_a_tombstone_conflicts_instead_of_deleting() {
        let local = record("S1", "edited locally", 9_000);
        let mut remote = record("S1", "", 5_000);
        remote.deleted = true;
        remote.name = None;
        remote.url = None;

        let merged = merge_subscriptions(
            std::slice::from_ref(&local),
            std::slice::from_ref(&remote),
            ConflictPolicy::LastModified,
            1,
            2,
        );

        assert!(!merged.records[0].deleted);
        assert!(
            merged.conflicts.is_empty(),
            "a losing tombstone has no content worth retaining"
        );
    }

    #[test]
    fn equal_timestamps_use_revision_then_local_and_keep_the_remote_copy() {
        let local = record("S1", "local", 1_000);
        let remote = record("S1", "remote", 1_000);

        let remote_wins = merge_subscriptions(
            std::slice::from_ref(&local),
            std::slice::from_ref(&remote),
            ConflictPolicy::LastModified,
            1,
            2,
        );
        assert_eq!(remote_wins.records[0].name.as_deref(), Some("remote"));

        let tied = merge_subscriptions(
            std::slice::from_ref(&local),
            std::slice::from_ref(&remote),
            ConflictPolicy::LastModified,
            2,
            2,
        );
        assert_eq!(tied.records[0].name.as_deref(), Some("local"));
        assert_eq!(tied.conflicts.len(), 1);
        assert_eq!(tied.conflicts[0].side, "remote");
    }

    #[test]
    fn records_merge_by_sync_id_and_fall_back_to_normalized_url() {
        let local = record("S1", "same id", 10);
        let remote = record("S1", "remote name", 20);
        let merged = merge_subscriptions(
            std::slice::from_ref(&local),
            std::slice::from_ref(&remote),
            ConflictPolicy::LastModified,
            1,
            2,
        );
        assert_eq!(merged.records[0].name.as_deref(), Some("remote name"));

        let mut url_local = record("", "url only", 10);
        url_local.sync_id = "local-only".into();
        let mut url_remote = record("", "url remote", 20);
        url_remote.sync_id = "remote-only".into();
        let merged = merge_subscriptions(
            std::slice::from_ref(&url_local),
            std::slice::from_ref(&url_remote),
            ConflictPolicy::LastModified,
            1,
            2,
        );
        assert_eq!(merged.records.len(), 1);
        assert_eq!(merged.records[0].sync_id, "remote-only");
    }

    #[test]
    fn policy_local_keeps_local_but_accepts_remote_only_records() {
        let local = record("S1", "local", 10);
        let remote = record("S1", "remote", 20);
        let remote_only = record("S2", "new", 20);
        let merged = merge_subscriptions(&[local], &[remote, remote_only], ConflictPolicy::Local, 1, 2);
        assert_eq!(merged.records.len(), 2);
        assert_eq!(
            merged
                .records
                .iter()
                .find(|record| record.sync_id == "S1")
                .and_then(|record| record.name.as_deref()),
            Some("local")
        );
    }

    #[test]
    fn fetched_content_is_never_part_of_the_metadata_record() {
        let record = record("S1", "name", 10);
        let yaml = String::from_utf8(render_document(&[record]).unwrap()).unwrap();
        for forbidden in ["uid", "file:", "selected", "upload", "download", "expire", "extra"] {
            assert!(!yaml.contains(forbidden), "{forbidden} must not be uploaded");
        }
    }
}
