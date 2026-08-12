//! 轻量 CDP（Chrome DevTools Protocol）WebSocket 客户端。
//!
//! 连接 Chrome 的 **browser 级** WebSocket（`/json/version` 的 `webSocketDebuggerUrl`），
//! 以 flatten 模式 attach 页面 target 后通过 `sessionId` 路由各域命令（Page/Input/Target 等）。
//! 一条连接即可管理多个标签页；事件（含 `Page.screencastFrame` 帧流）经 channel 上抛给
//! BrowserManager。
//!
//! 参考样板：`features/remote_control/relay_client.rs`（tokio-tungstenite 0.30 用法）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{connect_async_with_config, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// 上抛给管理器的 CDP 事件。
#[derive(Debug, Clone)]
pub enum CdpEvent {
    /// 请求-响应匹配结果（由 `call` 内部消费，管理器一般不需要）。
    Response {
        id: u64,
        result: Result<Value, String>,
    },
    /// 域事件（如 `Page.screencastFrame`、`Page.frameNavigated`、`Target.targetCreated`）。
    Event {
        session_id: Option<String>,
        method: String,
        params: Value,
    },
}

/// CDP 会话：单条 browser 级 WebSocket，命令经 id 匹配、事件经 channel 分发。
pub struct CdpSession {
    port: u16,
    write: Mutex<futures_util::stream::SplitSink<Ws, WsMessage>>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
}

impl CdpSession {
    /// 向指定 session（或 browser 级）发送命令并等待响应。
    ///
    /// 带超时兜底：WebSocket 断开后读循环会立即唤醒在途调用，但断连之后新发起的
    /// 调用没有读循环消费响应，必须靠本超时返回错误，避免持有方永久挂起。
    pub async fn call(
        &self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let mut msg = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        if let Some(sid) = session_id {
            msg["sessionId"] = json!(sid);
        }
        let frame = serde_json::to_string(&msg).map_err(|e| e.to_string())?;
        // 发送路径同样带超时：Chrome 进程存活但停止读取（wedged/半开 TCP）时，
        // 写锁 + send 若无限等待会令所有后续 call 永久挂起（响应 30s 超时名存实亡）。
        let sent = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let mut write = self.write.lock().await;
            write.send(WsMessage::Text(frame.into())).await
        })
        .await;
        let send_result = match sent {
            Err(_) => Err("CDP 发送超时（30s）".to_string()),
            Ok(Err(e)) => Err(format!("CDP 发送失败: {e}")),
            Ok(Ok(())) => Ok(()),
        };
        if let Err(e) = send_result {
            // 发送失败：摘除 pending 条目，避免条目泄漏（断连后 read 循环已退出，
            // 不会再清理此后插入的条目）。
            self.pending.lock().await.remove(&id);
            return Err(e);
        }
        let response = match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Err(_) => {
                // 超时：从 pending 摘除，避免条目泄漏。
                self.pending.lock().await.remove(&id);
                return Err("CDP 响应超时（30s）".to_string());
            }
            Ok(Err(_)) => return Err("CDP 响应超时（连接已关闭）".to_string()),
            Ok(Ok(r)) => r,
        };
        response
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

/// 建立连接的结果：会话 + 事件接收端。
pub struct Connected {
    pub session: Arc<CdpSession>,
    pub events: mpsc::Receiver<CdpEvent>,
}

/// 连接 Chrome 的 browser 级 CDP 端点，返回会话与事件接收端。
pub async fn connect(port: u16) -> anyhow::Result<Connected> {
    let version_url = format!("http://127.0.0.1:{port}/json/version");
    let body = reqwest::get(&version_url)
        .await
        .context("GET /json/version")?
        .error_for_status()
        .context("CDP 版本端点非 2xx")?
        .text()
        .await?;
    let version: Value = serde_json::from_str(&body).context("解析 /json/version")?;
    let ws_url = version
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("CDP 响应缺少 webSocketDebuggerUrl"))?;

    let config = WebSocketConfig::default();
    let (ws, _resp) = connect_async_with_config(ws_url, Some(config), false)
        .await
        .context("连接 browser CDP WebSocket")?;
    let (write, mut read) = ws.split();

    // 有界事件通道：screencast 帧密集（数十帧/秒），前端消费慢时丢弃旧帧而不是
    // 无界堆积内存膨胀（前端 rAF 节流本来只取最新帧）。
    let (events_tx, events_rx) = mpsc::channel(128);
    let session = Arc::new(CdpSession {
        port,
        write: Mutex::new(write),
        next_id: AtomicU64::new(1),
        pending: Mutex::new(HashMap::new()),
    });

    let session_clone = Arc::clone(&session);
    tokio::spawn(async move {
        while let Some(Ok(msg)) = read.next().await {
            let text = match msg {
                WsMessage::Text(t) => t.to_string(),
                WsMessage::Binary(b) => match String::from_utf8(b.to_vec()) {
                    Ok(s) => s,
                    Err(_) => continue,
                },
                _ => continue,
            };
            let Ok(v) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if let Some(id) = v.get("id").and_then(Value::as_u64) {
                // 请求-响应
                let result = if let Some(err) = v.get("error") {
                    Err(err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("CDP error")
                        .to_string())
                } else {
                    Ok(v.get("result").cloned().unwrap_or(Value::Null))
                };
                if let Some(tx) = session_clone.pending.lock().await.remove(&id) {
                    let _ = tx.send(result);
                }
            } else if let Some(method) = v.get("method").and_then(Value::as_str) {
                // 域事件
                let ev = CdpEvent::Event {
                    session_id: v.get("sessionId").and_then(Value::as_str).map(String::from),
                    method: method.to_string(),
                    params: v.get("params").cloned().unwrap_or(Value::Null),
                };
                let _ = events_tx.send(ev);
            }
        }
        // WebSocket 已关闭：唤醒所有在途请求，避免持有 inner 锁的调用永久挂起
        // （Chrome 崩溃/被 kill 时 manager 靠这些错误感知并走恢复路径）。
        let mut pending = session_clone.pending.lock().await;
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err("CDP 连接已关闭".to_string()));
        }
    });

    Ok(Connected {
        session,
        events: events_rx,
    })
}
