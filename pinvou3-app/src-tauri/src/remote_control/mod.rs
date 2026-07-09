mod manager;
mod protocol;
mod relay_client;
mod snapshot;

pub use manager::RemoteControlManager;
pub use protocol::{RemoteControlStatus, RemotePairingInfo};

use rand::distr::Alphanumeric;
use rand::Rng;
use serde_json::Value;
use tauri::{AppHandle, Manager, State};

use crate::bridge::sessions::SessionStore;
use crate::engine_pool::EnginePool;

pub(crate) fn short_token(len: usize) -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

pub(crate) fn forward_app_event(app: &AppHandle, event: &str, payload: Value) {
    if let Some(manager) = app.try_state::<RemoteControlManager>() {
        manager.forward_local_event(event, payload);
    }
}

#[tauri::command]
pub fn remote_control_start(
    session_id: Option<String>,
    manager: State<'_, RemoteControlManager>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<RemotePairingInfo, String> {
    let sid = session_id
        .or_else(|| store.active_id())
        .ok_or_else(|| "no active session".to_string())?;
    let info = manager.start(sid.clone(), store.inner().clone(), pool.inner().clone())?;
    let _ = manager.send_snapshot(&store, &sid);
    Ok(info)
}

#[tauri::command]
pub fn remote_control_stop(manager: State<'_, RemoteControlManager>) -> Result<(), String> {
    manager.stop_current();
    Ok(())
}

#[tauri::command]
pub fn remote_control_status(
    manager: State<'_, RemoteControlManager>,
) -> Result<RemoteControlStatus, String> {
    Ok(manager.status())
}

#[tauri::command]
pub fn remote_control_refresh_qr(
    session_id: Option<String>,
    manager: State<'_, RemoteControlManager>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<RemotePairingInfo, String> {
    remote_control_start(session_id, manager, store, pool)
}

#[tauri::command]
pub fn remote_control_publish_user_message(
    session_id: String,
    content: String,
    client_message_id: Option<String>,
    manager: State<'_, RemoteControlManager>,
) -> Result<(), String> {
    manager.publish_user_message(&session_id, content, client_message_id)
}
