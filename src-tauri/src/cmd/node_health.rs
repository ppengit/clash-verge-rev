use super::{CmdResult, StringifyErr as _};
use crate::core::node_health::{HealthScope, NodeHealthManager, NodeHealthSnapshot, NodeVerificationResult};
use clash_verge_logging::{Type, logging};

/// Current health snapshot; the frontend rereads it after `verge://node-health-updated`.
#[tauri::command]
#[specta::specta]
pub async fn get_node_health() -> CmdResult<NodeHealthSnapshot> {
    Ok(NodeHealthManager::global().snapshot().await)
}

/// Trigger a manual cycle without waiting for it to finish.
#[tauri::command]
#[specta::specta]
pub async fn run_node_health_check(scope: Option<std::string::String>) -> CmdResult<NodeHealthSnapshot> {
    let scope = scope.and_then(|raw| {
        let parsed = HealthScope::parse(&raw);
        if parsed.is_none() {
            logging!(
                warn,
                Type::Network,
                "Unknown node health scope {raw:?}; using the configured scope"
            );
        }
        parsed
    });
    NodeHealthManager::global().trigger_manual(scope);
    Ok(NodeHealthManager::global().snapshot().await)
}

/// Run L1 plus a selected-node content probe for one node.
#[tauri::command]
#[specta::specta]
pub async fn verify_node(
    profile_uid: Option<std::string::String>,
    group: std::string::String,
    node: std::string::String,
) -> CmdResult<NodeVerificationResult> {
    NodeHealthManager::global()
        .verify_node(profile_uid.as_deref(), group.as_str(), node.as_str())
        .await
        .stringify_err()
}
