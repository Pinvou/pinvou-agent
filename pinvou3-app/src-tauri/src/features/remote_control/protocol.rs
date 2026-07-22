use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayEnvelope {
    pub v: u8,
    pub id: String,
    pub room_id: String,
    pub session_id: String,
    pub direction: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub ts: String,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemotePairingInfo {
    pub room_id: String,
    pub session_id: String,
    pub url: String,
    pub qr_data_url: Option<String>,
    pub status: RemoteControlStatusKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteControlStatus {
    pub active: bool,
    pub room_id: Option<String>,
    pub session_id: Option<String>,
    pub url: Option<String>,
    pub status: RemoteControlStatusKind,
    pub relay_url: String,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteControlStatusKind {
    Idle,
    ConnectingRelay,
    WaitingMobile,
    MobileConnected,
    MobileDisconnected,
    Expired,
    Stopped,
    Error,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MobileAction {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub client_message_id: Option<String>,
    #[serde(default)]
    pub payload: Value,
}

pub fn now_ts() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn envelope(room_id: &str, session_id: &str, kind: &str, payload: Value) -> RelayEnvelope {
    RelayEnvelope {
        v: PROTOCOL_VERSION,
        id: format!("evt_{}", crate::features::remote_control::short_token(18)),
        room_id: room_id.to_string(),
        session_id: session_id.to_string(),
        direction: "desktop_to_mobile".to_string(),
        kind: kind.to_string(),
        ts: now_ts(),
        payload,
    }
}
