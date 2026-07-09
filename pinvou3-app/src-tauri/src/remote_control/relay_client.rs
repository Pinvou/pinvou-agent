use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use super::protocol::{envelope, now_ts, RelayEnvelope};

#[derive(Debug)]
pub enum RelayOutbound {
    Envelope(RelayEnvelope),
    Close { room_id: String, session_id: String },
}

#[derive(Debug)]
pub enum RelayInbound {
    MobileAction {
        room_id: String,
        session_id: String,
        payload: Value,
    },
    Status {
        room_id: String,
        status: String,
        message: Option<String>,
    },
    Error(String),
}

pub type RelaySender = mpsc::UnboundedSender<RelayOutbound>;
pub type RelayReceiver = mpsc::UnboundedReceiver<RelayInbound>;

#[derive(Clone)]
struct RegisterInfo {
    room_id: String,
    session_id: String,
    pairing_token: String,
    desktop_secret: String,
    expires_at: String,
}

pub fn spawn(
    relay_ws_url: String,
    room_id: String,
    session_id: String,
    pairing_token: String,
    desktop_secret: String,
    expires_at: String,
) -> (RelaySender, RelayReceiver) {
    let (tx_out, mut rx_out) = mpsc::unbounded_channel::<RelayOutbound>();
    let (tx_in, rx_in) = mpsc::unbounded_channel::<RelayInbound>();
    let register = RegisterInfo {
        room_id,
        session_id,
        pairing_token,
        desktop_secret,
        expires_at,
    };
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_loop(relay_ws_url, tx_in.clone(), &mut rx_out, register).await {
            let _ = tx_in.send(RelayInbound::Error(e));
        }
    });
    (tx_out, rx_in)
}

async fn run_loop(
    relay_ws_url: String,
    tx_in: mpsc::UnboundedSender<RelayInbound>,
    rx_out: &mut mpsc::UnboundedReceiver<RelayOutbound>,
    register: RegisterInfo,
) -> Result<(), String> {
    loop {
        let (ws, _) = match connect_async(&relay_ws_url).await {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("[remote-control] connect relay {relay_ws_url} failed: {e}");
                tokio::time::sleep(Duration::from_millis(900)).await;
                continue;
            }
        };
        let (mut write, mut read) = ws.split();
        let register_value = json!({
            "v": 1,
            "type": "desktop_register",
            "room_id": &register.room_id,
            "session_id": &register.session_id,
            "pairing_token": &register.pairing_token,
            "desktop_secret": &register.desktop_secret,
            "expires_at": &register.expires_at,
        });
        if let Err(e) = write.send(Message::Text(register_value.to_string())).await {
            eprintln!("[remote-control] relay register failed: {e}");
            tokio::time::sleep(Duration::from_millis(900)).await;
            continue;
        }

        loop {
            tokio::select! {
                outbound = rx_out.recv() => {
                    let Some(outbound) = outbound else { return Ok(()); };
                    match outbound {
                        RelayOutbound::Envelope(env) => {
                            let value = serde_json::to_value(env).map_err(|e| e.to_string())?;
                            if let Err(e) = write.send(Message::Text(value.to_string())).await {
                                eprintln!("[remote-control] relay send failed: {e}");
                                break;
                            }
                        }
                        RelayOutbound::Close { room_id, session_id } => {
                            let value = serde_json::to_value(envelope(&room_id, &session_id, "desktop_disconnect", json!({ "ts": now_ts() }))).map_err(|e| e.to_string())?;
                            let _ = write.send(Message::Text(value.to_string())).await;
                            return Ok(());
                        }
                    }
                }
                msg = read.next() => {
                    let Some(msg) = msg else { break; };
                    let msg = match msg {
                        Ok(msg) => msg,
                        Err(e) => {
                            eprintln!("[remote-control] relay read failed: {e}");
                            break;
                        }
                    };
                    if !msg.is_text() {
                        continue;
                    }
                    let value: Value = match serde_json::from_str(msg.to_text().unwrap_or("")) {
                        Ok(value) => value,
                        Err(e) => {
                            eprintln!("[remote-control] relay json failed: {e}");
                            continue;
                        }
                    };
                    match value.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                        "mobile_action" => {
                            let _ = tx_in.send(RelayInbound::MobileAction {
                                room_id: value.get("room_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                session_id: value.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                payload: value.get("payload").cloned().unwrap_or(Value::Null),
                            });
                        }
                        "mobile_connected" | "mobile_disconnected" | "room_registered" => {
                            let _ = tx_in.send(RelayInbound::Status {
                                room_id: value.get("room_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                status: value.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                message: value.get("message").and_then(|v| v.as_str()).map(ToString::to_string),
                            });
                        }
                        _ => {}
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(900)).await;
    }
}
