use super::{CmdResult, coded_error};
use crate::{
    config::{
        Config,
        profiles::{ensure_profile_sync_id as ensure_sync_id, profile_sync_id},
    },
    core::CoreManager,
    core::validate::ValidationOutcome,
    enhance::custom_rules::{
        self, CustomRule, MAX_RULES, RuleIssue, RulePosition, RuleProblem, RuleTargets, duplicate_rule,
    },
    utils::help,
};
use clash_verge_logging::{Type, logging};
use serde::Deserialize;
use smartstring::alias::String;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CustomRuleInput {
    #[specta(type = std::string::String)]
    pub profile_uid: String,
    #[specta(type = std::string::String)]
    pub rule_type: String,
    #[serde(default)]
    #[specta(type = std::string::String)]
    pub value: String,
    #[specta(type = std::string::String)]
    pub policy: String,
    #[specta(type = Option<std::string::String>)]
    pub policy_kind: Option<String>,
    pub position: RulePosition,
    #[serde(default)]
    pub no_resolve: bool,
    #[serde(default)]
    pub apply_to_all: bool,
}

#[derive(Debug)]
struct RuleCommandError {
    code: &'static str,
    detail: std::string::String,
}

impl std::fmt::Display for RuleCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for RuleCommandError {}

fn rule_error(code: &'static str, detail: impl std::fmt::Display) -> anyhow::Error {
    anyhow::Error::new(RuleCommandError {
        code,
        detail: detail.to_string().into(),
    })
}

fn command_error(error: anyhow::Error) -> super::CommandFailure {
    if let Some(error) = error.downcast_ref::<RuleCommandError>() {
        return coded_error(error.code, &error.detail);
    }
    if let Some(problem) = error.downcast_ref::<RuleProblem>() {
        return coded_error(problem_code(problem.code), &problem.message);
    }
    coded_error("CUSTOM_RULE_FAILED", format_args!("{error:#}"))
}

fn problem_code(code: &str) -> &'static str {
    match code {
        "limit" => "CUSTOM_RULE_LIMIT",
        "policy" => "CUSTOM_RULE_POLICY_NOT_FOUND",
        "provider" => "CUSTOM_RULE_PROVIDER_NOT_FOUND",
        "subRule" => "CUSTOM_RULE_SUB_RULE_NOT_FOUND",
        "duplicate" => "CUSTOM_RULE_DUPLICATE",
        _ => "CUSTOM_RULE_INVALID",
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

async fn current_profile_uid() -> Option<String> {
    Config::profiles().await.latest_arc().current.clone()
}

async fn sync_id_or_error(uid: &str) -> CmdResult<String> {
    ensure_sync_id(uid).await.map_err(coded_error_profile)
}

fn coded_error_profile(error: anyhow::Error) -> super::CommandFailure {
    coded_error("CUSTOM_RULE_PROFILE_NOT_FOUND", format_args!("{error:#}"))
}

/// Re-applies the current profile after a rule change; failures roll the rule file back.
async fn reapply_current_profile(rule_profile: &str, profile_uid: Option<&str>) -> CmdResult<()> {
    let snapshot = Config::profiles().await.latest_arc();
    let affects_current =
        rule_profile == "*" || profile_uid.is_some_and(|uid| snapshot.current.as_deref() == Some(uid));
    if !affects_current {
        return Ok(());
    }

    match CoreManager::global().update_config_forced().await {
        Ok(outcome) if outcome.is_valid() => {
            crate::core::handle::Handle::refresh_clash();
            Ok(())
        }
        Ok(outcome @ (ValidationOutcome::Skipped { .. } | ValidationOutcome::Busy)) => {
            logging!(
                info,
                Type::Config,
                "custom rule change saved but runtime apply was skipped: {outcome}"
            );
            Ok(())
        }
        Ok(outcome) => Err(coded_error("CUSTOM_RULE_APPLY_FAILED", outcome.to_string())),
        Err(error) => Err(coded_error("CUSTOM_RULE_APPLY_FAILED", format_args!("{error:#}"))),
    }
}

#[tauri::command]
#[specta::specta]
pub async fn list_custom_rules(profile_uid: Option<std::string::String>) -> CmdResult<Vec<CustomRule>> {
    let uid = match profile_uid {
        Some(uid) => String::from(uid),
        None => current_profile_uid()
            .await
            .ok_or_else(|| coded_error("CUSTOM_RULE_PROFILE_NOT_FOUND", "no profile is selected"))?,
    };

    let sync_id = sync_id_or_error(uid.as_str()).await?;
    let rules = custom_rules::load_rules().await;
    Ok(rules
        .iter()
        .filter(|rule| !rule.deleted && (rule.profile == sync_id || rule.profile == "*"))
        .cloned()
        .collect())
}

#[tauri::command]
#[specta::specta]
pub async fn get_rule_targets(profile_uid: std::string::String) -> CmdResult<RuleTargets> {
    let uid = String::from(profile_uid);
    Config::profiles()
        .await
        .latest_arc()
        .get_item(uid.as_str())
        .map_err(coded_error_profile)?;
    custom_rules::targets_for_profile(uid.as_str())
        .await
        .map_err(|error| coded_error("CUSTOM_RULE_TARGETS_FAILED", format_args!("{error:#}")))
}

#[tauri::command]
#[specta::specta]
pub async fn ensure_profile_sync_id(uid: std::string::String) -> CmdResult<std::string::String> {
    let sync_id = crate::config::profiles::ensure_profile_sync_id(uid.as_str())
        .await
        .map_err(coded_error_profile)?;
    Ok(sync_id.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn add_custom_rule(payload: CustomRuleInput) -> CmdResult<CustomRule> {
    let _operation = custom_rules::CUSTOM_RULES_OPERATION_LOCK.lock().await;
    let rule_type = payload.rule_type.trim().to_owned();
    if rule_type.eq_ignore_ascii_case("MATCH") {
        return Err(coded_error(
            "CUSTOM_RULE_INVALID",
            "MATCH is not available from quick add",
        ));
    }
    let value = payload.value.trim().to_owned();
    let policy = payload.policy.trim().to_owned();

    custom_rules::validate_rule_shape(&rule_type, &value, &policy)
        .map_err(|problem| command_error(anyhow::Error::new(problem)))?;

    let (target_uid, rule_profile, policy_kind) = if payload.apply_to_all {
        if payload.policy_kind.as_deref() == Some("group") {
            return Err(coded_error(
                "CUSTOM_RULE_INVALID",
                "global rules support built-in policies only",
            ));
        }
        let current = current_profile_uid()
            .await
            .ok_or_else(|| coded_error("CUSTOM_RULE_PROFILE_NOT_FOUND", "no profile is selected"))?;
        (current, String::from("*"), String::from("builtin"))
    } else {
        let uid = String::from(payload.profile_uid.trim());
        let sync_id = sync_id_or_error(uid.as_str()).await?;
        (
            uid,
            sync_id,
            String::from(payload.policy_kind.as_deref().unwrap_or("group")),
        )
    };

    let targets = custom_rules::targets_for_profile(target_uid.as_str())
        .await
        .map_err(|error| coded_error("CUSTOM_RULE_TARGETS_FAILED", format_args!("{error:#}")))?;

    let candidate = CustomRule {
        id: String::from(help::get_uid("C")),
        profile: rule_profile.clone(),
        position: payload.position,
        rule_type: rule_type.into(),
        value: value.into(),
        policy: policy.into(),
        policy_kind: policy_kind.clone(),
        no_resolve: payload.no_resolve,
        enabled: true,
        created_at: now_millis(),
        updated_at: now_millis(),
        deleted: false,
    };

    if let Err(problem) = custom_rules::validate_rule_against_targets(&candidate, &targets) {
        let script_group =
            problem.code == "policy" && policy_kind != "builtin" && targets.has_script && !payload.apply_to_all;
        if !script_group {
            return Err(command_error(anyhow::Error::new(problem)));
        }
    }

    let previous = custom_rules::snapshot_file_checked().await.map_err(command_error)?;
    let created = custom_rules::mutate_rules(|rules| {
        if rules.iter().filter(|rule| !rule.deleted).count() >= MAX_RULES {
            return Err(rule_error(
                "CUSTOM_RULE_LIMIT",
                format!("at most {MAX_RULES} custom rules are supported"),
            ));
        }
        if let Some(existing) = duplicate_rule(rules, &candidate) {
            return Err(rule_error(
                "CUSTOM_RULE_DUPLICATE",
                format!("the same rule already exists as {}", existing.id),
            ));
        }
        rules.push(candidate.clone());
        Ok(candidate.clone())
    })
    .await
    .map_err(command_error)?;

    if let Err(error) = reapply_current_profile(&created.profile, Some(target_uid.as_str())).await {
        if let Err(restore_error) = custom_rules::restore_snapshot(previous).await {
            logging!(
                error,
                Type::Config,
                "failed to restore custom rules after apply failure: {restore_error:#}"
            );
        }
        let _ = reapply_current_profile(&created.profile, Some(target_uid.as_str())).await;
        return Err(error);
    }

    logging!(
        info,
        Type::Config,
        "added custom rule {} for profile {}",
        created.rule_string(),
        created.profile
    );
    crate::core::sync::SyncManager::notify_local_change();
    Ok(created)
}

#[tauri::command]
#[specta::specta]
pub async fn delete_custom_rule(id: std::string::String) -> CmdResult {
    let _operation = custom_rules::CUSTOM_RULES_OPERATION_LOCK.lock().await;
    let rules = custom_rules::load_rules().await;
    rules
        .iter()
        .find(|rule| !rule.deleted && rule.id == id)
        .ok_or_else(|| coded_error("CUSTOM_RULE_NOT_FOUND", format!("custom rule \"{id}\" was not found")))?;

    let previous = custom_rules::snapshot_file_checked().await.map_err(command_error)?;
    let removed = custom_rules::mutate_rules(|rules| {
        let rule = rules
            .iter_mut()
            .find(|rule| rule.id == id)
            .ok_or_else(|| rule_error("CUSTOM_RULE_NOT_FOUND", format!("custom rule \"{id}\" was not found")))?;
        rule.deleted = true;
        rule.updated_at = now_millis();
        Ok(rule.clone())
    })
    .await
    .map_err(command_error)?;

    let profile_uid = Config::profiles().await.latest_arc().items.as_ref().and_then(|items| {
        items
            .iter()
            .find(|item| item.sync_id.as_deref() == Some(removed.profile.as_str()))
            .and_then(|item| item.uid.clone())
    });

    if let Err(error) = reapply_current_profile(&removed.profile, profile_uid.as_deref()).await {
        if let Err(restore_error) = custom_rules::restore_snapshot(previous).await {
            logging!(
                error,
                Type::Config,
                "failed to restore custom rules after delete failure: {restore_error:#}"
            );
        }
        let _ = reapply_current_profile(&removed.profile, profile_uid.as_deref()).await;
        return Err(error);
    }

    let _ = rules;
    logging!(info, Type::Config, "deleted custom rule {id}");
    crate::core::sync::SyncManager::notify_local_change();
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn update_custom_rule_policy(id: std::string::String, policy: std::string::String) -> CmdResult<CustomRule> {
    let _operation = custom_rules::CUSTOM_RULES_OPERATION_LOCK.lock().await;
    let policy = String::from(policy.trim());
    if policy.is_empty() {
        return Err(coded_error("CUSTOM_RULE_POLICY_NOT_FOUND", "policy is required"));
    }

    let rules = custom_rules::load_rules().await;
    let existing = rules
        .iter()
        .find(|rule| !rule.deleted && rule.id == id)
        .cloned()
        .ok_or_else(|| coded_error("CUSTOM_RULE_NOT_FOUND", format!("custom rule \"{id}\" was not found")))?;

    let profile_uid = Config::profiles().await.latest_arc().items.as_ref().and_then(|items| {
        items
            .iter()
            .find(|item| item.sync_id.as_deref() == Some(existing.profile.as_str()))
            .and_then(|item| item.uid.clone())
    });

    let policy_kind = if custom_rules::BUILTIN_POLICIES.contains(&policy.as_str()) {
        String::from("builtin")
    } else {
        let targets_uid = match profile_uid.as_ref() {
            Some(uid) => uid.clone(),
            None if existing.profile == "*" => {
                return Err(coded_error(
                    "CUSTOM_RULE_INVALID",
                    "global rules support built-in policies only",
                ));
            }
            None => {
                return Err(coded_error(
                    "CUSTOM_RULE_PROFILE_NOT_FOUND",
                    "the rule profile no longer exists",
                ));
            }
        };
        let targets = custom_rules::targets_for_profile(targets_uid.as_str())
            .await
            .map_err(|error| coded_error("CUSTOM_RULE_TARGETS_FAILED", format_args!("{error:#}")))?;
        if !targets.has_policy(policy.as_str()) && !targets.has_script {
            return Err(coded_error(
                "CUSTOM_RULE_POLICY_NOT_FOUND",
                format!("policy \"{policy}\" does not exist in this profile"),
            ));
        }
        String::from("group")
    };

    let previous = custom_rules::snapshot_file_checked().await.map_err(command_error)?;
    let updated = custom_rules::mutate_rules(|rules| {
        let rule = rules
            .iter_mut()
            .find(|rule| rule.id == id)
            .ok_or_else(|| rule_error("CUSTOM_RULE_NOT_FOUND", format!("custom rule \"{id}\" was not found")))?;
        rule.policy = policy.clone();
        rule.policy_kind = policy_kind.clone();
        rule.updated_at = now_millis();
        Ok(rule.clone())
    })
    .await
    .map_err(command_error)?;

    if let Err(error) = reapply_current_profile(&updated.profile, profile_uid.as_deref()).await {
        if let Err(restore_error) = custom_rules::restore_snapshot(previous).await {
            logging!(
                error,
                Type::Config,
                "failed to restore custom rules after rebind failure: {restore_error:#}"
            );
        }
        let _ = reapply_current_profile(&updated.profile, profile_uid.as_deref()).await;
        return Err(error);
    }

    crate::core::sync::SyncManager::notify_local_change();
    Ok(updated)
}

#[tauri::command]
#[specta::specta]
pub async fn validate_custom_rules(profile_uid: std::string::String) -> CmdResult<Vec<RuleIssue>> {
    let uid = String::from(profile_uid);
    let sync_id = sync_id_or_error(uid.as_str()).await?;
    let targets = custom_rules::targets_for_profile(uid.as_str())
        .await
        .map_err(|error| coded_error("CUSTOM_RULE_TARGETS_FAILED", format_args!("{error:#}")))?;
    let (issues, _) = custom_rules::validate_rules_for_profile(&sync_id, &targets, true)
        .await
        .map_err(|error| coded_error("CUSTOM_RULE_FAILED", format_args!("{error:#}")))?;
    Ok(issues)
}

pub(crate) async fn profile_sync_id_for_uid(uid: &str) -> Option<String> {
    let profiles = Config::profiles().await;
    let snapshot = profiles.latest_arc();
    profile_sync_id(&snapshot, uid)
}
