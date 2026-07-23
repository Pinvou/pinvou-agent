use super::prelude::*;
use crate::features::remote_control as remote_domain;
use remote_domain::*;
use serde_json::Value;

sync_command_passthrough!(remote_domain, remote_control_start(session_id: Option<String>, manager: State<'_, RemoteControlManager>, store: State<'_, SessionStore>, pool: State<'_, EnginePool>) -> Result<RemotePairingInfo, String>);
sync_command_passthrough!(remote_domain, remote_control_stop(manager: State<'_, RemoteControlManager>) -> Result<(), String>);
sync_command_passthrough!(remote_domain, remote_control_status(manager: State<'_, RemoteControlManager>) -> Result<RemoteControlStatus, String>);
sync_command_passthrough!(remote_domain, remote_control_refresh_qr(session_id: Option<String>, manager: State<'_, RemoteControlManager>, store: State<'_, SessionStore>, pool: State<'_, EnginePool>) -> Result<RemotePairingInfo, String>);
sync_command_passthrough!(remote_domain, remote_control_publish_user_message(session_id: String, content: String, client_message_id: Option<String>, manager: State<'_, RemoteControlManager>) -> Result<(), String>);
sync_command_passthrough!(remote_domain, remote_control_publish_event(session_id: String, kind: String, payload: Value, manager: State<'_, RemoteControlManager>) -> Result<(), String>);
