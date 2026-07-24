use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u8 = 2;

/// The long-lived desktop endpoint credentials persisted in
/// `~/.pinvou3/web-access.json`.
///
/// `access_token` is safe to put in the browser URL fragment.  The
/// `desktop_secret` is never returned to the UI and is only used to prove the
/// desktop role to the relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebAccessConfig {
    pub relay_url: String,
    pub endpoint_id: String,
    pub access_token: String,
    pub desktop_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebAccessInfo {
    pub endpoint_id: String,
    pub url: String,
    pub qr_data_url: Option<String>,
    pub status: WebAccessStatusKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebAccessStatus {
    pub active: bool,
    pub endpoint_id: Option<String>,
    pub url: Option<String>,
    pub qr_data_url: Option<String>,
    pub status: WebAccessStatusKind,
    pub relay_url: String,
    pub web_client_connected: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebAccessStatusKind {
    Idle,
    ConnectingRelay,
    WaitingWebClient,
    WebClientConnected,
    WebClientDisconnected,
    Revoked,
    Stopped,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_config_round_trips_without_schema_ambiguity() {
        let config = WebAccessConfig {
            relay_url: "wss://example.test/ws".into(),
            endpoint_id: "ep_123".into(),
            access_token: "browser-token".into(),
            desktop_secret: "desktop-secret".into(),
        };
        let json = serde_json::to_string(&config).expect("serialize config");
        let decoded: WebAccessConfig = serde_json::from_str(&json).expect("parse config");
        assert_eq!(decoded, config);
        assert!(json.contains("endpoint_id"));
    }

    #[test]
    fn protocol_version_is_v2() {
        assert_eq!(PROTOCOL_VERSION, 2);
    }
}
