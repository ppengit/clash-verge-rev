use crate::config::Config;
use crate::{
    config::{DEFAULT_PAC, deserialize_encrypted, serialize_encrypted},
    constants::network,
    utils::{dirs, help},
};
use anyhow::Result;
use clash_verge_logging::{Type, logging};
use log::LevelFilter;
use serde::{Deserialize, Serialize};
use smartstring::alias::String;

/// ### `verge.yaml` schema
#[derive(Default, Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(default)]
pub struct IVerge {
    /// silent | error | warn | info | debug | trace
    #[specta(type = Option<std::string::String>)]
    pub app_log_level: Option<String>,

    /// Maximum app log size in KiB.
    pub app_log_max_size: Option<u64>,

    pub app_log_max_count: Option<usize>,
    #[specta(type = Option<std::string::String>)]
    pub language: Option<String>,

    /// `light` or `dark` or `system`
    #[specta(type = Option<std::string::String>)]
    pub theme_mode: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub tray_event: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub env_type: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub start_page: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub startup_script: Option<String>,

    pub traffic_graph: Option<bool>,

    /// show memory info (only for Clash Meta)
    pub enable_memory_usage: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_group_icon: Option<bool>,

    pub pause_render_traffic_stats_on_blur: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub common_tray_icon: Option<bool>,

    #[cfg(target_os = "macos")]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<std::string::String>)]
    pub tray_icon: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<std::string::String>)]
    pub menu_icon: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<Vec<std::string::String>>)]
    pub menu_order: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<std::string::String>)]
    pub proxy_group_tools_position: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<std::string::String>)]
    pub notice_position: Option<String>,

    pub collapse_navbar: Option<bool>,

    pub sysproxy_tray_icon: Option<bool>,

    pub tun_tray_icon: Option<bool>,

    pub enable_tun_mode: Option<bool>,

    pub enable_auto_launch: Option<bool>,

    pub enable_silent_start: Option<bool>,

    pub enable_system_proxy: Option<bool>,

    pub enable_proxy_guard: Option<bool>,

    pub enable_bypass_check: Option<bool>,

    /// Initial DNS override preference for profiles without a saved setting.
    pub enable_dns_settings: Option<bool>,

    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    #[specta(type = std::collections::BTreeMap<std::string::String, super::dns::ProfileDnsSettings>)]
    pub profile_dns_settings: std::collections::BTreeMap<String, super::dns::ProfileDnsSettings>,

    pub use_default_bypass: Option<bool>,
    #[specta(type = Option<std::string::String>)]
    pub system_proxy_bypass: Option<String>,

    pub proxy_guard_duration: Option<u64>,

    pub proxy_auto_config: Option<bool>,
    #[specta(type = Option<std::string::String>)]
    pub pac_file_content: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub proxy_host: Option<String>,

    pub theme_setting: Option<IVergeTheme>,
    #[specta(type = Option<Vec<std::string::String>>)]
    pub web_ui_list: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<std::string::String>)]
    pub clash_core: Option<String>,

    /// format: {func},{key}
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<Vec<std::string::String>>)]
    pub hotkeys: Option<Vec<String>>,

    pub enable_global_hotkey: Option<bool>,

    #[specta(type = Option<crate::ipc::JsonValue>)]
    pub home_cards: Option<serde_json::Value>,

    pub auto_close_connection: Option<bool>,

    pub auto_check_update: Option<bool>,
    #[specta(type = Option<std::string::String>)]
    pub default_latency_test: Option<String>,

    pub default_latency_timeout: Option<i16>,

    pub enable_auto_delay_detection: Option<bool>,

    /// 自动检测当前节点延迟的间隔（分钟）
    pub auto_delay_detection_interval_minutes: Option<u64>,

    pub enable_builtin_enhanced: Option<bool>,

    pub proxy_layout_column: Option<u8>,

    pub test_list: Option<Vec<IVergeTestItem>>,

    /// 0: 不清理; 1: 1天；2: 7天; 3: 30天; 4: 90天
    pub auto_log_clean: Option<i32>,

    pub enable_auto_backup_schedule: Option<bool>,

    /// Automatic backup interval in hours
    pub auto_backup_interval_hours: Option<u64>,

    pub auto_backup_on_change: Option<bool>,

    /// verge 的各种 port 用于覆盖 clash 的各种 port
    #[cfg(not(target_os = "windows"))]
    pub verge_redir_port: Option<u16>,

    #[cfg(not(target_os = "windows"))]
    pub verge_redir_enabled: Option<bool>,

    #[cfg(target_os = "linux")]
    pub verge_tproxy_port: Option<u16>,

    #[cfg(target_os = "linux")]
    pub verge_tproxy_enabled: Option<bool>,

    pub verge_mixed_port: Option<u16>,

    pub verge_socks_port: Option<u16>,

    pub verge_socks_enabled: Option<bool>,

    pub verge_port: Option<u16>,

    pub verge_http_enabled: Option<bool>,

    #[serde(
        serialize_with = "serialize_encrypted",
        deserialize_with = "deserialize_encrypted",
        skip_serializing_if = "Option::is_none",
        default
    )]
    #[specta(type = Option<std::string::String>)]
    pub webdav_url: Option<String>,

    #[serde(
        serialize_with = "serialize_encrypted",
        deserialize_with = "deserialize_encrypted",
        skip_serializing_if = "Option::is_none",
        default
    )]
    #[specta(type = Option<std::string::String>)]
    pub webdav_username: Option<String>,

    #[serde(
        serialize_with = "serialize_encrypted",
        deserialize_with = "deserialize_encrypted",
        skip_serializing_if = "Option::is_none",
        default
    )]
    #[specta(type = Option<std::string::String>)]
    pub webdav_password: Option<String>,

    #[cfg(target_os = "macos")]
    pub enable_tray_speed: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(type = Option<std::string::String>)]
    pub tray_proxy_groups_display_mode: Option<String>,
    pub tray_inline_outbound_modes: Option<bool>,

    pub enable_auto_light_weight_mode: Option<bool>,

    /// 自动进入轻量模式的延迟（分钟）
    pub auto_light_weight_minutes: Option<u64>,

    pub enable_hover_jump_navigator: Option<bool>,

    /// 代理页面自动滚动延迟（毫秒）
    pub hover_jump_navigator_delay: Option<u64>,

    pub enable_external_controller: Option<bool>,

    /// Subscription update defaults.
    pub subscription_settings: Option<SubscriptionSettings>,

    /// Scheduled node health verification and selected-node checks.
    pub node_health_settings: Option<NodeHealthSettings>,

    /// Global node visibility defaults.
    pub node_display_settings: Option<NodeDisplaySettings>,

    /// Subscription traffic and expiry presentation.
    pub subscription_info_settings: Option<SubscriptionInfoSettings>,

    /// Scheduled WebDAV sync of user-owned configuration.
    pub sync_settings: Option<SyncSettings>,
}

#[derive(Default, Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(default)]
pub struct SubscriptionSettings {
    /// Default auto-update interval in minutes for new subscriptions.
    pub default_update_interval: Option<u64>,
    pub update_on_startup: Option<bool>,
    pub update_only_current: Option<bool>,
    pub notify_on_failure: Option<bool>,
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(default)]
pub struct ProbeTarget {
    pub url: std::string::String,
    /// `204`, `200`, `200-299`, or a `/`-separated combination.
    pub expected: std::string::String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyword: Option<std::string::String>,
    pub timeout_ms: Option<u32>,
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(default)]
pub struct NodeHealthSettings {
    pub enabled: Option<bool>,
    /// L1 (link/latency) interval in minutes.
    pub interval_minutes: Option<u64>,
    /// L2 (real-site status) interval in minutes.
    pub deep_interval_minutes: Option<u64>,
    /// `current_node`, `current_group`, or `all_groups`.
    pub scope: Option<std::string::String>,
    pub l1_targets: Option<Vec<ProbeTarget>>,
    pub l2_targets: Option<Vec<ProbeTarget>>,
    pub verify_on_select: Option<bool>,
    pub auto_failover: Option<bool>,
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(default)]
pub struct NodeDisplaySettings {
    pub hide_timeout: Option<bool>,
    pub hide_untested: Option<bool>,
    pub hide_unhealthy: Option<bool>,
    /// Hides nodes slower than this threshold; zero disables the filter.
    pub hide_slow_ms: Option<u16>,
    pub default_sort: Option<u8>,
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(default)]
pub struct SubscriptionInfoSettings {
    pub show_traffic: Option<bool>,
    pub expiry_warning_days: Option<u64>,
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(default)]
pub struct SyncSettings {
    pub enabled: Option<bool>,
    /// `two_way`, `upload`, or `download`.
    #[specta(type = Option<std::string::String>)]
    pub mode: Option<std::string::String>,
    /// `subscriptions`, `overrides`, `custom_rules`, `settings`.
    #[specta(type = Option<Vec<std::string::String>>)]
    pub scopes: Option<Vec<std::string::String>>,
    pub interval_minutes: Option<u64>,
    pub on_startup: Option<bool>,
    pub on_change: Option<bool>,
    /// `last_modified`, `local`, `remote`, or `ask`.
    #[specta(type = Option<std::string::String>)]
    pub conflict_policy: Option<std::string::String>,
    #[specta(type = Option<std::string::String>)]
    pub device_name: Option<std::string::String>,
    pub encrypt: Option<bool>,
    /// Local-only payload password; never included in the synced payload.
    #[serde(
        serialize_with = "serialize_encrypted",
        deserialize_with = "deserialize_encrypted",
        skip_serializing_if = "Option::is_none",
        default
    )]
    #[specta(type = Option<std::string::String>)]
    pub password: Option<std::string::String>,
    /// Explicit opt-in for self-signed WebDAV certificates; verification stays on by default.
    pub danger_accept_invalid_certs: Option<bool>,
}

impl SyncSettings {
    fn patch_config(&mut self, patch: &Self) {
        merge_option(&mut self.enabled, &patch.enabled);
        merge_option(&mut self.mode, &patch.mode);
        merge_option(&mut self.scopes, &patch.scopes);
        merge_option(&mut self.interval_minutes, &patch.interval_minutes);
        merge_option(&mut self.on_startup, &patch.on_startup);
        merge_option(&mut self.on_change, &patch.on_change);
        merge_option(&mut self.conflict_policy, &patch.conflict_policy);
        merge_option(&mut self.device_name, &patch.device_name);
        merge_option(&mut self.encrypt, &patch.encrypt);
        merge_option(&mut self.password, &patch.password);
        merge_option(
            &mut self.danger_accept_invalid_certs,
            &patch.danger_accept_invalid_certs,
        );
    }
}

#[derive(Default, Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(default)]
pub struct IVergeTestItem {
    #[specta(type = Option<std::string::String>)]
    pub uid: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub name: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub icon: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub url: Option<String>,
}

#[derive(Default, Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(default)]
pub struct IVergeTheme {
    #[specta(type = Option<std::string::String>)]
    pub primary_color: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub secondary_color: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub primary_text: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub secondary_text: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub info_color: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub error_color: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub warning_color: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub success_color: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub font_family: Option<String>,
    #[specta(type = Option<std::string::String>)]
    pub css_injection: Option<String>,
}

fn merge_option<T: Clone>(target: &mut Option<T>, incoming: &Option<T>) {
    if incoming.is_some() {
        *target = incoming.clone();
    }
}

impl SubscriptionSettings {
    fn patch_config(&mut self, patch: &Self) {
        merge_option(&mut self.default_update_interval, &patch.default_update_interval);
        merge_option(&mut self.update_on_startup, &patch.update_on_startup);
        merge_option(&mut self.update_only_current, &patch.update_only_current);
        merge_option(&mut self.notify_on_failure, &patch.notify_on_failure);
    }
}

impl NodeHealthSettings {
    fn patch_config(&mut self, patch: &Self) {
        merge_option(&mut self.enabled, &patch.enabled);
        merge_option(&mut self.interval_minutes, &patch.interval_minutes);
        merge_option(&mut self.deep_interval_minutes, &patch.deep_interval_minutes);
        merge_option(&mut self.scope, &patch.scope);
        merge_option(&mut self.l1_targets, &patch.l1_targets);
        merge_option(&mut self.l2_targets, &patch.l2_targets);
        merge_option(&mut self.verify_on_select, &patch.verify_on_select);
        merge_option(&mut self.auto_failover, &patch.auto_failover);
    }
}

impl NodeDisplaySettings {
    fn patch_config(&mut self, patch: &Self) {
        merge_option(&mut self.hide_timeout, &patch.hide_timeout);
        merge_option(&mut self.hide_untested, &patch.hide_untested);
        merge_option(&mut self.hide_unhealthy, &patch.hide_unhealthy);
        merge_option(&mut self.hide_slow_ms, &patch.hide_slow_ms);
        merge_option(&mut self.default_sort, &patch.default_sort);
    }
}

impl SubscriptionInfoSettings {
    fn patch_config(&mut self, patch: &Self) {
        merge_option(&mut self.show_traffic, &patch.show_traffic);
        merge_option(&mut self.expiry_warning_days, &patch.expiry_warning_days);
    }
}

impl IVerge {
    pub const VALID_CLASH_CORES: &'static [&'static str] = &["verge-mihomo", "verge-mihomo-alpha"];

    pub async fn validate_and_fix_config() -> Result<()> {
        let config_path = dirs::verge_path()?;
        let mut config = match help::read_yaml::<Self>(&config_path).await {
            Ok(config) => config,
            Err(_) => Self::template(),
        };

        let mut needs_fix = false;

        if let Some(ref core) = config.clash_core {
            let core_str = core.trim();
            if core_str.is_empty() || !Self::VALID_CLASH_CORES.contains(&core_str) {
                logging!(
                    warn,
                    Type::Config,
                    "启动时发现无效的clash_core配置: '{}', 将自动修正为 'verge-mihomo'",
                    core
                );
                config.clash_core = Some("verge-mihomo".into());
                needs_fix = true;
            }
        } else {
            logging!(
                info,
                Type::Config,
                "启动时发现未配置clash_core, 将设置为默认值 'verge-mihomo'"
            );
            config.clash_core = Some("verge-mihomo".into());
            needs_fix = true;
        }

        if needs_fix {
            logging!(debug, Type::Config, "正在保存修正后的配置文件...");
            help::save_yaml(&config_path, &config, Some("# Clash Verge Config")).await?;
            logging!(info, Type::Config, "配置文件修正完成，需要重新加载配置");

            Self::reload_config_after_fix(config).await;
        } else {
            logging!(debug, Type::Config, "clash_core配置验证通过: {:?}", config.clash_core);
        }

        Ok(())
    }

    async fn reload_config_after_fix(updated_config: Self) {
        logging!(
            info,
            Type::Config,
            "内存配置已强制更新，新的clash_core: {:?}",
            &updated_config.clash_core
        );

        let config_draft = Config::verge().await;
        config_draft.edit_draft(|d| {
            *d = updated_config;
        });
        config_draft.apply();
    }

    pub fn get_valid_clash_core(&self) -> String {
        self.clash_core.clone().unwrap_or_else(|| "verge-mihomo".into())
    }

    pub async fn new() -> Self {
        match dirs::verge_path() {
            Ok(path) => match help::read_yaml::<Self>(&path).await {
                Ok(mut config) => {
                    if let Some(start_page) = config.start_page.clone()
                        && start_page == "/home"
                    {
                        config.start_page = Some(String::from("/"));
                    }
                    config
                }
                Err(err) => {
                    logging!(error, Type::Config, "failed to read verge config: {err:#}");
                    Self::template()
                }
            },
            Err(err) => {
                logging!(error, Type::Config, "failed to get verge config path: {err:#}");
                Self::template()
            }
        }
    }

    pub fn template() -> Self {
        Self {
            app_log_max_size: Some(128),
            app_log_max_count: Some(8),
            clash_core: Some("verge-mihomo".into()),
            language: Some(clash_verge_i18n::system_language().into()),
            theme_mode: Some("system".into()),
            #[cfg(not(target_os = "windows"))]
            env_type: Some("bash".into()),
            #[cfg(target_os = "windows")]
            env_type: Some("powershell".into()),
            start_page: Some("/".into()),
            traffic_graph: Some(true),
            enable_memory_usage: Some(true),
            enable_group_icon: Some(true),
            pause_render_traffic_stats_on_blur: Some(true),
            #[cfg(target_os = "macos")]
            tray_icon: Some("monochrome".into()),
            menu_icon: Some("monochrome".into()),
            notice_position: Some("top-right".into()),
            collapse_navbar: Some(false),
            common_tray_icon: Some(false),
            sysproxy_tray_icon: Some(false),
            tun_tray_icon: Some(false),
            enable_auto_launch: Some(false),
            enable_silent_start: Some(false),
            enable_hover_jump_navigator: Some(true),
            hover_jump_navigator_delay: Some(280),
            enable_system_proxy: Some(false),
            proxy_auto_config: Some(false),
            pac_file_content: Some(DEFAULT_PAC.into()),
            proxy_host: Some("127.0.0.1".into()),
            #[cfg(not(target_os = "windows"))]
            verge_redir_port: Some(7895),
            #[cfg(not(target_os = "windows"))]
            verge_redir_enabled: Some(false),
            #[cfg(target_os = "linux")]
            verge_tproxy_port: Some(7896),
            #[cfg(target_os = "linux")]
            verge_tproxy_enabled: Some(false),
            verge_mixed_port: Some(network::ports::DEFAULT_MIXED),
            verge_socks_port: Some(7898),
            verge_socks_enabled: Some(false),
            verge_port: Some(7899),
            verge_http_enabled: Some(false),
            enable_proxy_guard: Some(false),
            enable_bypass_check: Some(true),
            use_default_bypass: Some(true),
            proxy_guard_duration: Some(30),
            auto_close_connection: Some(true),
            auto_check_update: Some(true),
            enable_builtin_enhanced: Some(true),
            auto_log_clean: Some(2), // 1: 1天, 2: 7天, 3: 30天, 4: 90天
            enable_auto_backup_schedule: Some(false),
            auto_backup_interval_hours: Some(24),
            auto_backup_on_change: Some(true),
            webdav_url: None,
            webdav_username: None,
            webdav_password: None,
            #[cfg(target_os = "macos")]
            enable_tray_speed: Some(false),
            tray_proxy_groups_display_mode: Some("default".into()),
            tray_inline_outbound_modes: Some(false),
            enable_global_hotkey: Some(true),
            enable_auto_light_weight_mode: Some(false),
            auto_light_weight_minutes: Some(10),
            enable_dns_settings: Some(false),
            home_cards: None,
            enable_external_controller: Some(false),
            subscription_settings: Some(SubscriptionSettings {
                default_update_interval: Some(1440),
                update_on_startup: Some(false),
                update_only_current: Some(false),
                notify_on_failure: Some(true),
            }),
            node_health_settings: Some(NodeHealthSettings {
                enabled: Some(false),
                interval_minutes: Some(10),
                deep_interval_minutes: Some(60),
                scope: Some("current_group".into()),
                l1_targets: Some(vec![ProbeTarget {
                    url: "https://cp.cloudflare.com/generate_204".into(),
                    expected: "204".into(),
                    keyword: None,
                    timeout_ms: None,
                }]),
                l2_targets: Some(vec![
                    ProbeTarget {
                        url: "https://www.google.com/robots.txt".into(),
                        expected: "200".into(),
                        keyword: None,
                        timeout_ms: None,
                    },
                    ProbeTarget {
                        url: "https://github.com/favicon.ico".into(),
                        expected: "200".into(),
                        keyword: None,
                        timeout_ms: None,
                    },
                    ProbeTarget {
                        url: "https://www.cloudflare.com/cdn-cgi/trace".into(),
                        expected: "200".into(),
                        keyword: None,
                        timeout_ms: None,
                    },
                ]),
                verify_on_select: Some(true),
                auto_failover: Some(false),
            }),
            node_display_settings: Some(NodeDisplaySettings {
                hide_timeout: Some(false),
                hide_untested: Some(false),
                hide_unhealthy: Some(false),
                hide_slow_ms: Some(0),
                default_sort: Some(0),
            }),
            subscription_info_settings: Some(SubscriptionInfoSettings {
                show_traffic: Some(true),
                expiry_warning_days: Some(3),
            }),
            sync_settings: Some(SyncSettings {
                enabled: Some(false),
                mode: Some("two_way".into()),
                scopes: Some(vec!["subscriptions".into(), "overrides".into(), "custom_rules".into()]),
                interval_minutes: Some(360),
                on_startup: Some(true),
                on_change: Some(false),
                conflict_policy: Some("last_modified".into()),
                device_name: None,
                encrypt: Some(false),
                password: None,
                danger_accept_invalid_certs: Some(false),
            }),
            ..Self::default()
        }
    }

    pub async fn save_file(&self) -> Result<()> {
        help::save_yaml(&dirs::verge_path()?, &self, Some("# Clash Verge Config")).await
    }

    #[allow(clippy::cognitive_complexity)]
    pub fn patch_config(&mut self, patch: &Self) {
        macro_rules! patch {
            ($key: tt) => {
                if patch.$key.is_some() {
                    self.$key = patch.$key.clone();
                }
            };
        }

        patch!(app_log_level);
        patch!(app_log_max_size);
        patch!(app_log_max_count);

        patch!(language);
        patch!(theme_mode);
        patch!(tray_event);
        patch!(env_type);
        patch!(start_page);
        patch!(startup_script);
        patch!(traffic_graph);
        patch!(enable_memory_usage);
        patch!(enable_group_icon);
        patch!(pause_render_traffic_stats_on_blur);
        #[cfg(target_os = "macos")]
        patch!(tray_icon);
        patch!(menu_icon);
        patch!(menu_order);
        patch!(proxy_group_tools_position);
        patch!(notice_position);
        patch!(collapse_navbar);
        patch!(common_tray_icon);
        patch!(sysproxy_tray_icon);
        patch!(tun_tray_icon);

        patch!(enable_tun_mode);
        patch!(enable_auto_launch);
        patch!(enable_silent_start);
        patch!(enable_hover_jump_navigator);
        patch!(hover_jump_navigator_delay);
        #[cfg(not(target_os = "windows"))]
        patch!(verge_redir_port);
        #[cfg(not(target_os = "windows"))]
        patch!(verge_redir_enabled);
        #[cfg(target_os = "linux")]
        patch!(verge_tproxy_port);
        #[cfg(target_os = "linux")]
        patch!(verge_tproxy_enabled);
        patch!(verge_mixed_port);
        patch!(verge_socks_port);
        patch!(verge_socks_enabled);
        patch!(verge_port);
        patch!(verge_http_enabled);
        patch!(enable_system_proxy);
        patch!(enable_proxy_guard);
        patch!(enable_bypass_check);
        patch!(use_default_bypass);
        patch!(system_proxy_bypass);
        patch!(proxy_guard_duration);
        patch!(proxy_auto_config);
        patch!(pac_file_content);
        patch!(proxy_host);
        patch!(theme_setting);
        patch!(web_ui_list);
        patch!(clash_core);
        patch!(hotkeys);
        patch!(enable_global_hotkey);

        patch!(auto_close_connection);
        patch!(auto_check_update);
        patch!(default_latency_test);
        patch!(default_latency_timeout);
        patch!(enable_auto_delay_detection);
        patch!(auto_delay_detection_interval_minutes);
        patch!(enable_builtin_enhanced);
        patch!(proxy_layout_column);
        patch!(test_list);
        patch!(auto_log_clean);
        patch!(enable_auto_backup_schedule);
        patch!(auto_backup_interval_hours);
        patch!(auto_backup_on_change);

        patch!(webdav_url);
        patch!(webdav_username);
        patch!(webdav_password);
        #[cfg(target_os = "macos")]
        patch!(enable_tray_speed);
        patch!(tray_proxy_groups_display_mode);
        patch!(tray_inline_outbound_modes);
        patch!(enable_auto_light_weight_mode);
        patch!(auto_light_weight_minutes);
        patch!(enable_dns_settings);
        patch!(home_cards);
        patch!(enable_external_controller);

        if let Some(incoming) = patch.subscription_settings.as_ref() {
            match self.subscription_settings.as_mut() {
                Some(current) => current.patch_config(incoming),
                None => self.subscription_settings = Some(incoming.clone()),
            }
        }
        if let Some(incoming) = patch.node_health_settings.as_ref() {
            match self.node_health_settings.as_mut() {
                Some(current) => current.patch_config(incoming),
                None => self.node_health_settings = Some(incoming.clone()),
            }
        }
        if let Some(incoming) = patch.node_display_settings.as_ref() {
            match self.node_display_settings.as_mut() {
                Some(current) => current.patch_config(incoming),
                None => self.node_display_settings = Some(incoming.clone()),
            }
        }
        if let Some(incoming) = patch.subscription_info_settings.as_ref() {
            match self.subscription_info_settings.as_mut() {
                Some(current) => current.patch_config(incoming),
                None => self.subscription_info_settings = Some(incoming.clone()),
            }
        }
        if let Some(incoming) = patch.sync_settings.as_ref() {
            match self.sync_settings.as_mut() {
                Some(current) => current.patch_config(incoming),
                None => self.sync_settings = Some(incoming.clone()),
            }
        }
    }

    pub fn get_log_level(&self) -> LevelFilter {
        if let Some(level) = self.app_log_level.as_ref() {
            match level.to_lowercase().as_str() {
                "silent" => LevelFilter::Off,
                "error" => LevelFilter::Error,
                "warn" => LevelFilter::Warn,
                "info" => LevelFilter::Info,
                "debug" => LevelFilter::Debug,
                "trace" => LevelFilter::Trace,
                _ => LevelFilter::Info,
            }
        } else {
            LevelFilter::Info
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_settings_patches_only_overwrite_present_fields() {
        let mut config = IVerge::template();
        let patch = IVerge {
            node_health_settings: Some(NodeHealthSettings {
                enabled: Some(true),
                interval_minutes: Some(5),
                ..NodeHealthSettings::default()
            }),
            node_display_settings: Some(NodeDisplaySettings {
                hide_timeout: Some(true),
                ..NodeDisplaySettings::default()
            }),
            sync_settings: Some(SyncSettings {
                enabled: Some(true),
                ..SyncSettings::default()
            }),
            ..IVerge::default()
        };

        config.patch_config(&patch);

        let health = config.node_health_settings.as_ref();
        assert_eq!(health.and_then(|settings| settings.enabled), Some(true));
        assert_eq!(health.and_then(|settings| settings.interval_minutes), Some(5));
        assert_eq!(health.and_then(|settings| settings.deep_interval_minutes), Some(60));
        assert_eq!(
            health.and_then(|settings| settings.scope.as_deref()),
            Some("current_group")
        );
        assert_eq!(
            health.and_then(|settings| settings.l1_targets.as_ref().map(Vec::len)),
            Some(1)
        );

        let display = config.node_display_settings.as_ref();
        assert_eq!(display.and_then(|settings| settings.hide_timeout), Some(true));
        assert_eq!(display.and_then(|settings| settings.hide_untested), Some(false));
        assert_eq!(display.and_then(|settings| settings.hide_slow_ms), Some(0));

        let sync = config.sync_settings.as_ref();
        assert_eq!(sync.and_then(|settings| settings.enabled), Some(true));
        assert_eq!(sync.and_then(|settings| settings.mode.as_deref()), Some("two_way"));
        assert_eq!(sync.and_then(|settings| settings.interval_minutes), Some(360));
        assert_eq!(sync.and_then(|settings| settings.on_startup), Some(true));
        assert_eq!(sync.and_then(|settings| settings.on_change), Some(false));
        assert_eq!(
            sync.and_then(|settings| settings.scopes.as_ref().map(Vec::len)),
            Some(3)
        );
    }
}
