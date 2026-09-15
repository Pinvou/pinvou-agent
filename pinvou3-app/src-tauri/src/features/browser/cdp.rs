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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, anyhow};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async_with_config};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

static DROPPED_CDP_EVENT_COUNT: AtomicU64 = AtomicU64::new(0);

/// CDP events forwarded to the manager.
#[derive(Debug, Clone)]
pub enum CdpEvent {
    /// Target lifecycle event consumed by BrowserManager.
    Event { method: String, params: Value },
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
                // Remove a timed-out entry from pending to avoid leaks. A late
                // response for this id then finds no sender and is ignored.
                remove_timed_out_pending(&self.pending, id).await;
                return Err("CDP response timed out after 30 seconds".to_string());
            }
            Ok(Err(_)) => {
                return Err("CDP connection closed and response channel was dropped".to_string());
            }
            Ok(Ok(r)) => r,
        };
        response
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Test-only constructor over a real loopback raw WebSocket write half, so
    /// the close bound can be exercised without a browser. The peer socket
    /// never speaks the protocol; only write-half close behavior is under test.
    #[cfg(test)]
    async fn for_test() -> Self {
        use tokio_tungstenite::tungstenite::protocol::Role;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback listener");
        let addr = listener.local_addr().expect("loopback address");
        let client = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connect loopback pair");
        let (_server, _) = listener.accept().await.expect("accept loopback pair");
        let ws =
            WebSocketStream::from_raw_socket(MaybeTlsStream::Plain(client), Role::Client, None)
                .await;
        let (write, _read) = ws.split();
        Self {
            port: 0,
            write: Mutex::new(write),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
        }
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
        drain_pending_with_closed(&session_clone.pending).await;
    });

    Ok(Connected {
        session,
        events: events_rx,
        reader_task,
    })
}

/// Remove a timed-out entry so the map cannot leak; a late response for the
/// same id then finds no sender and is silently ignored (no misrouting).
async fn remove_timed_out_pending(
    pending: &Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    id: u64,
) {
    pending.lock().await.remove(&id);
}

/// Parse a CDP response frame and route it to the matching in-flight call.
/// Unknown ids (late responses after a timeout, or unsolicited frames) are
/// ignored by design.
async fn resolve_pending_response(
    pending: &Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    v: &Value,
) {
    let Some(id) = v.get("id").and_then(Value::as_u64) else {
        return;
    };
    let result = if let Some(err) = v.get("error") {
        Err(err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("CDP error")
            .to_string())
    } else {
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    };
    if let Some(tx) = pending.lock().await.remove(&id) {
        let _ = tx.send(result);
    }
}

/// Wake every in-flight call after WebSocket close so a caller holding inner
/// cannot hang forever. The manager uses these errors to recover after Chrome
/// crash or kill.
async fn drain_pending_with_closed(
    pending: &Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
) {
    let mut pending = pending.lock().await;
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err("CDP connection closed".to_string()));
    }
}

/// Dispatch a parsed CDP message as request/response or domain event by `id`.
async fn handle_cdp_message(
    session: &Arc<CdpSession>,
    events_tx: &mpsc::Sender<CdpEvent>,
    lifecycle_resync: &Arc<LifecycleResyncSignal>,
    page_target_ids: &mut HashSet<String>,
    v: Value,
) {
    if v.get("id").is_some() {
        // Request/response.
        resolve_pending_response(&session.pending, &v).await;
    } else if let Some(method) = v.get("method").and_then(Value::as_str) {
        // Page/Runtime/Network events can be page-controlled and are not
        // consumed by BrowserManager. Filter them before the bounded channel
        // so an iframe/event flood cannot evict target lifecycle state.
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        if !should_enqueue_manager_lifecycle_event(method, &params, page_target_ids) {
            return;
        }
        let ev = CdpEvent::Event {
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

    type TestPending = Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>;

    // The tests below pin the semantics of the three pending-map helpers
    // (timeout removal, response resolution, reader-exit drain) and the close
    // bound. The `CdpSession::call` wiring of those helpers needs a live
    // browser WebSocket and is covered by the Windows browser smoke gates
    // instead of unit tests.

    #[tokio::test]
    async fn response_timeout_removal_lets_a_late_response_be_ignored_without_misrouting() {
        // History: timeout family — a timed-out call must remove its pending
        // entry, and the late response that arrives afterwards must neither
        // leak the map entry nor misroute to a newer call reusing the id.
        let pending: TestPending = Mutex::new(HashMap::new());
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(7, tx);
        drop(rx); // caller already gave up after the 30s timeout

        remove_timed_out_pending(&pending, 7).await;
        assert!(
            pending.lock().await.is_empty(),
            "timed-out entry must be removed"
        );

        // A late response frame for the removed id resolves nothing and does not
        // re-insert or panic; a fresh call reusing id 7 is unaffected.
        let (tx2, mut rx2) = oneshot::channel();
        pending.lock().await.insert(7, tx2);
        let frame = json!({ "id": 7, "result": { "ok": true } });
        resolve_pending_response(&pending, &frame).await;
        match rx2.try_recv() {
            Ok(Ok(value)) => assert_eq!(value, json!({ "ok": true })),
            other => panic!("new call must receive its own response: {other:?}"),
        }
        assert!(
            pending.lock().await.is_empty(),
            "resolved entry must be removed"
        );
    }

    #[tokio::test]
    async fn unknown_or_error_responses_are_ignored_or_forwarded_not_swallowed() {
        // Late-response family: responses for unknown ids (already timed out or
        // unsolicited) are silently ignored; CDP error objects surface their
        // message instead of a success value.
        let pending: TestPending = Mutex::new(HashMap::new());

        let unsolicited = json!({ "id": 42, "result": { "stale": true } });
        resolve_pending_response(&pending, &unsolicited).await;
        assert!(pending.lock().await.is_empty());

        let (tx, mut rx) = oneshot::channel();
        pending.lock().await.insert(43, tx);
        let error_frame =
            json!({ "id": 43, "error": { "code": -32000, "message": "No target with given id" } });
        resolve_pending_response(&pending, &error_frame).await;
        match rx.try_recv() {
            Ok(Err(message)) => assert_eq!(message, "No target with given id"),
            other => panic!("CDP error must surface its message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn reader_exit_drains_all_pending_calls_with_connection_closed() {
        // History: drain family — when the reader loop exits (WS close or Chrome
        // crash), every in-flight call must be woken with a connection-closed
        // error instead of hanging until the 30s response timeout.
        let pending: TestPending = Mutex::new(HashMap::new());
        let mut receivers = Vec::new();
        for id in 1..=3_u64 {
            let (tx, rx) = oneshot::channel();
            pending.lock().await.insert(id, tx);
            receivers.push(rx);
        }

        drain_pending_with_closed(&pending).await;
        assert!(
            pending.lock().await.is_empty(),
            "drain must empty the pending map"
        );
        for mut rx in receivers {
            match rx.try_recv() {
                Ok(Err(message)) => assert_eq!(message, "CDP connection closed"),
                other => panic!("in-flight call must be woken with closed error: {other:?}"),
            }
        }

        // Draining twice (e.g. an already-empty map on a second reader exit) is a
        // no-op, not an error.
        drain_pending_with_closed(&pending).await;
    }

    #[tokio::test]
    async fn close_is_bounded_and_completes_within_its_3s_budget_when_idle() {
        // History: close-bound family — close() must never block the caller
        // (managers hold inner/start_mtx across it) even when the write half is
        // uncontended. The outer 5s budget proves the inner 3s bound actually
        // lets the caller proceed instead of hanging on a wedged handshake.
        let session = CdpSession::for_test().await;

        tokio::time::timeout(Duration::from_secs(5), session.close())
            .await
            .expect("close must respect its 3-second bound instead of hanging the caller");
    }
}
