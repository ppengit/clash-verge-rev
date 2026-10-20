use super::manifest::{MANIFEST_MAX_BYTES, PAYLOAD_MAX_BYTES, SyncManifest};
use crate::{core::backup::parse_webdav_list, utils::help};
use anyhow::{Context as _, Result, bail};
use futures::StreamExt as _;
use reqwest_dav::list_cmd::ListEntity;
use std::{path::Path, time::Duration};
use tokio::{
    fs,
    io::AsyncWriteExt as _,
    time::{sleep, timeout},
};

pub const SYNC_REMOTE_DIR: &str = "clash-verge-sync";
const HISTORY_DIR: &str = "history";

const TIMEOUT_MANIFEST: Duration = Duration::from_secs(15);
const TIMEOUT_LIST: Duration = Duration::from_secs(30);
const TIMEOUT_TRANSFER: Duration = Duration::from_secs(300);
const MAX_ATTEMPTS: usize = 3;
const HISTORY_LIST_MAX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_HISTORY_ENTRIES: usize = 200;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub struct SyncWebDavConfig {
    pub url: std::string::String,
    pub username: std::string::String,
    pub password: std::string::String,
    pub danger_accept_invalid_certs: bool,
}

impl SyncWebDavConfig {
    pub fn from_verge(verge: &crate::config::IVerge) -> Result<Self> {
        let url = verge.webdav_url.as_ref().map(ToString::to_string).unwrap_or_default();
        let username = verge
            .webdav_username
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        let password = verge
            .webdav_password
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        if url.trim().is_empty() || username.trim().is_empty() || password.is_empty() {
            bail!("WebDAV sync requires the WebDAV URL, username, and password to be configured");
        }
        Ok(Self {
            url,
            username,
            password,
            danger_accept_invalid_certs: verge
                .sync_settings
                .as_ref()
                .and_then(|settings| settings.danger_accept_invalid_certs)
                .unwrap_or(false),
        })
    }
}

pub fn manifest_path() -> String {
    format!("{SYNC_REMOTE_DIR}/manifest.json")
}

pub fn payload_path() -> String {
    format!("{SYNC_REMOTE_DIR}/payload.zip")
}

pub fn history_zip_path(revision: u64, device_id: &str) -> String {
    format!("{SYNC_REMOTE_DIR}/{HISTORY_DIR}/{revision}-{device_id}.zip")
}

pub fn history_manifest_path(revision: u64, device_id: &str) -> String {
    format!("{SYNC_REMOTE_DIR}/{HISTORY_DIR}/{revision}-{device_id}.json")
}

pub struct SyncWebDavClient {
    client: reqwest_dav::Client,
}

impl SyncWebDavClient {
    pub fn new(config: &SyncWebDavConfig) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .use_rustls_tls()
            .timeout(TIMEOUT_TRANSFER)
            .user_agent(format!(
                "clash-verge/{} ({} WebDAV-Sync)",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS
            ))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    attempt.error("too many redirects")
                } else {
                    attempt.follow()
                }
            }));
        if config.danger_accept_invalid_certs {
            builder = builder.danger_accept_invalid_certs(true);
        }

        let client = reqwest_dav::ClientBuilder::new()
            .set_agent(builder.build()?)
            .set_host(config.url.trim_end_matches('/').to_owned())
            .set_auth(reqwest_dav::Auth::Basic(
                config.username.clone(),
                config.password.clone(),
            ))
            .build()?;
        Ok(Self { client })
    }

    /// Creates the sync root and history collection; a pre-existing collection is fine.
    pub async fn ensure_directories(&self) -> Result<()> {
        self.mkcol(SYNC_REMOTE_DIR).await?;
        self.mkcol(&format!("{SYNC_REMOTE_DIR}/{HISTORY_DIR}")).await?;
        Ok(())
    }

    async fn mkcol(&self, path: &str) -> Result<()> {
        match self.client.mkcol(path).await {
            Ok(()) => Ok(()),
            Err(error) => {
                let status = dav_error_status(&error);
                let message = error.to_string().to_ascii_lowercase();
                let already_exists = status == Some(405)
                    || (matches!(status, Some(code) if (400..500).contains(&code) && code != 401 && code != 403)
                        && (message.contains("already exist") || message.contains("already taken")));
                if already_exists {
                    return Ok(());
                }
                if status == Some(409) {
                    bail!("the WebDAV sync directory's parent folder does not exist");
                }
                Err(anyhow::Error::new(error)).context("failed to create the WebDAV sync directory")
            }
        }
    }

    pub async fn fetch_manifest(&self) -> Result<Option<SyncManifest>> {
        let response = retry(|| async {
            timeout(TIMEOUT_MANIFEST, self.client.get_raw(&manifest_path()))
                .await
                .map_err(|_| anyhow::anyhow!("timed out reading the WebDAV sync manifest"))?
                .map_err(anyhow::Error::new)
        })
        .await?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            bail!("the WebDAV server rejected the sync credentials ({status})");
        }
        if !status.is_success() {
            bail!("the WebDAV server returned {status} for the sync manifest");
        }

        let bytes = read_body_bounded(response, MANIFEST_MAX_BYTES).await?;
        let manifest: SyncManifest =
            serde_json::from_slice(&bytes).context("the WebDAV sync manifest is not valid JSON")?;
        manifest.validate()?;
        Ok(Some(manifest))
    }

    pub async fn upload_manifest(&self, manifest: &SyncManifest) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(manifest)?;
        self.upload_bytes(&manifest_path(), bytes).await
    }

    pub async fn upload_payload(&self, bytes: Vec<u8>) -> Result<()> {
        if bytes.len() as u64 > PAYLOAD_MAX_BYTES {
            bail!("refusing to upload an oversized sync payload");
        }
        self.upload_bytes(&payload_path(), bytes).await
    }

    async fn upload_bytes(&self, path: &str, bytes: Vec<u8>) -> Result<()> {
        retry(|| async {
            timeout(TIMEOUT_TRANSFER, self.client.put(path, bytes.clone()))
                .await
                .map_err(|_| anyhow::anyhow!("timed out uploading to WebDAV"))?
                .map_err(anyhow::Error::new)
        })
        .await
        .with_context(|| format!("failed to upload {path}"))
    }

    pub async fn download_to_file(&self, path: &str, destination: &Path, cap: u64) -> Result<u64> {
        let response = retry(|| async {
            timeout(TIMEOUT_TRANSFER, self.client.get_raw(path))
                .await
                .map_err(|_| anyhow::anyhow!("timed out downloading from WebDAV"))?
                .map_err(anyhow::Error::new)
        })
        .await?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            bail!("the requested sync snapshot no longer exists on the WebDAV server");
        }
        if !status.is_success() {
            bail!("the WebDAV server returned {status} for {path}");
        }
        if response.content_length().is_some_and(|length| length > cap) {
            bail!("the remote sync payload exceeds the {cap} byte limit");
        }

        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }
        let mut file = fs::File::create(destination).await?;
        let mut stream = response.bytes_stream();
        let mut written = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("failed while downloading the sync payload")?;
            written = written.saturating_add(chunk.len() as u64);
            if written > cap {
                drop(file);
                let _ = fs::remove_file(destination).await;
                bail!("the remote sync payload exceeds the {cap} byte limit");
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        Ok(written)
    }

    pub async fn download_small(&self, path: &str, cap: u64) -> Result<Option<Vec<u8>>> {
        let response = retry(|| async {
            timeout(TIMEOUT_MANIFEST, self.client.get_raw(path))
                .await
                .map_err(|_| anyhow::anyhow!("timed out reading from WebDAV"))?
                .map_err(anyhow::Error::new)
        })
        .await?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            bail!("the WebDAV server rejected the sync credentials ({status})");
        }
        if !status.is_success() {
            bail!("the WebDAV server returned {status}");
        }
        Ok(Some(read_body_bounded(response, cap).await?))
    }

    pub async fn list_history(&self) -> Result<Vec<RemoteHistoryFile>> {
        let path = format!("{SYNC_REMOTE_DIR}/{HISTORY_DIR}/");
        let response = retry(|| async {
            timeout(TIMEOUT_LIST, self.client.list_raw(&path, reqwest_dav::Depth::Number(1)))
                .await
                .map_err(|_| anyhow::anyhow!("timed out listing WebDAV history"))?
                .map_err(anyhow::Error::new)
        })
        .await?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if !status.is_success() {
            bail!("the WebDAV server returned {status} while listing sync history");
        }
        let xml = read_body_bounded(response, HISTORY_LIST_MAX_BYTES).await?;
        let xml = String::from_utf8_lossy(&xml);
        let mut files = Vec::new();
        for entity in parse_webdav_list(&xml)? {
            if let ListEntity::File(file) = entity {
                let name = file
                    .href
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                if name.is_empty() {
                    continue;
                }
                files.push(RemoteHistoryFile {
                    name,
                    size: u64::try_from(file.content_length).unwrap_or(0),
                    modified: file.last_modified,
                });
            }
        }
        files.sort_by(|left, right| right.name.cmp(&left.name));
        files.truncate(MAX_HISTORY_ENTRIES);
        Ok(files)
    }

    pub async fn delete_history_file(&self, name: &str) -> Result<()> {
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
            bail!("refusing to delete an unsafe WebDAV history entry name");
        }
        let path = format!("{SYNC_REMOTE_DIR}/{HISTORY_DIR}/{name}");
        self.delete_path(&path).await
    }

    async fn delete_path(&self, path: &str) -> Result<()> {
        let response = retry(|| async {
            timeout(TIMEOUT_LIST, self.client.delete_raw(path))
                .await
                .map_err(|_| anyhow::anyhow!("timed out deleting a WebDAV entry"))?
                .map_err(anyhow::Error::new)
        })
        .await?;
        let status = response.status();
        if !status.is_success() && status != reqwest::StatusCode::NOT_FOUND {
            bail!("the WebDAV server returned {status} while deleting {path}");
        }
        Ok(())
    }

    /// Writes a small probe file, reads it back, and removes it; no user data is uploaded.
    pub async fn probe(&self, device_id: &str) -> Result<()> {
        let name = format!(
            "{SYNC_REMOTE_DIR}/.probe-{}-{}",
            sanitize_probe_segment(device_id),
            help::get_uid("p")
        );
        let content = help::get_uid("probe").as_bytes().to_vec();
        self.upload_bytes(&name, content.clone()).await?;
        let read_back = self
            .download_small(&name, 1024)
            .await?
            .ok_or_else(|| anyhow::anyhow!("the WebDAV probe file disappeared after upload"))?;
        let _ = self.delete_path(&name).await;
        if read_back != content {
            bail!("the WebDAV server returned different bytes for the probe file");
        }
        Ok(())
    }

    pub async fn history_upload(
        &self,
        revision: u64,
        device_id: &str,
        payload: &[u8],
        manifest: &SyncManifest,
    ) -> Result<()> {
        let zip_path = history_zip_path(revision, device_id);
        self.upload_bytes(&zip_path, payload.to_vec()).await?;
        let manifest_path = history_manifest_path(revision, device_id);
        let bytes = serde_json::to_vec_pretty(manifest)?;
        self.upload_bytes(&manifest_path, bytes).await
    }

    /// Keeps the newest `keep` revisions; failures are logged by the caller as non-fatal.
    pub async fn prune_history(&self, keep: usize) -> Result<()> {
        let files = self.list_history().await?;
        let mut revisions: Vec<(u64, Vec<&RemoteHistoryFile>)> = Vec::new();
        for file in &files {
            let Some(revision) = revision_from_history_name(&file.name) else {
                continue;
            };
            if let Some((_, group)) = revisions.iter_mut().find(|(value, _)| *value == revision) {
                group.push(file);
            } else {
                revisions.push((revision, vec![file]));
            }
        }
        revisions.sort_by_key(|(revision, _)| std::cmp::Reverse(*revision));
        for (_, group) in revisions.into_iter().skip(keep) {
            for file in group {
                self.delete_history_file(&file.name).await?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RemoteHistoryFile {
    pub name: String,
    pub size: u64,
    pub modified: chrono::DateTime<chrono::Utc>,
}

pub fn revision_from_history_name(name: &str) -> Option<u64> {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    stem.split('-').next()?.parse().ok()
}

fn sanitize_probe_segment(value: &str) -> String {
    value.chars().filter(|c| c.is_ascii_alphanumeric()).take(24).collect()
}

async fn read_body_bounded(response: reqwest::Response, cap: u64) -> Result<Vec<u8>> {
    if response.content_length().is_some_and(|length| length > cap) {
        bail!("the remote manifest exceeds the {cap} byte limit");
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("failed while reading the WebDAV response body")?;
        if bytes.len() as u64 + chunk.len() as u64 > cap {
            bail!("the WebDAV response exceeds the {cap} byte limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn retry<T, F, Fut>(operation: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut delay = RETRY_BASE_DELAY;
    let mut last_error = None;
    for attempt in 0..MAX_ATTEMPTS {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                if is_terminal(&error) {
                    return Err(error);
                }
                last_error = Some(error);
                if attempt + 1 < MAX_ATTEMPTS {
                    sleep(delay).await;
                    delay = delay.saturating_mul(2);
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("WebDAV request failed")))
}

fn is_terminal(error: &anyhow::Error) -> bool {
    matches!(error_status(error), Some(400 | 401 | 403 | 404 | 405 | 409 | 507))
}

/// Extracts the HTTP status carried anywhere in an error chain.
pub fn error_status(error: &anyhow::Error) -> Option<u16> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<reqwest_dav::Error>()
            .and_then(dav_error_status)
            .or_else(|| {
                cause
                    .downcast_ref::<reqwest::Error>()
                    .and_then(reqwest::Error::status)
                    .map(|status| status.as_u16())
            })
    })
}

fn dav_error_status(error: &reqwest_dav::Error) -> Option<u16> {
    match error {
        reqwest_dav::Error::Reqwest(error) => error.status().map(|status| status.as_u16()),
        reqwest_dav::Error::Decode(reqwest_dav::DecodeError::Server(server)) => Some(server.response_code),
        reqwest_dav::Error::Decode(reqwest_dav::DecodeError::StatusMismatched(mismatch)) => {
            Some(mismatch.response_code)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_names_round_trip_to_their_revision() {
        assert_eq!(revision_from_history_name("12-Dabc.zip"), Some(12));
        assert_eq!(revision_from_history_name("7-Dabc.json"), Some(7));
        assert_eq!(revision_from_history_name("not-a-revision.zip"), None);
    }

    #[test]
    fn sync_paths_stay_under_the_sync_root() {
        assert_eq!(manifest_path(), "clash-verge-sync/manifest.json");
        assert_eq!(history_zip_path(3, "Dabc"), "clash-verge-sync/history/3-Dabc.zip");
    }
}
