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
use std::time::Duration;

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
            Ok(Err(_)) => return Err("CDP 连接已关闭（响应通道被丢弃）".to_string()),
            Ok(Ok(r)) => r,
        };
        response
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// 优雅关闭 WebSocket（close 握手后读循环 `read.next()` 返回 None 自行退出并
    /// drain pending）。用于 stop()/崩溃重置的兜底：`Browser.close` 失败（wedged）
    /// 且无子进程句柄可 kill 时，至少截断读循环与帧推流，避免资源永久残留。
    pub async fn close(&self) {
        // 无界 close 握手在 TCP 半开/wedged（写缓冲满）时会永久阻塞；调用方常持
        // inner/start_mtx 锁，必须限时，否则冻结整个 BrowserManager。
        let _ = tokio::time::timeout(Duration::from_secs(3), async {
            let mut write = self.write.lock().await;
            let _ = write.close().await;
        })
        .await;
    }

    /// 尽力发送一帧（读循环专用）：ack 类小帧丢弃即失序，失败打日志便于现场诊断。
    async fn notify(&self, session_id: Option<&str>, method: &str, params: Value) {
        if let Err(e) = self.call(session_id, method, params).await {
            eprintln!("[browser] CDP 通知帧发送失败（{method}）: {e}");
        }
    }
}

/// 建立连接的结果：会话 + 事件接收端 + 读循环任务句柄（stop 时可中止）。
pub struct Connected {
    pub session: Arc<CdpSession>,
    pub events: mpsc::Receiver<CdpEvent>,
    /// WebSocket 读循环任务：WS 关闭/Chrome 崩溃时自行退出；stop() 经 `close()`
    /// 关闭 WS 后 join 或 abort 兜底，避免读循环与帧推流残留。
    pub reader_task: tokio::task::JoinHandle<()>,
}

/// 连接 Chrome 的 browser 级 CDP 端点，返回会话与事件接收端。
pub async fn connect(port: u16) -> anyhow::Result<Connected> {
    let version_url = format!("http://127.0.0.1:{port}/json/version");
    // 全路径带超时：Chrome 可能"接受 TCP 但永不响应"（wedged / SIGSTOP），
    // 若此处无界等待，ensure_started 持 start_mtx 会把整个浏览器生命周期冻结
    // （stop()/watch 自动接入/后续 ensure_started 全部被卡住）。reqwest 默认
    // client 无超时，WS 握手同样无超时，必须显式包 timeout。
    // 回环探测不走系统代理：reqwest 默认 auto_sys_proxy，设了 HTTP_PROXY 且未配
    // NO_PROXY 的用户会把 127.0.0.1 请求发往代理而失败（每次启动仅探测一次，
    // 一次性 client 即可）。
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .context("构建 HTTP client")?;
    let body = tokio::time::timeout(Duration::from_secs(10), async {
        client
            .get(&version_url)
            .send()
            .await
            .context("GET /json/version")?
            .error_for_status()
            .context("CDP 版本端点非 2xx")?
            .text()
            .await
            .context("读取 /json/version")
    })
    .await
    .map_err(|_| anyhow!("CDP 版本端点请求超时（10s）"))??;
    let version: Value = serde_json::from_str(&body).context("解析 /json/version")?;
    let ws_url = version
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("CDP 响应缺少 webSocketDebuggerUrl"))?;
    // 纵深防御：端点响应由本地端口占有者控制（Chrome 崩溃后的短暂窗口内可能是
    // 抢占端口的进程）。CDP 无鉴权，不校验会把命令发往任意地址——pin 到本次
    // 连接的回环端口，拒绝响应中的其他 host。
    let expected = format!("ws://127.0.0.1:{port}/");
    if !ws_url.starts_with(&expected) {
        return Err(anyhow!("webSocketDebuggerUrl 非预期回环地址: {ws_url}"));
    }

    let config = WebSocketConfig::default();
    let (ws, _resp) = tokio::time::timeout(
        Duration::from_secs(10),
        connect_async_with_config(ws_url, Some(config), false),
    )
    .await
    .map_err(|_| anyhow!("CDP WebSocket 连接超时（10s）"))?
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
    let reader_task = tokio::spawn(async move {
        loop {
            match read.next().await {
                Some(Ok(msg)) => {
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
                    handle_cdp_message(&session_clone, &events_tx, v).await;
                }
                // WS 协议错误/TCP reset：记录后退出循环（pending 由退出后的
                // drain 唤醒，调用方感知"连接已关闭"），不得静默吞掉错误。
                Some(Err(e)) => {
                    eprintln!("[browser] CDP 读循环错误退出: {e}");
                    break;
                }
                None => break,
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
        reader_task,
    })
}

/// 处理一条已解析的 CDP 消息：按有无 `id` 分发为请求-响应或域事件。
async fn handle_cdp_message(
    session: &Arc<CdpSession>,
    events_tx: &mpsc::Sender<CdpEvent>,
    v: Value,
) {
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
        if let Some(tx) = session.pending.lock().await.remove(&id) {
            let _ = tx.send(result);
        }
    } else if let Some(method) = v.get("method").and_then(Value::as_str) {
        // screencast 帧**在读循环侧先行 ack**：Chromium 要求每帧 ack 后才继续
        // 推帧（in-flight 上限 2）；有界事件通道满时帧会被丢弃，若 ack 依赖
        // 下游消费（事件循环）触发，被丢弃的帧永不 ack → 3 次后截图流
        // 永久停摆。先 ack 再入队，丢弃只损失画面帧率，握手不断。
        if method == "Page.screencastFrame" {
            let frame_sid = v
                .pointer("/params/sessionId")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            // ack 必须 fire-and-forget：`call` 会等待 pending oneshot，而该
            // 响应只能由本读循环投递——在读循环内 await 它会自锁 30s/帧
            // （帧率停摆 + 所有并发命令排队撞满超时）。ack 帧丢失仅损失
            // 帧率（in-flight 上限 2 的握手会随下一帧重试），不可同步等待。
            let ack_session = Arc::clone(session);
            let ack_sid = v.get("sessionId").and_then(Value::as_str).map(String::from);
            tokio::spawn(async move {
                ack_session
                    .notify(
                        ack_sid.as_deref(),
                        "Page.screencastFrameAck",
                        serde_json::json!({ "sessionId": frame_sid }),
                    )
                    .await;
            });
        }
        // 域事件
        let ev = CdpEvent::Event {
            session_id: v.get("sessionId").and_then(Value::as_str).map(String::from),
            method: method.to_string(),
            params: v.get("params").cloned().unwrap_or(Value::Null),
        };
        // try_send：有界通道满时丢弃新事件而非阻塞读循环（阻塞会连带卡死
        // 命令响应与 screencast 握手）。切勿用 `send(ev)` 无 .await 的写法——
        // 那会直接 drop 未 poll 的 future，导致所有域事件静默丢失。
        //
        // 但通道被截图帧填满时无差别丢弃会吞掉 Target.targetCreated /
        // targetDestroyed 等生命周期事件：激活页销毁后的自愈切换失效、截图流
        // 冻结在最后一帧。帧可丢（只损失帧率），控制事件不可静默丢——通道满时
        // 对非帧事件退回带限时的异步 send（spawn 投递，不阻塞读循环），并打
        // 日志便于现场诊断。
        if let Err(err) = events_tx.try_send(ev) {
            // 帧直接丢：只损失画面帧率，ack 已在读循环侧先行完成，握手不断。
            if method != "Page.screencastFrame" {
                let ev = err.into_inner();
                eprintln!("[browser] 事件通道已满，控制事件 {method} 转入限时异步投递");
                let tx = events_tx.clone();
                tokio::spawn(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(2), tx.send(ev)).await;
                });
            }
        }
    }
}
