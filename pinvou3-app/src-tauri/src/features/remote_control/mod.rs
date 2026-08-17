pub(crate) mod file_access;
pub(crate) mod manager;
mod platform;
mod protocol;
mod relay_client;

pub use manager::{RelaySettingsInfo, RemoteControlManager};
pub use protocol::{WebAccessInfo, WebAccessStatus};

pub(crate) const MAX_TRANSFER_CHUNK_BYTES: usize = 256 * 1024;

use rand::distr::Alphanumeric;
use rand::Rng;
use serde_json::Value;
use tauri::{AppHandle, Manager};

pub(crate) fn short_token(len: usize) -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

/// Forward an allowlisted desktop event only while a WebUI client is connected.
pub(crate) fn forward_app_event(app: &AppHandle, event: &str, payload: Value) {
    if let Some(manager) = app.try_state::<RemoteControlManager>() {
        manager.forward_local_event(event, payload);
    }
}
