use crate::{
    config::{Config, PrfItem},
    utils::{dirs, help},
};
use anyhow::{Context as _, Result, bail};
use clash_verge_logging::{Type, logging};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};
use smartstring::alias::String;
use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    path::PathBuf,
    sync::{Arc, LazyLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::fs;

pub const FILE_NAME: &str = "custom-rules.yaml";
pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_RULES: usize = 10_000;
pub const MAX_VALUE_LEN: usize = 2_048;
pub const MAX_POLICY_LEN: usize = 256;

/// Tombstones only exist to make deletions syncable; prune them after 90 days.
const TOMBSTONE_TTL_MS: u64 = 90 * 24 * 60 * 60 * 1_000;

pub const BUILTIN_POLICIES: &[&str] = &["DIRECT", "REJECT", "REJECT-DROP", "PASS"];

/// Rule types quick add may create; `MATCH` is deliberately absent.
pub const QUICK_ADD_RULE_TYPES: &[&str] = &[
    "DOMAIN",
    "DOMAIN-SUFFIX",
    "DOMAIN-KEYWORD",
    "DOMAIN-REGEX",
    "GEOSITE",
    "GEOIP",
    "SRC-GEOIP",
    "IP-ASN",
    "SRC-IP-ASN",
    "IP-CIDR",
    "IP-CIDR6",
    "SRC-IP-CIDR",
    "IP-SUFFIX",
    "SRC-IP-SUFFIX",
    "SRC-PORT",
    "DST-PORT",
    "IN-PORT",
    "DSCP",
    "PROCESS-NAME",
    "PROCESS-PATH",
    "PROCESS-NAME-REGEX",
    "PROCESS-PATH-REGEX",
    "NETWORK",
    "UID",
    "IN-TYPE",
    "IN-USER",
    "IN-NAME",
    "SUB-RULE",
    "RULE-SET",
    "AND",
    "OR",
    "NOT",
];

pub static CUSTOM_RULES_WRITE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// Serializes snapshot -> mutate -> reapply -> rollback sequences across commands.
pub static CUSTOM_RULES_OPERATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const WARNING_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum RulePosition {
    Prepend,
    Append,
}

impl RulePosition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepend => "prepend",
            Self::Append => "append",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub struct CustomRule {
    #[specta(type = std::string::String)]
    pub id: String,
    #[specta(type = std::string::String)]
    pub profile: String,
    pub position: RulePosition,
    #[serde(rename = "type")]
    #[specta(type = std::string::String)]
    pub rule_type: String,
    #[serde(default)]
    #[specta(type = std::string::String)]
    pub value: String,
    #[specta(type = std::string::String)]
    pub policy: String,
    #[serde(default = "default_policy_kind")]
    #[specta(type = std::string::String)]
    pub policy_kind: String,
    #[serde(default)]
    pub no_resolve: bool,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
    #[serde(default)]
    pub deleted: bool,
}

fn default_policy_kind() -> String {
    "builtin".into()
}

const fn default_enabled() -> bool {
    true
}

impl CustomRule {
    pub fn rule_string(&self) -> String {
        let mut rule = self.rule_type.clone();
        if !self.value.is_empty() {
            rule.push(',');
            rule.push_str(&self.value);
        }
        rule.push(',');
        rule.push_str(&self.policy);
        if self.no_resolve {
            rule.push_str(",no-resolve");
        }
        rule
    }

    fn same_identity(&self, other: &Self) -> bool {
        self.profile == other.profile
            && self.rule_type == other.rule_type
            && self.value == other.value
            && self.policy == other.policy
            && self.no_resolve == other.no_resolve
    }
}

#[derive(Serialize)]
struct CustomRulesFileRef<'a> {
    version: u32,
    rules: &'a [CustomRule],
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RuleTargets {
    #[specta(type = Vec<std::string::String>)]
    pub groups: Vec<String>,
    #[specta(type = Vec<std::string::String>)]
    pub policies: Vec<String>,
    #[specta(type = Vec<std::string::String>)]
    pub rule_providers: Vec<String>,
    #[specta(type = Vec<std::string::String>)]
    pub sub_rules: Vec<String>,
    #[specta(type = Vec<std::string::String>)]
    pub builtin_policies: Vec<String>,
    pub has_script: bool,
}

impl RuleTargets {
    pub fn from_config(config: &Mapping, has_script: bool) -> Self {
        let names = |field: &str| -> Vec<String> {
            config
                .get(field)
                .and_then(Value::as_sequence)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            item.as_mapping()
                                .and_then(|map| map.get("name"))
                                .and_then(Value::as_str)
                                .map(String::from)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };

        let groups = names("proxy-groups");
        let proxies = names("proxies");
        let rule_providers = mapping_keys(config, "rule-providers");
        let sub_rules = mapping_keys(config, "sub-rules");
        let builtin_policies: Vec<String> = BUILTIN_POLICIES.iter().map(|policy| String::from(*policy)).collect();

        let mut policies = groups.clone();
        policies.extend(proxies);
        policies.extend(builtin_policies.iter().cloned());

        Self {
            groups,
            policies,
            rule_providers,
            sub_rules,
            builtin_policies,
            has_script,
        }
    }

    pub fn is_builtin(&self, policy: &str) -> bool {
        self.builtin_policies.iter().any(|builtin| builtin == policy)
    }

    pub fn has_policy(&self, policy: &str) -> bool {
        self.policies.iter().any(|known| known == policy)
    }

    pub fn has_group(&self, policy: &str) -> bool {
        self.groups.iter().any(|group| group == policy)
    }

    pub fn has_provider(&self, provider: &str) -> bool {
        self.rule_providers.iter().any(|known| known == provider)
    }

    pub fn has_sub_rule(&self, sub_rule: &str) -> bool {
        self.sub_rules.iter().any(|known| known == sub_rule)
    }
}

fn mapping_keys(config: &Mapping, field: &str) -> Vec<String> {
    config
        .get(field)
        .and_then(Value::as_mapping)
        .map(|map| {
            map.keys()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RuleIssue {
    #[specta(type = Option<std::string::String>)]
    pub rule_id: Option<String>,
    #[specta(type = std::string::String)]
    pub code: String,
    pub message: std::string::String,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RuleConflict {
    pub source: std::string::String,
    #[specta(type = Option<std::string::String>)]
    pub rule_id: Option<String>,
    pub rule: std::string::String,
    pub position: Option<std::string::String>,
    pub code: std::string::String,
    pub missing: std::string::String,
}

#[derive(Debug)]
pub struct RuleProblem {
    pub code: &'static str,
    pub message: std::string::String,
}

impl RuleProblem {
    fn new(code: &'static str, message: impl Into<std::string::String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RuleProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RuleProblem {}

#[derive(Debug, Clone)]
struct CacheEntry {
    modified: Option<SystemTime>,
    size: u64,
    rules: Arc<Vec<CustomRule>>,
}

static CACHE: LazyLock<RwLock<Option<CacheEntry>>> = LazyLock::new(|| RwLock::new(None));

static WARNINGS: LazyLock<Mutex<HashMap<std::string::String, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn warn_rate_limited(key: impl Into<std::string::String>, message: impl std::fmt::Display) {
    let key = key.into();
    let now = Instant::now();
    {
        let mut warnings = WARNINGS.lock();
        if warnings
            .get(&key)
            .is_some_and(|last| now.duration_since(*last) < WARNING_INTERVAL)
        {
            return;
        }
        if warnings.len() > 512 {
            warnings.clear();
        }
        warnings.insert(key, now);
    }
    logging!(warn, Type::Config, "{}", message);
}

pub fn custom_rules_path() -> Result<PathBuf> {
    Ok(dirs::app_home_dir()?.join(FILE_NAME))
}

pub(crate) fn invalidate_cache() {
    *CACHE.write() = None;
}

/// Reads the file once per `mtime + size` change and keeps the last good snapshot on failure.
pub async fn load_rules() -> Arc<Vec<CustomRule>> {
    let Ok(path) = custom_rules_path() else {
        return Arc::new(Vec::new());
    };

    let metadata = fs::metadata(&path).await.ok();
    let modified = metadata.as_ref().and_then(|metadata| metadata.modified().ok());
    let size = metadata.as_ref().map_or(0, std::fs::Metadata::len);

    if let Some(entry) = CACHE.read().as_ref()
        && entry.modified == modified
        && entry.size == size
    {
        return entry.rules.clone();
    }

    let Some(metadata) = metadata else {
        let rules = Arc::new(Vec::new());
        *CACHE.write() = Some(CacheEntry {
            modified: None,
            size: 0,
            rules: rules.clone(),
        });
        return rules;
    };

    if metadata.len() > MAX_FILE_BYTES {
        warn_rate_limited(
            "file-too-large",
            format!("{FILE_NAME} is larger than {MAX_FILE_BYTES} bytes; keeping the last good rules"),
        );
        return CACHE
            .read()
            .as_ref()
            .map_or_else(|| Arc::new(Vec::new()), |entry| entry.rules.clone());
    }

    let parsed = match fs::read_to_string(&path).await {
        Ok(text) => match parse_rules_lenient(&text) {
            Ok(rules) => rules,
            Err(error) => {
                warn_rate_limited(
                    "parse-failed",
                    format!("failed to parse {FILE_NAME}: {error:#}; keeping the last good rules"),
                );
                return CACHE
                    .read()
                    .as_ref()
                    .map_or_else(|| Arc::new(Vec::new()), |entry| entry.rules.clone());
            }
        },
        Err(error) => {
            warn_rate_limited(
                "read-failed",
                format!("failed to read {FILE_NAME}: {error:#}; keeping the last good rules"),
            );
            return CACHE
                .read()
                .as_ref()
                .map_or_else(|| Arc::new(Vec::new()), |entry| entry.rules.clone());
        }
    };

    let rules = Arc::new(parsed);
    *CACHE.write() = Some(CacheEntry {
        modified,
        size,
        rules: rules.clone(),
    });
    rules
}

fn parse_rules_strict(text: &str) -> Result<Vec<CustomRule>> {
    let value: Value = serde_yaml_ng::from_str(text)?;
    let map = value.as_mapping().context("custom rules file is not a mapping")?;

    let version = map
        .get("version")
        .and_then(Value::as_u64)
        .unwrap_or(SCHEMA_VERSION as u64);
    if version != SCHEMA_VERSION as u64 {
        bail!("unsupported custom rules schema version {version}");
    }

    let Some(entries) = map.get("rules").and_then(Value::as_sequence) else {
        return Ok(Vec::new());
    };

    if entries.len() > MAX_RULES {
        bail!("custom rules file contains more than {MAX_RULES} rules");
    }

    let mut rules = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let rule = serde_yaml_ng::from_value::<CustomRule>(entry.clone())
            .with_context(|| format!("malformed custom rule at index {index}"))?;
        if rule.id.trim().is_empty() {
            bail!("custom rule at index {index} has an empty id");
        }
        rules.push(rule);
    }
    Ok(rules)
}

fn parse_rules_lenient(text: &str) -> Result<Vec<CustomRule>> {
    let value: Value = serde_yaml_ng::from_str(text)?;
    let map = value.as_mapping().context("custom rules file is not a mapping")?;

    let version = map
        .get("version")
        .and_then(Value::as_u64)
        .unwrap_or(SCHEMA_VERSION as u64);
    if version != SCHEMA_VERSION as u64 {
        warn_rate_limited(
            "unsupported-version",
            format!("{FILE_NAME} uses unsupported schema version {version}"),
        );
    }

    let Some(entries) = map.get("rules").and_then(Value::as_sequence) else {
        return Ok(Vec::new());
    };

    if entries.len() > MAX_RULES {
        warn_rate_limited(
            "too-many-rules",
            format!("{FILE_NAME} contains more than {MAX_RULES} rules; only the first are loaded"),
        );
    }

    let mut rules = Vec::with_capacity(entries.len().min(MAX_RULES));
    for (index, entry) in entries.iter().take(MAX_RULES).enumerate() {
        let fallback_id = format!("entry-{index}");
        match serde_yaml_ng::from_value::<CustomRule>(entry.clone()) {
            Ok(rule) => {
                if rule.id.trim().is_empty() {
                    warn_rate_limited(
                        format!("empty-id:{fallback_id}"),
                        format!("skipping custom rule at index {index}: empty id"),
                    );
                    continue;
                }
                rules.push(rule);
            }
            Err(error) => {
                let id = entry
                    .as_mapping()
                    .and_then(|map| map.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or(&fallback_id);
                warn_rate_limited(
                    format!("malformed:{id}:{index}"),
                    format!("skipping malformed custom rule \"{id}\": {error:#}"),
                );
            }
        }
    }
    Ok(rules)
}

/// Serializes a mutation under one lock and keeps the previous file as `.bak`.
pub async fn mutate_rules<T>(f: impl FnOnce(&mut Vec<CustomRule>) -> Result<T>) -> Result<T> {
    let _guard = CUSTOM_RULES_WRITE_LOCK.lock().await;
    let path = custom_rules_path()?;
    if let Ok(metadata) = fs::metadata(&path).await
        && metadata.len() > MAX_FILE_BYTES
    {
        bail!("custom rules file exceeds {MAX_FILE_BYTES} bytes");
    }
    let previous = fs::read(&path).await.ok();
    let mut rules = match previous.as_ref() {
        Some(bytes) => {
            let text = std::str::from_utf8(bytes).context("custom rules file is not valid UTF-8")?;
            parse_rules_strict(text)?
        }
        None => Vec::new(),
    };

    let result = f(&mut rules)?;
    save_rules_locked(&path, &rules, previous.as_deref()).await?;
    Ok(result)
}

async fn save_rules_locked(path: &std::path::Path, rules: &[CustomRule], previous: Option<&[u8]>) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64);
    let compacted = rules
        .iter()
        .filter(|rule| !rule.deleted || now.saturating_sub(rule.updated_at) <= TOMBSTONE_TTL_MS)
        .cloned()
        .collect::<Vec<_>>();

    if compacted.len() > MAX_RULES {
        bail!("too many custom rules: {} (limit {MAX_RULES})", compacted.len());
    }

    let data = CustomRulesFileRef {
        version: SCHEMA_VERSION,
        rules: &compacted,
    };
    let body = serde_yaml_ng::to_string(&data)?;
    let yaml_str = format!("# Custom Rules for Clash Verge\n\n{body}");
    if yaml_str.len() as u64 > MAX_FILE_BYTES {
        bail!("custom rules file would exceed {MAX_FILE_BYTES} bytes");
    }

    let backup = path.with_extension("yaml.bak");
    if let Some(previous) = previous {
        if let Err(error) = fs::write(&backup, previous).await {
            logging!(
                warn,
                Type::Config,
                "failed to write backup {}: {error:#}",
                backup.display()
            );
        }
    }

    help::save_yaml_str(path, &yaml_str).await?;
    invalidate_cache();
    Ok(())
}

pub async fn restore_snapshot(previous: Option<Vec<u8>>) -> Result<()> {
    let _guard = CUSTOM_RULES_WRITE_LOCK.lock().await;
    let path = custom_rules_path()?;
    match previous {
        Some(bytes) => fs::write(&path, bytes)
            .await
            .with_context(|| format!("failed to restore {}", path.display()))?,
        None => match fs::remove_file(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("failed to remove custom rules file"),
        },
    }
    invalidate_cache();
    Ok(())
}

pub async fn snapshot_file_checked() -> Result<Option<Vec<u8>>> {
    let path = custom_rules_path()?;
    let metadata = match fs::metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("failed to inspect {}", path.display())),
    };
    if metadata.len() > MAX_FILE_BYTES {
        bail!("custom rules file exceeds {MAX_FILE_BYTES} bytes");
    }
    match fs::read(&path).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

pub fn is_known_rule_type(rule_type: &str) -> bool {
    rule_type == "MATCH" || QUICK_ADD_RULE_TYPES.contains(&rule_type)
}

pub fn validate_rule_shape(rule_type: &str, value: &str, policy: &str) -> Result<(), RuleProblem> {
    if rule_type.trim().is_empty() {
        return Err(RuleProblem::new("invalid", "rule type is required"));
    }
    if !is_known_rule_type(rule_type) {
        return Err(RuleProblem::new(
            "invalid",
            format!("unsupported rule type \"{rule_type}\""),
        ));
    }
    if policy.trim().is_empty() {
        return Err(RuleProblem::new("invalid", "policy is required"));
    }
    if policy.len() > MAX_POLICY_LEN {
        return Err(RuleProblem::new(
            "limit",
            format!("policy must be at most {MAX_POLICY_LEN} characters"),
        ));
    }
    if value.len() > MAX_VALUE_LEN {
        return Err(RuleProblem::new(
            "limit",
            format!("value must be at most {MAX_VALUE_LEN} characters"),
        ));
    }

    let value = value.trim();
    if value.is_empty() && rule_type != "MATCH" {
        return Err(RuleProblem::new("invalid", "rule value is required"));
    }
    if value.contains('\n') || value.contains('\r') {
        return Err(RuleProblem::new("invalid", "rule value must be a single line"));
    }

    let allows_comma = matches!(
        rule_type,
        "AND"
            | "OR"
            | "NOT"
            | "SUB-RULE"
            | "DOMAIN-REGEX"
            | "PROCESS-NAME-REGEX"
            | "PROCESS-PATH-REGEX"
            | "PROCESS-PATH"
    );
    if !allows_comma && value.contains(',') {
        return Err(RuleProblem::new("invalid", "rule value must not contain a comma"));
    }

    match rule_type {
        "IP-CIDR" | "IP-CIDR6" | "SRC-IP-CIDR" | "IP-SUFFIX" | "SRC-IP-SUFFIX" => {
            if !is_valid_cidr(value) {
                return Err(RuleProblem::new("invalid", "value is not a valid CIDR"));
            }
        }
        "SRC-PORT" | "DST-PORT" | "IN-PORT" => {
            if !is_valid_port_or_range(value) {
                return Err(RuleProblem::new("invalid", "value is not a valid port or range"));
            }
        }
        "IP-ASN" | "SRC-IP-ASN" | "UID" => {
            if value.parse::<u64>().is_err() {
                return Err(RuleProblem::new("invalid", "value must be a number"));
            }
        }
        "DSCP" => {
            if value.parse::<u8>().is_err() {
                return Err(RuleProblem::new("invalid", "value must be a number"));
            }
        }
        "NETWORK" => {
            if !value.eq_ignore_ascii_case("tcp") && !value.eq_ignore_ascii_case("udp") {
                return Err(RuleProblem::new("invalid", "network must be tcp or udp"));
            }
        }
        "DOMAIN-REGEX" | "PROCESS-NAME-REGEX" | "PROCESS-PATH-REGEX" => {}
        _ => {}
    }

    Ok(())
}

fn is_valid_cidr(value: &str) -> bool {
    let Some((address, prefix)) = value.rsplit_once('/') else {
        return false;
    };
    let Ok(address) = address.trim().parse::<IpAddr>() else {
        return false;
    };
    let Ok(prefix) = prefix.parse::<u8>() else {
        return false;
    };
    match address {
        IpAddr::V4(_) => prefix <= 32,
        IpAddr::V6(_) => prefix <= 128,
    }
}

fn is_valid_port_or_range(value: &str) -> bool {
    let valid_port = |port: &str| port.parse::<u16>().is_ok_and(|port| port > 0);
    match value.split_once('-') {
        Some((start, end)) => valid_port(start.trim()) && valid_port(end.trim()),
        None => valid_port(value),
    }
}

fn sub_rule_name(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.starts_with('(') {
        return None;
    }
    Some(value)
}

pub fn validate_rule_against_targets(rule: &CustomRule, targets: &RuleTargets) -> Result<(), RuleProblem> {
    validate_rule_shape(&rule.rule_type, &rule.value, &rule.policy)?;

    if rule.profile == "*" && rule.policy_kind != "builtin" {
        return Err(RuleProblem::new(
            "invalid",
            "global rules may only use built-in policies",
        ));
    }

    validate_rule_targets(
        &rule.rule_type,
        &rule.value,
        &rule.policy,
        Some(rule.policy_kind.as_str()),
        targets,
    )
}

pub fn validate_rule_targets(
    rule_type: &str,
    value: &str,
    policy: &str,
    policy_kind: Option<&str>,
    targets: &RuleTargets,
) -> Result<(), RuleProblem> {
    match policy_kind {
        Some("builtin") if !targets.is_builtin(policy) => {
            return Err(RuleProblem::new(
                "policy",
                format!("built-in policy \"{policy}\" does not exist"),
            ));
        }
        Some("builtin") => {}
        Some("group") if !targets.has_group(policy) => {
            return Err(RuleProblem::new(
                "policy",
                format!("policy group \"{policy}\" does not exist in this profile"),
            ));
        }
        Some("group") => {}
        _ if !targets.has_policy(policy) => {
            return Err(RuleProblem::new(
                "policy",
                format!("policy \"{policy}\" does not exist in this profile"),
            ));
        }
        _ => {}
    }

    match rule_type {
        "RULE-SET" if !targets.has_provider(value) => {
            return Err(RuleProblem::new(
                "provider",
                format!("rule provider \"{value}\" does not exist"),
            ));
        }
        "SUB-RULE" => {
            if let Some(name) = sub_rule_name(value)
                && !targets.has_sub_rule(name)
            {
                return Err(RuleProblem::new(
                    "subRule",
                    format!("sub-rule \"{name}\" does not exist"),
                ));
            }
        }
        _ => {}
    }

    Ok(())
}

/// Validates a raw rule string used by legacy `r<uid>.yaml` rule lists.
pub fn validate_raw_rule(raw: &str, targets: &RuleTargets, allow_script_groups: bool) -> Option<RuleProblem> {
    let parts = raw.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() < 2 {
        return Some(RuleProblem::new("invalid", "rule is not valid"));
    }
    let rule_type = parts[0];
    if rule_type.eq_ignore_ascii_case("MATCH") {
        return None;
    }
    if !is_known_rule_type(rule_type) {
        return None;
    }

    let mut end = parts.len();
    if parts.last().is_some_and(|part| part.eq_ignore_ascii_case("no-resolve")) {
        end -= 1;
    }
    if end < 2 {
        return Some(RuleProblem::new("invalid", "rule is not valid"));
    }

    let policy = parts[end - 1];
    let value = if end > 2 {
        parts[1..end - 1].join(",")
    } else {
        std::string::String::new()
    };
    if let Err(problem) = validate_rule_shape(rule_type, &value, policy) {
        return Some(problem);
    }
    if let Err(problem) = validate_rule_targets(rule_type, &value, policy, None, targets) {
        let script_group = allow_script_groups && problem.code == "policy" && targets.has_script;
        if !script_group {
            return Some(problem);
        }
    }
    None
}

pub fn duplicate_rule<'a>(rules: &'a [CustomRule], candidate: &CustomRule) -> Option<&'a CustomRule> {
    rules
        .iter()
        .find(|rule| !rule.deleted && rule.enabled && rule.same_identity(candidate))
}

/// Injects custom rules into an enhanced config. Invalid references are skipped, never fatal.
pub fn inject_rules(mut config: Mapping, rules: &[CustomRule], targets: &RuleTargets) -> (Mapping, Vec<RuleIssue>) {
    let mut prepend = Vec::new();
    let mut append = Vec::new();
    let mut issues = Vec::new();
    let mut seen = HashSet::new();

    for rule in rules.iter().filter(|rule| !rule.deleted && rule.enabled) {
        let rule_string = rule.rule_string();
        if !seen.insert(rule_string.clone()) {
            continue;
        }
        if let Err(problem) = validate_rule_against_targets(rule, targets) {
            issues.push(RuleIssue {
                rule_id: Some(rule.id.clone()),
                code: problem.code.into(),
                message: problem.message.as_str().into(),
            });
            warn_rate_limited(
                format!("inject:{}:{}", rule.id, problem.code),
                format!(
                    "skipping custom rule \"{rule_string}\": {} (rule id {})",
                    problem.message, rule.id
                ),
            );
            continue;
        }

        match rule.position {
            RulePosition::Prepend => prepend.push(rule_string),
            RulePosition::Append => append.push(rule_string),
        }
    }

    if prepend.is_empty() && append.is_empty() {
        return (config, issues);
    }

    let mut existing = match config.remove("rules") {
        Some(Value::Sequence(rules)) => rules,
        Some(value) => {
            config.insert("rules".into(), value);
            return (config, issues);
        }
        None => serde_yaml_ng::Sequence::new(),
    };

    let append_at = existing
        .iter()
        .position(|item| {
            item.as_str()
                .and_then(|rule| rule.split(',').next())
                .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("MATCH"))
        })
        .unwrap_or(existing.len());
    existing.splice(
        append_at..append_at,
        append.into_iter().map(|rule| Value::String(rule.as_str().into())),
    );
    existing.splice(
        0..0,
        prepend.into_iter().map(|rule| Value::String(rule.as_str().into())),
    );

    config.insert("rules".into(), Value::Sequence(existing));
    (config, issues)
}

/// Applies stored rules to the enhanced config. A read problem yields the config unchanged.
pub async fn apply_custom_rules(config: Mapping, sync_id: Option<&str>) -> (Mapping, Vec<RuleIssue>) {
    let Some(sync_id) = sync_id else {
        return (config, Vec::new());
    };

    let rules = load_rules().await;
    let has_script = false;
    let targets = RuleTargets::from_config(&config, has_script);
    let selected = rules
        .iter()
        .filter(|rule| rule.profile == sync_id || rule.profile == "*")
        .cloned()
        .collect::<Vec<_>>();
    inject_rules(config, &selected, &targets)
}

/// Effective groups, providers and sub-rules for a profile outside the runtime config.
pub async fn targets_for_profile(uid: &str) -> Result<RuleTargets> {
    let profiles = Config::profiles().await;
    let snapshot = profiles.latest_arc();
    let item = snapshot.get_item(uid)?;
    let file = item.file.clone().context("profile file is missing")?;
    let path = dirs::app_profiles_dir()?.join(file.as_str());
    let config = help::read_mapping(&path).await?;
    Ok(targets_with_config(&snapshot, item, config).await)
}

/// Same as [`targets_for_profile`], but for staged subscription content that is not on disk yet.
pub async fn targets_for_profile_with_config(uid: &str, config: Mapping) -> Result<RuleTargets> {
    let profiles = Config::profiles().await;
    let snapshot = profiles.latest_arc();
    let item = snapshot.get_item(uid)?;
    Ok(targets_with_config(&snapshot, item, config).await)
}

async fn targets_with_config(snapshot: &crate::config::IProfiles, item: &PrfItem, mut config: Mapping) -> RuleTargets {
    let has_script = script_has_logic(snapshot, item.current_script().map(String::as_str)).await
        || script_has_logic(snapshot, Some("Script")).await;

    if let Some(uid) = item.current_merge() {
        if let Some(merge) = read_merge(snapshot, uid).await {
            merge_targets(&mut config, &merge);
        }
    }
    if let Some(merge) = read_merge(snapshot, "Merge").await {
        merge_targets(&mut config, &merge);
    }

    let own_groups = match item.current_groups().and_then(|uid| snapshot.get_item(uid).ok()) {
        Some(groups_item) => read_seq_group_names(groups_item).await,
        None => Vec::new(),
    };
    if !own_groups.is_empty() {
        let mut groups = config
            .get("proxy-groups")
            .and_then(Value::as_sequence)
            .cloned()
            .unwrap_or_default();
        groups.extend(own_groups.iter().map(|name| {
            let mut group = Mapping::new();
            group.insert("name".into(), Value::String(name.as_str().into()));
            Value::Mapping(group)
        }));
        config.insert("proxy-groups".into(), Value::Sequence(groups));
    }

    RuleTargets::from_config(&config, has_script)
}

async fn script_has_logic(snapshot: &crate::config::IProfiles, uid: Option<&str>) -> bool {
    let Some(uid) = uid else {
        return false;
    };
    let Ok(item) = snapshot.get_item(uid) else {
        return false;
    };
    let Some(file) = item.file.as_ref() else {
        return false;
    };
    let Ok(dir) = dirs::app_profiles_dir() else {
        return false;
    };
    let Ok(text) = fs::read_to_string(dir.join(file.as_str())).await else {
        return false;
    };

    let normalized = text.replace("\r\n", "\n");
    let normalized = normalized.trim_start_matches('\u{feff}');
    let compact = strip_js_comments(normalized)
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<std::string::String>();

    // Match the default body across formatting/comment changes and older templates.
    if compact.is_empty() {
        return false;
    }
    !matches!(
        compact.as_str(),
        "functionmain(config,profileName){returnconfig;}"
            | "functionmain(config,profileName){returnconfig}"
            | "functionmain(config,profileName){returnconfig;};"
    )
}

/// Removes JS line/block comments while preserving string literals.
fn strip_js_comments(source: &str) -> std::string::String {
    let mut out = std::string::String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_line = false;
    let mut in_block = false;
    let mut quote: Option<char> = None;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if in_line {
            if ch == '\n' {
                in_line = false;
                out.push(ch);
            }
            continue;
        }
        if in_block {
            if ch == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_block = false;
            }
            continue;
        }
        if let Some(q) = quote {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            in_line = true;
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            in_block = true;
            continue;
        }
        if ch == '"' || ch == '\'' || ch == '`' {
            quote = Some(ch);
        }
        out.push(ch);
    }

    out
}

async fn read_merge(snapshot: &crate::config::IProfiles, uid: &str) -> Option<Mapping> {
    let item = snapshot.get_item(uid).ok()?;
    let file = item.file.as_ref()?;
    let dir = dirs::app_profiles_dir().ok()?;
    help::read_mapping(&dir.join(file.as_str())).await.ok()
}

async fn read_seq_group_names(item: &PrfItem) -> Vec<String> {
    let Some(file) = item.file.as_ref() else {
        return Vec::new();
    };
    let Ok(dir) = dirs::app_profiles_dir() else {
        return Vec::new();
    };
    let Ok(seq) = help::read_yaml::<crate::enhance::seq::SeqMap>(&dir.join(file.as_str())).await else {
        return Vec::new();
    };
    seq.prepend
        .iter()
        .chain(seq.append.iter())
        .filter_map(|item| {
            item.as_mapping()
                .and_then(|map| map.get("name"))
                .and_then(Value::as_str)
                .map(String::from)
        })
        .collect()
}

fn merge_targets(config: &mut Mapping, merge: &Mapping) {
    for field in ["rule-providers", "sub-rules"] {
        if let Some(Value::Mapping(extra)) = merge.get(field) {
            let mut merged = config
                .get(field)
                .and_then(Value::as_mapping)
                .cloned()
                .unwrap_or_default();
            for (key, value) in extra {
                merged.insert(key.clone(), value.clone());
            }
            config.insert(field.into(), Value::Mapping(merged));
        }
    }
    if let Some(Value::Sequence(groups)) = merge.get("proxy-groups") {
        let mut merged = config
            .get("proxy-groups")
            .and_then(Value::as_sequence)
            .cloned()
            .unwrap_or_default();
        merged.extend(groups.iter().cloned());
        config.insert("proxy-groups".into(), Value::Sequence(merged));
    }
}

pub async fn validate_rules_for_profile(
    sync_id: &str,
    targets: &RuleTargets,
    allow_script_groups: bool,
) -> Result<(Vec<RuleIssue>, Vec<RuleConflict>)> {
    let rules = load_rules().await;
    let selected = rules
        .iter()
        .filter(|rule| !rule.deleted && rule.enabled && (rule.profile == sync_id || rule.profile == "*"))
        .cloned()
        .collect::<Vec<_>>();

    let mut issues = Vec::new();
    let mut conflicts = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for rule in &selected {
        let identity = format!(
            "{}|{}|{}|{}|{}|{}",
            rule.profile,
            rule.position.as_str(),
            rule.rule_type,
            rule.value,
            rule.policy,
            rule.no_resolve
        );
        let duplicate = !seen.insert(identity);

        if let Err(problem) = validate_rule_against_targets(rule, targets) {
            let is_script_group =
                allow_script_groups && problem.code == "policy" && rule.policy_kind != "builtin" && targets.has_script;
            if !is_script_group {
                issues.push(RuleIssue {
                    rule_id: Some(rule.id.clone()),
                    code: problem.code.into(),
                    message: problem.message.as_str().into(),
                });
                let missing = if matches!(problem.code, "provider" | "subRule") {
                    rule.value.clone()
                } else {
                    rule.policy.clone()
                };
                conflicts.push(RuleConflict {
                    source: "custom".into(),
                    rule_id: Some(rule.id.clone()),
                    rule: rule.rule_string().as_str().into(),
                    position: Some(rule.position.as_str().into()),
                    code: problem.code.into(),
                    missing: missing.to_string(),
                });
            }
        }
        if duplicate {
            issues.push(RuleIssue {
                rule_id: Some(rule.id.clone()),
                code: "duplicate".into(),
                message: "duplicate rule".into(),
            });
        }
    }

    Ok((issues, conflicts))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, reason = "tests assert by panicking")]
mod tests {
    use super::*;

    fn rule(id: &str, position: RulePosition, rule_type: &str, value: &str, policy: &str) -> CustomRule {
        CustomRule {
            id: id.into(),
            profile: "Sprofile".into(),
            position,
            rule_type: rule_type.into(),
            value: value.into(),
            policy: policy.into(),
            policy_kind: "group".into(),
            no_resolve: false,
            enabled: true,
            created_at: 0,
            updated_at: 0,
            deleted: false,
        }
    }

    fn config(yaml: &str) -> Mapping {
        serde_yaml_ng::from_str(yaml).expect("test config should parse")
    }

    fn targets(config: &Mapping) -> RuleTargets {
        RuleTargets::from_config(config, false)
    }

    fn rule_strings(config: &Mapping) -> Vec<std::string::String> {
        config
            .get("rules")
            .and_then(Value::as_sequence)
            .expect("rules should exist")
            .iter()
            .filter_map(Value::as_str)
            .map(std::string::String::from)
            .collect()
    }

    #[test]
    fn prepend_rules_go_to_the_top_and_append_rules_before_match() {
        let config = config(
            r#"
proxy-groups:
  - name: Proxy
rules:
  - DOMAIN,keep.example,DIRECT
  - MATCH,DIRECT
"#,
        );
        let rules = vec![
            rule("a", RulePosition::Prepend, "DOMAIN-SUFFIX", "prepend.example", "Proxy"),
            rule("b", RulePosition::Append, "DOMAIN-SUFFIX", "append.example", "Proxy"),
        ];

        let targets = targets(&config);
        let (result, issues) = inject_rules(config, &rules, &targets);

        assert!(issues.is_empty());
        assert_eq!(
            rule_strings(&result),
            vec![
                "DOMAIN-SUFFIX,prepend.example,Proxy",
                "DOMAIN,keep.example,DIRECT",
                "DOMAIN-SUFFIX,append.example,Proxy",
                "MATCH,DIRECT",
            ]
        );
    }

    #[test]
    fn append_falls_back_to_the_end_without_a_match_rule() {
        let config = config("proxy-groups:\n  - name: Proxy\nrules:\n  - DOMAIN,keep.example,DIRECT\n");
        let rules = vec![rule("a", RulePosition::Append, "DOMAIN", "append.example", "Proxy")];

        let targets = targets(&config);
        let (result, _) = inject_rules(config, &rules, &targets);

        assert_eq!(
            rule_strings(&result),
            vec!["DOMAIN,keep.example,DIRECT", "DOMAIN,append.example,Proxy"]
        );
    }

    #[test]
    fn match_detection_is_case_insensitive() {
        let config = config("proxy-groups:\n  - name: Proxy\nrules:\n  - match,DIRECT\n");
        let rules = vec![rule("a", RulePosition::Append, "DOMAIN", "append.example", "Proxy")];

        let targets = targets(&config);
        let (result, _) = inject_rules(config, &rules, &targets);

        assert_eq!(
            rule_strings(&result),
            vec!["DOMAIN,append.example,Proxy", "match,DIRECT"]
        );
    }

    #[test]
    fn rules_with_missing_targets_are_skipped() {
        let config = config(
            r#"
proxy-groups:
  - name: Proxy
rule-providers:
  known:
    type: http
rules:
  - MATCH,DIRECT
"#,
        );
        let rules = vec![
            rule("a", RulePosition::Prepend, "DOMAIN", "ok.example", "Proxy"),
            rule("b", RulePosition::Prepend, "DOMAIN", "bad.example", "Missing"),
            rule("c", RulePosition::Prepend, "RULE-SET", "unknown", "Proxy"),
            rule("d", RulePosition::Prepend, "SUB-RULE", "missing", "Proxy"),
        ];

        let targets = targets(&config);
        let (result, issues) = inject_rules(config, &rules, &targets);

        assert_eq!(rule_strings(&result), vec!["DOMAIN,ok.example,Proxy", "MATCH,DIRECT"]);
        assert_eq!(issues.len(), 3);
        assert_eq!(issues[0].code, "policy");
        assert_eq!(issues[1].code, "provider");
        assert_eq!(issues[2].code, "subRule");
    }

    #[test]
    fn disabled_and_deleted_rules_are_not_injected() {
        let config = config("rules:\n  - MATCH,DIRECT\n");
        let mut disabled = rule("a", RulePosition::Prepend, "DOMAIN", "disabled.example", "Proxy");
        disabled.enabled = false;
        let mut deleted = rule("b", RulePosition::Prepend, "DOMAIN", "deleted.example", "Proxy");
        deleted.deleted = true;

        let targets = targets(&config);
        let (result, _) = inject_rules(config, &[disabled, deleted], &targets);

        assert_eq!(rule_strings(&result), vec!["MATCH,DIRECT"]);
    }

    #[test]
    fn shape_validation_rejects_unknown_types_and_bad_values() {
        assert!(validate_rule_shape("MATCH", "", "DIRECT").is_ok());
        assert!(validate_rule_shape("IP-CIDR", "not-a-cidr", "DIRECT").is_err());
        assert!(validate_rule_shape("DST-PORT", "0", "DIRECT").is_err());
        assert!(validate_rule_shape("NETWORK", "sctp", "DIRECT").is_err());
        assert!(validate_rule_shape("DOMAIN", "a,b", "DIRECT").is_err());
        assert!(validate_rule_shape("DOMAIN", "example.com", "DIRECT").is_ok());
    }

    #[test]
    fn duplicate_rule_strings_are_injected_once() {
        let config = config(
            r#"
proxy-groups:
  - name: Proxy
rules:
  - MATCH,DIRECT
"#,
        );
        let rules = vec![
            rule("a", RulePosition::Prepend, "DOMAIN", "example.com", "Proxy"),
            rule("b", RulePosition::Prepend, "DOMAIN", "example.com", "Proxy"),
        ];

        let targets = targets(&config);
        let (result, issues) = inject_rules(config, &rules, &targets);

        assert!(issues.is_empty());
        assert_eq!(rule_strings(&result), vec!["DOMAIN,example.com,Proxy", "MATCH,DIRECT"]);
    }

    #[test]
    fn duplicate_detection_ignores_deleted_and_disabled_rules() {
        let existing = rule("a", RulePosition::Prepend, "DOMAIN", "example.com", "Proxy");
        let candidate = rule("b", RulePosition::Prepend, "DOMAIN", "example.com", "Proxy");

        assert!(duplicate_rule(std::slice::from_ref(&existing), &candidate).is_some());

        let deleted = CustomRule {
            deleted: true,
            ..existing.clone()
        };
        assert!(duplicate_rule(std::slice::from_ref(&deleted), &candidate).is_none());
    }
}
