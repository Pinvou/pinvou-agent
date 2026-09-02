//! Commands for bounded, privacy-safe frontend diagnostics.

use serde_json::Value;

#[tauri::command]
pub async fn record_authority_sync_diagnostics(entries: Vec<Value>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::features::sessions::diagnostics::record_frontend_batch(entries)
    })
    .await
    .map_err(|error| format!("write authority-sync diagnostics task: {error}"))?
}
