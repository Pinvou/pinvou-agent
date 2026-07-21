use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use super::protocol::{envelope, now_ts, RelayEnvelope};

#[derive(Debug)]
pub enum RelayOutbound {
    Envelope(RelayEnvelope),
    Close {
        room_id: String,
        session_id: String,
        reason: String,
    },
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
/// 下载分块走独立有界通道：发送端 `.await` 即背压，避免大文件 Base64 在无界队列里堆积。
pub type DownloadSender = mpsc::Sender<RelayOutbound>;
/// 下载通道容量(块数)。每块 base64 后约 1MiB,4 块峰值约 4MiB。
pub const DOWNLOAD_CHANNEL_CAPACITY: usize = 4;

#[derive(Clone)]
struct RegisterInfo {
    room_id: String,
    session_id: String,
    pairing_token: String,
    desktop_secret: String,
}

pub fn spawn(
    relay_ws_url: String,
    room_id: String,
    session_id: String,
    pairing_token: String,
    desktop_secret: String,
) -> (RelaySender, DownloadSender, RelayReceiver) {
    let (tx_out, mut rx_out) = mpsc::unbounded_channel::<RelayOutbound>();
    let (tx_download, mut rx_download) = mpsc::channel::<RelayOutbound>(DOWNLOAD_CHANNEL_CAPACITY);
    let (tx_in, rx_in) = mpsc::unbounded_channel::<RelayInbound>();
    let register = RegisterInfo {
        room_id,
        session_id,
        pairing_token,
        desktop_secret,
    };
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_loop(
            relay_ws_url,
            tx_in.clone(),
            &mut rx_out,
            &mut rx_download,
            register,
        )
        .await
        {
            let _ = tx_in.send(RelayInbound::Error(e));
        }
    });
    (tx_out, tx_download, rx_in)
}

async fn run_loop(
    relay_ws_url: String,
    tx_in: mpsc::UnboundedSender<RelayInbound>,
    rx_out: &mut mpsc::UnboundedReceiver<RelayOutbound>,
    rx_download: &mut mpsc::Receiver<RelayOutbound>,
    register: RegisterInfo,
) -> Result<(), String> {
    loop {
        let (ws, _) = match connect_async(&relay_ws_url).await {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("[remote-control] connect relay {relay_ws_url} failed: {e}");
                let _ = tx_in.send(RelayInbound::Status {
                    room_id: register.room_id.clone(),
                    status: "connecting_relay".to_string(),
                    message: Some(format!("connect relay failed: {e}")),
                });
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
                            if !send_envelope(&mut write, env).await? {
                                break;
                            }
                        }
                        RelayOutbound::Close { room_id, session_id, reason } => {
                            let value = serde_json::to_value(envelope(&room_id, &session_id, "desktop_disconnect", json!({ "ts": now_ts(), "reason": reason }))).map_err(|e| e.to_string())?;
                            let _ = write.send(Message::Text(value.to_string())).await;
                            return Ok(());
                        }
                    }
                }
                // 下载分块专用有界通道:容量满时发送端挂起,形成背压。
                download = rx_download.recv() => {
                    match download {
                        Some(RelayOutbound::Envelope(env)) => {
                            if !send_envelope(&mut write, env).await? {
                                break;
                            }
                        }
                        // 下载通道只承载 Envelope;Close 永远走无界控制通道。
                        Some(RelayOutbound::Close { .. }) => {}
                        None => return Ok(()),
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
                        "error" => {
                            let message = value
                                .get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("relay error")
                                .to_string();
                            let _ = tx_in.send(RelayInbound::Error(message));
                        }
                        _ => {}
                    }
                }
            }
        }
        let _ = tx_in.send(RelayInbound::Status {
            room_id: register.room_id.clone(),
            status: "connecting_relay".to_string(),
            message: Some("relay connection lost, reconnecting".to_string()),
        });
        tokio::time::sleep(Duration::from_millis(900)).await;
    }
}

/// 序列化并写入一条 envelope;返回 Ok(false) 表示 WS 写失败,调用方应 break 走重连。
async fn send_envelope<S>(write: &mut S, env: super::protocol::RelayEnvelope) -> Result<bool, String>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let value = serde_json::to_value(env).map_err(|e| e.to_string())?;
    if let Err(e) = write.send(Message::Text(value.to_string())).await {
        eprintln!("[remote-control] relay send failed: {e}");
        return Ok(false);
    }
    Ok(true)
}
