use super::{
    overrides::{MergedOverrides, parse_override_path, role_item_type},
    records::{SubscriptionOption, SubscriptionRecord},
    rules, settings,
    state::now_ms,
};
use crate::{
    config::{Config, IProfiles, IVerge, PrfItem, PrfOption},
    core::{
        backup,
        node_health::NodeHealthManager,
        notify::{Refresh, announce},
        timer::Timer,
        validate::ValidationOutcome,
    },
    enhance::custom_rules::CustomRule,
    process::AsyncHandler,
    utils::dirs::PathBufExec as _,
    utils::{dirs, help, tmpl},
};
use anyhow::{Context as _, Result, bail};
use clash_verge_logging::{Type, logging};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};
use tokio::fs;

/// Loser copies retained under `sync/conflicts/<time>-<device>/`.
#[derive(Default)]
pub struct ConflictSet {
    pub directory: Option<PathBuf>,
    pub records: Vec<(std::string::String, Vec<u8>)>,
    pub rules: Vec<(std::string::String, Vec<u8>)>,
    pub overrides: Vec<(std::string::String, Vec<u8>)>,
}

impl ConflictSet {
    pub const fn is_empty(&self) -> bool {
        self.records.is_empty() && self.rules.is_empty() && self.overrides.is_empty()
    }
}

pub struct ApplyRequest<'a> {
    pub subscriptions: Option<&'a [SubscriptionRecord]>,
    pub overrides: Option<&'a MergedOverrides>,
    pub rules: Option<&'a [CustomRule]>,
    pub settings: Option<&'a BTreeMap<std::string::String, serde_json::Value>>,
}

pub struct ApplyOutcome {
    /// Kept until the caller finishes rebuilding and finalizes the sync state.
    pub backup_path: PathBuf,
    /// Previous Verge settings, used when a post-apply rebuild must be rolled back.
    pub verge_previous: IVerge,
}

struct ApplyPlan {
    profiles: IProfiles,
    file_writes: BTreeMap<std::string::String, Vec<u8>>,
    deletions: Vec<PathBuf>,
    created_profiles: Vec<std::string::String>,
    profiles_changed: bool,
    rules: Option<Vec<CustomRule>>,
    settings: Option<BTreeMap<std::string::String, serde_json::Value>>,
    rules_previous: Option<Vec<u8>>,
    verge_previous: IVerge,
}

pub async fn apply(request: ApplyRequest<'_>, conflicts: &mut ConflictSet) -> Result<ApplyOutcome> {
    let now = now_ms();
    conflicts.directory =
        Some(dirs::sync_conflicts_dir()?.join(super::overrides::conflict_copy_name(&device_name().await, now)));
    write_conflicts(conflicts).await;
    if let Err(error) = super::overrides::prune_conflicts(super::manifest::CONFLICTS_KEEP).await {
        logging!(warn, Type::Backup, "Failed to prune sync conflicts: {error:#}");
    }

    let config_write = Config::lock_config_write().await;
    let profile_write = crate::config::profiles::PROFILE_WRITE_LOCK.lock().await;

    let (backup_name, backup_path) = backup::create_backup()
        .await
        .context("failed to create a local backup")?;
    logging!(info, Type::Backup, "Sync apply backup created: {backup_name}");

    let plan = match prepare_plan(request, now).await {
        Ok(plan) => plan,
        Err(error) => {
            let _ = backup_path.remove_if_exists().await;
            drop(profile_write);
            drop(config_write);
            return Err(error);
        }
    };

    let applied = apply_plan(&plan).await;
    drop(profile_write);
    drop(config_write);

    if let Err(error) = applied {
        logging!(
            error,
            Type::Backup,
            "Sync apply failed; restoring the local backup: {error:#}"
        );
        if let Err(rollback_error) = rollback(&plan, &backup_path).await {
            logging!(
                error,
                Type::Backup,
                "Sync rollback failed; keeping the backup at {}: {rollback_error:#}",
                backup_path.display()
            );
            return Err(anyhow::anyhow!(
                "sync apply failed and rollback failed; backup kept at {}: {rollback_error:#}; original error: {error:#}",
                backup_path.display()
            ));
        }
        let _ = backup_path.remove_if_exists().await;
        return Err(error);
    }

    if let Err(error) = enhance_and_refresh().await {
        logging!(
            error,
            Type::Backup,
            "The synced configuration was rejected; restoring the local backup: {error:#}"
        );
        if let Err(rollback_error) = rollback(&plan, &backup_path).await {
            logging!(
                error,
                Type::Backup,
                "Sync rollback failed; keeping the backup at {}: {rollback_error:#}",
                backup_path.display()
            );
            return Err(anyhow::anyhow!(
                "the synced configuration was rejected and rollback failed; backup kept at {}: {rollback_error:#}; original error: {error:#}",
                backup_path.display()
            ));
        }
        if let Err(reapply_error) = enhance_and_refresh().await {
            logging!(
                error,
                Type::Backup,
                "Restored configuration could not be re-applied; keeping the backup at {}: {reapply_error:#}",
                backup_path.display()
            );
            return Err(anyhow::anyhow!(
                "the synced configuration was rejected; rollback succeeded but re-applying the restored configuration failed: {reapply_error:#}; backup kept at {}",
                backup_path.display()
            ));
        }
        let _ = backup_path.remove_if_exists().await;
        return Err(error);
    }

    announce(Refresh::Profiles);
    announce(Refresh::Verge);

    fetch_created_profiles(plan.created_profiles.clone());
    Ok(ApplyOutcome {
        backup_path,
        verge_previous: plan.verge_previous.clone(),
    })
}

async fn rollback(plan: &ApplyPlan, backup_path: &Path) -> Result<()> {
    let mut errors = Vec::new();
    if let Err(error) = restore_backup(backup_path).await {
        errors.push(format!("restore backup: {error:#}"));
    }
    if let Err(error) = crate::enhance::custom_rules::restore_snapshot(plan.rules_previous.clone()).await {
        errors.push(format!("restore custom rules: {error:#}"));
    }
    if let Err(error) = restore_verge(&plan.verge_previous).await {
        errors.push(format!("restore settings: {error:#}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(errors.join("; ")))
    }
}

async fn device_name() -> std::string::String {
    let verge = Config::verge().await;
    let settings = verge.latest_arc().sync_settings.clone();
    settings
        .and_then(|settings| settings.device_name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| std::string::String::from("device"))
}

async fn write_conflicts(conflicts: &ConflictSet) {
    if conflicts.is_empty() {
        return;
    }
    let Some(directory) = conflicts.directory.as_ref() else {
        return;
    };
    for (name, bytes) in &conflicts.records {
        let path = format!("subscriptions/{name}.yaml");
        if let Err(error) = super::overrides::write_conflict_copy(directory, &path, bytes).await {
            logging!(
                warn,
                Type::Backup,
                "Failed to retain a subscription conflict copy: {error:#}"
            );
        }
    }
    for (id, bytes) in &conflicts.rules {
        let path = format!("custom-rules/{id}.yaml");
        if let Err(error) = super::overrides::write_conflict_copy(directory, &path, bytes).await {
            logging!(warn, Type::Backup, "Failed to retain a rule conflict copy: {error:#}");
        }
    }
    for (path, bytes) in &conflicts.overrides {
        if let Err(error) = super::overrides::write_conflict_copy(directory, path, bytes).await {
            logging!(
                warn,
                Type::Backup,
                "Failed to retain an override conflict copy: {error:#}"
            );
        }
    }
}

async fn prepare_plan(request: ApplyRequest<'_>, now: u64) -> Result<ApplyPlan> {
    let profiles = Config::profiles().await;
    let mut candidate = (**profiles.latest_arc()).clone();
    let verge = Config::verge().await;
    let verge_previous = (**verge.latest_arc()).clone();

    let mut file_writes = BTreeMap::new();
    let mut deletions = Vec::new();
    let mut created_profiles = Vec::new();
    let mut profiles_changed = false;

    if let Some(records) = request.subscriptions {
        let outcome = reconcile_subscriptions(records, &mut candidate, now, &mut file_writes, &mut deletions)?;
        created_profiles = outcome.created;
        profiles_changed |= outcome.changed;
    }

    if let Some(merged) = request.overrides {
        profiles_changed |= reconcile_overrides(merged, &mut candidate, now, &mut file_writes, &mut deletions)?;
    }

    let rules_previous = crate::enhance::custom_rules::snapshot_file_checked().await?;

    Ok(ApplyPlan {
        profiles: candidate,
        file_writes,
        deletions,
        created_profiles,
        profiles_changed,
        rules: request.rules.map(<[CustomRule]>::to_vec),
        settings: request.settings.cloned(),
        rules_previous,
        verge_previous,
    })
}

struct ReconcileOutcome {
    created: Vec<std::string::String>,
    changed: bool,
}

fn find_profile_index(items: &[PrfItem], record: &SubscriptionRecord) -> Option<usize> {
    items
        .iter()
        .position(|item| item.sync_id.as_deref() == Some(record.sync_id.as_str()))
        .or_else(|| {
            let url = record.url.as_deref().and_then(super::records::normalize_url)?;
            items.iter().position(|item| {
                item.itype.as_deref() == Some("remote")
                    && item.url.as_deref().and_then(super::records::normalize_url).as_deref() == Some(url.as_str())
            })
        })
}

fn update_profile_metadata(item: &mut PrfItem, record: &SubscriptionRecord) -> bool {
    let mut changed = item.sync_id.as_deref() != Some(record.sync_id.as_str());
    if changed {
        item.sync_id = Some(record.sync_id.as_str().into());
    }
    if item.itype.as_deref() != Some("remote") {
        item.itype = Some("remote".into());
        changed = true;
    }
    if item.name.as_deref() != record.name.as_deref() {
        item.name = record.name.as_deref().map(Into::into);
        changed = true;
    }
    if item.url.as_deref() != record.url.as_deref() {
        item.url = record.url.as_deref().map(Into::into);
        changed = true;
    }
    let current = SubscriptionOption::from_prf(item.option.as_ref());
    if current != record.option {
        let option = item.option.get_or_insert_with(PrfOption::default);
        record.option.apply_to_prf(option);
        changed = true;
    }
    changed
}

fn new_remote_item(record: &SubscriptionRecord, now: u64) -> (PrfItem, std::string::String) {
    let uid = super::records::new_profile_uid();
    let file = format!("{uid}.yaml");
    let item = PrfItem {
        uid: Some(uid.as_str().into()),
        sync_id: Some(record.sync_id.as_str().into()),
        itype: Some("remote".into()),
        name: record.name.as_deref().map(Into::into),
        file: Some(file.as_str().into()),
        url: record.url.as_deref().map(Into::into),
        option: Some(PrfOption {
            update_interval: record.option.update_interval,
            allow_auto_update: record.option.allow_auto_update,
            user_agent: record.option.user_agent.as_ref().map(|value| value.as_str().into()),
            timeout_seconds: record.option.timeout_seconds,
            with_proxy: record.option.with_proxy,
            self_proxy: record.option.self_proxy,
            danger_accept_invalid_certs: record.option.danger_accept_invalid_certs,
            ..PrfOption::default()
        }),
        updated: Some((now / 1_000) as usize),
        ..PrfItem::default()
    };
    (item, file)
}

fn first_usable_uid(items: &[PrfItem]) -> Option<std::string::String> {
    items
        .iter()
        .find(|item| matches!(item.itype.as_deref(), Some("remote" | "local")))
        .and_then(|item| item.uid.as_ref().map(ToString::to_string))
}

fn reconcile_subscriptions(
    desired: &[SubscriptionRecord],
    profiles: &mut IProfiles,
    now: u64,
    file_writes: &mut BTreeMap<std::string::String, Vec<u8>>,
    deletions: &mut Vec<PathBuf>,
) -> Result<ReconcileOutcome> {
    let mut items = profiles.items.take().unwrap_or_default();
    let mut changed = false;
    let mut created = Vec::new();

    let mut matched = HashSet::new();
    for record in desired.iter().filter(|record| !record.deleted) {
        let index = find_profile_index(&items, record).filter(|index| matched.insert(*index));
        match index {
            Some(index) => changed |= update_profile_metadata(&mut items[index], record),
            None => {
                let (item, file) = new_remote_item(record, now);
                file_writes.insert(file, tmpl::ITEM_LOCAL.as_bytes().to_vec());
                created.push(item.uid.as_ref().map(ToString::to_string).unwrap_or_default());
                items.push(item);
                matched.insert(items.len() - 1);
                changed = true;
            }
        }
    }

    for record in desired.iter().filter(|record| record.deleted) {
        let Some(index) = find_profile_index(&items, record) else {
            continue;
        };
        let item = items.remove(index);
        schedule_item_deletion(&item, deletions)?;
        changed = true;
    }

    // Reorder only subscription slots so unrelated local items keep their positions.
    let desired_order: BTreeMap<&str, u32> = desired
        .iter()
        .filter(|record| !record.deleted)
        .map(|record| (record.sync_id.as_str(), record.order))
        .collect();
    let slots: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.itype.as_deref() == Some("remote"))
        .map(|(index, _)| index)
        .collect();
    if !slots.is_empty() {
        let mut remote_items: Vec<PrfItem> = slots.iter().map(|index| items[*index].clone()).collect();
        remote_items.sort_by_key(|item| {
            desired_order
                .get(item.sync_id.as_deref().unwrap_or_default())
                .copied()
                .unwrap_or(u32::MAX)
        });
        let order_changed = slots
            .iter()
            .map(|index| items[*index].uid.as_ref())
            .ne(remote_items.iter().map(|item| item.uid.as_ref()));
        if order_changed {
            changed = true;
        }
        for (slot, item) in slots.iter().zip(remote_items) {
            items[*slot] = item;
        }
    }

    if profiles.current.is_none()
        && let Some(uid) = first_usable_uid(&items)
    {
        profiles.current = Some(uid.as_str().into());
        changed = true;
    }
    if let Some(current) = profiles.current.clone()
        && !items.iter().any(|item| item.uid.as_ref() == Some(&current))
    {
        profiles.current = first_usable_uid(&items).map(|uid| uid.as_str().into());
        changed = true;
    }

    profiles.items = Some(items);
    Ok(ReconcileOutcome { created, changed })
}

fn is_safe_file_name(file: &str) -> bool {
    !file.is_empty() && !file.contains('/') && !file.contains('\\') && file != "." && file != ".."
}

fn ensure_item(items: &mut Vec<PrfItem>, uid: &str, role_type: &str, now: u64) -> usize {
    if let Some(index) = items.iter().position(|item| item.uid.as_deref() == Some(uid)) {
        return index;
    }
    let item = PrfItem {
        uid: Some(uid.into()),
        itype: Some(role_type.into()),
        file: Some(if role_type == "script" {
            format!("{uid}.js").into()
        } else {
            format!("{uid}.yaml").into()
        }),
        updated: Some((now / 1_000) as usize),
        ..PrfItem::default()
    };
    items.push(item);
    items.len() - 1
}

fn set_role_reference(item: &mut PrfItem, role: &str, uid: Option<std::string::String>) {
    let option = item.option.get_or_insert_with(PrfOption::default);
    let uid = uid.map(Into::into);
    match role {
        "script" => option.script = uid,
        "merge" => option.merge = uid,
        "proxies" => option.proxies = uid,
        "groups" => option.groups = uid,
        _ => {}
    }
}

fn role_reference(item: &PrfItem, role: &str) -> Option<std::string::String> {
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

fn reconcile_overrides(
    merged: &MergedOverrides,
    profiles: &mut IProfiles,
    now: u64,
    file_writes: &mut BTreeMap<std::string::String, Vec<u8>>,
    deletions: &mut Vec<PathBuf>,
) -> Result<bool> {
    let mut items = profiles.items.take().unwrap_or_default();
    let mut changed = false;

    for (path, content) in &merged.files {
        let Some((scope, role)) = parse_override_path(path) else {
            continue;
        };
        let Some(role_type) = role_item_type(role) else {
            continue;
        };
        if scope == "global" {
            let uid = if role == "script" { "Script" } else { "Merge" };
            if items
                .iter()
                .any(|item| item.uid.as_deref() == Some(uid) && item.itype.as_deref() != Some(role_type))
            {
                bail!("profile uid {uid} is reserved for a global override");
            }
            let index = ensure_item(&mut items, uid, role_type, now);
            if items[index].itype.as_deref() != Some(role_type) {
                items[index].itype = Some(role_type.into());
                changed = true;
            }
            let file = match items[index].file.clone() {
                Some(file) => file,
                None => {
                    let file: smartstring::alias::String = (if role == "script" {
                        format!("{uid}.js")
                    } else {
                        format!("{uid}.yaml")
                    })
                    .into();
                    items[index].file = Some(file.clone());
                    changed = true;
                    file
                }
            };
            if !is_safe_file_name(file.as_str()) {
                bail!("override file name is not a single filename: {file}");
            }
            file_writes.insert(file.to_string(), content.clone());
            continue;
        }

        let Some(profile_index) = items
            .iter()
            .position(|item| item.sync_id.as_deref() == Some(scope.as_str()))
        else {
            continue;
        };
        let linked_uid = role_reference(&items[profile_index], role);
        let linked_uid = linked_uid.filter(|uid| {
            items
                .iter()
                .any(|item| item.uid.as_deref() == Some(uid.as_str()) && item.itype.as_deref() == Some(role_type))
        });
        let uid = match linked_uid {
            Some(uid) => uid,
            None => {
                let prefix = match role {
                    "script" => "s",
                    "merge" => "m",
                    "proxies" => "p",
                    "groups" => "g",
                    _ => "x",
                };
                let uid = help::get_uid(prefix).to_string();
                set_role_reference(&mut items[profile_index], role, Some(uid.clone().into()));
                changed = true;
                uid
            }
        };
        let index = ensure_item(&mut items, &uid, role_type, now);
        if items[index].itype.as_deref() != Some(role_type) {
            items[index].itype = Some(role_type.into());
            changed = true;
        }
        let file = match items[index].file.clone() {
            Some(file) => file,
            None => {
                let file: smartstring::alias::String = (if role == "script" {
                    format!("{uid}.js")
                } else {
                    format!("{uid}.yaml")
                })
                .into();
                items[index].file = Some(file.clone());
                changed = true;
                file
            }
        };
        if !is_safe_file_name(file.as_str()) {
            bail!("override file name is not a single filename: {file}");
        }
        file_writes.insert(file.to_string(), content.clone());
    }

    for path in &merged.deleted {
        let Some((scope, role)) = parse_override_path(path) else {
            continue;
        };
        if scope == "global" {
            // Keep the global item and write a neutral template: removing it would let the
            // next launch recreate a default item that looks like a fresh local addition.
            let uid = if role == "script" { "Script" } else { "Merge" };
            let role_type = role_item_type(role).unwrap_or("script");
            if items
                .iter()
                .any(|item| item.uid.as_deref() == Some(uid) && item.itype.as_deref() != Some(role_type))
            {
                bail!("profile uid {uid} is reserved for a global override");
            }
            let index = ensure_item(&mut items, uid, role_type, now);
            let file = match items[index].file.clone() {
                Some(file) => file,
                None => {
                    let file: smartstring::alias::String = (if role == "script" {
                        format!("{uid}.js")
                    } else {
                        format!("{uid}.yaml")
                    })
                    .into();
                    items[index].file = Some(file.clone());
                    changed = true;
                    file
                }
            };
            if !is_safe_file_name(file.as_str()) {
                bail!("override file name is not a single filename: {file}");
            }
            let neutral = if role == "script" {
                tmpl::ITEM_SCRIPT.as_bytes().to_vec()
            } else {
                tmpl::ITEM_MERGE_EMPTY.as_bytes().to_vec()
            };
            file_writes.insert(file.to_string(), neutral);
            continue;
        }
        let Some(profile_index) = items
            .iter()
            .position(|item| item.sync_id.as_deref() == Some(scope.as_str()))
        else {
            continue;
        };
        let Some(uid) = role_reference(&items[profile_index], role) else {
            continue;
        };
        set_role_reference(&mut items[profile_index], role, None);
        changed = true;
        if !is_referenced(&items, uid.as_str())
            && let Some(index) = items.iter().position(|item| item.uid.as_deref() == Some(uid.as_str()))
        {
            let item = items.remove(index);
            schedule_item_deletion(&item, deletions)?;
        }
    }

    profiles.items = Some(items);
    Ok(changed)
}

fn is_referenced(items: &[PrfItem], uid: &str) -> bool {
    items.iter().any(|item| {
        ["script", "merge", "proxies", "groups"]
            .iter()
            .any(|role| role_reference(item, role).as_deref() == Some(uid))
    })
}

fn schedule_item_deletion(item: &PrfItem, deletions: &mut Vec<PathBuf>) -> Result<()> {
    if let Some(file) = item.file.as_ref()
        && is_safe_file_name(file.as_str())
    {
        deletions.push(dirs::app_profiles_dir()?.join(file.as_str()));
    }
    Ok(())
}

async fn apply_plan(plan: &ApplyPlan) -> Result<()> {
    let profiles_dir = dirs::app_profiles_dir()?;
    for (file, content) in &plan.file_writes {
        if !is_safe_file_name(file) {
            bail!("refusing to write an override outside the profiles directory");
        }
        super::state::write_bytes_atomic(&profiles_dir.join(file), content).await?;
    }
    if plan.profiles_changed {
        plan.profiles.save_file().await?;
    }
    for path in &plan.deletions {
        match fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => logging!(
                warn,
                Type::Backup,
                "Failed to remove a deleted profile file {}: {error:#}",
                path.display()
            ),
        }
    }

    if let Some(rules) = plan.rules.as_ref() {
        rules::apply_rules(rules).await?;
    }

    if let Some(values) = plan.settings.as_ref()
        && !values.is_empty()
    {
        let draft = Config::verge().await;
        let mut updated = (**draft.latest_arc()).clone();
        settings::apply_values(&mut updated, values)?;
        updated.save_file().await?;
        draft.edit_draft(|draft| *draft = updated);
        draft.apply();
    }

    if plan.profiles_changed {
        let profiles_draft = Config::profiles().await;
        let updated = plan.profiles.clone();
        profiles_draft.edit_draft(|draft| *draft = updated);
        profiles_draft.apply();
    }
    Ok(())
}

pub(crate) async fn restore_backup(backup_path: &Path) -> Result<()> {
    const RESTORE_TOTAL_MAX: u64 = 200 * 1024 * 1024;

    let path = backup_path.to_path_buf();
    let file = AsyncHandler::spawn_blocking(move || std::fs::File::open(&path)).await??;
    let mut zip = zip::ZipArchive::new(file)?;

    let home = dirs::app_home_dir()?;
    let profiles_dir = dirs::app_profiles_dir()?;

    // Fully read and validate the archive before touching the live profile directory.
    // Otherwise a corrupt/truncated backup would leave profiles deleted or half-restored.
    let mut restored_files = Vec::new();
    let mut total = 0_u64;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        let target = if let Ok(relative) = enclosed.strip_prefix("profiles") {
            profiles_dir.join(relative)
        } else if enclosed == std::path::Path::new(dirs::PROFILE_YAML)
            || enclosed == std::path::Path::new(crate::enhance::custom_rules::FILE_NAME)
        {
            home.join(&enclosed)
        } else {
            continue;
        };
        let entry_size = entry.size();
        if entry_size > RESTORE_TOTAL_MAX || total.saturating_add(entry_size) > RESTORE_TOTAL_MAX {
            anyhow::bail!("backup exceeds the restore size limit");
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes)?;
        total = total.saturating_add(bytes.len() as u64);
        if total > RESTORE_TOTAL_MAX {
            anyhow::bail!("backup exceeds the restore size limit");
        }
        restored_files.push((target, bytes));
    }

    let _profile_write = crate::config::profiles::PROFILE_WRITE_LOCK.lock().await;
    match fs::remove_dir_all(&profiles_dir).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("failed to clear profile files before restore"),
    }
    fs::create_dir_all(&profiles_dir).await?;

    for (target, bytes) in restored_files {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).await?;
        }
        super::state::write_bytes_atomic(&target, &bytes).await?;
    }

    let restored = IProfiles::new().await;
    let profiles_draft = Config::profiles().await;
    profiles_draft.edit_draft(|draft| *draft = restored);
    profiles_draft.apply();
    crate::enhance::custom_rules::invalidate_cache();
    Ok(())
}

pub(crate) async fn restore_verge(previous: &IVerge) -> Result<()> {
    let draft = Config::verge().await;
    let restored = previous.clone();
    draft.edit_draft(|draft| *draft = restored);
    draft.apply();
    previous.save_file().await
}

pub(crate) async fn enhance_and_refresh() -> Result<()> {
    match crate::feat::enhance_profiles().await? {
        ValidationOutcome::Valid => {}
        ValidationOutcome::Skipped { .. } | ValidationOutcome::Busy => {
            logging!(
                info,
                Type::Backup,
                "Sync apply skipped runtime validation; local files are applied"
            );
        }
        outcome => bail!("the updated configuration was rejected: {outcome}"),
    }
    Timer::global().refresh().await?;
    NodeHealthManager::global().refresh_settings().await?;
    Ok(())
}

fn fetch_created_profiles(created: Vec<std::string::String>) {
    if created.is_empty() {
        return;
    }
    AsyncHandler::spawn(move || async move {
        for uid in created {
            let uid: smartstring::alias::String = uid.into();
            match crate::feat::update_profile(&uid, None, true).await {
                Ok(_) => logging!(
                    info,
                    Type::Backup,
                    "Fetched content for a subscription restored from sync: {uid}"
                ),
                Err(error) => logging!(
                    warn,
                    Type::Backup,
                    "Could not fetch a subscription restored from sync ({uid}): {error:#}"
                ),
            }
        }
    });
}

pub fn record_conflict_yaml(record: &SubscriptionRecord) -> Vec<u8> {
    serde_yaml_ng::to_string(record)
        .map(|body| body.into_bytes())
        .unwrap_or_default()
}

pub fn rule_conflicts(conflicts: &[super::rules::RuleConflict]) -> Vec<(std::string::String, Vec<u8>)> {
    conflicts
        .iter()
        .map(|conflict| {
            let body = serde_yaml_ng::to_string(&conflict.rule)
                .map(|body| format!("# side: {}\n\n{body}", conflict.side).into_bytes())
                .unwrap_or_default();
            (conflict.rule.id.to_string(), body)
        })
        .collect()
}

pub fn record_conflicts(conflicts: &[super::records::RecordConflict]) -> Vec<(std::string::String, Vec<u8>)> {
    conflicts
        .iter()
        .map(|conflict| {
            let body = record_conflict_yaml(&conflict.record);
            let mut bytes = format!(
                "# side: {}
",
                conflict.side
            )
            .into_bytes();
            bytes.extend_from_slice(&body);
            (conflict.record.sync_id.to_string(), bytes)
        })
        .collect()
}

pub fn override_conflicts(merged: &MergedOverrides) -> Vec<(std::string::String, Vec<u8>)> {
    merged
        .conflicts
        .iter()
        .map(|(path, bytes, _side)| (path.clone(), bytes.clone()))
        .collect()
}
