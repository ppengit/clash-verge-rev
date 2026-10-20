//! Background node health verification.
//!
//! The manager owns scheduling, the bounded health cache and its persistence. Probes snapshot the
//! active profile, groups and nodes first, then release every configuration guard before touching
//! the network. A generation counter invalidates results when the profile or settings change under
//! a running cycle.

use crate::{
    config::{Config, IProfiles, IVerge, ProbeTarget, profiles::profile_sync_id},
    core::{
        CoreManager,
        handle::Handle,
        manager::RunningMode,
        node_health_probe::{
            ContentProbeResult, ContentProbeSpec, ControllerClient, MAX_TARGETS, ProbeResult, ProbeTargetSpec,
            probe_content,
        },
        proxy_view::{ProxyGroupView, ProxyMemberRef, ProxyNodeSource, ProxyViewV1},
    },
    process::AsyncHandler,
    utils::{dirs, resolve::is_resolve_done},
};
use anyhow::{Context as _, Result};
use chrono::Utc;
use clash_verge_logging::{Type, logging};
use futures::{StreamExt as _, stream};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::async_runtime::JoinHandle;
use tokio::sync::{Mutex as AsyncMutex, watch};

const DEFAULT_INTERVAL_MINUTES: u64 = 10;
const DEFAULT_DEEP_INTERVAL_MINUTES: u64 = 60;
const MIN_INTERVAL_MINUTES: u64 = 1;
const MAX_INTERVAL_MINUTES: u64 = 24 * 60;
const DEFAULT_L1_TIMEOUT_MS: u32 = 5_000;
const DEFAULT_L2_TIMEOUT_MS: u32 = 8_000;
const DEFAULT_L1_URL: &str = "https://cp.cloudflare.com/generate_204";
const DEFAULT_L2_TARGETS: [(&str, &str); 3] = [
    ("https://www.google.com/robots.txt", "200"),
    ("https://github.com/favicon.ico", "200"),
    ("https://www.cloudflare.com/cdn-cgi/trace", "200"),
];

const CACHE_MAX_ENTRIES: usize = 5_000;
const SAMPLE_LIMIT: usize = 8;
const MAX_NODES_PER_CYCLE: usize = 2_000;
const MAX_GROUPS: usize = 100;
const L1_CONCURRENCY: usize = 10;
const L2_CONCURRENCY: usize = 5;
const CYCLE_TIMEOUT: Duration = Duration::from_secs(60);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(15);
const STARTUP_DELAY: Duration = Duration::from_secs(15);
const READY_WAIT: Duration = Duration::from_secs(60);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const PERSIST_INTERVAL_SECS: i64 = 5 * 60;
const MAX_PERSIST_BYTES: usize = 2 * 1024 * 1024;
const NODE_HEALTH_FILE: &str = "node-health.json";

const DEGRADED_JITTER_MS: u32 = 150;
const DEGRADED_LATENCY_MS: u32 = 1_500;
const MAX_BACKOFF_MULTIPLIER: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum NodeHealthState {
    Healthy,
    Degraded,
    Unusable,
    SiteBlocked,
    Unverified,
    Testing,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum HealthScope {
    CurrentNode,
    #[default]
    CurrentGroup,
    AllGroups,
}

impl HealthScope {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "current_node" | "current-node" | "node" => Some(Self::CurrentNode),
            "current_group" | "current-group" | "group" => Some(Self::CurrentGroup),
            "all_groups" | "all-groups" | "all" => Some(Self::AllGroups),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CurrentNode => "current_node",
            Self::CurrentGroup => "current_group",
            Self::AllGroups => "all_groups",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HealthKey {
    pub profile: std::string::String,
    pub group: std::string::String,
    pub node: std::string::String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TargetResult {
    pub url: std::string::String,
    pub ok: bool,
    pub delay_ms: Option<u32>,
    pub error: Option<std::string::String>,
    pub checked_at: i64,
}

impl TargetResult {
    fn new(target: &ProbeTargetSpec, result: &ProbeResult, checked_at: i64) -> Self {
        Self {
            url: target.url.clone(),
            ok: result.ok,
            delay_ms: result.delay_ms,
            error: result.error.clone(),
            checked_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HealthSample {
    pub at: i64,
    pub ok: bool,
    pub delay_ms: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct NodeHealthEntry {
    pub key: HealthKey,
    pub state: NodeHealthState,
    pub delay_ms: Option<u32>,
    pub l1: Vec<TargetResult>,
    pub l2: Vec<TargetResult>,
    pub l3: Option<ContentProbeResult>,
    pub failed_targets: Vec<std::string::String>,
    pub checked_at: i64,
    pub l2_checked_at: i64,
    pub consecutive_failures: u32,
    pub samples: Vec<HealthSample>,
}

impl NodeHealthEntry {
    const fn empty(key: HealthKey) -> Self {
        Self {
            key,
            state: NodeHealthState::Unverified,
            delay_ms: None,
            l1: Vec::new(),
            l2: Vec::new(),
            l3: None,
            failed_targets: Vec::new(),
            checked_at: 0,
            l2_checked_at: 0,
            consecutive_failures: 0,
            samples: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct NodeHealthSnapshot {
    pub schema_version: u8,
    pub enabled: bool,
    pub scope: std::string::String,
    pub interval_minutes: u64,
    pub deep_interval_minutes: u64,
    pub verify_on_select: bool,
    pub auto_failover: bool,
    pub profile: Option<std::string::String>,
    pub generation: u64,
    pub running: bool,
    pub updated_at: i64,
    pub entries: Vec<NodeHealthEntry>,
}

#[derive(Clone, Debug, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct NodeVerificationResult {
    pub profile: Option<std::string::String>,
    pub group: std::string::String,
    pub node: std::string::String,
    /// Whether the node was the group's selected node when L3 started.
    pub selected: bool,
    pub state: NodeHealthState,
    pub l1: Vec<TargetResult>,
    pub l3: Option<ContentProbeResult>,
    /// The selection changed while L3 was in flight, so the result was not stored.
    pub discarded: bool,
    pub checked_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RuntimeSettings {
    enabled: bool,
    interval_minutes: u64,
    deep_interval_minutes: u64,
    scope: HealthScope,
    l1_targets: Vec<ProbeTargetSpec>,
    l2_targets: Vec<ProbeTargetSpec>,
    verify_on_select: bool,
    auto_failover: bool,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: DEFAULT_INTERVAL_MINUTES,
            deep_interval_minutes: DEFAULT_DEEP_INTERVAL_MINUTES,
            scope: HealthScope::CurrentGroup,
            l1_targets: default_l1_targets(),
            l2_targets: default_l2_targets(),
            verify_on_select: true,
            auto_failover: false,
        }
    }
}

impl RuntimeSettings {
    fn from_verge(verge: &IVerge) -> Self {
        let configured = verge.node_health_settings.as_ref();
        let legacy_enabled = verge.enable_auto_delay_detection.unwrap_or(false);
        let enabled_unset = configured.and_then(|settings| settings.enabled).is_none();
        let legacy_active = legacy_enabled && enabled_unset;
        if legacy_active {
            log_legacy_migration_once();
        }

        let enabled = configured
            .and_then(|settings| settings.enabled)
            .unwrap_or(legacy_active);

        // The legacy auto delay setting measured the selected node every few minutes.
        let interval_minutes = configured
            .and_then(|settings| settings.interval_minutes)
            .or_else(|| legacy_active.then(|| verge.auto_delay_detection_interval_minutes.unwrap_or(5)))
            .unwrap_or(DEFAULT_INTERVAL_MINUTES)
            .clamp(MIN_INTERVAL_MINUTES, MAX_INTERVAL_MINUTES);

        let scope = match configured.and_then(|settings| settings.scope.as_deref()) {
            Some(raw) => HealthScope::parse(raw).unwrap_or_else(|| {
                logging!(
                    warn,
                    Type::Network,
                    "Unknown node health scope {raw:?}; using current_group"
                );
                HealthScope::CurrentGroup
            }),
            None if legacy_active => HealthScope::CurrentNode,
            None => HealthScope::CurrentGroup,
        };

        let deep_interval_minutes = configured
            .and_then(|settings| settings.deep_interval_minutes)
            .unwrap_or(DEFAULT_DEEP_INTERVAL_MINUTES)
            .clamp(MIN_INTERVAL_MINUTES, MAX_INTERVAL_MINUTES)
            .max(interval_minutes);

        let l1_targets = resolve_targets(
            configured.and_then(|settings| settings.l1_targets.as_ref()),
            DEFAULT_L1_TIMEOUT_MS,
            default_l1_targets,
        );
        let l2_targets = resolve_targets(
            configured.and_then(|settings| settings.l2_targets.as_ref()),
            DEFAULT_L2_TIMEOUT_MS,
            default_l2_targets,
        );

        Self {
            enabled,
            interval_minutes,
            deep_interval_minutes,
            scope,
            l1_targets,
            l2_targets,
            verify_on_select: configured
                .and_then(|settings| settings.verify_on_select)
                .unwrap_or(true),
            auto_failover: configured.and_then(|settings| settings.auto_failover).unwrap_or(false),
        }
    }

    const fn interval(&self) -> Duration {
        Duration::from_secs(self.interval_minutes.saturating_mul(60))
    }
}

fn resolve_targets(
    configured: Option<&Vec<ProbeTarget>>,
    default_timeout_ms: u32,
    defaults: fn() -> Vec<ProbeTargetSpec>,
) -> Vec<ProbeTargetSpec> {
    let Some(configured) = configured else {
        return defaults();
    };
    let targets: Vec<ProbeTargetSpec> = configured
        .iter()
        .filter_map(|target| ProbeTargetSpec::from_config(target, default_timeout_ms))
        .take(MAX_TARGETS)
        .collect();
    if targets.is_empty() {
        if !configured.is_empty() {
            logging!(
                warn,
                Type::Network,
                "No valid node health targets configured; falling back to defaults"
            );
        }
        return defaults();
    }
    targets
}

fn default_l1_targets() -> Vec<ProbeTargetSpec> {
    vec![ProbeTargetSpec {
        url: DEFAULT_L1_URL.into(),
        expected: "204".into(),
        keyword: None,
        timeout_ms: DEFAULT_L1_TIMEOUT_MS,
    }]
}

fn default_l2_targets() -> Vec<ProbeTargetSpec> {
    DEFAULT_L2_TARGETS
        .iter()
        .map(|(url, expected)| ProbeTargetSpec {
            url: (*url).into(),
            expected: (*expected).into(),
            keyword: None,
            timeout_ms: DEFAULT_L2_TIMEOUT_MS,
        })
        .collect()
}

fn log_legacy_migration_once() {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !LOGGED.swap(true, Ordering::AcqRel) {
        logging!(
            info,
            Type::Network,
            "Migrating enable_auto_delay_detection to scheduled node health checks"
        );
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SnapshotStamp {
    profile: std::string::String,
    groups: Vec<std::string::String>,
}

struct ResolvedNode {
    key_node: std::string::String,
    probe_node: std::string::String,
    provider: Option<std::string::String>,
}

struct ProbeSlot {
    node: std::string::String,
    provider: Option<std::string::String>,
    keys: Vec<HealthKey>,
    l2_due: bool,
}

struct CyclePlan {
    profile: std::string::String,
    keys: Vec<HealthKey>,
    view_keys: BTreeSet<HealthKey>,
    slots: Vec<ProbeSlot>,
    l1_targets: Vec<ProbeTargetSpec>,
    l2_targets: Vec<ProbeTargetSpec>,
}

#[derive(Clone, Copy, Debug)]
enum CycleOutcome {
    Completed { succeeded: usize, failed: usize },
    Skipped,
    Stale,
}

impl CycleOutcome {
    const fn activity(self) -> Option<(usize, usize)> {
        match self {
            Self::Completed { succeeded, failed } => Some((succeeded, failed)),
            _ => None,
        }
    }
}

enum ProbePhase {
    Completed { succeeded: usize, failed: usize },
    Stale,
    Failed,
}

#[derive(Serialize)]
struct PersistedHealthRef<'a> {
    schema_version: u8,
    entries: &'a [NodeHealthEntry],
}

#[derive(Deserialize)]
struct PersistedHealth {
    schema_version: u8,
    entries: Vec<NodeHealthEntry>,
}

pub struct NodeHealthManager {
    settings: RwLock<RuntimeSettings>,
    settings_tx: watch::Sender<RuntimeSettings>,
    cache: Mutex<BTreeMap<HealthKey, NodeHealthEntry>>,
    stamp: Mutex<Option<SnapshotStamp>>,
    generation: AtomicU64,
    cycle_running: AtomicBool,
    worker_started: AtomicBool,
    exit: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
    last_persist: AtomicI64,
    persist_lock: AsyncMutex<()>,
    last_snapshot_at: AtomicI64,
    failure_multiplier: AtomicU32,
}

impl NodeHealthManager {
    pub fn global() -> &'static Self {
        static INSTANCE: OnceLock<NodeHealthManager> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            let (settings_tx, _settings_rx) = watch::channel(RuntimeSettings::default());
            Self {
                settings: RwLock::new(RuntimeSettings::default()),
                settings_tx,
                cache: Mutex::new(BTreeMap::new()),
                stamp: Mutex::new(None),
                generation: AtomicU64::new(1),
                cycle_running: AtomicBool::new(false),
                worker_started: AtomicBool::new(false),
                exit: AtomicBool::new(false),
                worker: Mutex::new(None),
                last_persist: AtomicI64::new(0),
                persist_lock: AsyncMutex::new(()),
                last_snapshot_at: AtomicI64::new(0),
                failure_multiplier: AtomicU32::new(1),
            }
        })
    }

    pub async fn init(&self) -> Result<()> {
        if self.worker_started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        let settings = Self::load_settings().await;
        {
            *self.settings.write() = settings.clone();
        }
        // Subscribe before publishing so the worker cannot miss the initial value.
        let rx = self.settings_tx.subscribe();
        self.settings_tx.send_replace(settings);

        match self.load_persisted().await {
            Ok(entries) => {
                let mut cache = self.cache.lock();
                for entry in entries.into_iter().take(CACHE_MAX_ENTRIES) {
                    cache.insert(entry.key.clone(), entry);
                }
                let count = cache.len();
                drop(cache);
                logging!(debug, Type::Network, "Restored {count} node health entries");
            }
            Err(error) => {
                logging!(warn, Type::Network, "Failed to load node health cache: {error:#}");
            }
        }
        self.last_persist.store(now_secs(), Ordering::Release);

        let worker = AsyncHandler::spawn(move || async move {
            Self::global().run_worker(rx).await;
        });
        *self.worker.lock() = Some(worker);
        logging!(info, Type::Network, "Node health manager initialized");
        Ok(())
    }

    pub async fn refresh_settings(&self) -> Result<()> {
        let settings = Self::load_settings().await;
        let changed = {
            let mut current = self.settings.write();
            if *current == settings {
                false
            } else {
                *current = settings.clone();
                true
            }
        };
        if changed {
            self.generation.fetch_add(1, Ordering::AcqRel);
            self.settings_tx.send_replace(settings);
            logging!(debug, Type::Network, "Node health settings changed");
        }
        Ok(())
    }

    /// Invalidates in-flight results after the active profile changes and restarts the schedule.
    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        *self.stamp.lock() = None;
        self.settings_tx.send_replace(self.settings.read().clone());
    }

    pub async fn shutdown(&self) {
        if self.exit.swap(true, Ordering::AcqRel) {
            return;
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.settings_tx.send_replace(self.settings.read().clone());

        let worker = self.worker.lock().take();
        if let Some(mut worker) = worker
            && tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut worker).await.is_err()
        {
            logging!(
                warn,
                Type::Network,
                "Node health worker did not stop within {SHUTDOWN_TIMEOUT:?}; aborting it"
            );
            worker.abort();
        }
        if self.worker_started.load(Ordering::Acquire) {
            self.persist(true).await;
        }
    }

    pub fn trigger_manual(&self, scope: Option<HealthScope>) {
        if self.exit.load(Ordering::Acquire) || self.cycle_running.load(Ordering::Acquire) {
            return;
        }
        AsyncHandler::spawn(move || async move {
            let outcome = Self::global().run_cycle(scope).await;
            Self::global().record_outcome(outcome);
        });
    }

    pub async fn snapshot(&self) -> NodeHealthSnapshot {
        let settings = self.settings.read().clone();
        let profiles = Config::profiles().await.data_arc();
        let profile = profiles.current.as_ref().map(|uid| profile_identity(&profiles, uid));
        let now = now_secs();
        let interval_secs = settings.interval_minutes.saturating_mul(60) as i64;
        let running = self.cycle_running.load(Ordering::Acquire);

        let entries = {
            let cache = self.cache.lock();
            let mut entries: Vec<NodeHealthEntry> = cache
                .values()
                .filter(|entry| profile.as_ref().is_none_or(|profile| &entry.key.profile == profile))
                .cloned()
                .map(|mut entry| {
                    // A cycle that was aborted mid-flight must not leave nodes marked as testing.
                    entry.state = if entry.state == NodeHealthState::Testing && !running {
                        NodeHealthState::Unverified
                    } else {
                        effective_state(&entry, now, interval_secs)
                    };
                    entry
                })
                .collect();
            drop(cache);
            entries.sort_by(|left, right| (&left.key.group, &left.key.node).cmp(&(&right.key.group, &right.key.node)));
            entries
        };

        NodeHealthSnapshot {
            schema_version: 1,
            enabled: settings.enabled,
            scope: settings.scope.as_str().into(),
            interval_minutes: settings.interval_minutes,
            deep_interval_minutes: settings.deep_interval_minutes,
            verify_on_select: settings.verify_on_select,
            auto_failover: settings.auto_failover,
            profile,
            generation: self.generation.load(Ordering::Acquire),
            running,
            updated_at: self.last_snapshot_at.load(Ordering::Acquire),
            entries,
        }
    }

    pub async fn verify_node(
        &self,
        profile_uid: Option<&str>,
        group: &str,
        node: &str,
    ) -> Result<NodeVerificationResult> {
        let settings = self.settings.read().clone();
        let profiles = Config::profiles().await.data_arc();
        let uid = profile_uid
            .map(str::to_owned)
            .or_else(|| profiles.current.as_ref().map(|uid| uid.to_string()))
            .context("no active profile to verify a node against")?;
        let profile = profile_identity(&profiles, &uid);

        let view = crate::cmd::proxy::proxy_view(Some(Duration::from_secs(10)))
            .await
            .map_err(|failure| anyhow::anyhow!("{failure}"))?;
        let group_view = find_group(&view, group).with_context(|| format!("proxy group {group:?} was not found"))?;
        let resolved = group_view
            .members
            .iter()
            .find_map(|member| {
                match member {
                    ProxyMemberRef::Node { name, .. } if name.as_str() == node => {}
                    _ => return None,
                }
                resolve_node(&view, member)
            })
            .with_context(|| format!("node {node:?} is not a member of group {group:?}"))?;
        let selected = group_view.now.as_deref() == Some(resolved.key_node.as_str());
        let key = HealthKey {
            profile: profile.clone(),
            group: group.to_owned(),
            node: resolved.key_node.clone(),
        };

        let client = ControllerClient::from_config().await?;
        let now = now_secs();
        let mut l1 = Vec::with_capacity(settings.l1_targets.len());
        for target in &settings.l1_targets {
            let result = client
                .probe(&resolved.probe_node, resolved.provider.as_deref(), target)
                .await;
            l1.push(TargetResult::new(target, &result, now));
        }

        // Content validation only makes sense while this node is the one the local proxy uses.
        // The first L2 target is the real-site URL used for that check.
        let l3 = if selected {
            match settings.l2_targets.first().and_then(|target| {
                ContentProbeSpec::new(
                    &target.url,
                    &target.expected,
                    target.keyword.as_deref(),
                    target.timeout_ms,
                )
            }) {
                Some(spec) => Some(probe_content(&spec).await),
                None => None,
            }
        } else {
            None
        };

        let mut discarded = false;
        if selected {
            match crate::cmd::proxy::proxy_view(Some(Duration::from_secs(5))).await {
                Ok(after) => {
                    let still_selected = find_group(&after, group)
                        .and_then(|group| group.now.as_deref())
                        .is_some_and(|selected| selected == resolved.key_node.as_str());
                    discarded = !still_selected;
                }
                Err(error) => {
                    logging!(
                        debug,
                        Type::Network,
                        "Could not confirm the selected node after verification: {error}"
                    );
                }
            }
        }

        let entry = self.merge_verification(&key, l1.clone(), l3.clone(), discarded);
        self.emit();

        Ok(NodeVerificationResult {
            profile: Some(profile),
            group: group.to_owned(),
            node: resolved.key_node,
            selected,
            state: entry.state,
            l1,
            l3,
            discarded,
            checked_at: entry.checked_at,
        })
    }

    fn merge_verification(
        &self,
        key: &HealthKey,
        l1: Vec<TargetResult>,
        l3: Option<ContentProbeResult>,
        discarded: bool,
    ) -> NodeHealthEntry {
        let now = now_secs();
        let mut cache = self.cache.lock();
        if discarded {
            return cache
                .get(key)
                .cloned()
                .unwrap_or_else(|| NodeHealthEntry::empty(key.clone()));
        }

        let previous = cache.get(key).cloned();
        let mut samples = previous.as_ref().map_or_else(Vec::new, |entry| entry.samples.clone());
        let aggregate_ok = !l1.is_empty() && l1.iter().all(|result| result.ok);
        let aggregate_delay = l1
            .iter()
            .filter(|result| result.ok)
            .filter_map(|result| result.delay_ms)
            .min();
        push_sample(
            &mut samples,
            HealthSample {
                at: now,
                ok: aggregate_ok,
                delay_ms: aggregate_delay,
            },
        );
        let l2 = previous.as_ref().map_or_else(Vec::new, |entry| entry.l2.clone());
        let (mut state, mut failed_targets) = compute_health_state(&l1, &l2, &samples);
        if let Some(content) = l3.as_ref()
            && !content.ok
        {
            if !failed_targets.contains(&content.url) {
                failed_targets.push(content.url.clone());
            }
            if !matches!(state, NodeHealthState::Unusable) {
                state = NodeHealthState::SiteBlocked;
            }
        }
        let entry = NodeHealthEntry {
            key: key.clone(),
            state,
            delay_ms: aggregate_delay,
            l1,
            l2,
            l3: l3.or_else(|| previous.as_ref().and_then(|entry| entry.l3.clone())),
            failed_targets,
            checked_at: now,
            l2_checked_at: previous.as_ref().map_or(0, |entry| entry.l2_checked_at),
            consecutive_failures: if aggregate_ok {
                0
            } else {
                previous
                    .as_ref()
                    .map_or(0, |entry| entry.consecutive_failures)
                    .saturating_add(1)
            },
            samples,
        };
        cache.insert(key.clone(), entry.clone());
        trim_cache(&mut cache);
        drop(cache);
        entry
    }

    async fn run_worker(&self, mut rx: watch::Receiver<RuntimeSettings>) {
        self.wait_until_ready().await;
        let mut next_cycle_at: Option<Instant> = None;

        loop {
            if self.exit.load(Ordering::Acquire) {
                break;
            }
            let settings = rx.borrow().clone();
            if !settings.enabled {
                next_cycle_at = None;
                tokio::select! {
                    changed = rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                }
                continue;
            }

            let now = Instant::now();
            let due = next_cycle_at.unwrap_or(now + STARTUP_DELAY);
            if let Some(wait) = due.checked_duration_since(now).filter(|wait| !wait.is_zero()) {
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    changed = rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        // Any settings change restarts the schedule; a disable is picked up above.
                        next_cycle_at = None;
                        continue;
                    }
                }
            }
            if self.exit.load(Ordering::Acquire) {
                break;
            }

            let outcome = self.run_cycle(None).await;
            self.record_outcome(outcome);
            next_cycle_at = Some(Instant::now() + self.current_interval());
        }
    }

    async fn wait_until_ready(&self) {
        let _ = tokio::time::timeout(READY_WAIT, async {
            while !is_resolve_done() {
                if self.exit.load(Ordering::Acquire) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        })
        .await;
    }

    fn current_interval(&self) -> Duration {
        let interval = self.settings.read().interval();
        interval.saturating_mul(self.failure_multiplier.load(Ordering::Acquire).max(1))
    }

    fn record_outcome(&self, outcome: CycleOutcome) {
        let Some((succeeded, failed)) = outcome.activity() else {
            return;
        };
        if succeeded == 0 && failed > 0 {
            let current = self.failure_multiplier.load(Ordering::Acquire).max(1);
            self.failure_multiplier.store(
                (current.saturating_mul(2)).min(MAX_BACKOFF_MULTIPLIER),
                Ordering::Release,
            );
        } else if succeeded > 0 {
            self.failure_multiplier.store(1, Ordering::Release);
        }
    }

    async fn run_cycle(&self, scope_override: Option<HealthScope>) -> CycleOutcome {
        if self.exit.load(Ordering::Acquire) {
            return CycleOutcome::Skipped;
        }
        if self.cycle_running.swap(true, Ordering::AcqRel) {
            logging!(debug, Type::Network, "Node health cycle already running; skipping");
            return CycleOutcome::Skipped;
        }
        let _running = scopeguard::guard((), |_| self.cycle_running.store(false, Ordering::Release));

        if matches!(*CoreManager::global().get_running_mode(), RunningMode::NotRunning) {
            logging!(debug, Type::Network, "Core is not running; skipping node health cycle");
            return CycleOutcome::Skipped;
        }

        let settings = self.settings.read().clone();
        let Some(plan) = self
            .build_plan(scope_override.unwrap_or(settings.scope), &settings)
            .await
        else {
            return CycleOutcome::Skipped;
        };
        let generation = self.generation.load(Ordering::Acquire);
        let testing = self.mark_testing(&plan.keys);
        self.emit();

        let phase = self.execute_probes(&plan, generation).await;
        let outcome = match phase {
            ProbePhase::Completed { succeeded, failed } => CycleOutcome::Completed { succeeded, failed },
            ProbePhase::Stale => {
                logging!(
                    debug,
                    Type::Network,
                    "Node health cycle became stale; discarding results"
                );
                CycleOutcome::Stale
            }
            ProbePhase::Failed => CycleOutcome::Skipped,
        };

        // Partial cycles may leave slots without a complete L1 result; restore any remaining
        // Testing markers so a truncated cycle does not erase the previous state.
        self.restore_testing(&testing);
        self.last_snapshot_at.store(now_secs(), Ordering::Release);
        self.emit();
        if matches!(outcome, CycleOutcome::Completed { .. }) {
            self.persist(false).await;
        }
        outcome
    }

    async fn build_plan(&self, scope: HealthScope, settings: &RuntimeSettings) -> Option<CyclePlan> {
        let view = match tokio::time::timeout(SNAPSHOT_TIMEOUT, crate::cmd::proxy::proxy_view(None)).await {
            Ok(Ok(view)) => view,
            Ok(Err(error)) => {
                logging!(debug, Type::Network, "Failed to read proxies for node health: {error}");
                return None;
            }
            Err(_) => {
                logging!(warn, Type::Network, "Timed out reading proxies for node health");
                return None;
            }
        };

        let profiles = Config::profiles().await.data_arc();
        let Some(uid) = profiles.current.as_ref() else {
            logging!(debug, Type::Network, "No active profile; skipping node health cycle");
            return None;
        };
        let profile = profile_identity(&profiles, uid);

        let groups: Vec<&ProxyGroupView> = view.groups.iter().chain(view.global.iter()).take(MAX_GROUPS).collect();
        let mut view_keys = BTreeSet::new();
        for group in &groups {
            for member in &group.members {
                if let Some(resolved) = resolve_node(&view, member) {
                    view_keys.insert(HealthKey {
                        profile: profile.clone(),
                        group: group.name.as_str().to_owned(),
                        node: resolved.key_node,
                    });
                }
            }
        }

        let scoped_groups: Vec<&ProxyGroupView> = match scope {
            // The backend view covers the active profile only: `all_groups` differs by also
            // including the GLOBAL group.
            HealthScope::AllGroups => groups.clone(),
            HealthScope::CurrentGroup => view.groups.iter().take(MAX_GROUPS).collect(),
            HealthScope::CurrentNode => groups.clone(),
        };

        let now = now_secs();
        let deep_interval_secs = settings.deep_interval_minutes.saturating_mul(60) as i64;
        let mut slot_index: BTreeMap<(Option<std::string::String>, std::string::String), usize> = BTreeMap::new();
        let mut slots: Vec<ProbeSlot> = Vec::new();
        let mut keys = Vec::new();
        let cache = self.cache.lock();

        for group in scoped_groups {
            for member in &group.members {
                let Some(resolved) = resolve_node(&view, member) else {
                    continue;
                };
                if scope == HealthScope::CurrentNode && group.now.as_deref() != Some(resolved.key_node.as_str()) {
                    continue;
                }
                if slots.len() >= MAX_NODES_PER_CYCLE {
                    break;
                }
                let key = HealthKey {
                    profile: profile.clone(),
                    group: group.name.as_str().to_owned(),
                    node: resolved.key_node.clone(),
                };
                let slot_id = (resolved.provider.clone(), resolved.probe_node.clone());
                let index = *slot_index.entry(slot_id).or_insert_with(|| {
                    let index = slots.len();
                    slots.push(ProbeSlot {
                        node: resolved.probe_node.clone(),
                        provider: resolved.provider.clone(),
                        keys: Vec::new(),
                        l2_due: false,
                    });
                    index
                });
                slots[index].keys.push(key.clone());
                if cache.get(&key).is_none_or(|entry| {
                    entry.l2_checked_at <= 0 || now.saturating_sub(entry.l2_checked_at) >= deep_interval_secs
                }) {
                    slots[index].l2_due = true;
                }
                keys.push(key);
            }
        }
        // Start with the least recently checked slots so a cycle truncated by the deadline does
        // not starve the tail of a large profile on every run.
        slots.sort_by_key(|slot| {
            slot.keys
                .iter()
                .map(|key| cache.get(key).map_or(0, |entry| entry.checked_at))
                .min()
                .unwrap_or(0)
        });
        drop(cache);

        if slots.is_empty() {
            logging!(debug, Type::Network, "Node health cycle found no nodes to test");
            return None;
        }

        let group_names = groups.iter().map(|group| group.name.as_str().to_owned()).collect();
        self.update_stamp(SnapshotStamp {
            profile: profile.clone(),
            groups: group_names,
        });

        Some(CyclePlan {
            profile,
            keys,
            view_keys,
            slots,
            l1_targets: settings.l1_targets.clone(),
            l2_targets: settings.l2_targets.clone(),
        })
    }

    fn update_stamp(&self, stamp: SnapshotStamp) {
        let mut current = self.stamp.lock();
        if current.as_ref() != Some(&stamp) {
            *current = Some(stamp);
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
        drop(current);
    }

    fn mark_testing(&self, keys: &[HealthKey]) -> Vec<(HealthKey, NodeHealthState)> {
        let mut cache = self.cache.lock();
        let mut previous = Vec::with_capacity(keys.len());
        for key in keys {
            let entry = cache
                .entry(key.clone())
                .or_insert_with(|| NodeHealthEntry::empty(key.clone()));
            previous.push((key.clone(), entry.state));
            entry.state = NodeHealthState::Testing;
        }
        drop(cache);
        previous
    }

    fn restore_testing(&self, previous: &[(HealthKey, NodeHealthState)]) {
        let mut cache = self.cache.lock();
        for (key, state) in previous {
            if let Some(entry) = cache.get_mut(key)
                && entry.state == NodeHealthState::Testing
            {
                entry.state = *state;
            }
        }
    }

    async fn execute_probes(&self, plan: &CyclePlan, generation: u64) -> ProbePhase {
        let client = match ControllerClient::from_config().await {
            Ok(client) => client,
            Err(error) => {
                logging!(
                    warn,
                    Type::Network,
                    "Failed to build node health controller client: {error:#}"
                );
                return ProbePhase::Failed;
            }
        };

        let deadline = Instant::now() + CYCLE_TIMEOUT;
        let mut l1: Vec<Vec<Option<ProbeResult>>> = vec![vec![None; plan.l1_targets.len()]; plan.slots.len()];
        match tokio::time::timeout(
            remaining(deadline),
            self.probe_level(&client, plan, generation, &plan.l1_targets, L1_CONCURRENCY, &mut l1),
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => return ProbePhase::Stale,
            Err(_) => logging!(
                warn,
                Type::Network,
                "Node health L1 probes hit the {CYCLE_TIMEOUT:?} cycle limit; keeping completed results"
            ),
        }

        let mut l2: Vec<Option<Vec<Option<ProbeResult>>>> = plan
            .slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let l1_passed =
                    l1[index].iter().all(Option::is_some) && l1[index].iter().flatten().all(|result| result.ok);
                (slot.l2_due && l1_passed).then(|| vec![None; plan.l2_targets.len()])
            })
            .collect();
        if l2.iter().any(Option::is_some) {
            match tokio::time::timeout(remaining(deadline), self.probe_l2(&client, plan, generation, &mut l2)).await {
                Ok(true) => {}
                Ok(false) => return ProbePhase::Stale,
                Err(_) => logging!(
                    warn,
                    Type::Network,
                    "Node health L2 probes hit the {CYCLE_TIMEOUT:?} cycle limit; keeping completed results"
                ),
            }
        }

        if self.generation.load(Ordering::Acquire) != generation {
            return ProbePhase::Stale;
        }
        let (succeeded, failed) = self.apply_results(plan, generation, &l1, &l2);
        ProbePhase::Completed { succeeded, failed }
    }

    async fn probe_level(
        &self,
        client: &ControllerClient,
        plan: &CyclePlan,
        generation: u64,
        targets: &[ProbeTargetSpec],
        concurrency: usize,
        results: &mut [Vec<Option<ProbeResult>>],
    ) -> bool {
        let slot_count = plan.slots.len();
        let target_count = targets.len();
        let work = (0..slot_count)
            .flat_map(move |slot_index| (0..target_count).map(move |target_index| (slot_index, target_index)));
        let mut probes = stream::iter(work.map(|(slot_index, target_index)| {
            let slot = &plan.slots[slot_index];
            let target = &targets[target_index];
            async move {
                (
                    slot_index,
                    target_index,
                    client.probe(&slot.node, slot.provider.as_deref(), target).await,
                )
            }
        }))
        .buffer_unordered(concurrency);

        while let Some((slot_index, target_index, result)) = probes.next().await {
            if self.generation.load(Ordering::Acquire) != generation || self.exit.load(Ordering::Acquire) {
                return false;
            }
            results[slot_index][target_index] = Some(result);
        }
        true
    }

    // The slot list is collected before streaming so the immutable borrow of `results` used for
    // the due check ends before results are written back.
    #[allow(clippy::needless_collect)]
    async fn probe_l2(
        &self,
        client: &ControllerClient,
        plan: &CyclePlan,
        generation: u64,
        results: &mut [Option<Vec<Option<ProbeResult>>>],
    ) -> bool {
        // Collected before the stream starts: it guards the mutable borrow of `results` below.
        let due_slots: Vec<usize> = plan
            .slots
            .iter()
            .enumerate()
            .filter(|(index, _)| results[*index].is_some())
            .map(|(index, _)| index)
            .collect();
        let target_count = plan.l2_targets.len();
        let work = due_slots
            .into_iter()
            .flat_map(move |slot_index| (0..target_count).map(move |target_index| (slot_index, target_index)));
        let mut probes = stream::iter(work.map(|(slot_index, target_index)| {
            let slot = &plan.slots[slot_index];
            let target = &plan.l2_targets[target_index];
            async move {
                (
                    slot_index,
                    target_index,
                    client.probe(&slot.node, slot.provider.as_deref(), target).await,
                )
            }
        }))
        .buffer_unordered(L2_CONCURRENCY);

        while let Some((slot_index, target_index, result)) = probes.next().await {
            if self.generation.load(Ordering::Acquire) != generation || self.exit.load(Ordering::Acquire) {
                return false;
            }
            if let Some(slot_results) = results[slot_index].as_mut() {
                slot_results[target_index] = Some(result);
            }
        }
        true
    }

    #[allow(clippy::cognitive_complexity)]
    fn apply_results(
        &self,
        plan: &CyclePlan,
        generation: u64,
        l1: &[Vec<Option<ProbeResult>>],
        l2: &[Option<Vec<Option<ProbeResult>>>],
    ) -> (usize, usize) {
        if self.generation.load(Ordering::Acquire) != generation {
            return (0, 0);
        }
        let now = now_secs();
        let mut cache = self.cache.lock();
        let mut succeeded = 0usize;
        let mut failed = 0usize;

        for (slot_index, slot) in plan.slots.iter().enumerate() {
            let mut l1_results = Vec::with_capacity(plan.l1_targets.len());
            let mut l1_complete = true;
            for (target_index, target) in plan.l1_targets.iter().enumerate() {
                let result = l1
                    .get(slot_index)
                    .and_then(|results| results.get(target_index))
                    .and_then(Option::as_ref);
                if let Some(result) = result {
                    if result.ok {
                        succeeded += 1;
                    } else {
                        failed += 1;
                    }
                    l1_results.push(TargetResult::new(target, result, now));
                } else {
                    // A cycle cut short by its deadline keeps the previous state for this node.
                    l1_complete = false;
                }
            }
            if !l1_complete {
                continue;
            }

            let l2_ran = l2.get(slot_index).is_some_and(|results| {
                results
                    .as_ref()
                    .is_some_and(|results| results.iter().all(Option::is_some))
            });
            let mut l2_results = Vec::new();
            if let Some(Some(results)) = l2.get(slot_index) {
                for (target_index, target) in plan.l2_targets.iter().enumerate() {
                    let result = results.get(target_index).and_then(Option::as_ref);
                    if let Some(result) = result {
                        if result.ok {
                            succeeded += 1;
                        } else {
                            failed += 1;
                        }
                        l2_results.push(TargetResult::new(target, result, now));
                    }
                }
            }

            let aggregate_ok = !l1_results.is_empty() && l1_results.iter().all(|result| result.ok);
            let aggregate_delay = l1_results
                .iter()
                .filter(|result| result.ok)
                .filter_map(|result| result.delay_ms)
                .min();

            for key in &slot.keys {
                let previous = cache.get(key).cloned();
                let effective_l2 = if l2_ran {
                    l2_results.clone()
                } else {
                    previous.as_ref().map_or_else(Vec::new, |entry| entry.l2.clone())
                };
                let mut samples = previous.as_ref().map_or_else(Vec::new, |entry| entry.samples.clone());
                push_sample(
                    &mut samples,
                    HealthSample {
                        at: now,
                        ok: aggregate_ok,
                        delay_ms: aggregate_delay,
                    },
                );
                let (state, failed_targets) = compute_health_state(&l1_results, &effective_l2, &samples);
                let entry = NodeHealthEntry {
                    key: key.clone(),
                    state,
                    delay_ms: aggregate_delay,
                    l1: l1_results.clone(),
                    l2: effective_l2,
                    l3: previous.as_ref().and_then(|entry| entry.l3.clone()),
                    failed_targets,
                    checked_at: now,
                    l2_checked_at: if l2_ran {
                        now
                    } else {
                        previous.as_ref().map_or(0, |entry| entry.l2_checked_at)
                    },
                    consecutive_failures: if aggregate_ok {
                        0
                    } else {
                        previous
                            .as_ref()
                            .map_or(0, |entry| entry.consecutive_failures)
                            .saturating_add(1)
                    },
                    samples,
                };
                cache.insert(key.clone(), entry);
            }
        }

        prune_cache(&mut cache, &plan.profile, &plan.view_keys);
        drop(cache);
        (succeeded, failed)
    }

    async fn load_settings() -> RuntimeSettings {
        let verge = Config::verge().await;
        RuntimeSettings::from_verge(&verge.latest_arc())
    }

    async fn load_persisted(&self) -> Result<Vec<NodeHealthEntry>> {
        let path = dirs::app_home_dir()?.join(NODE_HEALTH_FILE);
        let metadata = match tokio::fs::metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        if metadata.len() > MAX_PERSIST_BYTES as u64 {
            logging!(
                warn,
                Type::Network,
                "Node health cache file is larger than {MAX_PERSIST_BYTES} bytes; starting empty"
            );
            return Ok(Vec::new());
        }
        let content = tokio::fs::read(&path).await?;
        match serde_json::from_slice::<PersistedHealth>(&content) {
            Ok(persisted) if persisted.schema_version == 1 => {
                Ok(persisted.entries.into_iter().take(CACHE_MAX_ENTRIES).collect())
            }
            Ok(persisted) => {
                logging!(
                    warn,
                    Type::Network,
                    "Unsupported node health cache schema {}; starting empty",
                    persisted.schema_version
                );
                Ok(Vec::new())
            }
            Err(error) => {
                logging!(
                    warn,
                    Type::Network,
                    "Corrupt node health cache; starting empty: {error}"
                );
                Ok(Vec::new())
            }
        }
    }

    async fn persist(&self, force: bool) {
        let _persist_guard = self.persist_lock.lock().await;
        let now = now_secs();
        let last = self.last_persist.load(Ordering::Acquire);
        if !force && now.saturating_sub(last) < PERSIST_INTERVAL_SECS {
            return;
        }
        if self
            .last_persist
            .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        let mut entries: Vec<NodeHealthEntry> = {
            let cache = self.cache.lock();
            cache.values().cloned().collect()
        };
        entries.sort_by_key(|entry| entry.checked_at);
        let Ok(mut bytes) = serde_json::to_vec(&PersistedHealthRef {
            schema_version: 1,
            entries: &entries,
        }) else {
            logging!(warn, Type::Network, "Failed to serialize the node health cache");
            return;
        };
        while bytes.len() > MAX_PERSIST_BYTES && entries.len() > 1 {
            let drop_count = (entries.len() / 10).max(1);
            entries.drain(..drop_count);
            match serde_json::to_vec(&PersistedHealthRef {
                schema_version: 1,
                entries: &entries,
            }) {
                Ok(serialized) => bytes = serialized,
                Err(error) => {
                    logging!(
                        warn,
                        Type::Network,
                        "Failed to serialize the node health cache: {error}"
                    );
                    return;
                }
            }
        }
        if bytes.len() > MAX_PERSIST_BYTES {
            logging!(
                warn,
                Type::Network,
                "Node health cache exceeds {MAX_PERSIST_BYTES} bytes; skipping persistence"
            );
            return;
        }

        let Ok(home) = dirs::app_home_dir() else {
            return;
        };
        let path = home.join(NODE_HEALTH_FILE);
        let tmp = home.join(format!("{NODE_HEALTH_FILE}.tmp"));
        if let Err(error) = tokio::fs::write(&tmp, &bytes).await {
            logging!(warn, Type::Network, "Failed to write the node health cache: {error}");
            return;
        }
        if let Err(error) = tokio::fs::rename(&tmp, &path).await {
            logging!(warn, Type::Network, "Failed to replace the node health cache: {error}");
            let _ = tokio::fs::remove_file(&tmp).await;
        }
    }

    fn emit(&self) {
        if crate::APP_HANDLE.get().is_none() {
            return;
        }
        Handle::notify_node_health_updated(self.generation.load(Ordering::Acquire));
    }
}

fn find_group<'a>(view: &'a ProxyViewV1, name: &str) -> Option<&'a ProxyGroupView> {
    view.groups
        .iter()
        .chain(view.global.iter())
        .find(|group| group.name.as_str() == name)
}

fn resolve_node(view: &ProxyViewV1, member: &ProxyMemberRef) -> Option<ResolvedNode> {
    let ProxyMemberRef::Node { name, record_id } = member else {
        return None;
    };
    match view.records.get(record_id).map(|record| &record.source) {
        Some(ProxyNodeSource::Provider {
            provider_name,
            proxy_name,
        }) => Some(ResolvedNode {
            key_node: name.as_str().to_owned(),
            probe_node: proxy_name.as_str().to_owned(),
            provider: Some(provider_name.as_str().to_owned()),
        }),
        _ => Some(ResolvedNode {
            key_node: name.as_str().to_owned(),
            probe_node: name.as_str().to_owned(),
            provider: None,
        }),
    }
}

fn push_sample(samples: &mut Vec<HealthSample>, sample: HealthSample) {
    samples.push(sample);
    let excess = samples.len().saturating_sub(SAMPLE_LIMIT);
    if excess > 0 {
        samples.drain(..excess);
    }
}

fn is_degraded(samples: &[HealthSample], l1: &[TargetResult]) -> bool {
    let delays: Vec<u32> = samples.iter().filter_map(|sample| sample.delay_ms).collect();
    if delays.len() >= 3 {
        let max = delays.iter().max().copied().unwrap_or_default();
        let min = delays.iter().min().copied().unwrap_or_default();
        if max.saturating_sub(min) > DEGRADED_JITTER_MS {
            return true;
        }
    }
    if samples.len() >= 3 {
        let successes = samples.iter().filter(|sample| sample.ok).count();
        // Fewer than 80% successful samples.
        if successes.saturating_mul(5) < samples.len().saturating_mul(4) {
            return true;
        }
    }
    l1.iter()
        .filter(|result| result.ok)
        .filter_map(|result| result.delay_ms)
        .min()
        .is_some_and(|delay| delay > DEGRADED_LATENCY_MS)
}

pub(crate) fn compute_health_state(
    l1: &[TargetResult],
    l2: &[TargetResult],
    samples: &[HealthSample],
) -> (NodeHealthState, Vec<std::string::String>) {
    let mut failed_targets: Vec<std::string::String> = l1
        .iter()
        .filter(|result| !result.ok)
        .map(|result| result.url.clone())
        .collect();
    failed_targets.extend(l2.iter().filter(|result| !result.ok).map(|result| result.url.clone()));

    if l1.is_empty() {
        return (NodeHealthState::Unverified, failed_targets);
    }
    let l1_passed = l1.iter().filter(|result| result.ok).count();
    if l1_passed == 0 {
        return (NodeHealthState::Unusable, failed_targets);
    }
    if l2.iter().any(|result| !result.ok) {
        return (NodeHealthState::SiteBlocked, failed_targets);
    }
    if l1_passed < l1.len() || is_degraded(samples, l1) {
        return (NodeHealthState::Degraded, failed_targets);
    }
    (NodeHealthState::Healthy, failed_targets)
}

fn effective_state(entry: &NodeHealthEntry, now: i64, interval_secs: i64) -> NodeHealthState {
    if entry.state == NodeHealthState::Testing {
        return entry.state;
    }
    if entry.checked_at <= 0 || now.saturating_sub(entry.checked_at) > interval_secs.saturating_mul(2) {
        return NodeHealthState::Unverified;
    }
    entry.state
}

fn prune_cache(cache: &mut BTreeMap<HealthKey, NodeHealthEntry>, profile: &str, view_keys: &BTreeSet<HealthKey>) {
    cache.retain(|key, _| key.profile == profile && view_keys.contains(key));
    trim_cache(cache);
}

fn trim_cache(cache: &mut BTreeMap<HealthKey, NodeHealthEntry>) {
    while cache.len() > CACHE_MAX_ENTRIES {
        let oldest = cache
            .iter()
            .min_by_key(|(_, entry)| entry.checked_at)
            .map(|(key, _)| key.clone());
        let Some(oldest) = oldest else {
            break;
        };
        cache.remove(&oldest);
    }
}

fn profile_identity(profiles: &IProfiles, uid: &str) -> std::string::String {
    profile_sync_id(profiles, uid)
        .map(|sync_id| sync_id.to_string())
        .unwrap_or_else(|| uid.to_owned())
}

fn now_secs() -> i64 {
    Utc::now().timestamp()
}

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(url: &str, ok: bool, delay_ms: Option<u32>) -> TargetResult {
        TargetResult {
            url: url.into(),
            ok,
            delay_ms,
            error: (!ok).then(|| "failed".to_owned()),
            checked_at: 1_000,
        }
    }

    fn sample(ok: bool, delay_ms: Option<u32>) -> HealthSample {
        HealthSample {
            at: 1_000,
            ok,
            delay_ms,
        }
    }

    #[test]
    fn all_l1_failures_are_unusable() {
        let l1 = vec![target("a", false, None), target("b", false, None)];
        let (state, failed) = compute_health_state(&l1, &[], &[sample(false, None)]);
        assert_eq!(state, NodeHealthState::Unusable);
        assert_eq!(failed, ["a", "b"]);
    }

    #[test]
    fn a_failed_l2_target_is_site_blocked() {
        let l1 = vec![target("a", true, Some(40))];
        let l2 = vec![target("site", true, Some(120)), target("other", false, None)];
        let (state, failed) = compute_health_state(&l1, &l2, &[sample(true, Some(40))]);
        assert_eq!(state, NodeHealthState::SiteBlocked);
        assert_eq!(failed, ["other"]);
    }

    #[test]
    fn passing_targets_are_healthy_until_samples_show_degradation() {
        let l1 = vec![target("a", true, Some(50))];
        let healthy = vec![sample(true, Some(50)), sample(true, Some(48)), sample(true, Some(52))];
        assert_eq!(compute_health_state(&l1, &[], &healthy).0, NodeHealthState::Healthy);

        let jittery = vec![sample(true, Some(40)), sample(true, Some(50)), sample(true, Some(260))];
        assert_eq!(compute_health_state(&l1, &[], &jittery).0, NodeHealthState::Degraded);

        let unreliable = vec![sample(true, Some(50)), sample(false, None), sample(false, None)];
        assert_eq!(compute_health_state(&l1, &[], &unreliable).0, NodeHealthState::Degraded);

        let slow = vec![target("a", true, Some(DEGRADED_LATENCY_MS + 1))];
        assert_eq!(
            compute_health_state(&slow, &[], &[sample(true, Some(DEGRADED_LATENCY_MS + 1))]).0,
            NodeHealthState::Degraded
        );
    }

    #[test]
    fn partial_l1_failure_is_degraded_even_when_l2_passes() {
        let l1 = vec![target("a", true, Some(40)), target("b", false, None)];
        let (state, failed) = compute_health_state(&l1, &[], &[sample(false, Some(40))]);
        assert_eq!(state, NodeHealthState::Degraded);
        assert_eq!(failed, ["b"]);
    }

    #[test]
    fn staleness_turns_a_previous_state_into_unverified() {
        let key = HealthKey {
            profile: "profile".into(),
            group: "group".into(),
            node: "node".into(),
        };
        let mut entry = NodeHealthEntry::empty(key);
        entry.state = NodeHealthState::Healthy;
        entry.checked_at = 1_000;

        assert_eq!(
            effective_state(&entry, 1_000 + 10 * 60, 10 * 60),
            NodeHealthState::Healthy
        );
        assert_eq!(
            effective_state(&entry, 1_000 + 21 * 60, 10 * 60),
            NodeHealthState::Unverified
        );
        assert_eq!(
            effective_state(
                &NodeHealthEntry {
                    state: NodeHealthState::Testing,
                    checked_at: 0,
                    ..NodeHealthEntry::empty(HealthKey {
                        profile: "p".into(),
                        group: "g".into(),
                        node: "n".into(),
                    })
                },
                10_000,
                600
            ),
            NodeHealthState::Testing
        );
    }

    #[test]
    fn scope_parsing_accepts_wire_and_legacy_names() {
        assert_eq!(HealthScope::parse("current_node"), Some(HealthScope::CurrentNode));
        assert_eq!(HealthScope::parse("current-group"), Some(HealthScope::CurrentGroup));
        assert_eq!(HealthScope::parse(" ALL_GROUPS "), Some(HealthScope::AllGroups));
        assert_eq!(HealthScope::parse("bogus"), None);
    }

    #[test]
    fn legacy_auto_delay_settings_migrate_to_selected_node_testing() {
        let verge = IVerge {
            enable_auto_delay_detection: Some(true),
            auto_delay_detection_interval_minutes: Some(7),
            ..IVerge::default()
        };
        let settings = RuntimeSettings::from_verge(&verge);
        assert!(settings.enabled);
        assert_eq!(settings.scope, HealthScope::CurrentNode);
        assert_eq!(settings.interval_minutes, 7);
        assert_eq!(settings.l1_targets.len(), 1);
        assert!(!settings.l2_targets.is_empty());
    }

    #[test]
    fn explicit_health_settings_win_over_legacy_fields() {
        let settings = crate::config::NodeHealthSettings {
            enabled: Some(false),
            interval_minutes: Some(30),
            ..crate::config::NodeHealthSettings::default()
        };
        let verge = IVerge {
            enable_auto_delay_detection: Some(true),
            auto_delay_detection_interval_minutes: Some(5),
            node_health_settings: Some(settings),
            ..IVerge::default()
        };
        let settings = RuntimeSettings::from_verge(&verge);
        assert!(!settings.enabled);
        assert_eq!(settings.scope, HealthScope::CurrentGroup);
        assert_eq!(settings.interval_minutes, 30);
    }

    #[test]
    fn invalid_targets_and_intervals_fall_back_to_defaults() {
        let health = crate::config::NodeHealthSettings {
            interval_minutes: Some(0),
            deep_interval_minutes: Some(0),
            scope: Some("not-a-scope".into()),
            l1_targets: Some(vec![crate::config::ProbeTarget {
                url: "ftp://example.com".into(),
                expected: "200".into(),
                keyword: None,
                timeout_ms: None,
            }]),
            ..crate::config::NodeHealthSettings::default()
        };
        let verge = IVerge {
            node_health_settings: Some(health),
            ..IVerge::default()
        };
        let settings = RuntimeSettings::from_verge(&verge);
        assert_eq!(settings.interval_minutes, MIN_INTERVAL_MINUTES);
        assert_eq!(settings.deep_interval_minutes, MIN_INTERVAL_MINUTES);
        assert_eq!(settings.scope, HealthScope::CurrentGroup);
        assert_eq!(settings.l1_targets, default_l1_targets());
    }
}
