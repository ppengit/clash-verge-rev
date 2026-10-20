//! Expected-status probes against the mihomo controller and content probes through the local
//! mixed proxy.
//!
//! `tauri-plugin-mihomo` only forwards `url` and `timeout` to the delay endpoints, so node health
//! verification talks to the external controller directly to pass `expected`.

use crate::config::{ClashInfo, Config};
use crate::core::manager::{CoreManager, RunningMode};
use crate::utils::dirs;
use clash_verge_logging::{Type, logging};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

pub const MAX_TARGETS: usize = 5;
pub const MIN_TIMEOUT_MS: u32 = 1_000;
pub const MAX_TIMEOUT_MS: u32 = 30_000;

const RESPONSE_BODY_LIMIT: usize = 4 * 1024;
const REQUEST_TIMEOUT_GRACE: Duration = Duration::from_secs(2);
const CONTENT_REDIRECT_LIMIT: usize = 5;

/// A validated probe target, ready to be sent to the controller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeTargetSpec {
    pub url: std::string::String,
    pub expected: std::string::String,
    /// Only used by content probes; the controller delay endpoints ignore it.
    pub keyword: Option<std::string::String>,
    pub timeout_ms: u32,
}

impl ProbeTargetSpec {
    /// Returns `None` for targets that cannot be probed safely.
    pub fn from_config(target: &crate::config::ProbeTarget, default_timeout_ms: u32) -> Option<Self> {
        let url = target.url.trim();
        if !is_probe_url(url) {
            logging!(
                debug,
                Type::Network,
                "Ignoring node health target with invalid URL: {url:?}"
            );
            return None;
        }
        let expected = target.expected.trim();
        if !validate_expected(expected) {
            logging!(
                debug,
                Type::Network,
                "Ignoring node health target with invalid expected status {expected:?}"
            );
            return None;
        }
        let timeout_ms = target
            .timeout_ms
            .unwrap_or(default_timeout_ms)
            .clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS);
        Some(Self {
            url: url.into(),
            expected: expected.into(),
            keyword: target
                .keyword
                .as_deref()
                .map(str::trim)
                .filter(|keyword| !keyword.is_empty())
                .map(Into::into),
            timeout_ms,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProbeResult {
    pub ok: bool,
    pub delay_ms: Option<u32>,
    pub error: Option<std::string::String>,
}

impl ProbeResult {
    fn failed(error: impl Into<std::string::String>) -> Self {
        Self {
            ok: false,
            delay_ms: None,
            error: Some(error.into()),
        }
    }

    const fn succeeded(delay_ms: u32) -> Self {
        Self {
            ok: true,
            delay_ms: Some(delay_ms),
            error: None,
        }
    }
}

pub fn is_probe_url(raw: &str) -> bool {
    let raw = raw.trim();
    raw.split_once("://")
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        && Url::parse(raw).is_ok_and(|url| url.has_host() && matches!(url.scheme(), "http" | "https"))
}

/// Accepts `204`, `200`, `200-299`, and `/`-separated combinations such as `200/204`.
pub fn validate_expected(expected: &str) -> bool {
    parse_expected_ranges(expected).is_some()
}

pub fn status_matches_expected(expected: &str, status: u16) -> bool {
    parse_expected_ranges(expected)
        .is_none_or(|ranges| ranges.iter().any(|(start, end)| (*start..=*end).contains(&status)))
}

fn parse_expected_ranges(expected: &str) -> Option<Vec<(u16, u16)>> {
    let expected = expected.trim();
    if expected.is_empty() {
        return None;
    }
    let mut ranges = Vec::new();
    for part in expected.split('/') {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        let (start, end) = match part.split_once('-') {
            Some((start, end)) => (start.trim().parse::<u16>().ok()?, end.trim().parse::<u16>().ok()?),
            None => {
                let code = part.parse::<u16>().ok()?;
                (code, code)
            }
        };
        if !(100..=599).contains(&start) || !(100..=599).contains(&end) || start > end {
            return None;
        }
        ranges.push((start, end));
    }
    (!ranges.is_empty()).then_some(ranges)
}

/// Sends per-node delay probes to the mihomo external controller.
pub struct ControllerClient {
    base_url: Url,
    secret: Option<std::string::String>,
    client: reqwest::Client,
}

impl ControllerClient {
    pub async fn from_config() -> anyhow::Result<Self> {
        let info = Config::clash().await.data_arc().get_client_info();
        if tcp_controller_enabled().await {
            Self::new(&info)
        } else {
            // Service-managed cores listen on the service pipe; sidecar cores use their own
            // per-process pipe. The plugin tracks the same distinction when the core starts.
            let socket_path = if matches!(*CoreManager::global().get_running_mode(), RunningMode::Service) {
                dirs::ipc_path()?
            } else {
                dirs::sidecar_ipc_path()?
            };
            Self::new_ipc(&socket_path)
        }
    }

    pub fn new(info: &ClashInfo) -> anyhow::Result<Self> {
        let base_url = Url::parse(&format!("http://{}", info.server.trim()))
            .map_err(|error| anyhow::anyhow!("invalid controller address {:?}: {error}", info.server))?;
        let client = reqwest::Client::builder()
            // The controller is local; a system proxy pointed at the core must not wrap this call.
            .no_proxy()
            .build()?;
        let secret = info
            .secret
            .as_deref()
            .map(str::trim)
            .filter(|secret| !secret.is_empty())
            .map(str::to_owned);
        Ok(Self {
            base_url,
            secret,
            client,
        })
    }

    fn new_ipc(socket_path: &std::path::Path) -> anyhow::Result<Self> {
        let socket_path = socket_path.to_string_lossy().into_owned();
        let builder = reqwest::Client::builder().no_proxy();
        #[cfg(unix)]
        let builder = builder.unix_socket(socket_path.as_str());
        #[cfg(windows)]
        let builder = builder.windows_named_pipe(socket_path.as_str());
        Ok(Self {
            base_url: Url::parse("http://localhost")?,
            secret: None,
            client: builder.build()?,
        })
    }

    /// Probes one node through either the proxy delay endpoint or the provider healthcheck one.
    pub async fn probe(&self, node: &str, provider: Option<&str>, target: &ProbeTargetSpec) -> ProbeResult {
        let url = match self.endpoint(node, provider, target) {
            Ok(url) => url,
            Err(error) => return ProbeResult::failed(format!("invalid controller endpoint: {error}")),
        };
        let mut request = self
            .client
            .get(url)
            .timeout(Duration::from_millis(target.timeout_ms as u64) + REQUEST_TIMEOUT_GRACE);
        if let Some(secret) = self.secret.as_deref() {
            request = request.bearer_auth(secret);
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => return ProbeResult::failed(format!("controller request failed: {error}")),
        };
        let status = response.status();
        let body = match read_body_limited(response, RESPONSE_BODY_LIMIT).await {
            Ok(body) => body,
            Err(error) => return ProbeResult::failed(error),
        };

        if !status.is_success() {
            let message = parse_error_message(&body)
                .unwrap_or_else(|| std::string::String::from_utf8_lossy(&body).trim().to_owned());
            return ProbeResult::failed(if message.is_empty() {
                format!("controller returned HTTP {status}")
            } else {
                message
            });
        }

        match serde_json::from_slice::<DelayResponse>(&body) {
            Ok(DelayResponse { delay }) if delay > 0 => ProbeResult::succeeded(delay),
            Ok(_) => ProbeResult::failed("controller reported an empty delay"),
            Err(error) => ProbeResult::failed(format!("invalid controller response: {error}")),
        }
    }

    fn endpoint(&self, node: &str, provider: Option<&str>, target: &ProbeTargetSpec) -> anyhow::Result<Url> {
        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("controller address cannot hold path segments"))?;
            segments.clear();
            match provider {
                Some(provider) => {
                    segments.push("providers");
                    segments.push("proxies");
                    segments.push(provider);
                    segments.push(node);
                    segments.push("healthcheck");
                }
                None => {
                    segments.push("proxies");
                    segments.push(node);
                    segments.push("delay");
                }
            }
        }
        url.query_pairs_mut()
            .append_pair("url", &target.url)
            .append_pair("timeout", &target.timeout_ms.to_string())
            .append_pair("expected", &target.expected);
        Ok(url)
    }
}

#[derive(Deserialize)]
struct DelayResponse {
    #[serde(default)]
    delay: u32,
}

#[derive(Deserialize)]
struct ErrorResponse {
    #[serde(default)]
    message: Option<std::string::String>,
}

fn parse_error_message(body: &[u8]) -> Option<std::string::String> {
    serde_json::from_slice::<ErrorResponse>(body)
        .ok()
        .and_then(|response| response.message)
        .map(|message| message.trim().to_owned())
        .filter(|message| !message.is_empty())
}

async fn read_body_limited(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, std::string::String> {
    let mut body = Vec::with_capacity(limit.min(1024));
    while body.len() < limit {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let take = chunk.len().min(limit - body.len());
                body.extend_from_slice(&chunk[..take]);
            }
            Ok(None) => break,
            Err(error) => return Err(format!("failed to read controller response: {error}")),
        }
    }
    Ok(body)
}

/// Options for a real-site content probe through the local mixed proxy.
pub struct ContentProbeSpec {
    pub url: std::string::String,
    pub expected: std::string::String,
    pub keyword: Option<std::string::String>,
    pub timeout_ms: u32,
    pub max_body_bytes: usize,
}

impl ContentProbeSpec {
    pub const MAX_BODY_BYTES: usize = 64 * 1024;

    pub fn new(url: &str, expected: &str, keyword: Option<&str>, timeout_ms: u32) -> Option<Self> {
        if !is_probe_url(url) || !validate_expected(expected) {
            return None;
        }
        Some(Self {
            url: url.trim().into(),
            expected: expected.trim().into(),
            keyword: keyword
                .map(str::trim)
                .filter(|keyword| !keyword.is_empty())
                .map(Into::into),
            timeout_ms: timeout_ms.clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS),
            max_body_bytes: Self::MAX_BODY_BYTES,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ContentProbeResult {
    pub url: std::string::String,
    pub ok: bool,
    pub status: Option<u16>,
    /// Time until response headers arrived.
    pub ttfb_ms: u64,
    pub total_ms: u64,
    pub body_bytes: u64,
    pub keyword_matched: Option<bool>,
    pub error: Option<std::string::String>,
}

impl ContentProbeResult {
    fn failed(url: &str, started: Instant, error: impl Into<std::string::String>) -> Self {
        Self {
            url: url.into(),
            ok: false,
            status: None,
            ttfb_ms: 0,
            total_ms: elapsed_millis(started),
            body_bytes: 0,
            keyword_matched: None,
            error: Some(error.into()),
        }
    }
}

/// Probes one URL through the core's mixed port. Only the selected node should be verified this
/// way; it is never part of a background all-node cycle.
pub async fn probe_content(spec: &ContentProbeSpec) -> ContentProbeResult {
    let started = Instant::now();
    let proxy = match mixed_proxy_url().await {
        Ok(proxy) => proxy,
        Err(error) => return ContentProbeResult::failed(&spec.url, started, format!("{error:#}")),
    };
    let proxy = match reqwest::Proxy::all(&proxy) {
        Ok(proxy) => proxy,
        Err(error) => return ContentProbeResult::failed(&spec.url, started, format!("invalid proxy: {error}")),
    };
    let client = match reqwest::Client::builder()
        .proxy(proxy)
        .redirect(reqwest::redirect::Policy::limited(CONTENT_REDIRECT_LIMIT))
        .timeout(Duration::from_millis(spec.timeout_ms as u64))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return ContentProbeResult::failed(&spec.url, started, format!("failed to build probe client: {error}"));
        }
    };

    let response = match client.get(&spec.url).send().await {
        Ok(response) => response,
        Err(error) => return ContentProbeResult::failed(&spec.url, started, format!("request failed: {error}")),
    };
    let ttfb_ms = elapsed_millis(started);
    let status = response.status().as_u16();
    let body = match read_body_limited(response, spec.max_body_bytes).await {
        Ok(body) => body,
        Err(error) => return ContentProbeResult::failed(&spec.url, started, error),
    };
    let total_ms = elapsed_millis(started);
    let keyword_matched = spec.keyword.as_deref().map(|keyword| {
        String::from_utf8_lossy(&body)
            .to_lowercase()
            .contains(&keyword.to_lowercase())
    });
    let status_ok = status_matches_expected(&spec.expected, status);
    let ok = status_ok && keyword_matched.unwrap_or(true);
    let error = if ok {
        None
    } else if !status_ok {
        Some(format!(
            "HTTP {status} does not match expected status {:?}",
            spec.expected
        ))
    } else {
        Some("keyword not found in response body".to_owned())
    };

    ContentProbeResult {
        url: spec.url.clone(),
        ok,
        status: Some(status),
        ttfb_ms,
        total_ms,
        body_bytes: body.len() as u64,
        keyword_matched,
        error,
    }
}

async fn mixed_proxy_url() -> anyhow::Result<std::string::String> {
    let info = Config::clash().await.data_arc().get_client_info();
    anyhow::ensure!(info.mixed_port > 0, "core mixed port is unavailable");
    Ok(format!("http://127.0.0.1:{}", info.mixed_port))
}

/// Whether the generated runtime config leaves the TCP controller listening.
async fn tcp_controller_enabled() -> bool {
    let runtime = Config::runtime().await.data_arc();
    runtime
        .config
        .as_ref()
        .and_then(|config| config.get("external-controller"))
        .and_then(|value| value.as_str())
        .is_some_and(|value| !value.trim().is_empty())
}

fn elapsed_millis(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProbeTarget;

    fn target(url: &str, expected: &str) -> ProbeTarget {
        ProbeTarget {
            url: url.into(),
            expected: expected.into(),
            keyword: None,
            timeout_ms: None,
        }
    }

    #[test]
    fn expected_status_syntax_accepts_codes_ranges_and_combinations() {
        for valid in ["204", "200", "200-299", "200/204", " 200 - 299 ", "301/200-299"] {
            assert!(validate_expected(valid), "{valid}");
        }
        for invalid in [
            "",
            "0",
            "99",
            "600",
            "300-200",
            "abc",
            "200-",
            "-200",
            "200//204",
            "https://example.com",
        ] {
            assert!(!validate_expected(invalid), "{invalid}");
        }
    }

    #[test]
    fn expected_status_matching_covers_each_range() {
        assert!(status_matches_expected("204", 204));
        assert!(!status_matches_expected("204", 200));
        assert!(status_matches_expected("200-299", 201));
        assert!(!status_matches_expected("200-299", 403));
        assert!(status_matches_expected("200/403", 403));
        assert!(!status_matches_expected("200/403", 500));
    }

    #[test]
    fn controller_targets_reject_invalid_urls_and_statuses() {
        assert!(is_probe_url("https://cp.cloudflare.com/generate_204"));
        assert!(is_probe_url(" http://example.com/ "));
        for invalid in ["", "example.com", "ftp://example.com", "http://bad host/", "http://"] {
            assert!(!is_probe_url(invalid), "{invalid}");
        }

        assert!(ProbeTargetSpec::from_config(&target("https://example.com", "200"), 5_000).is_some());
        assert!(ProbeTargetSpec::from_config(&target("", "200"), 5_000).is_none());
        assert!(ProbeTargetSpec::from_config(&target("https://example.com", "nope"), 5_000).is_none());
    }

    #[test]
    fn controller_targets_clamp_timeouts_to_the_safe_window() {
        let low = ProbeTargetSpec::from_config(&target("https://example.com", "200"), 0);
        assert_eq!(low.map(|target| target.timeout_ms), Some(MIN_TIMEOUT_MS));

        let high = ProbeTargetSpec::from_config(&target("https://example.com", "200"), u32::MAX);
        assert_eq!(high.map(|target| target.timeout_ms), Some(MAX_TIMEOUT_MS));
    }
}
