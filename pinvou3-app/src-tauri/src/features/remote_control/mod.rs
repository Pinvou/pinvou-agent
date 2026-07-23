mod manager;
mod protocol;
mod relay_client;
mod snapshot;

pub use manager::RemoteControlManager;
pub use protocol::{RemoteControlStatus, RemotePairingInfo};

use rand::distr::Alphanumeric;
use rand::Rng;
use serde_json::Value;
use tauri::State;

use crate::features::sessions::SessionStore;
use crate::features::assistant::engine_pool::EnginePool;

pub(crate) fn short_token(len: usize) -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}
pub fn remote_control_start(
    session_id: Option<String>,
    manager: State<'_, RemoteControlManager>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<RemotePairingInfo, String> {
    let sid = session_id.unwrap_or_default();
    let info = manager.start(sid.clone(), store.inner().clone(), pool.inner().clone())?;
    if sid.is_empty() {
        let _ = manager.send_session_list(&store, "");
    } else {
        let _ = manager.send_snapshot_with_live_request(&store, &sid);
    }
    Ok(info)
}
pub fn remote_control_stop(manager: State<'_, RemoteControlManager>) -> Result<(), String> {
    manager.stop_current();
    Ok(())
}
pub fn remote_control_status(
    manager: State<'_, RemoteControlManager>,
) -> Result<RemoteControlStatus, String> {
    Ok(manager.status())
}
pub fn remote_control_refresh_qr(
    session_id: Option<String>,
    manager: State<'_, RemoteControlManager>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
) -> Result<RemotePairingInfo, String> {
    remote_control_start(session_id, manager, store, pool)
}
pub fn remote_control_publish_user_message(
    session_id: String,
    content: String,
    client_message_id: Option<String>,
    manager: State<'_, RemoteControlManager>,
) -> Result<(), String> {
    manager.publish_user_message(&session_id, content, client_message_id)
}
pub fn remote_control_publish_event(
    session_id: String,
    kind: String,
    payload: Value,
    manager: State<'_, RemoteControlManager>,
) -> Result<(), String> {
    manager.publish_desktop_event(&session_id, &kind, payload)
}
