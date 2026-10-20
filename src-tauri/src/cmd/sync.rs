use super::{CmdResult, coded_error};
use crate::{
    core::sync::{SyncHistoryEntry, SyncManager, SyncRunResult, SyncStatus},
    utils::help,
};
use smartstring::alias::String;

fn sync_failure(code: &'static str, error: &anyhow::Error) -> super::CommandFailure {
    coded_error(code, help::mask_err(&format!("{error:#}")))
}

#[tauri::command]
#[specta::specta]
pub async fn get_sync_status() -> CmdResult<SyncStatus> {
    SyncManager::status()
        .await
        .map_err(|error| sync_failure("SYNC_STATUS_FAILED", &error))
}

#[tauri::command]
#[specta::specta]
pub async fn sync_now() -> CmdResult<SyncRunResult> {
    SyncManager::sync_now()
        .await
        .map_err(|error| sync_failure("SYNC_FAILED", &error))
}

#[tauri::command]
#[specta::specta]
pub async fn test_webdav_sync() -> CmdResult<()> {
    SyncManager::test_connection()
        .await
        .map_err(|error| sync_failure("SYNC_WEBDAV_TEST_FAILED", &error))
}

#[tauri::command]
#[specta::specta]
pub async fn list_sync_history() -> CmdResult<Vec<SyncHistoryEntry>> {
    SyncManager::list_history()
        .await
        .map_err(|error| sync_failure("SYNC_HISTORY_FAILED", &error))
}

#[tauri::command]
#[specta::specta]
pub async fn restore_sync_history(filename: std::string::String) -> CmdResult<SyncRunResult> {
    let filename: String = filename.into();
    SyncManager::restore_history(filename.as_str())
        .await
        .map_err(|error| sync_failure("SYNC_RESTORE_FAILED", &error))
}
