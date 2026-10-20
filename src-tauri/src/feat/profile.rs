use crate::core::notify::NoticeStatus;
use crate::core::notify::{Refresh, announce};
use crate::enhance::custom_rules::{self, RuleConflict};
use crate::enhance::seq::SeqMap;
use crate::{
    cmd,
    config::{Config, PrfItem, PrfOption, profiles::profiles_update_item_safe},
    core::{CoreManager, handle, tray, validate::ValidationOutcome},
    utils::{
        dirs, help,
        help::{mask_err, mask_url},
    },
};
use anyhow::{Context as _, Result, bail};
use clash_verge_logging::{Type, logging, logging_error};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};
use smartstring::alias::String;
use std::{collections::HashMap, path::PathBuf, sync::LazyLock};

const MAX_STAGED_UPDATES: usize = 32;

#[derive(Default)]
struct StagedSnapshots {
    custom: Option<Option<Vec<u8>>>,
    legacy: Option<Option<(PathBuf, Vec<u8>)>>,
}

static STAGED_UPDATES: LazyLock<Mutex<HashMap<String, (PrfItem, StagedSnapshots)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ProfileUpdateReport {
    Updated,
    Skipped,
    Conflict {
        conflicts: Vec<RuleConflict>,
        staged: bool,
        targets: custom_rules::RuleTargets,
    },
    Cancelled,
}

#[derive(Debug, Clone, Copy, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ConflictAction {
    Remap,
    Direct,
    Reject,
    Delete,
}

#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RuleConflictResolution {
    #[specta(type = Option<std::string::String>)]
    pub rule_id: Option<String>,
    #[serde(default)]
    #[specta(type = std::string::String)]
    pub rule: String,
    pub action: ConflictAction,
    #[specta(type = Option<std::string::String>)]
    pub policy: Option<String>,
}

fn stage_update(uid: &String, item: PrfItem) {
    stage_update_with(uid, item, StagedSnapshots::default());
}

fn stage_update_with(uid: &String, item: PrfItem, snapshots: StagedSnapshots) {
    let mut staged = STAGED_UPDATES.lock();
    if staged.len() >= MAX_STAGED_UPDATES && !staged.contains_key(uid.as_str()) {
        if let Some(oldest) = staged.keys().next().cloned() {
            staged.remove(oldest.as_str());
        }
    }
    staged.insert(uid.clone(), (item, snapshots));
}

fn take_staged_update(uid: &str) -> Option<(PrfItem, StagedSnapshots)> {
    STAGED_UPDATES.lock().remove(uid)
}

fn drop_staged_update(uid: &str) -> Option<(PrfItem, StagedSnapshots)> {
    STAGED_UPDATES.lock().remove(uid)
}

/// Toggle proxy profile
pub async fn toggle_proxy_profile(profile_index: String) {
    if let Err(err) = cmd::patch_profiles_config_by_profile_index(profile_index.clone()).await {
        logging!(
            error,
            Type::Config,
            "toggle proxy profile {profile_index} failed: {err}"
        );
    }
}

/// Tell the profile which node this group is on now.
///
/// The core is already switched by the time this runs, so failing to record is not a reason to
/// report the switch as failed — but it is worth a log line, because what the profile holds is
/// what gets re-applied the next time a core starts.
async fn record_switched_node(group_name: &str, proxy_name: &str) {
    if let Err(error) = crate::config::profiles::record_selected_node(group_name, proxy_name).await {
        logging!(
            warn,
            Type::Config,
            "切换代理成功但未能记录到配置: {} -> {}, 错误: {error:#}",
            group_name,
            proxy_name
        );
    }
}

pub async fn switch_proxy_node(group_name: &str, proxy_name: &str) {
    match handle::Handle::mihomo()
        .select_node_for_group(group_name, proxy_name)
        .await
    {
        Ok(_) => {
            record_switched_node(group_name, proxy_name).await;
            announce(Refresh::Proxies);
            let _ = tray::Tray::global().update_menu().await;
            return;
        }
        Err(err) => {
            logging!(error, Type::Tray, "切换代理失败: {err:?}");
        }
    }

    match handle::Handle::mihomo()
        .select_node_for_group(group_name, proxy_name)
        .await
    {
        Ok(_) => {
            logging!(info, Type::Tray, "代理切换回退成功: {} -> {}", group_name, proxy_name);
            record_switched_node(group_name, proxy_name).await;
            let _ = tray::Tray::global().update_menu().await;
        }
        Err(err) => {
            logging!(error, Type::Tray, "代理切换最终失败: {err:?}");
        }
    }
}

async fn should_update_profile(uid: &String, ignore_auto_update: bool) -> Result<Option<(String, Option<PrfOption>)>> {
    let profiles = Config::profiles().await;
    let profiles = profiles.latest_arc();
    let item = profiles.get_item(uid)?;
    let is_remote = item.itype.as_ref().is_some_and(|s| s == "remote");

    if !is_remote {
        logging!(info, Type::Config, "[订阅更新] 不是远程订阅，跳过更新");
        Ok(None)
    } else if item.url.is_none() {
        logging!(warn, Type::Config, "[订阅更新] 缺少URL，无法更新");
        bail!("failed to get the profile item url");
    } else if !ignore_auto_update && !item.option.as_ref().and_then(|o| o.allow_auto_update).unwrap_or(true) {
        logging!(info, Type::Config, "[订阅更新] 禁止自动更新，跳过更新");
        Ok(None)
    } else {
        logging!(
            info,
            Type::Config,
            "[订阅更新] 远程订阅，URL: {}",
            mask_url(
                item.url
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Profile URL is None"))?
            )
        );
        Ok(Some((
            item.url.clone().ok_or_else(|| anyhow::anyhow!("Profile URL is None"))?,
            item.option.clone(),
        )))
    }
}

async fn preflight_update(uid: &String, item: &PrfItem) -> Result<(Vec<RuleConflict>, custom_rules::RuleTargets)> {
    let content = item.file_data.as_deref().context("downloaded profile has no content")?;
    let config: Mapping = serde_yaml_ng::from_str(content).context("downloaded profile is not valid yaml")?;
    let targets = custom_rules::targets_for_profile_with_config(uid, config).await?;

    let mut conflicts = Vec::new();
    let sync_id = match cmd::custom_rules::profile_sync_id_for_uid(uid).await {
        Some(sync_id) => sync_id,
        None => crate::config::profiles::ensure_profile_sync_id(uid)
            .await
            .with_context(|| format!("failed to ensure a stable sync id for profile \"{uid}\""))?,
    };
    let (_, custom_conflicts) =
        custom_rules::validate_rules_for_profile(&sync_id, &targets, targets.has_script).await?;
    conflicts.extend(custom_conflicts);
    conflicts.extend(legacy_rule_conflicts(uid, &targets).await?);
    Ok((conflicts, targets))
}

async fn legacy_rule_conflicts(uid: &String, targets: &custom_rules::RuleTargets) -> Result<Vec<RuleConflict>> {
    let profiles = Config::profiles().await;
    let snapshot = profiles.latest_arc();
    let item = snapshot.get_item(uid)?;
    let rules_uid = item.current_rules().map_or("Rules", String::as_str);
    let Ok(rules_item) = snapshot.get_item(rules_uid) else {
        return Ok(Vec::new());
    };
    let Some(file) = rules_item.file.as_ref() else {
        return Ok(Vec::new());
    };
    let path = dirs::app_profiles_dir()?.join(file.as_str());
    if !path.exists() {
        return Ok(Vec::new());
    }
    let sequence = help::read_yaml::<SeqMap>(&path)
        .await
        .with_context(|| format!("failed to read legacy rules file \"{}\"", path.display()))?;

    let mut conflicts = Vec::new();
    for (position, rules) in [("prepend", &sequence.prepend), ("append", &sequence.append)] {
        for raw in rules.iter().filter_map(Value::as_str) {
            let Some(problem) = custom_rules::validate_raw_rule(raw, targets, targets.has_script) else {
                continue;
            };
            conflicts.push(RuleConflict {
                source: "legacy".into(),
                rule_id: None,
                rule: raw.into(),
                position: Some(position.into()),
                code: problem.code.into(),
                missing: problem.message.as_str().into(),
            });
        }
    }
    Ok(conflicts)
}

async fn handle_update_conflicts(
    uid: &String,
    item: PrfItem,
    conflicts: Vec<RuleConflict>,
    targets: custom_rules::RuleTargets,
    is_mannual_trigger: bool,
) -> Result<ProfileUpdateReport> {
    if is_mannual_trigger {
        stage_update(uid, item);
        return Ok(ProfileUpdateReport::Conflict {
            conflicts,
            staged: true,
            targets,
        });
    }

    let message = format!(
        "subscription \"{uid}\" was not updated because {} custom rule(s) reference targets the new profile no longer provides",
        conflicts.len()
    );
    if should_notify_update_failure().await {
        handle::Handle::notice(NoticeStatus::UpdateFailed, message.as_str());
    }
    bail!(message)
}

async fn should_notify_update_failure() -> bool {
    Config::verge()
        .await
        .latest_arc()
        .subscription_settings
        .as_ref()
        .and_then(|settings| settings.notify_on_failure)
        .unwrap_or(true)
}

/// Applies resolutions to staged rules, then re-validates and commits the staged profile.
pub async fn resolve_update_conflicts(
    uid: &String,
    resolutions: Vec<RuleConflictResolution>,
) -> Result<ProfileUpdateReport> {
    let _operation = custom_rules::CUSTOM_RULES_OPERATION_LOCK.lock().await;
    let Some((mut item, mut snapshots)) = take_staged_update(uid) else {
        bail!("no staged subscription update for \"{uid}\"");
    };

    if resolutions.is_empty() {
        if let Err(error) = restore_staged_snapshots(&snapshots).await {
            stage_update_with(uid, item, snapshots);
            return Err(error);
        }
        return Ok(ProfileUpdateReport::Cancelled);
    }

    if snapshots.custom.is_none() {
        match custom_rules::snapshot_file_checked().await {
            Ok(snapshot) => snapshots.custom = Some(snapshot),
            Err(error) => {
                stage_update_with(uid, item, snapshots);
                return Err(error);
            }
        }
    }
    if snapshots.legacy.is_none() {
        snapshots.legacy = Some(legacy_rules_snapshot(uid).await);
    }

    let applied = async {
        apply_custom_rule_resolutions(&resolutions).await?;
        apply_legacy_resolutions(uid, &resolutions).await?;
        preflight_update(uid, &item).await
    }
    .await;

    let (conflicts, targets) = match applied {
        Ok(value) => value,
        Err(error) => {
            restore_best_effort(&snapshots).await;
            stage_update_with(uid, item, snapshots);
            return Err(error);
        }
    };

    if !conflicts.is_empty() {
        stage_update_with(uid, item, snapshots);
        return Ok(ProfileUpdateReport::Conflict {
            conflicts,
            staged: true,
            targets,
        });
    }

    let file_data = item.file_data.clone();
    let previous_file = profile_file_snapshot(uid).await;
    if let Err(error) = profiles_update_item_safe(uid, &mut item).await {
        item.file_data = file_data;
        restore_profile_file_snapshot(previous_file).await;
        restore_best_effort(&snapshots).await;
        stage_update_with(uid, item, snapshots);
        return Err(error);
    }

    if let Err(error) = update_running_profile(uid, true, true).await {
        restore_best_effort(&snapshots).await;
        restore_profile_file_snapshot(previous_file).await;
        item.file_data = file_data;
        let _ = update_running_profile(uid, false, true).await;
        stage_update_with(uid, item, snapshots);
        return Err(error);
    }
    Ok(ProfileUpdateReport::Updated)
}

pub async fn cancel_staged_update(uid: &String) -> Result<ProfileUpdateReport> {
    let _operation = custom_rules::CUSTOM_RULES_OPERATION_LOCK.lock().await;
    if let Some((_, snapshots)) = drop_staged_update(uid) {
        restore_staged_snapshots(&snapshots).await?;
    }
    Ok(ProfileUpdateReport::Cancelled)
}

async fn legacy_rules_snapshot(uid: &String) -> Option<(PathBuf, Vec<u8>)> {
    let profiles = Config::profiles().await;
    let snapshot = profiles.latest_arc();
    let item = snapshot.get_item(uid).ok()?;
    let rules_uid = item.current_rules().map_or("Rules", String::as_str);
    let rules_item = snapshot.get_item(rules_uid).ok()?;
    let file = rules_item.file.as_ref()?;
    let path = dirs::app_profiles_dir().ok()?.join(file.as_str());
    let bytes = tokio::fs::read(&path).await.ok()?;
    Some((path, bytes))
}

async fn restore_staged_snapshots(snapshots: &StagedSnapshots) -> Result<()> {
    if let Some(custom) = snapshots.custom.clone() {
        custom_rules::restore_snapshot(custom)
            .await
            .context("failed to restore custom rules after cancelled conflict resolution")?;
    }

    if let Some(Some((path, bytes))) = snapshots.legacy.as_ref() {
        tokio::fs::write(path, bytes)
            .await
            .with_context(|| format!("failed to restore legacy rules file \"{}\"", path.display()))?;
    }

    Ok(())
}

async fn restore_best_effort(snapshots: &StagedSnapshots) {
    if let Err(error) = restore_staged_snapshots(snapshots).await {
        logging!(
            error,
            Type::Config,
            "failed to restore rules after conflict resolution error: {error:#}"
        );
    }
}

async fn apply_custom_rule_resolutions(resolutions: &[RuleConflictResolution]) -> Result<()> {
    let custom = resolutions
        .iter()
        .filter(|resolution| resolution.rule_id.is_some())
        .map(|resolution| {
            (
                resolution.rule_id.clone().unwrap_or_default(),
                resolution.action,
                resolution.policy.clone(),
            )
        })
        .collect::<Vec<_>>();
    if custom.is_empty() {
        return Ok(());
    }

    custom_rules::mutate_rules(|rules| {
        for (rule_id, action, policy) in &custom {
            let Some(rule) = rules
                .iter_mut()
                .find(|rule| rule.id == rule_id.as_str() && !rule.deleted)
            else {
                continue;
            };
            match action {
                ConflictAction::Delete => rule.deleted = true,
                ConflictAction::Direct => {
                    rule.policy = "DIRECT".into();
                    rule.policy_kind = "builtin".into();
                }
                ConflictAction::Reject => {
                    rule.policy = "REJECT".into();
                    rule.policy_kind = "builtin".into();
                }
                ConflictAction::Remap => {
                    if let Some(policy) = policy {
                        rule.policy = policy.as_str().into();
                        rule.policy_kind = if custom_rules::BUILTIN_POLICIES.contains(&policy.as_str()) {
                            "builtin".into()
                        } else {
                            "group".into()
                        };
                    }
                }
            }
            rule.updated_at = now_millis();
        }
        Ok(())
    })
    .await?;
    Ok(())
}

async fn apply_legacy_resolutions(uid: &String, resolutions: &[RuleConflictResolution]) -> Result<()> {
    let legacy = resolutions
        .iter()
        .filter(|resolution| resolution.rule_id.is_none() && !resolution.rule.is_empty())
        .collect::<Vec<_>>();
    if legacy.is_empty() {
        return Ok(());
    }

    let profiles = Config::profiles().await;
    let snapshot = profiles.latest_arc();
    let item = snapshot.get_item(uid)?;
    let rules_uid = item.current_rules().map_or("Rules", String::as_str);
    let rules_item = snapshot.get_item(rules_uid)?;
    let Some(file) = rules_item.file.as_ref() else {
        return Ok(());
    };
    let path = dirs::app_profiles_dir()?.join(file.as_str());
    let mut sequence = help::read_yaml::<SeqMap>(&path).await?;

    for resolution in legacy {
        let replacement = match resolution.action {
            ConflictAction::Delete => None,
            ConflictAction::Direct => Some(replace_rule_policy(&resolution.rule, "DIRECT")),
            ConflictAction::Reject => Some(replace_rule_policy(&resolution.rule, "REJECT")),
            ConflictAction::Remap => resolution
                .policy
                .as_ref()
                .map(|policy| replace_rule_policy(&resolution.rule, policy)),
        };

        for rules in [&mut sequence.prepend, &mut sequence.append] {
            if let Some(index) = rules
                .iter()
                .position(|value| value.as_str() == Some(resolution.rule.as_str()))
            {
                match &replacement {
                    Some(replacement) => rules[index] = Value::String(replacement.as_str().into()),
                    None => {
                        rules.remove(index);
                    }
                }
                break;
            }
        }
    }

    help::save_yaml(&path, &sequence, None).await?;
    Ok(())
}

fn replace_rule_policy(raw: &str, policy: &str) -> std::string::String {
    let parts = raw.split(',').map(str::trim).collect::<Vec<_>>();
    let mut end = parts.len();
    if parts.last().is_some_and(|part| part.eq_ignore_ascii_case("no-resolve")) {
        end -= 1;
    }
    if end < 2 {
        return raw.to_owned();
    }
    let no_resolve = end != parts.len();
    let mut updated = parts[..end - 1].to_vec();
    updated.push(policy);
    if no_resolve {
        updated.push("no-resolve");
    }
    updated.join(",")
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

#[tracing::instrument(skip_all, level = "info", fields(uid = %uid, strategy = tracing::field::Empty, configured_elapsed = tracing::field::Empty, clash_elapsed = tracing::field::Empty, system_elapsed = tracing::field::Empty))]
async fn perform_profile_update(
    uid: &String,
    url: &String,
    opt: Option<&PrfOption>,
    option: Option<&PrfOption>,
    is_mannual_trigger: bool,
) -> Result<ProfileUpdateReport> {
    let merged_opt = PrfOption::merge(opt, option);
    let profiles = Config::profiles().await;
    let profiles_arc = profiles.latest_arc();
    let profile_name = profiles_arc
        .get_item(uid)
        .ok()
        .and_then(|item| item.name.clone())
        .unwrap_or_else(|| String::from("UnKnown Profile"));

    #[derive(Clone, Copy)]
    enum Strategy {
        Configured,
        Clash,
        System,
    }

    let result =
        crate::utils::retry::try_strategies(Strategy::Configured, [Strategy::Clash, Strategy::System], |strategy| {
            let mut options = merged_opt.clone();
            async move {
                let (name, success, failure) = match strategy {
                    Strategy::Configured => (
                        "configured",
                        "[订阅更新] 更新订阅配置成功",
                        "[订阅更新] 直接更新失败: {}，尝试使用Clash代理更新",
                    ),
                    Strategy::Clash => (
                        "clash",
                        "[订阅更新] 使用 Clash代理 更新订阅配置成功",
                        "[订阅更新] Clash代理更新失败: {}，尝试使用系统代理更新",
                    ),
                    Strategy::System => (
                        "system",
                        "[订阅更新] 使用 系统代理 更新订阅配置成功",
                        "[订阅更新] 系统代理更新失败: {}，所有重试均已失败",
                    ),
                };
                match strategy {
                    Strategy::Configured => {}
                    Strategy::Clash | Strategy::System => {
                        let option = options.get_or_insert_with(PrfOption::default);
                        option.self_proxy = Some(matches!(strategy, Strategy::Clash));
                        option.with_proxy = Some(matches!(strategy, Strategy::System));
                    }
                }
                tracing::Span::current().record("strategy", name);
                let started = std::time::Instant::now();
                let result = PrfItem::from_url(url, None, None, options.as_ref()).await;
                let elapsed_field = match strategy {
                    Strategy::Configured => "configured_elapsed",
                    Strategy::Clash => "clash_elapsed",
                    Strategy::System => "system_elapsed",
                };
                tracing::Span::current().record(elapsed_field, tracing::field::debug(started.elapsed()));
                match &result {
                    Ok(_) => logging!(info, Type::Config, "{success}"),
                    Err(error) => logging!(
                        warn,
                        Type::Config,
                        "{}",
                        failure.replace("{}", &mask_err(&format!("{error:#}")))
                    ),
                }
                result
            }
        })
        .await;
    let last_err = match result {
        Ok((strategy, mut item)) => {
            let (conflicts, targets) = preflight_update(uid, &item).await?;
            if !conflicts.is_empty() {
                return handle_update_conflicts(uid, item, conflicts, targets, is_mannual_trigger).await;
            }

            // A persistence error must not replay a successful download with another transport.
            profiles_update_item_safe(uid, &mut item).await?;
            if !matches!(strategy, Strategy::Configured) {
                handle::Handle::notice(NoticeStatus::UpdateWithClashProxy, profile_name.as_str());
            }
            return Ok(ProfileUpdateReport::Updated);
        }
        Err(error) => error,
    };

    let last_err = mask_err(&last_err.to_string());
    if is_mannual_trigger {
        handle::Handle::notice(
            NoticeStatus::UpdateFailedEvenWithClash,
            format!("{profile_name} - {last_err}"),
        );
    } else if should_notify_update_failure().await {
        handle::Handle::notice(NoticeStatus::UpdateFailed, format!("{profile_name} - {last_err}"));
    }
    bail!(last_err)
}

async fn update_running_profile(uid: &String, profile_persisted: bool, is_mannual_trigger: bool) -> Result<()> {
    let profiles = Config::profiles().await;
    let is_current = profiles.latest_arc().current.as_ref() == Some(uid);
    let should_refresh = is_current || (!profile_persisted && is_mannual_trigger);

    if should_refresh {
        logging!(debug, Type::Config, "[订阅更新] 更新内核配置");
        match CoreManager::global().update_config_with_force(is_mannual_trigger).await {
            Ok(outcome) if outcome.is_valid() => {
                logging_error!(Type::Config, Config::sync_dns_override().await);
                announce(Refresh::Clash);
            }
            Ok(outcome @ (ValidationOutcome::Skipped { .. } | ValidationOutcome::Busy)) if !is_mannual_trigger => {
                logging!(info, Type::Config, "[订阅更新] 本次配置刷新已跳过: {}", outcome);
            }
            result => {
                let message = match result {
                    Ok(outcome) => outcome.to_string(),
                    Err(err) => err.to_string(),
                };
                let message = mask_err(&message);
                let message = if profile_persisted {
                    format!("订阅已保存，但未能确认已应用到内核: {message}")
                } else {
                    message
                };
                logging!(error, Type::Config, "[订阅更新] 更新失败: {}", message);
                if is_mannual_trigger || should_notify_update_failure().await {
                    handle::Handle::notice(NoticeStatus::UpdateFailed, message.as_str());
                }
                bail!(message);
            }
        }
    }

    Ok(())
}

async fn profile_file_snapshot(uid: &String) -> Option<(PathBuf, Vec<u8>)> {
    let profiles = Config::profiles().await;
    let snapshot = profiles.latest_arc();
    let item = snapshot.get_item(uid).ok()?;
    let file = item.file.as_ref()?;
    let path = dirs::app_profiles_dir().ok()?.join(file.as_str());
    let bytes = tokio::fs::read(&path).await.ok()?;
    Some((path, bytes))
}

async fn restore_profile_file_snapshot(snapshot: Option<(PathBuf, Vec<u8>)>) {
    if let Some((path, bytes)) = snapshot
        && let Err(error) = tokio::fs::write(&path, bytes).await
    {
        logging!(
            error,
            Type::Config,
            "Failed to restore the previous profile file {}: {error:#}",
            path.display()
        );
    }
}

#[tracing::instrument(skip_all, level = "info", fields(uid = %uid, manual = is_mannual_trigger))]
pub async fn update_profile(
    uid: &String,
    option: Option<&PrfOption>,
    is_mannual_trigger: bool,
) -> Result<ProfileUpdateReport> {
    let url_opt = should_update_profile(uid, is_mannual_trigger).await?;

    let Some((url, opt)) = url_opt else {
        update_running_profile(uid, false, is_mannual_trigger).await?;
        return Ok(ProfileUpdateReport::Skipped);
    };

    let previous_file = profile_file_snapshot(uid).await;
    let report = match perform_profile_update(uid, &url, opt.as_ref(), option, is_mannual_trigger).await {
        Ok(report) => report,
        Err(error) => {
            restore_profile_file_snapshot(previous_file.clone()).await;
            let _ = update_running_profile(uid, false, is_mannual_trigger).await;
            return Err(error);
        }
    };
    if matches!(report, ProfileUpdateReport::Conflict { .. }) {
        return Ok(report);
    }

    if let Err(error) = update_running_profile(uid, true, is_mannual_trigger).await {
        if let Some((path, bytes)) = previous_file {
            if let Err(restore_error) = tokio::fs::write(&path, bytes).await {
                logging!(
                    error,
                    Type::Config,
                    "Failed to restore the previous profile file after an apply failure: {restore_error:#}"
                );
            } else if let Err(reapply_error) = update_running_profile(uid, false, is_mannual_trigger).await {
                logging!(
                    error,
                    Type::Config,
                    "Restored the previous profile file but could not re-apply it: {reapply_error:#}"
                );
            }
        }
        return Err(error);
    }
    Ok(report)
}

/// 增强配置
pub async fn enhance_profiles() -> Result<ValidationOutcome> {
    let outcome = CoreManager::global().update_config_forced().await?;
    if outcome.is_valid() {
        logging_error!(Type::Config, Config::sync_dns_override().await);
    }
    Ok(outcome)
}
