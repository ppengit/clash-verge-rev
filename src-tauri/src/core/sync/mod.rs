pub mod apply;
pub mod client;
pub mod crypto;
pub mod manifest;
pub mod overrides;
pub mod payload;
pub mod records;
pub mod rules;
pub mod settings;
pub mod state;

use crate::{
    config::{Config, IVerge, SyncSettings},
    process::AsyncHandler,
    utils::{
        dirs::{self, PathBufExec as _},
        help,
    },
};
use anyhow::{Context as _, Result, bail};
use chrono::Utc;
use clash_verge_logging::{Type, logging};
use parking_lot::RwLock;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    fs,
    sync::{Mutex, watch},
    time::{sleep, timeout},
};

use self::{
    apply::{ApplyRequest, ConflictSet},
    client::{SyncWebDavClient, SyncWebDavConfig},
    manifest::{HISTORY_KEEP, MANIFEST_MAX_BYTES, PAYLOAD_MAX_BYTES, SCHEMA_VERSION, SyncManifest},
    overrides::OverrideFile,
    state::SyncState,
};

const MIN_INTERVAL_MINUTES: u64 = manifest::MIN_INTERVAL_MINUTES;
const DEFAULT_INTERVAL_MINUTES: u64 = manifest::DEFAULT_INTERVAL_MINUTES;
const STARTUP_JITTER_SECS: u64 = 60;
const CHANGE_DEBOUNCE: Duration = Duration::from_secs(60);
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
const BACKOFF_SECS: [u64; 3] = [60, 300, 900];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ConflictPolicy {
    #[default]
    LastModified,
    Local,
    Remote,
    Ask,
}

impl ConflictPolicy {
    fn parse(value: Option<&str>) -> Self {
        match value.unwrap_or("last_modified") {
            "local" => Self::Local,
            "remote" => Self::Remote,
            "ask" => Self::Ask,
            _ => Self::LastModified,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SyncMode {
    #[default]
    TwoWay,
    Upload,
    Download,
}

impl SyncMode {
    fn parse(value: Option<&str>) -> Self {
        match value.unwrap_or("two_way") {
            "upload" => Self::Upload,
            "download" => Self::Download,
            _ => Self::TwoWay,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::TwoWay => "two_way",
            Self::Upload => "upload",
            Self::Download => "download",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SyncTrigger {
    Startup,
    Scheduled,
    OnChange,
    Manual,
}

#[derive(Clone, Debug)]
struct RuntimeSettings {
    enabled: bool,
    mode: SyncMode,
    scopes: payload::SyncScopes,
    interval_minutes: u64,
    on_startup: bool,
    on_change: bool,
    conflict_policy: ConflictPolicy,
    device_name: Option<std::string::String>,
    encrypt: bool,
    password: Option<std::string::String>,
}

impl RuntimeSettings {
    fn from_verge(verge: &IVerge) -> Self {
        let raw = verge.sync_settings.clone().unwrap_or_default();
        let scopes = raw
            .scopes
            .as_deref()
            .map(payload::SyncScopes::from_names)
            .unwrap_or_default();
        let password = raw
            .password
            .as_deref()
            .map(ToString::to_string)
            .filter(|password| !password.is_empty());
        Self {
            enabled: raw.enabled.unwrap_or(false),
            mode: SyncMode::parse(raw.mode.as_deref()),
            scopes,
            interval_minutes: raw
                .interval_minutes
                .unwrap_or(DEFAULT_INTERVAL_MINUTES)
                .max(MIN_INTERVAL_MINUTES),
            on_startup: raw.on_startup.unwrap_or(true),
            on_change: raw.on_change.unwrap_or(false),
            conflict_policy: ConflictPolicy::parse(raw.conflict_policy.as_deref()),
            device_name: raw
                .device_name
                .as_deref()
                .map(|name| name.trim())
                .filter(|name| !name.is_empty())
                .map(|name| name.chars().take(32).collect()),
            encrypt: raw.encrypt.unwrap_or(false),
            password,
        }
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncRunResult {
    pub action: String,
    pub revision: u64,
    pub uploaded: bool,
    pub applied: bool,
    pub conflicts: bool,
    pub local_hash: String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncConflictCopy {
    pub name: String,
    pub created_at: u64,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncHistoryEntry {
    pub filename: String,
    pub revision: u64,
    pub device_id: String,
    pub device_name: String,
    pub created_at: u64,
    pub payload_size: u64,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub enabled: bool,
    pub running: bool,
    pub mode: String,
    pub scopes: Vec<String>,
    pub interval_minutes: u64,
    pub device_id: String,
    pub device_name: String,
    pub last_revision: u64,
    pub last_synced_at: Option<String>,
    pub last_error: Option<String>,
    pub needs_first_sync: bool,
    pub remote_available: Option<bool>,
    pub remote_revision: Option<u64>,
    pub remote_error: Option<String>,
    pub conflicts: Vec<SyncConflictCopy>,
    pub settings: Option<SyncSettings>,
}

pub struct SyncManager {
    settings: Arc<RwLock<RuntimeSettings>>,
    settings_tx: watch::Sender<RuntimeSettings>,
    shutdown_tx: watch::Sender<bool>,
    runner_started: AtomicBool,
    running: AtomicBool,
    pending: AtomicBool,
    shutdown: AtomicBool,
    failures: AtomicU32,
    change_generation: AtomicU64,
    exec_lock: Mutex<()>,
}

impl SyncManager {
    pub fn global() -> &'static Self {
        static INSTANCE: OnceLock<SyncManager> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            let (settings_tx, _) = watch::channel(RuntimeSettings::from_verge(&IVerge::default()));
            let (shutdown_tx, _) = watch::channel(false);
            Self {
                settings: Arc::new(RwLock::new(RuntimeSettings::from_verge(&IVerge::default()))),
                settings_tx,
                shutdown_tx,
                runner_started: AtomicBool::new(false),
                running: AtomicBool::new(false),
                pending: AtomicBool::new(false),
                shutdown: AtomicBool::new(false),
                failures: AtomicU32::new(0),
                change_generation: AtomicU64::new(0),
                exec_lock: Mutex::new(()),
            }
        })
    }

    pub async fn init(&self) -> Result<()> {
        if let Err(error) = overrides::clean_staging().await {
            logging!(warn, Type::Backup, "Failed to clean stale sync staging: {error:#}");
        }
        self.refresh_settings().await
    }

    pub async fn refresh_settings(&self) -> Result<()> {
        let verge = Config::verge().await;
        let settings = RuntimeSettings::from_verge(&verge.latest_arc());
        *self.settings.write() = settings.clone();
        let _ = self.settings_tx.send(settings.clone());
        self.maybe_start_runner(settings);
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn maybe_start_runner(&self, settings: RuntimeSettings) {
        if settings.enabled {
            self.ensure_runner();
        }
    }

    fn ensure_runner(&self) {
        if self.runner_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut settings_rx = self.settings_tx.subscribe();
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        AsyncHandler::spawn(move || async move {
            Self::run_scheduler(&mut settings_rx, &mut shutdown_rx).await;
        });
    }

    async fn run_scheduler(
        settings_rx: &mut watch::Receiver<RuntimeSettings>,
        shutdown_rx: &mut watch::Receiver<bool>,
    ) {
        let start = settings_rx.borrow().clone();
        if start.enabled && start.on_startup && !*shutdown_rx.borrow() {
            let jitter = startup_jitter();
            tokio::select! {
                () = sleep(Duration::from_secs(jitter)) => {
                    if !*shutdown_rx.borrow() && settings_rx.borrow().enabled {
                        Self::scheduled_run(SyncTrigger::Startup).await;
                    }
                }
                _ = shutdown_rx.changed() => return,
                _ = settings_rx.changed() => {}
            }
        }

        loop {
            if *shutdown_rx.borrow() {
                break;
            }
            let settings = settings_rx.borrow().clone();
            if !settings.enabled {
                tokio::select! {
                    _ = settings_rx.changed() => {}
                    _ = shutdown_rx.changed() => break,
                }
                continue;
            }

            let delay = Self::next_delay(&settings);
            tokio::select! {
                () = sleep(delay) => {
                    Self::scheduled_run(SyncTrigger::Scheduled).await;
                }
                changed = settings_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
                _ = shutdown_rx.changed() => break,
            }
        }
        logging!(debug, Type::Backup, "WebDAV sync scheduler stopped");
    }

    async fn scheduled_run(trigger: SyncTrigger) {
        let manager = Self::global();
        match manager.execute(trigger).await {
            Ok(Some(result)) => logging!(
                info,
                Type::Backup,
                "WebDAV sync finished ({:?}): {} at revision {}",
                trigger,
                result.action,
                result.revision
            ),
            Ok(None) => {}
            Err(error) => logging!(
                warn,
                Type::Backup,
                "WebDAV sync failed ({:?}): {}",
                trigger,
                help::mask_err(&format!("{error:#}"))
            ),
        }
    }

    fn next_delay(settings: &RuntimeSettings) -> Duration {
        let interval = Duration::from_secs(settings.interval_minutes.saturating_mul(60));
        let failures = Self::global().failures.load(Ordering::Acquire);
        if failures == 0 {
            return interval;
        }
        let index = usize::try_from(failures.saturating_sub(1))
            .unwrap_or(usize::MAX)
            .min(BACKOFF_SECS.len() - 1);
        Duration::from_secs(BACKOFF_SECS[index]).min(interval)
    }

    async fn execute(&self, trigger: SyncTrigger) -> Result<Option<SyncRunResult>> {
        if self.shutdown.load(Ordering::Acquire) {
            return Ok(None);
        }
        let Ok(_exec) = self.exec_lock.try_lock() else {
            // One pending rerun at most; later triggers collapse into it.
            self.pending.store(true, Ordering::Release);
            return Ok(None);
        };

        #[allow(unused_assignments)]
        let mut outcome: Result<Option<SyncRunResult>> = Ok(None);
        loop {
            self.pending.store(false, Ordering::Release);
            let running = RunningGuard::new(&self.running);
            let iteration = match self.run_once(trigger).await {
                Ok(result) => {
                    self.failures.store(0, Ordering::Release);
                    Ok(Some(result))
                }
                Err(error) => {
                    if !matches!(client::error_status(&error), Some(401) | Some(403)) {
                        self.failures.fetch_add(1, Ordering::AcqRel);
                    }
                    Err(error)
                }
            };
            outcome = iteration;
            drop(running);
            if !self.pending.swap(false, Ordering::AcqRel) || self.shutdown.load(Ordering::Acquire) {
                break;
            }
        }
        outcome
    }

    /// Debounced on-change trigger; skipped while a sync is already running.
    pub fn notify_local_change() {
        let manager = Self::global();
        let settings = RuntimeSettings::clone(&manager.settings.read());
        if !settings.enabled || !settings.on_change || manager.shutdown.load(Ordering::Acquire) {
            return;
        }
        if manager.running.load(Ordering::Acquire) {
            manager.pending.store(true, Ordering::Release);
            return;
        }
        let generation = manager.change_generation.fetch_add(1, Ordering::AcqRel) + 1;
        AsyncHandler::spawn(move || async move {
            sleep(CHANGE_DEBOUNCE).await;
            let manager = Self::global();
            if manager.change_generation.load(Ordering::Acquire) != generation
                || manager.running.load(Ordering::Acquire)
                || manager.shutdown.load(Ordering::Acquire)
            {
                return;
            }
            let current = RuntimeSettings::clone(&manager.settings.read());
            if !current.enabled || !current.on_change {
                return;
            }
            if let Err(error) = manager.execute(SyncTrigger::OnChange).await {
                logging!(
                    warn,
                    Type::Backup,
                    "WebDAV sync after a local change failed: {}",
                    help::mask_err(&format!("{error:#}"))
                );
            }
        });
    }

    pub async fn sync_now() -> Result<SyncRunResult> {
        let manager = Self::global();
        if manager.shutdown.load(Ordering::Acquire) {
            bail!("the application is shutting down");
        }
        let _exec = manager.exec_lock.lock().await;
        let running = RunningGuard::new(&manager.running);
        let result = manager.run_once(SyncTrigger::Manual).await;
        drop(running);
        match &result {
            Ok(_) => manager.failures.store(0, Ordering::Release),
            Err(error) => {
                if !matches!(client::error_status(error), Some(401) | Some(403)) {
                    manager.failures.fetch_add(1, Ordering::AcqRel);
                }
            }
        }
        result
    }

    pub async fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = self.shutdown_tx.send(true);
        if timeout(SHUTDOWN_WAIT, self.exec_lock.lock()).await.is_err() {
            logging!(
                warn,
                Type::Backup,
                "A WebDAV sync is still finishing after {} seconds during shutdown",
                SHUTDOWN_WAIT.as_secs()
            );
        }
    }

    async fn run_once(&self, trigger: SyncTrigger) -> Result<SyncRunResult> {
        let result = self.run_inner(trigger).await;
        if let Err(error) = &result
            && let Ok(mut state) = SyncState::load_or_create().await
        {
            state.last_error = Some(help::mask_err(&format!("{error:#}")));
            if let Err(save_error) = state.save().await {
                logging!(warn, Type::Backup, "Failed to record the sync error: {save_error:#}");
            }
        }
        result
    }

    #[allow(clippy::cognitive_complexity)]
    async fn run_inner(&self, _trigger: SyncTrigger) -> Result<SyncRunResult> {
        let settings = RuntimeSettings::clone(&self.settings.read());
        let verge = {
            let draft = Config::verge().await;
            (**draft.latest_arc()).clone()
        };
        let client_config = SyncWebDavConfig::from_verge(&verge)?;
        let client = SyncWebDavClient::new(&client_config)?;
        let state = SyncState::load_or_create().await?;
        let now = state::now_ms();
        let first_sync = state.last_revision == 0;
        let mut built = build_local(&settings, &state, first_sync, now).await?;
        let device_name = settings
            .device_name
            .clone()
            .unwrap_or_else(|| state.display_device_name());
        let remote = client.fetch_manifest().await?;

        let Some(manifest) = remote else {
            if state.last_revision > 0 || state.last_synced_at.is_some() {
                bail!("the remote sync data disappeared; refusing to overwrite the last synced revision");
            }
            if settings.mode == SyncMode::Download {
                bail!("there is no cloud sync data to download");
            }
            if first_sync {
                built = rebuild_local(&settings, &state, &built, now).await?;
            }
            let revision = upload(&client, &settings, &state, &built, 0, &device_name, now).await?;
            return finalize_upload(state, &built, revision, now).await;
        };

        if manifest.revision < state.last_revision {
            bail!(
                "the remote sync revision ({}) is older than the last synced revision ({}); refusing to roll back",
                manifest.revision,
                state.last_revision
            );
        }

        let scope_expansion = scope_expansion_requires_merge(&settings, &state, &manifest);

        match settings.mode {
            SyncMode::Upload => {
                if scope_expansion {
                    bail!(
                        "new sync scope(s) already exist in the cloud; run a download or two-way sync before uploading"
                    );
                }
                let unchanged = !first_sync
                    && manifest.revision == state.last_revision
                    && state.last_local_hash.as_deref() == Some(built.sha256.as_str());
                if unchanged {
                    return finalize_noop(state, manifest.revision, built.sha256, now, settings.scopes.names()).await;
                }
                if first_sync {
                    built = rebuild_local(&settings, &state, &built, now).await?;
                }
                let revision = upload(&client, &settings, &state, &built, manifest.revision, &device_name, now).await?;
                finalize_upload(state, &built, revision, now).await
            }
            SyncMode::Download => {
                let unchanged = !first_sync
                    && manifest.revision == state.last_revision
                    && state.last_local_hash.as_deref() == Some(built.sha256.as_str());
                if unchanged && !scope_expansion {
                    return finalize_noop(state, manifest.revision, built.sha256, now, settings.scopes.names()).await;
                }
                let applied = apply_remote(
                    &client,
                    &settings,
                    &state,
                    &built,
                    &manifest,
                    ConflictPolicy::Remote,
                    client::payload_path(),
                    now,
                )
                .await?;
                finalize_applied(applied, "download", now).await
            }
            SyncMode::TwoWay => {
                let remote_changed = manifest.revision != state.last_revision || scope_expansion;
                let local_changed = state.last_local_hash.as_deref() != Some(built.sha256.as_str());
                if !remote_changed && !local_changed {
                    return finalize_noop(state, manifest.revision, built.sha256, now, settings.scopes.names()).await;
                }
                if remote_changed && !local_changed {
                    let applied = apply_remote(
                        &client,
                        &settings,
                        &state,
                        &built,
                        &manifest,
                        settings.conflict_policy,
                        client::payload_path(),
                        now,
                    )
                    .await?;
                    if applied.rebuilt.sha256 == applied.plain_hash && !applied.had_conflicts {
                        return finalize_applied(applied, "download", now).await;
                    }
                    let revision = upload(
                        &client,
                        &settings,
                        &applied.state,
                        &applied.rebuilt,
                        manifest.revision,
                        &device_name,
                        now,
                    )
                    .await?;
                    return finalize_upload(applied.state, &applied.rebuilt, revision, now).await;
                }
                if !remote_changed && local_changed {
                    let revision =
                        upload(&client, &settings, &state, &built, manifest.revision, &device_name, now).await?;
                    return finalize_upload(state, &built, revision, now).await;
                }

                let applied = apply_remote(
                    &client,
                    &settings,
                    &state,
                    &built,
                    &manifest,
                    settings.conflict_policy,
                    client::payload_path(),
                    now,
                )
                .await?;
                if applied.rebuilt.sha256 == applied.plain_hash && !applied.had_conflicts {
                    return finalize_applied(applied, "merge", now).await;
                }
                let revision = upload(
                    &client,
                    &settings,
                    &applied.state,
                    &applied.rebuilt,
                    manifest.revision,
                    &device_name,
                    now,
                )
                .await?;
                finalize_upload(applied.state, &applied.rebuilt, revision, now).await
            }
        }
    }

    pub async fn status() -> Result<SyncStatus> {
        let state = SyncState::load_or_create().await?;
        let verge = {
            let draft = Config::verge().await;
            (**draft.latest_arc()).clone()
        };
        let settings = RuntimeSettings::from_verge(&verge);
        let mut remote_available = None;
        let mut remote_revision = None;
        let mut remote_error = None;
        if let Ok(config) = SyncWebDavConfig::from_verge(&verge)
            && let Ok(client) = SyncWebDavClient::new(&config)
        {
            match client.fetch_manifest().await {
                Ok(Some(manifest)) => {
                    remote_available = Some(true);
                    remote_revision = Some(manifest.revision);
                }
                Ok(None) => remote_available = Some(true),
                Err(error) => {
                    remote_available = Some(false);
                    remote_error = Some(help::mask_err(&format!("{error:#}")));
                }
            }
        }
        let conflicts = collect_conflict_copies().unwrap_or_default();
        let mut shown_settings = verge.sync_settings.clone();
        if let Some(settings) = shown_settings.as_mut() {
            settings.password = None;
        }
        Ok(SyncStatus {
            enabled: settings.enabled,
            running: Self::global().is_running(),
            mode: settings.mode.as_str().into(),
            scopes: settings.scopes.names(),
            interval_minutes: settings.interval_minutes,
            device_id: state.device_id.clone(),
            device_name: settings.device_name.unwrap_or_else(|| state.display_device_name()),
            last_revision: state.last_revision,
            last_synced_at: state.last_synced_at.clone(),
            last_error: state.last_error.clone(),
            needs_first_sync: state.last_revision == 0,
            remote_available,
            remote_revision,
            remote_error,
            conflicts,
            settings: shown_settings,
        })
    }

    pub async fn test_connection() -> Result<()> {
        let verge = {
            let draft = Config::verge().await;
            (**draft.latest_arc()).clone()
        };
        let config = SyncWebDavConfig::from_verge(&verge)?;
        let client = SyncWebDavClient::new(&config)?;
        let state = SyncState::load_or_create().await?;
        client.ensure_directories().await?;
        client.fetch_manifest().await?;
        client.probe(&state.device_id).await
    }

    pub async fn list_history() -> Result<Vec<SyncHistoryEntry>> {
        let verge = {
            let draft = Config::verge().await;
            (**draft.latest_arc()).clone()
        };
        let config = SyncWebDavConfig::from_verge(&verge)?;
        let client = SyncWebDavClient::new(&config)?;
        let files = client.list_history().await?;
        let mut entries: Vec<SyncHistoryEntry> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for file in files.iter().filter(|file| file.name.ends_with(".json")) {
            let path = format!("{}/history/{}", client::SYNC_REMOTE_DIR, file.name);
            let bytes = match client.download_small(&path, MANIFEST_MAX_BYTES).await {
                Ok(Some(bytes)) => bytes,
                Ok(None) => continue,
                Err(error) => {
                    logging!(
                        debug,
                        Type::Backup,
                        "Skipping unreadable sync history metadata {}: {error:#}",
                        file.name
                    );
                    continue;
                }
            };
            let Ok(manifest) = serde_json::from_slice::<SyncManifest>(&bytes) else {
                continue;
            };
            if manifest.validate().is_err() {
                continue;
            }
            let zip_name = format!("{}.zip", file.name.trim_end_matches(".json"));
            seen.insert(zip_name.clone());
            entries.push(SyncHistoryEntry {
                filename: zip_name,
                revision: manifest.revision,
                device_id: manifest.device_id,
                device_name: manifest.device_name,
                created_at: manifest.created_at,
                payload_size: manifest.payload_size,
            });
        }
        for file in files.iter().filter(|file| file.name.ends_with(".zip")) {
            if seen.contains(&file.name) {
                continue;
            }
            if let Some(revision) = client::revision_from_history_name(&file.name) {
                entries.push(SyncHistoryEntry {
                    filename: file.name.clone(),
                    revision,
                    device_id: String::new(),
                    device_name: String::new(),
                    created_at: file.modified.timestamp_millis().max(0) as u64,
                    payload_size: file.size,
                });
            }
        }
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.revision));
        Ok(entries)
    }

    pub async fn restore_history(filename: &str) -> Result<SyncRunResult> {
        if !is_safe_history_name(filename) {
            bail!("invalid sync history filename");
        }
        let manager = Self::global();
        if manager.shutdown.load(Ordering::Acquire) {
            bail!("the application is shutting down");
        }
        let _exec = manager.exec_lock.lock().await;
        let _running = RunningGuard::new(&manager.running);
        let sidecar = format!("{}.json", filename.trim_end_matches(".zip"));
        let verge = {
            let draft = Config::verge().await;
            (**draft.latest_arc()).clone()
        };
        let config = SyncWebDavConfig::from_verge(&verge)?;
        let client = SyncWebDavClient::new(&config)?;
        let settings = RuntimeSettings::from_verge(&verge);
        let state = SyncState::load_or_create().await?;
        let now = state::now_ms();
        let built = build_local(&settings, &state, false, now).await?;

        let sidecar_path = format!("{}/history/{}", client::SYNC_REMOTE_DIR, sidecar);
        let bytes = client
            .download_small(&sidecar_path, MANIFEST_MAX_BYTES)
            .await?
            .ok_or_else(|| anyhow::anyhow!("the selected snapshot has no manifest"))?;
        let manifest: SyncManifest =
            serde_json::from_slice(&bytes).context("the snapshot manifest is not valid JSON")?;
        manifest.validate()?;
        let snapshot_path = format!("{}/history/{}", client::SYNC_REMOTE_DIR, filename);
        let applied = apply_remote(
            &client,
            &settings,
            &state,
            &built,
            &manifest,
            ConflictPolicy::Remote,
            snapshot_path,
            now,
        )
        .await?;
        // Publish the restored snapshot as a new revision so it does not immediately
        // spring back to the newer remote state on the next scheduled sync.
        let device_name = settings
            .device_name
            .clone()
            .unwrap_or_else(|| state.display_device_name());
        let parent = client
            .fetch_manifest()
            .await?
            .map(|remote| remote.revision)
            .unwrap_or(manifest.revision);
        let mut result = match upload(
            &client,
            &settings,
            &applied.state,
            &applied.rebuilt,
            parent,
            &device_name,
            now,
        )
        .await
        {
            Ok(revision) => finalize_upload(applied.state, &applied.rebuilt, revision, now).await?,
            Err(error) => {
                logging!(
                    warn,
                    Type::Backup,
                    "Restored a sync snapshot locally but could not publish it: {error:#}"
                );
                finalize_applied(applied, "restore", now).await?
            }
        };
        result.action = "restore".into();
        result.applied = true;
        Ok(result)
    }
}

fn is_safe_history_name(name: &str) -> bool {
    name.ends_with(".zip")
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

struct RunningGuard<'a>(&'a AtomicBool);

impl<'a> RunningGuard<'a> {
    fn new(flag: &'a AtomicBool) -> Self {
        flag.store(true, Ordering::Release);
        Self(flag)
    }
}

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn startup_jitter() -> u64 {
    let mut bytes = [0_u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        return 0;
    }
    u64::from_le_bytes(bytes) % STARTUP_JITTER_SECS
}

fn scope_expansion_requires_merge(settings: &RuntimeSettings, state: &SyncState, manifest: &SyncManifest) -> bool {
    if state.last_revision == 0 {
        return false;
    }
    if state.last_scopes.is_empty() {
        // State written before last_scopes existed: we cannot know which cloud scopes this
        // device already merged. Force a download/merge before any upload.
        return !manifest.scopes.is_empty();
    }
    let new_scopes = settings
        .scopes
        .names()
        .into_iter()
        .filter(|scope| !state.last_scopes.iter().any(|old| old == scope))
        .collect::<Vec<_>>();
    !new_scopes.is_empty() && manifest.scopes.iter().any(|scope| new_scopes.contains(scope))
}

async fn build_local(
    settings: &RuntimeSettings,
    state: &SyncState,
    first_sync: bool,
    now: u64,
) -> Result<payload::BuiltPayload> {
    if !(settings.scopes.subscriptions
        || settings.scopes.overrides
        || settings.scopes.custom_rules
        || settings.scopes.settings)
    {
        bail!("select at least one sync scope");
    }
    if settings.scopes.custom_rules && !settings.scopes.overrides {
        bail!("custom rules require the overrides scope so their group/provider dependencies are synced");
    }
    let profiles = {
        let draft = Config::profiles().await;
        (**draft.latest_arc()).clone()
    };
    if profiles.items.is_none() {
        bail!("profiles.yaml could not be read; refusing to sync an empty profile list");
    }
    let verge = {
        let draft = Config::verge().await;
        (**draft.latest_arc()).clone()
    };
    payload::build(payload::BuildInput {
        profiles: &profiles,
        verge: &verge,
        state,
        scopes: settings.scopes,
        first_sync,
        now,
    })
    .await
}

/// Stamps local timestamps before a first upload so the next build is stable.
async fn rebuild_local(
    settings: &RuntimeSettings,
    state: &SyncState,
    built: &payload::BuiltPayload,
    now: u64,
) -> Result<payload::BuiltPayload> {
    let mut next = state.clone();
    records::seed_record_state(&mut next, &built.subscriptions, now);
    next.overrides = built.override_state.clone();
    next.override_tombstones = built.override_tombstones.clone();
    next.settings.clear();
    next.record_tombstones = built.record_tombstones.clone();
    build_local(settings, &next, false, now).await
}

async fn upload(
    client: &SyncWebDavClient,
    settings: &RuntimeSettings,
    state: &SyncState,
    built: &payload::BuiltPayload,
    parent_revision: u64,
    device_name: &str,
    now: u64,
) -> Result<u64> {
    // Never let a device with fewer scopes replace a cloud payload that contains data for
    // scopes it does not build; otherwise the missing files would disappear for every device.
    if let Some(remote) = client.fetch_manifest().await? {
        let local_scopes = settings.scopes.names();
        let missing = remote
            .scopes
            .iter()
            .filter(|scope| !local_scopes.iter().any(|local| local == *scope))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            bail!(
                "cloud sync contains scopes not enabled on this device ({}); enable them before uploading to avoid deleting cloud data",
                missing.join(", ")
            );
        }
    }

    client.ensure_directories().await?;
    if state.last_revision > 0 {
        let latest = build_local(settings, state, false, now).await?;
        if latest.sha256 != built.sha256 {
            bail!("local configuration changed during sync; retry the sync");
        }
    }
    let (payload, encryption) = if settings.encrypt {
        let password = settings
            .password
            .as_deref()
            .filter(|password| !password.is_empty())
            .ok_or_else(|| anyhow::anyhow!("sync encryption is enabled but no password is set"))?;
        let (payload, info) = crypto::encrypt_payload(&built.zip, password)?;
        (payload, Some(info))
    } else {
        (built.zip.clone(), None)
    };

    let revision = parent_revision.max(state.last_revision).saturating_add(1);
    let manifest = SyncManifest {
        schema: SCHEMA_VERSION,
        revision,
        parent_revision,
        device_id: state.device_id.clone(),
        device_name: device_name.to_owned(),
        created_at: now,
        scopes: settings.scopes.names(),
        payload_sha256: manifest::sha256_hex(&payload),
        payload_size: payload.len() as u64,
        encryption,
        files: built.manifest_files(),
        deleted: built.deleted.clone(),
    };
    manifest.validate()?;

    client.upload_payload(payload.clone()).await?;
    client.upload_manifest(&manifest).await?;
    if let Err(error) = client
        .history_upload(revision, &state.device_id, &payload, &manifest)
        .await
    {
        logging!(
            warn,
            Type::Backup,
            "Uploaded the sync revision but could not append history: {error:#}"
        );
    }
    if let Err(error) = client.prune_history(HISTORY_KEEP).await {
        logging!(warn, Type::Backup, "Failed to prune WebDAV sync history: {error:#}");
    }
    Ok(revision)
}

async fn finalize_upload(
    mut state: SyncState,
    built: &payload::BuiltPayload,
    revision: u64,
    now: u64,
) -> Result<SyncRunResult> {
    state.last_revision = revision;
    state.last_synced_at = Some(Utc::now().to_rfc3339());
    state.last_local_hash = Some(built.sha256.clone());
    state.last_error = None;
    state.last_scopes = built.scopes.names();
    state.local_records = built.record_state.clone();
    state.record_tombstones = built.record_tombstones.clone();
    state.overrides = built.override_state.clone();
    state.override_tombstones = built.override_tombstones.clone();
    state.settings = built.settings_state.clone();
    state.prune_tombstones(now);
    state.save().await?;
    Ok(SyncRunResult {
        action: "upload".into(),
        revision,
        uploaded: true,
        applied: false,
        conflicts: false,
        local_hash: built.sha256.clone(),
    })
}

async fn finalize_noop(
    mut state: SyncState,
    revision: u64,
    local_hash: String,
    now: u64,
    scopes: Vec<std::string::String>,
) -> Result<SyncRunResult> {
    state.last_synced_at = Some(Utc::now().to_rfc3339());
    state.last_error = None;
    state.last_scopes = scopes;
    state.prune_tombstones(now);
    state.save().await?;
    Ok(SyncRunResult {
        action: "noop".into(),
        revision,
        uploaded: false,
        applied: false,
        conflicts: false,
        local_hash,
    })
}

async fn finalize_applied(applied: AppliedRemote, action: &str, now: u64) -> Result<SyncRunResult> {
    let mut next = applied.state;
    next.last_revision = applied.manifest_revision;
    next.last_synced_at = Some(Utc::now().to_rfc3339());
    next.last_local_hash = Some(applied.rebuilt.sha256.clone());
    next.last_error = None;
    next.last_scopes = applied.rebuilt.scopes.names();
    next.local_records = applied.rebuilt.record_state.clone();
    next.record_tombstones = applied.rebuilt.record_tombstones.clone();
    next.overrides = applied.rebuilt.override_state.clone();
    next.override_tombstones = applied.rebuilt.override_tombstones.clone();
    next.settings = applied.rebuilt.settings_state.clone();
    next.prune_tombstones(now);
    next.save().await?;
    Ok(SyncRunResult {
        action: action.into(),
        revision: applied.manifest_revision,
        uploaded: false,
        applied: true,
        conflicts: applied.had_conflicts,
        local_hash: applied.rebuilt.sha256.clone(),
    })
}

struct AppliedRemote {
    state: SyncState,
    rebuilt: payload::BuiltPayload,
    plain_hash: String,
    manifest_revision: u64,
    had_conflicts: bool,
}

async fn apply_remote(
    client: &SyncWebDavClient,
    settings: &RuntimeSettings,
    state: &SyncState,
    built: &payload::BuiltPayload,
    manifest: &SyncManifest,
    policy: ConflictPolicy,
    remote_payload_path: std::string::String,
    now: u64,
) -> Result<AppliedRemote> {
    let staging = dirs::sync_staging_dir()?;
    overrides::clean_staging().await?;
    let payload_file = staging.join(format!("payload-{}.bin", help::get_uid("d")));
    let downloaded = client
        .download_to_file(&remote_payload_path, &payload_file, PAYLOAD_MAX_BYTES)
        .await?;
    let stored = fs::read(&payload_file)
        .await
        .context("failed to read the downloaded sync payload")?;
    let _ = fs::remove_file(&payload_file).await;
    if downloaded != manifest.payload_size {
        let _ = overrides::clean_staging().await;
        bail!("the downloaded sync payload size does not match its manifest");
    }
    payload::verify_payload_hash(&stored, manifest)?;

    let plain = match manifest.encryption.as_ref() {
        Some(info) => crypto::decrypt_payload(&stored, settings.password.as_deref().unwrap_or(""), info)?,
        None => stored,
    };
    let plain_hash = manifest::sha256_hex(&plain);

    let result = async {
        let extracted = payload::extract_and_validate(&plain, manifest, &staging)?;

        let merged_records = if settings.scopes.subscriptions {
            Some(records::merge_subscriptions(
                &built.subscriptions,
                &extracted.subscriptions,
                policy,
                state.last_revision,
                manifest.revision,
            ))
        } else {
            None
        };

        let remote_overrides: BTreeMap<std::string::String, OverrideFile> = extracted
            .files
            .iter()
            .filter(|(path, _)| path.starts_with("overrides/"))
            .map(|(path, file)| {
                (
                    path.clone(),
                    OverrideFile {
                        path: path.clone(),
                        content: file.bytes.clone(),
                        modified_at: file.modified_at,
                    },
                )
            })
            .collect();
        let remote_deleted_overrides: Vec<std::string::String> = manifest
            .deleted
            .iter()
            .filter(|path| path.starts_with("overrides/"))
            .cloned()
            .collect();
        let merged_overrides = if settings.scopes.overrides {
            Some(overrides::merge_overrides(
                &built.overrides,
                &built.override_tombstones,
                &remote_overrides,
                &remote_deleted_overrides,
                manifest.created_at,
                policy,
                manifest.revision > state.last_revision,
            ))
        } else {
            None
        };

        let merged_rules = if settings.scopes.custom_rules {
            Some(rules::merge_rules(
                &built.rules,
                &extracted.rules,
                policy,
                state.last_revision,
                manifest.revision,
            ))
        } else {
            None
        };

        let mut merged_settings_state = BTreeMap::new();
        let merged_settings = if settings.scopes.settings {
            let (values, state) = settings::merge_settings(
                &built.settings_values,
                &built.settings_state,
                &extracted.settings,
                policy,
                state.last_revision,
                manifest.revision,
                now,
            );
            merged_settings_state = state;
            values
        } else {
            BTreeMap::new()
        };

        let mut conflicts = ConflictSet {
            records: merged_records
                .as_ref()
                .map(|merged| apply::record_conflicts(&merged.conflicts))
                .unwrap_or_default(),
            rules: merged_rules
                .as_ref()
                .map(|merged| apply::rule_conflicts(&merged.conflicts))
                .unwrap_or_default(),
            overrides: merged_overrides
                .as_ref()
                .map(apply::override_conflicts)
                .unwrap_or_default(),
            ..ConflictSet::default()
        };
        let had_conflicts = !conflicts.is_empty();

        // Optimistic concurrency check: if the user edited a profile, rule, or override during
        // the network phase, applying the stale merge would silently overwrite that edit.
        if state.last_revision > 0 {
            let latest = build_local(settings, state, false, now).await?;
            if latest.sha256 != built.sha256 {
                bail!("local configuration changed during sync; retry the sync");
            }
        }

        let outcome = apply::apply(
            ApplyRequest {
                subscriptions: merged_records.as_ref().map(|merged| merged.records.as_slice()),
                overrides: merged_overrides.as_ref(),
                rules: merged_rules.as_ref().map(|merged| merged.rules.as_slice()),
                settings: Some(&merged_settings),
            },
            &mut conflicts,
        )
        .await?;

        let mut next = state.clone();
        if let Some(merged) = merged_records.as_ref() {
            records::seed_record_state(&mut next, &merged.records, now);
        }
        next.overrides = built.override_state.clone();
        next.override_tombstones = built.override_tombstones.clone();
        // Remote deletions must keep propagating, including paths this device never had.
        if let Some(merged) = merged_overrides.as_ref() {
            for path in &merged.deleted {
                next.override_tombstones
                    .entry(path.clone())
                    .or_insert_with(|| manifest.created_at.max(1));
            }
        }
        next.settings = if settings.scopes.settings {
            merged_settings_state
        } else {
            built.settings_state.clone()
        };
        next.last_revision = manifest.revision;
        let rebuilt = match build_local(settings, &next, false, now).await {
            Ok(rebuilt) => {
                let _ = outcome.backup_path.remove_if_exists().await;
                rebuilt
            }
            Err(error) => {
                logging!(
                    error,
                    Type::Backup,
                    "Sync apply committed but the rebuild failed; restoring the pre-sync backup: {error:#}"
                );
                if let Err(rollback_error) = apply::restore_backup(&outcome.backup_path).await {
                    logging!(
                        error,
                        Type::Backup,
                        "Sync rebuild rollback failed; keeping the backup at {}: {rollback_error:#}",
                        outcome.backup_path.display()
                    );
                    return Err(anyhow::anyhow!(
                        "sync rebuild failed and rollback failed; backup kept at {}: {rollback_error:#}; original error: {error:#}",
                        outcome.backup_path.display()
                    ));
                }
                if let Err(settings_error) = apply::restore_verge(&outcome.verge_previous).await {
                    logging!(
                        error,
                        Type::Backup,
                        "Sync rebuild rollback could not restore settings; keeping the backup at {}: {settings_error:#}",
                        outcome.backup_path.display()
                    );
                    return Err(anyhow::anyhow!(
                        "sync rebuild failed and settings rollback failed; backup kept at {}: {settings_error:#}; original error: {error:#}",
                        outcome.backup_path.display()
                    ));
                }
                if let Err(reapply_error) = apply::enhance_and_refresh().await {
                    logging!(
                        error,
                        Type::Backup,
                        "Restored configuration could not be re-applied; keeping the backup at {}: {reapply_error:#}",
                        outcome.backup_path.display()
                    );
                    return Err(anyhow::anyhow!(
                        "sync rebuild failed; rollback succeeded but re-applying the restored configuration failed: {reapply_error:#}; backup kept at {}",
                        outcome.backup_path.display()
                    ));
                }
                let _ = outcome.backup_path.remove_if_exists().await;
                return Err(error);
            }
        };

        Ok(AppliedRemote {
            state: next,
            rebuilt,
            plain_hash,
            manifest_revision: manifest.revision,
            had_conflicts,
        })
    }
    .await;
    let _ = overrides::clean_staging().await;
    result
}

fn collect_conflict_copies() -> Result<Vec<SyncConflictCopy>> {
    let dir = dirs::sync_conflicts_dir()?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("failed to read sync conflicts"),
    };
    let mut copies = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let created_at = name.split('-').next().and_then(|value| value.parse().ok()).unwrap_or(0);
        let mut files = Vec::new();
        collect_files(&entry.path(), &entry.path(), &mut files, 0);
        files.truncate(200);
        copies.push(SyncConflictCopy {
            name,
            created_at,
            files,
        });
    }
    copies.sort_by_key(|copy| std::cmp::Reverse(copy.created_at));
    Ok(copies)
}

fn collect_files(root: &std::path::Path, current: &std::path::Path, files: &mut Vec<String>, depth: usize) {
    if files.len() >= 200 || depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, files, depth + 1);
        } else if let Ok(relative) = path.strip_prefix(root) {
            files.push(relative.to_string_lossy().to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_policies_and_modes_parse_with_safe_defaults() {
        assert_eq!(ConflictPolicy::parse(Some("local")), ConflictPolicy::Local);
        assert_eq!(ConflictPolicy::parse(Some("remote")), ConflictPolicy::Remote);
        assert_eq!(ConflictPolicy::parse(Some("ask")), ConflictPolicy::Ask);
        assert_eq!(ConflictPolicy::parse(Some("bogus")), ConflictPolicy::LastModified);
        assert_eq!(SyncMode::parse(Some("upload")), SyncMode::Upload);
        assert_eq!(SyncMode::parse(Some("download")), SyncMode::Download);
        assert_eq!(SyncMode::parse(None), SyncMode::TwoWay);
    }

    #[test]
    fn intervals_never_drop_below_the_floor() {
        let mut verge = IVerge::default();
        verge.sync_settings = Some(SyncSettings {
            interval_minutes: Some(1),
            ..Default::default()
        });
        let settings = RuntimeSettings::from_verge(&verge);
        assert_eq!(settings.interval_minutes, 15);
    }

    #[test]
    fn history_filenames_cannot_escape_the_history_directory() {
        assert!(is_safe_history_name("12-Dabc.zip"));
        assert!(!is_safe_history_name("../12-Dabc.zip"));
        assert!(!is_safe_history_name("12/Dabc.zip"));
        assert!(!is_safe_history_name("12-Dabc.json"));
    }
}
