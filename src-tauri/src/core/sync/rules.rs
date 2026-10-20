use super::ConflictPolicy;
use crate::enhance::custom_rules::{self, CustomRule, MAX_RULES};
use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const DOCUMENT_VERSION: u32 = 1;

#[derive(Serialize)]
struct RulesDocumentRef<'a> {
    version: u32,
    rules: &'a [CustomRule],
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RulesDocument {
    version: u32,
    rules: Vec<CustomRule>,
}

pub fn parse_document(bytes: &[u8]) -> Result<Vec<CustomRule>> {
    let document: RulesDocument = serde_yaml_ng::from_slice(bytes).context("custom-rules.yaml is not valid YAML")?;
    if document.version != DOCUMENT_VERSION {
        bail!(
            "unsupported custom-rules.yaml schema {} (expected {DOCUMENT_VERSION})",
            document.version
        );
    }
    if document.rules.len() > MAX_RULES {
        bail!("custom-rules.yaml contains too many rules");
    }
    let mut ids = std::collections::HashSet::new();
    for rule in &document.rules {
        if rule.id.trim().is_empty() || !is_safe_rule_id(rule.id.as_str()) {
            bail!("custom-rules.yaml contains an invalid rule id");
        }
        if !ids.insert(rule.id.as_str()) {
            bail!("custom-rules.yaml contains a duplicate rule id");
        }
        if rule.rule_type.trim().is_empty() || rule.policy.trim().is_empty() || rule.profile.trim().is_empty() {
            bail!("custom-rules.yaml contains an incomplete rule");
        }
    }
    Ok(document.rules)
}

fn is_safe_rule_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn render_document(rules: &[CustomRule]) -> Result<Vec<u8>> {
    let body = serde_yaml_ng::to_string(&RulesDocumentRef {
        version: DOCUMENT_VERSION,
        rules,
    })?;
    Ok(format!("# Custom Rules for Clash Verge\n\n{body}").into_bytes())
}

fn same_content(left: &CustomRule, right: &CustomRule) -> bool {
    left.profile == right.profile
        && left.position == right.position
        && left.rule_type == right.rule_type
        && left.value == right.value
        && left.policy == right.policy
        && left.policy_kind == right.policy_kind
        && left.no_resolve == right.no_resolve
        && left.enabled == right.enabled
        && left.deleted == right.deleted
}

#[derive(Debug, Clone)]
pub struct RuleConflict {
    pub side: &'static str,
    pub rule: CustomRule,
}

pub struct MergedRules {
    pub rules: Vec<CustomRule>,
    pub conflicts: Vec<RuleConflict>,
}

pub fn merge_rules(
    local: &[CustomRule],
    remote: &[CustomRule],
    policy: ConflictPolicy,
    local_base_revision: u64,
    remote_revision: u64,
) -> MergedRules {
    let mut merged = local.to_vec();
    let mut index: HashMap<std::string::String, usize> = HashMap::new();
    for (position, rule) in merged.iter().enumerate() {
        index.insert(rule.id.to_string(), position);
    }
    let mut conflicts = Vec::new();

    for remote_rule in remote {
        let Some(position) = index.get(remote_rule.id.as_str()).copied() else {
            index.insert(remote_rule.id.to_string(), merged.len());
            merged.push(remote_rule.clone());
            continue;
        };
        let local_rule = &merged[position];
        if same_content(local_rule, remote_rule) {
            let mut winner = local_rule.clone();
            winner.updated_at = local_rule.updated_at.max(remote_rule.updated_at);
            merged[position] = winner;
            continue;
        }
        let remote_wins = match policy {
            ConflictPolicy::Local => false,
            ConflictPolicy::Remote => true,
            ConflictPolicy::LastModified | ConflictPolicy::Ask => {
                if remote_rule.updated_at > local_rule.updated_at {
                    true
                } else if local_rule.updated_at > remote_rule.updated_at {
                    false
                } else {
                    remote_revision > local_base_revision
                }
            }
        };
        if remote_wins {
            if !local_rule.deleted {
                conflicts.push(RuleConflict {
                    side: "local",
                    rule: local_rule.clone(),
                });
            }
            merged[position] = remote_rule.clone();
        } else if !remote_rule.deleted {
            conflicts.push(RuleConflict {
                side: "remote",
                rule: remote_rule.clone(),
            });
        }
    }

    MergedRules {
        rules: merged,
        conflicts,
    }
}

/// Reads the on-disk rules without falling back to a cache, so corruption cannot
/// silently turn into an empty (deleting) payload.
pub async fn load_local_rules() -> Result<Vec<CustomRule>> {
    match custom_rules::snapshot_file_checked().await? {
        Some(bytes) if bytes.len() as u64 > custom_rules::MAX_FILE_BYTES => {
            bail!("custom-rules.yaml exceeds the sync size limit")
        }
        Some(bytes) => parse_document(&bytes),
        None => Ok(Vec::new()),
    }
}

pub async fn apply_rules(rules: &[CustomRule]) -> Result<()> {
    let rules = rules.to_vec();
    custom_rules::mutate_rules(move |current| {
        *current = rules;
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enhance::custom_rules::RulePosition;

    fn rule(id: &str, policy: &str, updated_at: u64) -> CustomRule {
        CustomRule {
            id: id.into(),
            profile: "S1".into(),
            position: RulePosition::Append,
            rule_type: "DOMAIN".into(),
            value: id.into(),
            policy: policy.into(),
            policy_kind: "builtin".into(),
            no_resolve: false,
            enabled: true,
            created_at: 0,
            updated_at,
            deleted: false,
        }
    }

    #[test]
    fn merges_per_rule_and_keeps_the_loser() {
        let local = vec![rule("a", "DIRECT", 10), rule("local-only", "REJECT", 5)];
        let remote = vec![rule("a", "REJECT", 20), rule("remote-only", "DIRECT", 5)];

        let merged = merge_rules(&local, &remote, ConflictPolicy::LastModified, 1, 2);

        assert_eq!(merged.rules.len(), 3);
        assert_eq!(
            merged
                .rules
                .iter()
                .find(|rule| rule.id == "a")
                .map(|rule| rule.policy.as_str()),
            Some("REJECT")
        );
        assert_eq!(merged.conflicts.len(), 1);
        assert_eq!(merged.conflicts[0].side, "local");
    }

    #[test]
    fn equal_timestamps_prefer_the_newer_revision() {
        let local = vec![rule("a", "DIRECT", 10)];
        let remote = vec![rule("a", "REJECT", 10)];

        let remote_wins = merge_rules(&local, &remote, ConflictPolicy::LastModified, 1, 2);
        assert_eq!(remote_wins.rules[0].policy, "REJECT");

        let tied = merge_rules(&local, &remote, ConflictPolicy::LastModified, 2, 2);
        assert_eq!(tied.rules[0].policy, "DIRECT");
        assert_eq!(tied.conflicts[0].side, "remote");
    }

    #[test]
    fn a_newer_tombstone_deletes_and_an_older_one_does_not() {
        let local = vec![rule("a", "DIRECT", 10)];
        let mut tombstone = rule("a", "DIRECT", 20);
        tombstone.deleted = true;

        let deleted = merge_rules(&local, &[tombstone.clone()], ConflictPolicy::LastModified, 1, 2);
        assert!(deleted.rules[0].deleted);

        let mut older = tombstone;
        older.updated_at = 5;
        let resurrected = merge_rules(&local, &[older], ConflictPolicy::LastModified, 1, 2);
        assert!(!resurrected.rules[0].deleted);
    }
}
