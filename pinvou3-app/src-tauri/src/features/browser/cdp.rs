//! Lightweight Chrome DevTools Protocol (CDP) WebSocket client.
//!
//! Connects to Chrome's browser-level WebSocket from `/json/version`
//! `webSocketDebuggerUrl`, attaches page targets in flattened mode, and routes
//! Page/Input/Target domain commands by sessionId. One connection manages
//! multiple tabs. Only target lifecycle events consumed by BrowserManager cross
//! the channel. The task-owned native host publishes navigation/title changes;
//! CDP does not transport rendered frames.
//!
//! Reference: `features/remote_control/relay_client.rs` for tokio-tungstenite 0.30.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

static DROPPED_CDP_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);

/// CDP events forwarded to the manager.
#[derive(Debug, Clone)]
pub enum CdpEvent {
    /// Target lifecycle event consumed by BrowserManager.
    Event {
        session_id: Option<String>,
        method: String,
        params: Value,
    },
    /// At least one page-target lifecycle event could not enter the bounded
    /// channel. The consumer must rebuild its target/session cache from
    /// `Target.getTargets` instead of trying to replay an incomplete delta
    /// stream.
    LifecycleResync { signal: Arc<LifecycleResyncSignal> },
}

/// One coalesced wake-up per overflow burst. A task may wait for one bounded
/// channel slot, but a target churn flood cannot create one task per dropped
/// event. The consumer rearms the signal before it starts its authoritative
/// snapshot so a later overflow schedules the next reconciliation.
#[derive(Debug, Default)]
pub struct LifecycleResyncSignal {
    pending: AtomicBool,
}

impl LifecycleResyncSignal {
    pub(super) fn begin_consume(&self) {
        self.pending.store(false, Ordering::Release);
    }

    #[cfg(test)]
    fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }
}

/// CDP session over one browser-level WebSocket. Commands match by ID and events
/// dispatch through a channel.
pub struct CdpSession {
    port: u16,
    write: Mutex<futures_util::stream::SplitSink<Ws, WsMessage>>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
}

impl CdpSession {
    /// Send a command to a session or the browser level and await its response.
    ///
    /// Timeout fallback: after WebSocket disconnect the read loop immediately
    /// wakes in-flight calls, but calls started after disconnect have no reader
    /// to consume a response and need this timeout to avoid hanging forever.
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
        // Bound the send path too. If Chrome remains alive but stops reading due
        // to a wedged or half-open TCP connection, an unbounded write lock/send
        // would hang every later call and defeat the 30-second response timeout.
        let sent = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let mut write = self.write.lock().await;
            write.send(WsMessage::Text(frame.into())).await
        })
        .await;
        let send_result = match sent {
            Err(_) => Err("CDP send timed out after 30 seconds".to_string()),
            Ok(Err(e)) => Err(format!("CDP send failed: {e}")),
            Ok(Ok(())) => Ok(()),
        };
        if let Err(e) = send_result {
            // Remove pending after send failure. The read loop has exited after
            // disconnect and cannot clean entries inserted later.
            self.pending.lock().await.remove(&id);
            return Err(e);
        }
        let response = match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Err(_) => {
                // Remove a timed-out entry from pending to avoid leaks.
                self.pending.lock().await.remove(&id);
                return Err("CDP response timed out after 30 seconds".to_string());
            }
            Ok(Err(_)) => {
                return Err("CDP connection closed and response channel was dropped".to_string())
            }
            Ok(Ok(r)) => r,
        };
        response
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Gracefully close the WebSocket. After the close handshake, read.next()
    /// returns None and the reader exits after draining pending. This is the
    /// stop/crash-reset fallback: if Browser.close is wedged and there is no child
    /// process handle to kill, at least terminate the read loop.
    pub async fn close(&self) {
        // An unbounded close handshake can block forever on half-open/wedged TCP
        // with a full write buffer. Callers often hold inner/start_mtx, so bound it
        // to avoid freezing BrowserManager.
        let _ = tokio::time::timeout(Duration::from_secs(3), async {
            let mut write = self.write.lock().await;
            let _ = write.close().await;
        })
        .await;
    }
}

/// Connection result: session, event receiver, and reader task handle abortable by stop.
pub struct Connected {
    pub session: Arc<CdpSession>,
    pub events: mpsc::Receiver<CdpEvent>,
    /// WebSocket reader exits on WS close or Chrome crash. stop() closes the WS
    /// and then joins or aborts as a fallback so no reader remains.
    pub reader_task: tokio::task::JoinHandle<()>,
}

/// Connect to Chrome's browser-level CDP endpoint and return session/events.
pub async fn connect(port: u16) -> anyhow::Result<Connected> {
    let version_url = format!("http://127.0.0.1:{port}/json/version");
    // Bound the entire path. Chrome may accept TCP and never respond when wedged
    // or SIGSTOPed. An unbounded wait while ensure_started holds start_mtx freezes
    // stop, watcher attachment, and later starts. reqwest clients and WebSocket
    // handshakes otherwise have no relevant default timeout. Loopback probes also
    // bypass system proxies: HTTP_PROXY without NO_PROXY would send 127.0.0.1 to
    // the proxy and fail. Startup probes once, so a one-shot client is sufficient.
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .context("build HTTP client")?;
    let body = tokio::time::timeout(Duration::from_secs(10), async {
        client
            .get(&version_url)
            .send()
            .await
            .context("GET /json/version")?
            .error_for_status()
            .context("CDP version endpoint returned non-2xx")?
            .text()
            .await
            .context("read /json/version")
    })
    .await
    .map_err(|_| anyhow!("CDP version endpoint timed out after 10 seconds"))??;
    let version: Value = serde_json::from_str(&body).context("parse /json/version")?;
    let ws_url = version
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("CDP response is missing webSocketDebuggerUrl"))?;
    // Defense in depth: the local port owner controls this response and may be a
    // process that captured the port after Chrome crashed. CDP has no auth, so pin
    // to the current loopback port and reject any other host in the response.
    let expected = format!("ws://127.0.0.1:{port}/");
    if !ws_url.starts_with(&expected) {
        return Err(anyhow!(
            "webSocketDebuggerUrl is not the expected loopback address: {ws_url}"
        ));
    }

    let config = WebSocketConfig::default();
    let (ws, _resp) = tokio::time::timeout(
        Duration::from_secs(10),
        connect_async_with_config(ws_url, Some(config), false),
    )
    .await
    .map_err(|_| anyhow!("CDP WebSocket connection timed out after 10 seconds"))?
    .context("connect browser CDP WebSocket")?;
    let (write, mut read) = ws.split();

    // Bound event backlog so a hostile page cannot cause unbounded memory growth.
    let (events_tx, events_rx) = mpsc::channel(128);
    let lifecycle_resync = Arc::new(LifecycleResyncSignal::default());
    let session = Arc::new(CdpSession {
        port,
        write: Mutex::new(write),
        next_id: AtomicU64::new(1),
        pending: Mutex::new(HashMap::new()),
    });

    let session_clone = Arc::clone(&session);
    let reader_lifecycle_resync = Arc::clone(&lifecycle_resync);
    let reader_task = tokio::spawn(async move {
        // Target.targetDestroyed does not carry targetInfo.type. Track only
        // page targets observed from targetCreated so worker/OOPIF churn never
        // enters the manager's bounded lifecycle channel.
        let mut page_target_ids = HashSet::new();
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
                    handle_cdp_message(
                        &session_clone,
                        &events_tx,
                        &reader_lifecycle_resync,
                        &mut page_target_ids,
                        v,
                    )
                    .await;
                }
                // Log WS protocol errors/TCP reset and exit. Draining pending after
                // exit wakes callers with connection-closed; never swallow this.
                Some(Err(e)) => {
                    eprintln!("[browser] CDP reader exited after error: {e}");
                    break;
                }
                None => break,
            }
        }
        // Wake every in-flight request after WebSocket close so a caller holding
        // inner cannot hang forever. Manager uses these errors to recover after
        // Chrome crash or kill.
        let mut pending = session_clone.pending.lock().await;
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err("CDP connection closed".to_string()));
        }
    });

    Ok(Connected {
        session,
        events: events_rx,
        reader_task,
    })
}

/// Dispatch a parsed CDP message as request/response or domain event by `id`.
async fn handle_cdp_message(
    session: &Arc<CdpSession>,
    events_tx: &mpsc::Sender<CdpEvent>,
    lifecycle_resync: &Arc<LifecycleResyncSignal>,
    page_target_ids: &mut HashSet<String>,
    v: Value,
) {
    if let Some(id) = v.get("id").and_then(Value::as_u64) {
        // Request/response.
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
        // Page/Runtime/Network events can be page-controlled and are not
        // consumed by BrowserManager. Filter them before the bounded channel
        // so an iframe/event flood cannot evict target lifecycle state.
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        if !should_enqueue_manager_lifecycle_event(method, &params, page_target_ids) {
            return;
        }
        let ev = CdpEvent::Event {
            session_id: v.get("sessionId").and_then(Value::as_str).map(String::from),
            method: method.to_string(),
            params,
        };
        // Never spawn one fallback task per Full result: a target churn flood
        // could otherwise turn the nominally bounded channel into unbounded
        // queued futures and reorder later lifecycle events.
        let _ = try_deliver_event(
            events_tx,
            ev,
            method,
            &DROPPED_CDP_EVENT_COUNT,
            lifecycle_resync,
        );
    }
}

fn should_enqueue_manager_lifecycle_event(
    method: &str,
    params: &Value,
    page_target_ids: &mut HashSet<String>,
) -> bool {
    match method {
        "Target.targetCreated" => {
            let Some(info) = params.get("targetInfo") else {
                return false;
            };
            if info.get("type").and_then(Value::as_str) != Some("page") {
                return false;
            }
            let Some(target_id) = info.get("targetId").and_then(Value::as_str) else {
                return false;
            };
            page_target_ids.insert(target_id.to_string())
        }
        "Target.targetDestroyed" => params
            .get("targetId")
            .and_then(Value::as_str)
            .is_some_and(|target_id| page_target_ids.remove(target_id)),
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventDeliveryOutcome {
    Delivered,
    ChannelFull,
    ReceiverClosed,
}

fn record_dropped_event(counter: &AtomicU64, method: &str, reason: &str) {
    let total = counter.fetch_add(1, Ordering::Relaxed) + 1;
    // A hostile page can produce events much faster than the manager drains
    // them. Keep accounting exact but sample diagnostics logarithmically so a
    // bounded channel overload cannot become a synchronous stderr flood.
    if should_log_dropped_event(total) {
        eprintln!(
            "[browser] CDP events dropped: latest_method={method} reason={reason} total_dropped={total}"
        );
    }
}

fn should_log_dropped_event(total: u64) -> bool {
    total.is_power_of_two()
}

fn try_deliver_event(
    tx: &mpsc::Sender<CdpEvent>,
    event: CdpEvent,
    method: &str,
    dropped: &AtomicU64,
    lifecycle_resync: &Arc<LifecycleResyncSignal>,
) -> EventDeliveryOutcome {
    match tx.try_send(event) {
        Ok(()) => EventDeliveryOutcome::Delivered,
        Err(mpsc::error::TrySendError::Full(_)) => {
            record_dropped_event(dropped, method, "channel-full");
            schedule_lifecycle_resync(tx, lifecycle_resync);
            EventDeliveryOutcome::ChannelFull
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            record_dropped_event(dropped, method, "receiver-closed");
            EventDeliveryOutcome::ReceiverClosed
        }
    }
}

fn schedule_lifecycle_resync(tx: &mpsc::Sender<CdpEvent>, signal: &Arc<LifecycleResyncSignal>) {
    if signal
        .pending
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    let tx = tx.clone();
    let signal = Arc::clone(signal);
    tokio::spawn(async move {
        let event = CdpEvent::LifecycleResync {
            signal: Arc::clone(&signal),
        };
        if tx.send(event).await.is_err() {
            signal.pending.store(false, Ordering::Release);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(method: &str) -> CdpEvent {
        CdpEvent::Event {
            session_id: None,
            method: method.to_string(),
            params: Value::Null,
        }
    }

    #[test]
    fn bounded_delivery_records_closed_receiver_without_spawning_work() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let dropped = AtomicU64::new(0);
        let lifecycle_resync = Arc::new(LifecycleResyncSignal::default());

        let outcome = try_deliver_event(
            &tx,
            event("Target.targetDestroyed"),
            "Target.targetDestroyed",
            &dropped,
            &lifecycle_resync,
        );

        assert_eq!(outcome, EventDeliveryOutcome::ReceiverClosed);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert!(!lifecycle_resync.is_pending());
    }

    #[tokio::test]
    async fn bounded_delivery_coalesces_full_channel_into_one_rearmable_resync() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(event("Page.first")).unwrap();
        let dropped = AtomicU64::new(0);
        let lifecycle_resync = Arc::new(LifecycleResyncSignal::default());

        let outcome = try_deliver_event(
            &tx,
            event("Page.frameNavigated"),
            "Page.frameNavigated",
            &dropped,
            &lifecycle_resync,
        );
        let second_outcome = try_deliver_event(
            &tx,
            event("Target.targetDestroyed"),
            "Target.targetDestroyed",
            &dropped,
            &lifecycle_resync,
        );

        assert_eq!(outcome, EventDeliveryOutcome::ChannelFull);
        assert_eq!(second_outcome, EventDeliveryOutcome::ChannelFull);
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
        assert!(lifecycle_resync.is_pending());
        assert!(matches!(
            rx.try_recv(),
            Ok(CdpEvent::Event { method, .. }) if method == "Page.first"
        ));

        let first_signal = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("coalesced resync should acquire the released channel slot")
            .expect("resync channel remains open");
        let CdpEvent::LifecycleResync { signal } = first_signal else {
            panic!("overflow must enqueue a lifecycle resync");
        };
        assert!(Arc::ptr_eq(&signal, &lifecycle_resync));
        assert!(
            rx.try_recv().is_err(),
            "one overflow burst queues one wake-up"
        );

        signal.begin_consume();
        assert!(!lifecycle_resync.is_pending());
        tx.try_send(event("Page.second")).unwrap();
        assert_eq!(
            try_deliver_event(
                &tx,
                event("Target.targetCreated"),
                "Target.targetCreated",
                &dropped,
                &lifecycle_resync,
            ),
            EventDeliveryOutcome::ChannelFull
        );
        assert!(lifecycle_resync.is_pending());
        assert!(matches!(
            rx.try_recv(),
            Ok(CdpEvent::Event { method, .. }) if method == "Page.second"
        ));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .expect("rearmed resync should be delivered"),
            Some(CdpEvent::LifecycleResync { .. })
        ));
    }

    #[test]
    fn dropped_event_diagnostics_are_logarithmically_sampled() {
        for total in [1, 2, 4, 8, 16, 1 << 20] {
            assert!(should_log_dropped_event(total), "total={total}");
        }
        for total in [0, 3, 5, 6, 7, 9, 15, 17, (1 << 20) - 1] {
            assert!(!should_log_dropped_event(total), "total={total}");
        }
    }

    #[test]
    fn only_known_page_target_lifecycle_events_enter_the_bounded_channel() {
        let mut pages = HashSet::new();
        assert!(should_enqueue_manager_lifecycle_event(
            "Target.targetCreated",
            &json!({ "targetInfo": { "targetId": "page-1", "type": "page" } }),
            &mut pages,
        ));
        assert!(should_enqueue_manager_lifecycle_event(
            "Target.targetDestroyed",
            &json!({ "targetId": "page-1" }),
            &mut pages,
        ));
        assert!(!should_enqueue_manager_lifecycle_event(
            "Target.targetDestroyed",
            &json!({ "targetId": "unknown" }),
            &mut pages,
        ));
        for ignored in [
            "Page.frameNavigated",
            "Page.loadEventFired",
            "Runtime.consoleAPICalled",
            "Network.requestWillBeSent",
            "Target.targetInfoChanged",
        ] {
            assert!(
                !should_enqueue_manager_lifecycle_event(ignored, &Value::Null, &mut pages,),
                "method={ignored}"
            );
        }
        for target_type in ["worker", "service_worker", "iframe", "other"] {
            let target_id = format!("{target_type}-1");
            assert!(!should_enqueue_manager_lifecycle_event(
                "Target.targetCreated",
                &json!({ "targetInfo": { "targetId": target_id, "type": target_type } }),
                &mut pages,
            ));
            assert!(!should_enqueue_manager_lifecycle_event(
                "Target.targetDestroyed",
                &json!({ "targetId": target_id }),
                &mut pages,
            ));
        }
    }
}
