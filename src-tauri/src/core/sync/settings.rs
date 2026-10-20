use super::ConflictPolicy;
use crate::config::{IVerge, IVergeTheme};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DOCUMENT_VERSION: u32 = 1;

/// Presentation preferences only. Ports, TUN, proxy, hotkeys, window state, credentials,
/// device identity, and sync settings stay device-local.
pub const WHITELIST: &[&str] = &[
    "theme_mode",
    "theme_setting",
    "traffic_graph",
    "enable_group_icon",
    "collapse_navbar",
    "proxy_layout_column",
    "auto_close_connection",
    "auto_check_update",
    "default_latency_test",
    "default_latency_timeout",
    "enable_hover_jump_navigator",
    "hover_jump_navigator_delay",
    "enable_memory_usage",
    "pause_render_traffic_stats_on_blur",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingEntry {
    pub value: serde_json::Value,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsDocument {
    pub version: u32,
    pub settings: BTreeMap<std::string::String, SettingEntry>,
}

pub fn capture(verge: &IVerge) -> BTreeMap<std::string::String, serde_json::Value> {
    macro_rules! capture {
        ($map:ident, $verge:ident, $($field:ident),* $(,)?) => {
            $(
                if let Some(value) = $verge.$field.as_ref()
                    && let Ok(value) = serde_json::to_value(value)
                {
                    $map.insert(stringify!($field).into(), value);
                }
            )*
        };
    }
    let mut values = BTreeMap::new();
    capture!(
        values,
        verge,
        theme_mode,
        theme_setting,
        traffic_graph,
        enable_group_icon,
        collapse_navbar,
        proxy_layout_column,
        auto_close_connection,
        auto_check_update,
        default_latency_test,
        default_latency_timeout,
        enable_hover_jump_navigator,
        hover_jump_navigator_delay,
        enable_memory_usage,
        pause_render_traffic_stats_on_blur,
    );
    values
}

pub fn parse_document(bytes: &[u8]) -> Result<BTreeMap<std::string::String, SettingEntry>> {
    let document: SettingsDocument = serde_yaml_ng::from_slice(bytes)?;
    if document.version != DOCUMENT_VERSION {
        anyhow::bail!(
            "unsupported settings.yaml schema {} (expected {DOCUMENT_VERSION})",
            document.version
        );
    }
    Ok(document
        .settings
        .into_iter()
        .filter(|(key, _)| WHITELIST.contains(&key.as_str()))
        .collect())
}

pub fn collect_document(
    values: &BTreeMap<std::string::String, serde_json::Value>,
    state: &super::state::SyncState,
    first_sync: bool,
    now: u64,
) -> (
    SettingsDocument,
    BTreeMap<std::string::String, super::state::LocalSettingState>,
) {
    let mut settings = BTreeMap::new();
    let mut next_state = BTreeMap::new();
    for (key, value) in values {
        let fingerprint = serde_json::to_vec(value)
            .map(|data| super::manifest::sha256_hex(&data))
            .unwrap_or_default();
        let updated_at = if first_sync {
            0
        } else {
            state
                .settings
                .get(key)
                .filter(|previous| previous.fingerprint == fingerprint)
                .map_or(now, |previous| previous.updated_at)
        };
        settings.insert(
            key.clone(),
            SettingEntry {
                value: value.clone(),
                updated_at,
            },
        );
        next_state.insert(
            key.clone(),
            super::state::LocalSettingState {
                fingerprint,
                updated_at,
            },
        );
    }
    (
        SettingsDocument {
            version: DOCUMENT_VERSION,
            settings,
        },
        next_state,
    )
}

pub fn render_document(document: &SettingsDocument) -> Result<Vec<u8>> {
    let body = serde_yaml_ng::to_string(document)?;
    Ok(format!("# Clash Verge settings\n\n{body}").into_bytes())
}

pub fn merge_settings(
    local_values: &BTreeMap<std::string::String, serde_json::Value>,
    local_state: &BTreeMap<std::string::String, super::state::LocalSettingState>,
    remote: &BTreeMap<std::string::String, SettingEntry>,
    policy: ConflictPolicy,
    local_base_revision: u64,
    remote_revision: u64,
    now: u64,
) -> (
    BTreeMap<std::string::String, serde_json::Value>,
    BTreeMap<std::string::String, super::state::LocalSettingState>,
) {
    let fingerprint_of = |value: &serde_json::Value| {
        serde_json::to_vec(value)
            .map(|data| super::manifest::sha256_hex(&data))
            .unwrap_or_default()
    };

    let mut next_state = BTreeMap::new();
    for (key, value) in local_values {
        let fingerprint = fingerprint_of(value);
        let updated_at = local_state
            .get(key)
            .filter(|previous| previous.fingerprint == fingerprint)
            .map_or(now, |previous| previous.updated_at);
        next_state.insert(
            key.clone(),
            super::state::LocalSettingState {
                fingerprint,
                updated_at,
            },
        );
    }

    let mut apply = BTreeMap::new();
    for (key, remote_entry) in remote {
        if !WHITELIST.contains(&key.as_str()) {
            continue;
        }
        let remote_fingerprint = fingerprint_of(&remote_entry.value);
        let Some(local_value) = local_values.get(key) else {
            apply.insert(key.clone(), remote_entry.value.clone());
            next_state.insert(
                key.clone(),
                super::state::LocalSettingState {
                    fingerprint: remote_fingerprint,
                    updated_at: remote_entry.updated_at,
                },
            );
            continue;
        };
        if local_value == &remote_entry.value {
            // Adopt the remote timestamp so the rebuilt document is byte-stable against the
            // downloaded one; otherwise every download would look like a new local edit.
            next_state.insert(
                key.clone(),
                super::state::LocalSettingState {
                    fingerprint: remote_fingerprint,
                    updated_at: remote_entry.updated_at,
                },
            );
            continue;
        }
        let remote_wins = match policy {
            ConflictPolicy::Local => false,
            ConflictPolicy::Remote => true,
            ConflictPolicy::LastModified | ConflictPolicy::Ask => {
                let local_updated = next_state.get(key).map_or(0, |state| state.updated_at);
                if remote_entry.updated_at > local_updated {
                    true
                } else if local_updated > remote_entry.updated_at {
                    false
                } else {
                    remote_revision > local_base_revision
                }
            }
        };
        if remote_wins {
            apply.insert(key.clone(), remote_entry.value.clone());
            next_state.insert(
                key.clone(),
                super::state::LocalSettingState {
                    fingerprint: remote_fingerprint,
                    updated_at: remote_entry.updated_at,
                },
            );
        }
    }
    (apply, next_state)
}

pub fn apply_values(verge: &mut IVerge, values: &BTreeMap<std::string::String, serde_json::Value>) -> Result<()> {
    macro_rules! apply {
        ($field:ident, $ty:ty) => {
            if let Some(value) = values.get(stringify!($field)) {
                verge.$field = serde_json::from_value::<Option<$ty>>(value.clone())?;
            }
        };
    }
    apply!(theme_mode, smartstring::alias::String);
    apply!(theme_setting, IVergeTheme);
    apply!(traffic_graph, bool);
    apply!(enable_group_icon, bool);
    apply!(collapse_navbar, bool);
    apply!(proxy_layout_column, u8);
    apply!(auto_close_connection, bool);
    apply!(auto_check_update, bool);
    apply!(default_latency_test, smartstring::alias::String);
    apply!(default_latency_timeout, i16);
    apply!(enable_hover_jump_navigator, bool);
    apply!(hover_jump_navigator_delay, u64);
    apply!(enable_memory_usage, bool);
    apply!(pause_render_traffic_stats_on_blur, bool);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_whitelisted_settings_leave_the_device() {
        let mut verge = IVerge::default();
        verge.theme_mode = Some("dark".into());
        verge.enable_tun_mode = Some(true);
        verge.webdav_url = Some("https://user:secret@example.com/dav".into());
        verge.sync_settings = Some(crate::config::SyncSettings {
            enabled: Some(true),
            ..Default::default()
        });

        let values = capture(&verge);

        assert!(values.contains_key("theme_mode"));
        for forbidden in ["enable_tun_mode", "webdav_url", "sync_settings", "verge_mixed_port"] {
            assert!(!values.contains_key(forbidden), "{forbidden} must stay device-local");
        }
    }

    #[test]
    fn applies_whitelisted_values_by_key() {
        let mut verge = IVerge::default();
        verge.theme_mode = Some("light".into());

        let mut values = BTreeMap::new();
        values.insert("theme_mode".into(), serde_json::json!("dark"));
        values.insert("enable_tun_mode".into(), serde_json::json!(true));

        apply_values(&mut verge, &values).unwrap();

        assert_eq!(verge.theme_mode.as_deref(), Some("dark"));
        assert_eq!(verge.enable_tun_mode, None);
    }
}
