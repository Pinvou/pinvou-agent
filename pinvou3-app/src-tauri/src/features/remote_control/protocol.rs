use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u8 = 2;

/// The long-lived desktop endpoint credentials persisted in
/// `~/.pinvou3/web-access.json`.
///
/// `access_token` is safe to put in the browser URL fragment.  The
/// `desktop_secret` is never returned to the UI and is only used to prove the
/// desktop role to the relay.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebAccessConfig {
    pub relay_url: String,
    pub endpoint_id: String,
    pub access_token: String,
    pub desktop_secret: String,
    /// Desktop-owned consent for binding arbitrary host directories to Web
    /// code Sessions. Legacy configs fail closed until the desktop user
    /// explicitly enables access again.
    #[serde(default)]
    pub allow_host_workspace: bool,
}

// Debug is implemented by hand so logs and panic messages can never carry the
// plaintext credentials (`{:?}`-derived Debug would print every field).
impl std::fmt::Debug for WebAccessConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebAccessConfig")
            .field("relay_url", &self.relay_url)
            .field("endpoint_id", &self.endpoint_id)
            .field("access_token", &"[redacted]")
            .field("desktop_secret", &"[redacted]")
            .field("allow_host_workspace", &self.allow_host_workspace)
            .finish()
    }
}

/// url 内嵌 `token=` 片段、qr_data_url 是同一配对 URL 的二维码——与
/// [`WebAccessConfig`] 同口径手写 Debug,`{:?}` 日志/panic 不得携带它们。
#[derive(Clone, Serialize, Deserialize)]
pub struct WebAccessInfo {
    pub endpoint_id: String,
    pub url: String,
    pub qr_data_url: Option<String>,
    pub status: WebAccessStatusKind,
}

impl std::fmt::Debug for WebAccessInfo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebAccessInfo")
            .field("endpoint_id", &self.endpoint_id)
            .field("url", &"[redacted]")
            .field("qr_data_url", &"[redacted]")
            .field("status", &self.status)
            .finish()
    }
}

/// 字段口径同 [`WebAccessInfo`]。
#[derive(Clone, Serialize, Deserialize)]
pub struct WebAccessStatus {
    pub active: bool,
    pub endpoint_id: Option<String>,
    pub url: Option<String>,
    pub qr_data_url: Option<String>,
    pub status: WebAccessStatusKind,
    pub relay_url: String,
    pub web_client_connected: bool,
    pub host_workspace_authorized: bool,
    pub last_error: Option<String>,
}

impl std::fmt::Debug for WebAccessStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WebAccessStatus")
            .field("active", &self.active)
            .field("endpoint_id", &self.endpoint_id)
            .field("url", &"[redacted]")
            .field("qr_data_url", &"[redacted]")
            .field("status", &self.status)
            .field("relay_url", &self.relay_url)
            .field("web_client_connected", &self.web_client_connected)
            .field("host_workspace_authorized", &self.host_workspace_authorized)
            .field("last_error", &self.last_error)
            .finish()
    }
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
            allow_host_workspace: true,
        };
        let json = serde_json::to_string(&config).expect("serialize config");
        let decoded: WebAccessConfig = serde_json::from_str(&json).expect("parse config");
        assert_eq!(decoded, config);
        assert!(json.contains("endpoint_id"));
        assert!(decoded.allow_host_workspace);
    }

    #[test]
    fn legacy_config_does_not_implicitly_authorize_host_workspaces() {
        let decoded: WebAccessConfig = serde_json::from_str(
            r#"{
                "relay_url":"wss://example.test/ws",
                "endpoint_id":"ep_123",
                "access_token":"browser-token",
                "desktop_secret":"desktop-secret"
            }"#,
        )
        .expect("parse legacy config");
        assert!(!decoded.allow_host_workspace);
    }

    #[test]
    fn config_debug_output_never_contains_plaintext_credentials() {
        let config = WebAccessConfig {
            relay_url: "wss://example.test/ws".into(),
            endpoint_id: "ep_123".into(),
            access_token: "browser-token-secret-value".into(),
            desktop_secret: "desktop-secret-value".into(),
            allow_host_workspace: true,
        };
        let rendered = format!("{config:?}");
        assert!(
            rendered.contains("ep_123"),
            "non-secret fields stay visible"
        );
        assert!(!rendered.contains("browser-token-secret-value"));
        assert!(!rendered.contains("desktop-secret-value"));
        assert!(rendered.contains("[redacted]"));

        // status/info 的 url 内嵌 token 片段、qr 是同一 URL 的二维码:
        // Debug 同口径不得携带,防止下一个 log::debug!("{status:?}") 泄密。
        let token = "browser-token-secret-value";
        let info = WebAccessInfo {
            endpoint_id: "ep_123".into(),
            url: format!("https://relay.test/pinvou3/remote/#endpoint=ep_123&token={token}"),
            qr_data_url: Some(format!("data:image/png;base64,{token}")),
            status: WebAccessStatusKind::WaitingWebClient,
        };
        let rendered = format!("{info:?}");
        assert!(
            rendered.contains("ep_123"),
            "non-secret fields stay visible"
        );
        assert!(
            !rendered.contains(token),
            "info debug must redact url/qr: {rendered}"
        );
        assert!(rendered.contains("[redacted]"));

        let status = WebAccessStatus {
            active: true,
            endpoint_id: Some("ep_123".into()),
            url: Some(format!("https://relay.test/pinvou3/remote/#token={token}")),
            qr_data_url: Some(format!("data:image/png;base64,{token}")),
            status: WebAccessStatusKind::WaitingWebClient,
            relay_url: "wss://relay.test/ws".into(),
            web_client_connected: false,
            host_workspace_authorized: true,
            last_error: None,
        };
        let rendered = format!("{status:?}");
        assert!(
            rendered.contains("ep_123"),
            "non-secret fields stay visible"
        );
        assert!(
            !rendered.contains(token),
            "status debug must redact url/qr: {rendered}"
        );
        assert!(rendered.contains("[redacted]"));
    }
}
