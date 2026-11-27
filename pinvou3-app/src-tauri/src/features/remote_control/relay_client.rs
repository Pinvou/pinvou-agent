use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use rand::RngExt;
use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use tokio_util::sync::CancellationToken;

use super::protocol::{PROTOCOL_VERSION, WebAccessConfig};

const RECONNECT_BASE_DELAY_MS: u64 = 500;
const RECONNECT_MAX_DELAY_MS: u64 = 10_000;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);
const REVOKE_ACK_TIMEOUT: Duration = Duration::from_secs(12);

/// True when nothing inbound has arrived for two full heartbeat windows: a
/// half-open socket (sleep/resume, NAT rebind) keeps absorbing outbound
/// pings into kernel buffers, so only inbound activity is proof of life.
/// Pure function so the boundary (exactly two windows must reconnect, one
/// window must not) stays pinned by a test.
///
/// The 50s window assumes the relay pings at least every ~25s (the bundled
/// relay defaults to 15s, floor 5s). A deployment raising the relay's ping
/// interval above this window makes every idle connection cycle
/// connect→register→force-reconnect forever — raise `HEARTBEAT_INTERVAL`
/// together with the relay side, don't just retune the server.
fn inbound_silence_exceeded(silence: Duration) -> bool {
    silence >= HEARTBEAT_INTERVAL * 2
}

const MAX_PENDING_MESSAGES: usize = 2_048;
const OUTBOUND_CHANNEL_CAPACITY: usize = 2_048;
// RelayInbound queues raw JSON text, so 32 slots at the 4 MiB frame ceiling
// bound retained wire payload to roughly 128 MiB. Only the current frame is
// parsed at either side of the queue.
const INBOUND_CHANNEL_CAPACITY: usize = 32;
/// Covers the relay's DEFAULT deployment: PROTOCOL.md pins MAX_PAYLOAD_BYTES
/// at >= 4 MiB (the relay clamps the deployment knob to at most 16 MiB, the
/// same bound as its replay journal) with desktop
/// frames at or below 4 MiB - 64 KiB (the same bound as
/// `MAX_RELAY_FRAME_BYTES`). A lower ceiling would make the Relay's own large
/// frames disconnect us; a self-hosted relay raised ABOVE this bound can
/// still kill the socket with a legal frame — raising the desktop ceiling
/// must ride along with such a deployment (round-5 review: the old comment
/// justified the bound by the wrong direction; round-6 review: the 16 MiB
/// figure is the relay's server-side clamp, not a PROTOCOL.md number;
/// round-21 review: PROTOCOL.md now documents that clamp and the
/// 4 MiB − 64 KiB bound (server.js names its copy DESKTOP_MAX_FRAME_BYTES;
/// the byte equality is pinned in both directions as of round-26 — the
/// relay gate tests exercise the server-side gate against this bound and
/// the relay suite asserts this constant's exact spelling) — the operative
/// contract is the equality, not the proxy-variance wording).
const MAX_INBOUND_TEXT_BYTES: usize = (4 * 1024 * 1024) - (64 * 1024);
const OUTBOUND_BYTE_BUDGET: usize = 128 * 1024 * 1024;
// Stream resets are tiny control messages. Keeping at most one in-flight and
// one latest replacement outside the saturated data budget adds a hard 128 KiB
// ceiling while ensuring the recovery barrier itself cannot be starved.
pub(crate) const MAX_CONTROL_FRAME_BYTES: usize = 64 * 1024;
/// Keep complete serialized desktop -> Relay frames below the Relay's default
/// 4 MiB WebSocket payload ceiling, with room for deployment/proxy variance.
pub(crate) const MAX_RELAY_FRAME_BYTES: usize = (4 * 1024 * 1024) - (64 * 1024);

#[derive(Debug, Default)]
struct ReconnectBackoff {
    attempt: u32,
}

impl ReconnectBackoff {
    fn next_delay(&mut self) -> Duration {
        let jitter_per_mille = rand::rng().random_range(800_u64..=1_200);
        self.next_delay_with_jitter(jitter_per_mille)
    }

    fn next_delay_with_jitter(&mut self, jitter_per_mille: u64) -> Duration {
        let multiplier = 1_u64 << self.attempt.min(5);
        let base_ms = RECONNECT_BASE_DELAY_MS
            .saturating_mul(multiplier)
            .min(RECONNECT_MAX_DELAY_MS);
        self.attempt = self.attempt.saturating_add(1);
        Duration::from_millis(base_ms.saturating_mul(jitter_per_mille) / 1_000)
    }

    fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[derive(Debug)]
pub enum RelayOutbound {
    Message(Value),
    Shutdown,
}

#[derive(Debug)]
pub enum RelayInbound {
    /// Raw, size-checked JSON text. Keeping parsing out of the bounded queue
    /// makes its memory ceiling proportional to wire bytes rather than the
    /// potentially much larger `serde_json::Value` representation.
    Message(String),
    Connection {
        endpoint_id: String,
        connected: bool,
        error: Option<String>,
    },
}

pub(super) struct OutboundFrame {
    text: String,
    // The permit follows the serialized frame from the channel into the
    // reconnect queue and is released only after send or intentional discard.
    _byte_permit: OwnedSemaphorePermit,
}

struct SerializedFrame {
    text: String,
    permits: u32,
}

#[derive(Default)]
struct CoalescedControlState {
    worker_running: bool,
    latest: Option<SerializedFrame>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayTrySendError {
    Serialization(String),
    FrameTooLarge { bytes: usize, limit: usize },
    ByteBudgetExhausted,
    ChannelFull,
    ChannelClosed,
}

impl fmt::Display for RelayTrySendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialization(error) => write!(formatter, "serialize relay frame: {error}"),
            Self::FrameTooLarge { bytes, limit } => {
                write!(formatter, "relay frame is {bytes} bytes; limit is {limit}")
            }
            Self::ByteBudgetExhausted => {
                formatter.write_str("relay outbound byte budget exhausted")
            }
            Self::ChannelFull => formatter.write_str("relay outbound channel is full"),
            Self::ChannelClosed => formatter.write_str("relay outbound channel is closed"),
        }
    }
}

impl std::error::Error for RelayTrySendError {}

#[derive(Clone)]
pub struct RelaySender {
    tx: mpsc::Sender<OutboundFrame>,
    byte_budget: Arc<Semaphore>,
    max_frame_bytes: usize,
    shutdown: CancellationToken,
    coalesced_control: Arc<Mutex<CoalescedControlState>>,
}

impl RelaySender {
    fn serialize_message(&self, value: Value) -> Result<SerializedFrame, RelayTrySendError> {
        let text = serde_json::to_string(&value)
            .map_err(|error| RelayTrySendError::Serialization(error.to_string()))?;
        let bytes = text.len();
        if bytes > self.max_frame_bytes {
            return Err(RelayTrySendError::FrameTooLarge {
                bytes,
                limit: self.max_frame_bytes,
            });
        }
        let permits = u32::try_from(bytes).map_err(|_| RelayTrySendError::FrameTooLarge {
            bytes,
            limit: self.max_frame_bytes,
        })?;
        Ok(SerializedFrame { text, permits })
    }

    fn cancel(&self) {
        self.shutdown.cancel();
        self.coalesced_control.lock().latest = None;
    }

    fn try_send_serialized(&self, frame: SerializedFrame) -> Result<(), RelayTrySendError> {
        let byte_permit = self
            .byte_budget
            .clone()
            .try_acquire_many_owned(frame.permits)
            .map_err(|_| RelayTrySendError::ByteBudgetExhausted)?;
        let queued = OutboundFrame {
            text: frame.text,
            _byte_permit: byte_permit,
        };
        self.tx.try_send(queued).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => RelayTrySendError::ChannelFull,
            mpsc::error::TrySendError::Closed(_) => RelayTrySendError::ChannelClosed,
        })
    }

    async fn send_serialized(&self, frame: SerializedFrame) -> Result<(), RelayTrySendError> {
        let byte_permit = tokio::select! {
            biased;
            _ = self.shutdown.cancelled() => return Err(RelayTrySendError::ChannelClosed),
            result = self.byte_budget.clone().acquire_many_owned(frame.permits) => {
                result.map_err(|_| RelayTrySendError::ByteBudgetExhausted)?
            }
        };
        let queued = OutboundFrame {
            text: frame.text,
            _byte_permit: byte_permit,
        };
        tokio::select! {
            biased;
            _ = self.shutdown.cancelled() => Err(RelayTrySendError::ChannelClosed),
            result = self.tx.send(queued) => {
                result.map_err(|_| RelayTrySendError::ChannelClosed)
            }
        }
    }

    /// Serialize exactly once before enqueueing, then account the retained
    /// bytes across both the mpsc channel and the reconnect FIFO.
    pub fn try_send(&self, outbound: RelayOutbound) -> Result<(), RelayTrySendError> {
        match outbound {
            RelayOutbound::Shutdown => {
                // Control-plane shutdown must remain deliverable even when
                // every data slot and byte permit is occupied.
                self.cancel();
                Ok(())
            }
            RelayOutbound::Message(value) => {
                self.try_send_serialized(self.serialize_message(value)?)
            }
        }
    }

    /// Queue a stream-reset barrier without spawning one waiter per failure.
    /// While a single worker is blocked on the data channel, repeated resets
    /// coalesce to the latest lease/epoch and are sent immediately afterwards.
    pub fn enqueue_stream_reset(&self, message: Value) -> Result<(), RelayTrySendError> {
        if self.shutdown.is_cancelled() {
            return Err(RelayTrySendError::ChannelClosed);
        }
        let frame = self.serialize_message(message)?;
        if frame.text.len() > MAX_CONTROL_FRAME_BYTES {
            return Err(RelayTrySendError::FrameTooLarge {
                bytes: frame.text.len(),
                limit: MAX_CONTROL_FRAME_BYTES,
            });
        }

        let start_worker = {
            let mut state = self.coalesced_control.lock();
            if self.shutdown.is_cancelled() {
                return Err(RelayTrySendError::ChannelClosed);
            }
            state.latest = Some(frame);
            if state.worker_running {
                false
            } else {
                state.worker_running = true;
                true
            }
        };
        if start_worker {
            let sender = self.clone();
            tauri::async_runtime::spawn(async move {
                sender.run_coalesced_control_worker().await;
            });
        }
        Ok(())
    }

    async fn run_coalesced_control_worker(self) {
        loop {
            let next = {
                let mut state = self.coalesced_control.lock();
                let next = state.latest.take();
                if next.is_none() {
                    state.worker_running = false;
                }
                next
            };
            let Some(frame) = next else {
                return;
            };
            if let Err(error) = self.send_serialized(frame).await {
                let mut state = self.coalesced_control.lock();
                state.latest = None;
                state.worker_running = false;
                if !self.shutdown.is_cancelled() {
                    eprintln!("[web-access] coalesced stream reset enqueue failed: {error}");
                }
                return;
            }
        }
    }
}

pub type RelayReceiver = mpsc::Receiver<RelayInbound>;

/// Visible to the manager tests so they can mint a `RelaySender` for
/// `ActiveEndpoint` fixtures without spawning a relay task.
pub(super) fn outbound_channel(
    channel_capacity: usize,
    byte_budget: usize,
    max_frame_bytes: usize,
) -> (RelaySender, mpsc::Receiver<OutboundFrame>) {
    let (tx, rx) = mpsc::channel(channel_capacity);
    (
        RelaySender {
            tx,
            byte_budget: Arc::new(Semaphore::new(byte_budget)),
            max_frame_bytes,
            shutdown: CancellationToken::new(),
            coalesced_control: Arc::new(Mutex::new(CoalescedControlState::default())),
        },
        rx,
    )
}

pub fn spawn(config: WebAccessConfig) -> (RelaySender, RelayReceiver) {
    let (tx_out, rx_out) = outbound_channel(
        OUTBOUND_CHANNEL_CAPACITY,
        OUTBOUND_BYTE_BUDGET,
        MAX_RELAY_FRAME_BYTES,
    );
    let shutdown = tx_out.shutdown.clone();
    let (tx_in, rx_in) = mpsc::channel::<RelayInbound>(INBOUND_CHANNEL_CAPACITY);
    tauri::async_runtime::spawn(async move {
        run_loop(config, tx_in, rx_out, shutdown).await;
    });
    (tx_out, rx_in)
}

/// Retry a revocation independently from the active desktop endpoint.
///
/// The Relay accepts this message before authentication when the desktop
/// secret matches, so a durable revocation does not need to retain or expose
/// the browser access token. Dropping the returned receiver stops the worker;
/// otherwise it reconnects until the Relay gives a terminal acknowledgement.
pub fn spawn_revocation(
    relay_url: String,
    endpoint_id: String,
    desktop_secret: String,
) -> RelayReceiver {
    let (tx_in, rx_in) = mpsc::channel::<RelayInbound>(INBOUND_CHANNEL_CAPACITY);
    tauri::async_runtime::spawn(async move {
        run_revocation_loop(relay_url, endpoint_id, desktop_secret, tx_in).await;
    });
    rx_in
}

fn relay_websocket_config() -> WebSocketConfig {
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_INBOUND_TEXT_BYTES);
    config.max_frame_size = Some(MAX_INBOUND_TEXT_BYTES);
    config
}

async fn run_revocation_loop(
    relay_url: String,
    endpoint_id: String,
    desktop_secret: String,
    tx_in: mpsc::Sender<RelayInbound>,
) {
    let mut reconnect_backoff = ReconnectBackoff::default();
    loop {
        let connection =
            connect_async_with_config(&relay_url, Some(relay_websocket_config()), false);
        tokio::pin!(connection);
        let connected = tokio::select! {
            result = &mut connection => result,
            _ = tx_in.closed() => return,
        };
        let (ws, _) = match connected {
            Ok(pair) => pair,
            Err(error) => {
                // The revocation channel's Connection events are consumed
                // only by this loop's channel-closure detection (the manager
                // side matches only Message), so log the failure reason here,
                // aligned with the main loop's connect-failure log.
                eprintln!(
                    "[web-access] connect relay for revocation failed: {error} ({relay_url})"
                );
                if tx_in
                    .send(RelayInbound::Connection {
                        endpoint_id: endpoint_id.clone(),
                        connected: false,
                        error: Some(format!("connect relay for revocation failed: {error}")),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
                if !wait_revocation_reconnect_delay(&tx_in, &mut reconnect_backoff).await {
                    return;
                }
                continue;
            }
        };

        let (mut write, mut read) = ws.split();
        let revoke = revoke_message_parts(&endpoint_id, &desktop_secret);
        if let Err(error) = write.send(Message::Text(revoke.to_string().into())).await {
            eprintln!("[web-access] revoke endpoint failed: {error}");
            if tx_in
                .send(RelayInbound::Connection {
                    endpoint_id: endpoint_id.clone(),
                    connected: false,
                    error: Some(format!("revoke endpoint failed: {error}")),
                })
                .await
                .is_err()
            {
                return;
            }
            if !wait_revocation_reconnect_delay(&tx_in, &mut reconnect_backoff).await {
                return;
            }
            continue;
        }

        let deadline = tokio::time::Instant::now() + REVOKE_ACK_TIMEOUT;
        loop {
            tokio::select! {
                _ = tx_in.closed() => return,
                _ = tokio::time::sleep_until(deadline) => {
                    let _ = write.send(Message::Close(None)).await;
                    break;
                }
                message = read.next() => {
                    let Some(message) = message else { break; };
                    match message {
                        Ok(Message::Text(text)) => {
                            if inbound_text_too_large(text.len()) {
                                eprintln!(
                                    "[web-access] oversized relay revocation frame rejected: {} bytes",
                                    text.len()
                                );
                                let _ = write.send(Message::Close(None)).await;
                                break;
                            }
                            match serde_json::from_str::<Value>(&text) {
                                Ok(value) => {
                                    let terminal = terminal_message(&value, &endpoint_id);
                                    // Round-12 review P3: the worker's own
                                    // terminal set and the consumer's ack set
                                    // are two functions, previously coupled
                                    // only by the behavior of the single
                                    // existing call site. endpoint_not_found
                                    // folds to None as "recoverable" in
                                    // terminal_message (the register loop
                                    // self-heals via reconnect), but for the
                                    // revocation worker the endpoint no longer
                                    // exists, so the revocation is naturally
                                    // complete — a future spawn_revocation
                                    // call site that did not consume Message
                                    // frames would turn one extra retry into
                                    // an infinite 12s-timeout
                                    // connect→revoke loop. The worker judges
                                    // that code terminal here itself; frames
                                    // for foreign endpoints are still ignored
                                    // as None (same scope convention as the
                                    // folding predicate).
                                    let scoped_to_self = value
                                        .get("endpoint_id")
                                        .and_then(Value::as_str)
                                        .is_none_or(|scope| scope.is_empty() || scope == endpoint_id);
                                    let worker_terminal = terminal.is_some()
                                        || (scoped_to_self
                                            && value.get("code").and_then(Value::as_str)
                                                == Some("endpoint_not_found"));
                                    drop(value);
                                    if tx_in.send(RelayInbound::Message(text.to_string())).await.is_err() {
                                        return;
                                    }
                                    if worker_terminal {
                                        let _ = write.send(Message::Close(None)).await;
                                        return;
                                    }
                                }
                                Err(error) => {
                                    eprintln!("[web-access] relay revocation JSON ignored: {error}")
                                }
                            }
                        }
                        Ok(Message::Ping(bytes)) => {
                            if write.send(Message::Pong(bytes)).await.is_err() {
                                break;
                            }
                        }
                        Ok(Message::Close(_)) => break,
                        Ok(_) => {}
                        Err(error) => {
                            eprintln!("[web-access] relay revocation read failed: {error}");
                            break;
                        }
                    }
                }
            }
        }

        // Reached on both failure paths (read error) and the clean paths
        // (server close / local ACK-timeout close), so the wording must not
        // claim a lost connection.
        eprintln!("[web-access] relay revocation socket closed; reconnecting");
        if tx_in
            .send(RelayInbound::Connection {
                endpoint_id: endpoint_id.clone(),
                connected: false,
                error: Some("relay revocation socket closed; reconnecting".to_string()),
            })
            .await
            .is_err()
        {
            return;
        }
        if !wait_revocation_reconnect_delay(&tx_in, &mut reconnect_backoff).await {
            return;
        }
    }
}

async fn wait_revocation_reconnect_delay(
    tx_in: &mpsc::Sender<RelayInbound>,
    reconnect_backoff: &mut ReconnectBackoff,
) -> bool {
    let delay = reconnect_backoff.next_delay();
    tokio::select! {
        _ = tokio::time::sleep(delay) => true,
        _ = tx_in.closed() => false,
    }
}

async fn send_inbound_until_shutdown(
    tx_in: &mpsc::Sender<RelayInbound>,
    inbound: RelayInbound,
    shutdown: &CancellationToken,
) -> bool {
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => false,
        result = tx_in.send(inbound) => result.is_ok(),
    }
}

async fn wait_reconnect_delay(
    shutdown: &CancellationToken,
    reconnect_backoff: &mut ReconnectBackoff,
) -> bool {
    let delay = reconnect_backoff.next_delay();
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => false,
        _ = tokio::time::sleep(delay) => true,
    }
}

async fn run_loop(
    config: WebAccessConfig,
    tx_in: mpsc::Sender<RelayInbound>,
    mut rx_out: mpsc::Receiver<OutboundFrame>,
    shutdown: CancellationToken,
) {
    let mut pending = VecDeque::<OutboundFrame>::new();
    let mut reconnect_backoff = ReconnectBackoff::default();

    'reconnect: loop {
        let connection =
            connect_async_with_config(&config.relay_url, Some(relay_websocket_config()), false);
        tokio::pin!(connection);

        // Remain responsive to disable/rotate while the network connection is
        // being established. Ordinary messages are retained for the next
        // successful connection.
        let connected = loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => return,
                result = &mut connection => break result,
                outbound = rx_out.recv(), if pending_has_capacity(&pending) => {
                    match outbound {
                        Some(frame) => {
                            push_pending(&mut pending, frame);
                        }
                        None => return,
                    }
                }
            }
        };

        let (ws, _) = match connected {
            Ok(pair) => pair,
            Err(error) => {
                let message = format!("connect relay failed: {error}");
                eprintln!("[web-access] {} ({})", message, config.relay_url);
                if !send_inbound_until_shutdown(
                    &tx_in,
                    RelayInbound::Connection {
                        endpoint_id: config.endpoint_id.clone(),
                        connected: false,
                        error: Some(message),
                    },
                    &shutdown,
                )
                .await
                {
                    return;
                }
                if !wait_reconnect_delay(&shutdown, &mut reconnect_backoff).await {
                    return;
                }
                continue;
            }
        };

        let (mut write, mut read) = ws.split();
        let register = register_message(&config);
        let register_result = tokio::select! {
            biased;
            _ = shutdown.cancelled() => return,
            result = write.send(Message::Text(register.to_string().into())) => result,
        };
        if let Err(error) = register_result {
            if !send_inbound_until_shutdown(
                &tx_in,
                RelayInbound::Connection {
                    endpoint_id: config.endpoint_id.clone(),
                    connected: false,
                    error: Some(format!("register endpoint failed: {error}")),
                },
                &shutdown,
            )
            .await
            {
                return;
            }
            if !wait_reconnect_delay(&shutdown, &mut reconnect_backoff).await {
                return;
            }
            continue;
        }
        if !send_inbound_until_shutdown(
            &tx_in,
            RelayInbound::Connection {
                endpoint_id: config.endpoint_id.clone(),
                connected: true,
                error: None,
            },
            &shutdown,
        )
        .await
        {
            return;
        }

        while let Some(frame) = pending.pop_front() {
            let send_result = tokio::select! {
                biased;
                _ = shutdown.cancelled() => return,
                result = write.send(Message::Text(frame.text.clone().into())) => result,
            };
            if let Err(error) = send_result {
                push_pending_front(&mut pending, frame);
                if !send_inbound_until_shutdown(
                    &tx_in,
                    RelayInbound::Connection {
                        endpoint_id: config.endpoint_id.clone(),
                        connected: false,
                        error: Some(format!("relay send failed: {error}")),
                    },
                    &shutdown,
                )
                .await
                {
                    return;
                }
                if !wait_reconnect_delay(&shutdown, &mut reconnect_backoff).await {
                    return;
                }
                continue 'reconnect;
            }
        }

        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Consume interval's immediate first tick; the registration message is
        // already proof of life.
        heartbeat.tick().await;
        // Inbound-activity tracking: sending Pings into a half-open socket
        // keeps succeeding (kernel buffers absorb them), so without this the
        // client would report "connected" for tens of minutes on a dead path
        // (sleep/resume, NAT rebind) while every phone request queues.
        // Any inbound frame counts as proof of life; 2 missed heartbeat
        // windows with nothing inbound forces the reconnect path.
        //
        // Scope notes: (1) the check rides the heartbeat tick, so it is only
        // evaluated while no write/read is parked — a send blocked on a full
        // kernel buffer (active session when the path died) is still bounded
        // only by TCP retransmission timeouts, not by this check. (2)
        // `Instant` excludes suspend time, so after system resume detection
        // lands within one or two awake windows, not instantly. A healthy
        // relay answers every ping within one window (RFC 6455 auto-pong),
        // which is what keeps steady state from tripping this path.
        let mut last_inbound = std::time::Instant::now();
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    return;
                }
                outbound = rx_out.recv() => {
                    match outbound {
                        Some(frame) => {
                            let send_result = tokio::select! {
                                biased;
                                _ = shutdown.cancelled() => return,
                                result = write.send(Message::Text(frame.text.clone().into())) => result,
                            };
                            if let Err(error) = send_result {
                                push_pending_front(&mut pending, frame);
                                eprintln!("[web-access] relay send failed: {error}");
                                break;
                            }
                        }
                        None => {
                            let _ = write.send(Message::Close(None)).await;
                            return;
                        }
                    }
                }
                message = read.next() => {
                    let Some(message) = message else { break; };
                    // Any inbound frame (text, ping, pong) is proof of life.
                    last_inbound = std::time::Instant::now();
                    match message {
                        Ok(Message::Text(text)) => {
                            if inbound_text_too_large(text.len()) {
                                eprintln!(
                                    "[web-access] oversized relay frame rejected: {} bytes",
                                    text.len()
                                );
                                let _ = write.send(Message::Close(None)).await;
                                break;
                            }
                            match serde_json::from_str::<Value>(&text) {
                                Ok(mut value) => {
                                    let terminal = terminal_message(&value, &config.endpoint_id);
                                    if registration_acknowledges_endpoint(&value, &config.endpoint_id) {
                                        reconnect_backoff.reset();
                                    }
                                    // Relay's registration-class reject frames
                                    // carry no (or an empty) endpoint_id:
                                    // re-stamp this socket's in-flight
                                    // endpoint so the manager-side
                                    // explicit-match gate takes effect —
                                    // otherwise a late terminal frame during a
                                    // rotation race cannot be told apart from
                                    // frame ownership and may take down the
                                    // endpoint that just replaced it.
                                    let forwarded =
                                        if stamp_unscoped_registration_error(
                                            &mut value,
                                            &config.endpoint_id,
                                        ) {
                                            value.to_string()
                                        } else {
                                            text.to_string()
                                        };
                                    if !send_inbound_until_shutdown(
                                        &tx_in,
                                        RelayInbound::Message(forwarded),
                                        &shutdown,
                                    ).await {
                                        return;
                                    }
                                    if terminal.is_some() {
                                        let _ = write.send(Message::Close(None)).await;
                                        return;
                                    }
                                }
                                Err(error) => eprintln!("[web-access] relay JSON ignored: {error}"),
                            }
                        }
                        Ok(Message::Ping(bytes)) => {
                            let pong_result = tokio::select! {
                                biased;
                                _ = shutdown.cancelled() => return,
                                result = write.send(Message::Pong(bytes)) => result,
                            };
                            if pong_result.is_err() { break; }
                        }
                        Ok(Message::Close(_)) => break,
                        Ok(_) => {}
                        Err(error) => {
                            eprintln!("[web-access] relay read failed: {error}");
                            break;
                        }
                    }
                }
                _ = heartbeat.tick() => {
                    if inbound_silence_exceeded(last_inbound.elapsed()) {
                        // Nothing inbound for two heartbeat windows: treat
                        // the connection as dead and take the normal
                        // reconnect path (pending frames are preserved).
                        eprintln!("[web-access] relay connection silent, forcing reconnect");
                        break;
                    }
                    let ping_result = tokio::select! {
                        biased;
                        _ = shutdown.cancelled() => return,
                        result = write.send(Message::Ping(Vec::<u8>::new().into())) => result,
                    };
                    if ping_result.is_err() { break; }
                }
            }
        }

        if !send_inbound_until_shutdown(
            &tx_in,
            RelayInbound::Connection {
                endpoint_id: config.endpoint_id.clone(),
                connected: false,
                error: Some("relay connection lost; reconnecting".to_string()),
            },
            &shutdown,
        )
        .await
        {
            return;
        }
        if !wait_reconnect_delay(&shutdown, &mut reconnect_backoff).await {
            return;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelayTerminal {
    Revoked,
    Replaced,
    /// The relay deterministically rejected the registration itself
    /// (`endpoint_revoked` tombstone, mismatched credentials, malformed
    /// register frame): retrying with the same persisted config can never
    /// succeed, so the run loop stops instead of flapping
    /// Error↔Connecting every backoff window.
    RegisterRejected,
}

/// Deterministic registration-rejection codes: retrying with the same
/// persisted config can never succeed against them, so run_loop must stop
/// with a reason. Alignment between the manager-side
/// `TERMINAL_REGISTRATION_ERROR_CODES` and this list (both directions) is
/// pinned by the test `terminal_registration_code_lists_stay_aligned` — if
/// the lists drift, run_loop returns no events and the UI silently freezes
/// on Connecting, which is worse than the flapping this fixes.
pub(crate) const REGISTER_REJECTED_ERROR_CODES: &[&str] = &[
    "invalid_access_token",
    "invalid_desktop_secret",
    "bad_desktop_endpoint_register",
    // A protocol-version mismatch is equally deterministic for a given build
    // pair (it means "update the desktop app").
    "unsupported_protocol_version",
];

fn terminal_message(value: &Value, endpoint_id: &str) -> Option<RelayTerminal> {
    let message_endpoint = value.get("endpoint_id").and_then(Value::as_str);
    if value.get("type").and_then(Value::as_str) == Some("error") {
        // Round-5 review M6: an empty-string scope is treated the same as a
        // missing one (same convention as the re-stamp predicate below). The
        // classification used to run before re-stamping and treat the empty
        // string as a "foreign endpoint" while the re-stamp predicate treats
        // it as "our own frame without scope" — the two predicates in one
        // file contradicted each other, and a relay sending reject frames
        // with `endpoint_id:""` made this side show Error without ending the
        // loop, flapping Error↔Connecting forever.
        if !message_endpoint.is_none_or(|value| value.is_empty() || value == endpoint_id) {
            return None;
        }
        let code = value.get("code").and_then(Value::as_str);
        // endpoint_not_found is recoverable: the relay lost its in-memory
        // state for the endpoint (process restart/TTL cleanup); after
        // reconnecting, register rebuilds it with the same endpoint_id and
        // credentials, so it self-heals as an ordinary disconnect; stopping
        // permanently would only silently freeze the UI (and reject frames
        // carry no endpoint_id, so the manager entry drops them anyway —
        // not even the reason is visible).
        return if code == Some("endpoint_not_found") {
            None
        } else if code == Some("endpoint_revoked") {
            // Register retry against a tombstoned endpoint: equivalent to
            // desktop_endpoint_revoked.
            Some(RelayTerminal::Revoked)
        } else if code.is_some_and(|code| REGISTER_REJECTED_ERROR_CODES.contains(&code)) {
            Some(RelayTerminal::RegisterRejected)
        } else {
            // Transient rejections (endpoint_capacity_reached,
            // endpoint_creation_rate_limited, authentication_timeout, etc.)
            // keep the existing reconnect behavior.
            None
        };
    }
    if message_endpoint != Some(endpoint_id) {
        return None;
    }
    match value.get("type").and_then(Value::as_str) {
        Some("desktop_endpoint_revoked") => Some(RelayTerminal::Revoked),
        Some("desktop_endpoint_replaced") => Some(RelayTerminal::Replaced),
        _ => None,
    }
}

/// Whether an inbound relay message is terminal for the given endpoint
/// (revoked / replaced / endpoint-not-found). The live connection loop
/// consumes the full [`terminal_message`] classification, including the new
/// `RegisterRejected` stop; the persistence-driven pending-revocation replay
/// only acknowledges outcomes that prove the endpoint is genuinely gone or
/// displaced (a credential rejection proves nothing about the endpoint, so
/// the durable entry stays for a later manual cleanup), which is why the
/// boolean view deliberately excludes it.
pub(crate) fn terminal_relay_message(value: &Value, endpoint_id: &str) -> bool {
    if value.get("type").and_then(Value::as_str) == Some("error")
        && value.get("code").and_then(Value::as_str) == Some("endpoint_not_found")
        && value
            .get("endpoint_id")
            .and_then(Value::as_str)
            // Round-6 review: an empty-string scope is treated the same as a
            // missing one (the third fold of this module's M6 invariant
            // previously missed this spot — Some("") would have been treated
            // as a "scoped foreign frame", falling into terminal_message's
            // recoverable classification, so the probe frame never got an
            // ack and the persisted entry lingered). The direction is
            // fail-safe, but the convention must be consistent; a foreign id
            // still does not fold.
            .is_none_or(|framed| framed.is_empty() || framed == endpoint_id)
    {
        // The endpoint is already absent: the pending revocation is
        // trivially confirmed even though the live loop must keep
        // reconnecting. Frames naming a foreign endpoint are not proof
        // about ours; acking on them would delete a durable revocation
        // entry for an endpoint that may still exist relay-side.
        return true;
    }
    matches!(
        terminal_message(value, endpoint_id),
        Some(RelayTerminal::Revoked | RelayTerminal::Replaced)
    )
}

/// Registration-class terminal error frames (reject frames carry no
/// endpoint_id) that do not yet carry a scope: before being forwarded to the
/// manager these frames are re-stamped with this socket's endpoint_id so the
/// explicit-match gate takes effect. Empty string and missing are treated
/// alike — the explicit-match gate must not treat an empty id as "the
/// current endpoint", or a late terminal frame in a rotation race would
/// still take down the endpoint that just replaced it.
fn registration_error_without_scope(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("error")
        && value
            .get("endpoint_id")
            .and_then(Value::as_str)
            .map_or(true, str::is_empty)
        && value
            .get("code")
            .and_then(Value::as_str)
            .is_some_and(|code| {
                code == "endpoint_revoked" || REGISTER_REJECTED_ERROR_CODES.contains(&code)
            })
}

/// Pre-forward re-stamp (detection + write in one; consumed by both the loop
/// and the tests): frames matching [`registration_error_without_scope`] get
/// this socket's endpoint_id added. Returns whether a stamp was applied.
fn stamp_unscoped_registration_error(value: &mut Value, endpoint_id: &str) -> bool {
    if !registration_error_without_scope(value) {
        return false;
    }
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "endpoint_id".to_string(),
            Value::String(endpoint_id.to_string()),
        );
    }
    true
}

fn registration_acknowledges_endpoint(value: &Value, endpoint_id: &str) -> bool {
    value.get("type").and_then(Value::as_str) == Some("desktop_endpoint_registered")
        && value.get("endpoint_id").and_then(Value::as_str) == Some(endpoint_id)
}

fn inbound_text_too_large(bytes: usize) -> bool {
    bytes > MAX_INBOUND_TEXT_BYTES
}

fn pending_has_capacity(pending: &VecDeque<OutboundFrame>) -> bool {
    pending.len() < MAX_PENDING_MESSAGES
}

fn push_pending(pending: &mut VecDeque<OutboundFrame>, frame: OutboundFrame) {
    assert!(
        pending_has_capacity(pending),
        "reconnect queue must apply channel backpressure before reaching its count limit"
    );
    pending.push_back(frame);
}

fn push_pending_front(pending: &mut VecDeque<OutboundFrame>, frame: OutboundFrame) {
    assert!(
        pending.len() < MAX_PENDING_MESSAGES,
        "a popped reconnect frame must leave room for failure restoration"
    );
    pending.push_front(frame);
}

fn register_message(config: &WebAccessConfig) -> Value {
    json!({
        "v": PROTOCOL_VERSION,
        "type": "desktop_endpoint_register",
        "endpoint_id": config.endpoint_id,
        "access_token": config.access_token,
        "desktop_secret": config.desktop_secret,
    })
}

fn revoke_message_parts(endpoint_id: &str, desktop_secret: &str) -> Value {
    json!({
        "v": PROTOCOL_VERSION,
        "type": "desktop_endpoint_revoke",
        "endpoint_id": endpoint_id,
        "desktop_secret": desktop_secret,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> WebAccessConfig {
        WebAccessConfig {
            relay_url: "wss://example.test/ws".into(),
            endpoint_id: "ep_1".into(),
            access_token: "browser".into(),
            desktop_secret: "desktop".into(),
            allow_host_workspace: false,
        }
    }

    /// Two silent heartbeat windows mean dead path (forced reconnect); one
    /// window of silence must NOT disconnect a healthy-but-quiet socket.
    #[test]
    fn inbound_silence_boundary_matches_two_heartbeat_windows() {
        assert!(!inbound_silence_exceeded(HEARTBEAT_INTERVAL));
        assert!(!inbound_silence_exceeded(
            HEARTBEAT_INTERVAL * 2 - Duration::from_millis(1)
        ));
        assert!(inbound_silence_exceeded(HEARTBEAT_INTERVAL * 2));
        assert!(inbound_silence_exceeded(HEARTBEAT_INTERVAL * 3));
    }

    #[test]
    fn registration_matches_the_v2_wire_contract() {
        assert_eq!(
            register_message(&config()),
            json!({
                "v": 2,
                "type": "desktop_endpoint_register",
                "endpoint_id": "ep_1",
                "access_token": "browser",
                "desktop_secret": "desktop",
            })
        );
    }

    #[test]
    fn reconnect_backoff_grows_caps_and_resets_only_after_registration_ack() {
        let mut backoff = ReconnectBackoff::default();
        let delays = (0..7)
            .map(|_| backoff.next_delay_with_jitter(1_000))
            .collect::<Vec<_>>();
        assert_eq!(
            delays,
            vec![
                Duration::from_millis(500),
                Duration::from_millis(1_000),
                Duration::from_millis(2_000),
                Duration::from_millis(4_000),
                Duration::from_millis(8_000),
                Duration::from_millis(10_000),
                Duration::from_millis(10_000),
            ]
        );

        assert!(!registration_acknowledges_endpoint(
            &json!({ "type": "desktop_endpoint_registered", "endpoint_id": "ep_other" }),
            "ep_1"
        ));
        assert!(registration_acknowledges_endpoint(
            &json!({ "type": "desktop_endpoint_registered", "endpoint_id": "ep_1" }),
            "ep_1"
        ));
        backoff.reset();
        assert_eq!(
            backoff.next_delay_with_jitter(1_000),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn revoke_never_exposes_the_browser_token() {
        let config = config();
        let value = revoke_message_parts(&config.endpoint_id, &config.desktop_secret);
        assert_eq!(value["type"], "desktop_endpoint_revoke");
        assert_eq!(value["desktop_secret"], "desktop");
        assert!(value.get("access_token").is_none());
    }

    #[test]
    fn offline_pending_and_channel_capacity_apply_backpressure_without_dropping_fifo() {
        let budget = 1024 * 1024;
        let (sender, mut receiver) = outbound_channel(OUTBOUND_CHANNEL_CAPACITY, budget, 128);
        let mut pending = VecDeque::new();
        let mut pending_bytes = 0;
        for index in 0..MAX_PENDING_MESSAGES {
            sender
                .try_send(RelayOutbound::Message(json!(index)))
                .unwrap();
            let frame = receiver.try_recv().unwrap();
            pending_bytes += frame.text.len();
            push_pending(&mut pending, frame);
        }
        assert!(!pending_has_capacity(&pending));
        assert_eq!(pending.front().map(|frame| frame.text.as_str()), Some("0"));
        let expected_pending_back = (MAX_PENDING_MESSAGES - 1).to_string();
        assert_eq!(
            pending.back().map(|frame| frame.text.as_str()),
            Some(expected_pending_back.as_str())
        );

        for index in MAX_PENDING_MESSAGES..(MAX_PENDING_MESSAGES + OUTBOUND_CHANNEL_CAPACITY) {
            sender
                .try_send(RelayOutbound::Message(json!(index)))
                .unwrap();
        }
        let permits_before_rejection = sender.byte_budget.available_permits();
        assert!(matches!(
            sender.try_send(RelayOutbound::Message(json!("channel-full"))),
            Err(RelayTrySendError::ChannelFull)
        ));
        assert_eq!(
            sender.byte_budget.available_permits(),
            permits_before_rejection,
            "a failed channel enqueue must release its byte permit"
        );

        for expected in MAX_PENDING_MESSAGES..(MAX_PENDING_MESSAGES + OUTBOUND_CHANNEL_CAPACITY) {
            let frame = receiver.try_recv().unwrap();
            assert_eq!(frame.text, expected.to_string());
        }
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert_eq!(
            sender.byte_budget.available_permits(),
            budget - pending_bytes
        );
        drop(pending);
        assert_eq!(sender.byte_budget.available_permits(), budget);
    }

    #[test]
    fn outbound_frame_limit_fails_closed_before_enqueue() {
        let text = serde_json::to_string(&json!({ "payload": "0123456789" })).unwrap();
        let (sender, mut receiver) = outbound_channel(1, 1024, text.len() - 1);

        let result = sender.try_send(RelayOutbound::Message(json!({
            "payload": "0123456789"
        })));

        assert_eq!(
            result,
            Err(RelayTrySendError::FrameTooLarge {
                bytes: text.len(),
                limit: text.len() - 1,
            })
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert_eq!(sender.byte_budget.available_permits(), 1024);
    }

    #[test]
    fn outbound_byte_permit_survives_channel_to_pending_transition() {
        let value = json!({ "payload": "retained" });
        let bytes = serde_json::to_string(&value).unwrap().len();
        let budget = (bytes * 2) - 1;
        let (sender, mut receiver) = outbound_channel(2, budget, 1024);

        sender
            .try_send(RelayOutbound::Message(value.clone()))
            .unwrap();
        let frame = receiver.try_recv().unwrap();
        let mut pending = VecDeque::new();
        push_pending(&mut pending, frame);
        assert_eq!(sender.byte_budget.available_permits(), budget - bytes);
        assert_eq!(
            sender.try_send(RelayOutbound::Message(value.clone())),
            Err(RelayTrySendError::ByteBudgetExhausted)
        );

        drop(pending);
        assert_eq!(sender.byte_budget.available_permits(), budget);
        sender
            .try_send(RelayOutbound::Message(value))
            .expect("dropping the pending frame must release its byte budget");
    }

    #[test]
    fn shutdown_bypasses_a_saturated_data_channel() {
        let budget = 1024;
        let queued = json!({ "queued": true });
        let queued_bytes = serde_json::to_string(&queued).unwrap().len();
        let (sender, receiver) = outbound_channel(1, budget, 1024);
        sender.try_send(RelayOutbound::Message(queued)).unwrap();
        assert_eq!(
            sender.byte_budget.available_permits(),
            budget - queued_bytes
        );
        assert!(matches!(
            sender.try_send(RelayOutbound::Message(json!({ "full": true }))),
            Err(RelayTrySendError::ChannelFull)
        ));
        assert_eq!(
            sender.byte_budget.available_permits(),
            budget - queued_bytes,
            "the rejected data frame must not leak byte permits"
        );

        sender.try_send(RelayOutbound::Shutdown).unwrap();
        assert!(sender.shutdown.is_cancelled());
        // Repeated lifecycle requests remain idempotent and cannot be starved.
        sender.try_send(RelayOutbound::Shutdown).unwrap();
        drop(receiver);
        assert_eq!(sender.byte_budget.available_permits(), budget);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn saturated_stream_resets_singleflight_and_deliver_only_the_latest_waiter() {
        let budget = 4096;
        let (sender, mut receiver) = outbound_channel(1, budget, 1024);
        sender
            .try_send(RelayOutbound::Message(json!({ "kind": "blocker" })))
            .unwrap();

        let reset = |marker: &str| {
            json!({
                "v": 2,
                "type": "stream_reset",
                "lease_id": marker,
                "stream_epoch": marker,
            })
        };
        sender.enqueue_stream_reset(reset("A")).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let first_reset_is_in_flight = {
                    let state = sender.coalesced_control.lock();
                    state.worker_running && state.latest.is_none()
                };
                if first_reset_is_in_flight {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("control worker should block behind the saturated channel");

        sender.enqueue_stream_reset(reset("B")).unwrap();
        sender.enqueue_stream_reset(reset("C")).unwrap();
        {
            let state = sender.coalesced_control.lock();
            assert!(state.worker_running);
            let latest = state
                .latest
                .as_ref()
                .expect("latest reset must be retained");
            let latest: Value = serde_json::from_str(&latest.text).unwrap();
            assert_eq!(latest["lease_id"], "C");
        }

        let blocker = receiver.recv().await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&blocker.text).unwrap()["kind"],
            "blocker"
        );
        drop(blocker);

        let first = receiver.recv().await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&first.text).unwrap()["lease_id"],
            "A"
        );
        drop(first);
        let latest = receiver.recv().await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&latest.text).unwrap()["lease_id"],
            "C"
        );
        drop(latest);

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let stopped = {
                    let state = sender.coalesced_control.lock();
                    !state.worker_running && state.latest.is_none()
                };
                if stopped {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("control worker should stop after draining the latest reset");
        assert_eq!(sender.byte_budget.available_permits(), budget);
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn inbound_channel_and_text_frames_have_hard_bounds() {
        let websocket = relay_websocket_config();
        assert_eq!(websocket.max_message_size, Some(MAX_INBOUND_TEXT_BYTES));
        assert_eq!(websocket.max_frame_size, Some(MAX_INBOUND_TEXT_BYTES));
        // PROTOCOL.md: MAX_PAYLOAD_BYTES >= 4 MiB; the desktop frame cap is
        // 4 MiB - 64 KiB. The inbound cap must accommodate the largest frame
        // the desktop itself is allowed to send.
        assert_eq!(MAX_INBOUND_TEXT_BYTES, MAX_RELAY_FRAME_BYTES);
        assert_eq!(MAX_INBOUND_TEXT_BYTES, 4 * 1024 * 1024 - 64 * 1024);

        assert!(!inbound_text_too_large(MAX_INBOUND_TEXT_BYTES));
        assert!(inbound_text_too_large(MAX_INBOUND_TEXT_BYTES + 1));
    }

    #[test]
    fn revoke_ack_and_endpoint_replacement_are_terminal() {
        assert_eq!(
            terminal_message(
                &json!({
                    "type": "desktop_endpoint_revoked",
                    "endpoint_id": "ep_1"
                }),
                "ep_1"
            ),
            Some(RelayTerminal::Revoked)
        );
        assert_eq!(
            terminal_message(
                &json!({
                    "type": "desktop_endpoint_replaced",
                    "endpoint_id": "ep_1"
                }),
                "ep_1"
            ),
            Some(RelayTerminal::Replaced)
        );
        assert_eq!(
            // endpoint_not_found is recoverable: register rebuilds the
            // endpoint after reconnecting, without ending the connection
            // (otherwise the UI silently freezes and the manager never
            // learns any reason).
            terminal_message(
                &json!({
                    "type": "error",
                    "code": "endpoint_not_found"
                }),
                "ep_1"
            ),
            None
        );
        // The terminal judgment applies only to the registered endpoint:
        // another endpoint's message must not end this connection
        // (originally terminal_messages_are_scoped_to_the_registered_endpoint).
        assert_eq!(
            terminal_message(
                &json!({
                    "type": "desktop_endpoint_replaced",
                    "endpoint_id": "ep_other"
                }),
                "ep_1"
            ),
            None
        );
    }

    #[test]
    fn registration_rejections_are_terminal_but_transient_errors_keep_reconnecting() {
        // Tombstoned endpoint: retrying with the same credentials must fail
        // the same way, so reconnecting must stop.
        assert_eq!(
            terminal_message(
                &json!({
                    "type": "error",
                    "code": "endpoint_revoked",
                    "message": "endpoint was permanently revoked"
                }),
                "ep_1"
            ),
            Some(RelayTerminal::Revoked)
        );
        // Credentials deterministically rejected: stop reconnecting, leaving
        // the manager parked in an Error state with a concrete reason instead
        // of flapping between Error↔Connecting every ~10s.
        for code in [
            "invalid_access_token",
            "invalid_desktop_secret",
            "bad_desktop_endpoint_register",
            // A protocol-version mismatch is equally deterministic for a
            // given build pair.
            "unsupported_protocol_version",
        ] {
            assert_eq!(
                terminal_message(&json!({ "type": "error", "code": code }), "ep_1"),
                Some(RelayTerminal::RegisterRejected),
                "{code} must stop the reconnect loop"
            );
        }
        // Transient errors (capacity/rate limit/auth timeout) still reconnect.
        for code in [
            "endpoint_capacity_reached",
            "endpoint_creation_rate_limited",
            "authentication_timeout",
        ] {
            assert_eq!(
                terminal_message(&json!({ "type": "error", "code": code }), "ep_1"),
                None,
                "{code} must keep reconnecting"
            );
        }
        // Only a reject frame with no endpoint_id defaults to hitting this
        // connection; one naming a foreign endpoint does not end it.
        assert_eq!(
            terminal_message(
                &json!({
                    "type": "error",
                    "code": "endpoint_revoked",
                    "endpoint_id": "ep_other"
                }),
                "ep_1"
            ),
            None
        );
        // The revocation replay's boolean view only accepts terminal outcomes
        // that prove "the endpoint is confirmed gone/displaced"; a credential
        // rejection must not let the replay delete a durable revocation entry
        // as if confirmed.
        assert!(terminal_relay_message(
            &json!({ "type": "error", "code": "endpoint_revoked" }),
            "ep_1"
        ));
        // A missing endpoint is equally equivalent to the revocation being
        // confirmed.
        assert!(terminal_relay_message(
            &json!({ "type": "error", "code": "endpoint_not_found" }),
            "ep_1"
        ));
        // Round-6 review: an empty-string scope is treated the same as a
        // missing one — the probe frame's third fold previously missed this
        // spot, so Some("") was treated as a scoped foreign frame and the
        // ack never landed.
        assert!(terminal_relay_message(
            &json!({ "type": "error", "code": "endpoint_not_found", "endpoint_id": "" }),
            "ep_1"
        ));
        assert!(!terminal_relay_message(
            &json!({ "type": "error", "code": "endpoint_not_found", "endpoint_id": "ep_other" }),
            "ep_1"
        ));
        assert!(!terminal_relay_message(
            &json!({ "type": "error", "code": "invalid_desktop_secret" }),
            "ep_1"
        ));
    }

    /// Wiring pin (both directions): the manager's registration-terminal list
    /// and this module's `terminal_message` RegisterRejected/Revoked
    /// classification must cover each other — every code in the REGISTER list
    /// must be in the manager list, and every code in the manager list must
    /// be terminal; unscoped registration-class terminal frames get
    /// re-stamped by run_loop with this socket's endpoint_id; frames already
    /// carrying a scope must pass through verbatim, an empty-string scope is
    /// treated like a missing one (re-stamped), non-registration error codes
    /// must not be stamped by mistake; the stamp writes this socket's
    /// endpoint.
    #[test]
    fn unscoped_registration_errors_are_detected_for_stamping() {
        assert!(registration_error_without_scope(&json!({
            "type": "error",
            "code": "endpoint_revoked"
        })));
        assert!(registration_error_without_scope(&json!({
            "type": "error",
            "code": "invalid_access_token"
        })));
        assert!(registration_error_without_scope(&json!({
            "type": "error",
            "code": "endpoint_revoked",
            "endpoint_id": ""
        })));
        assert!(!registration_error_without_scope(&json!({
            "type": "error",
            "code": "endpoint_revoked",
            "endpoint_id": "ep_x"
        })));
        assert!(!registration_error_without_scope(&json!({
            "type": "desktop_endpoint_revoked",
            "code": "endpoint_revoked"
        })));
        // Round-6 review: the test doc claims "non-registration error codes
        // must not be stamped by mistake" — unscoped transient-class
        // (capacity/rate-limit) error frames must pass through verbatim;
        // stamping would make the manager record every transient rejection as
        // a sticky Error, flapping back and forth during capacity squeeze —
        // exactly the jitter class this PR set out to eliminate. Removing the
        // code allowlist from the predicate turns this red.
        for transient in [
            "endpoint_capacity_reached",
            "endpoint_creation_rate_limited",
        ] {
            assert!(
                !registration_error_without_scope(&json!({
                    "type": "error",
                    "code": transient
                })),
                "a transient unscoped error ({transient}) must not be stamped with our endpoint"
            );
        }

        // Round-5 review M6: registration reject frames with an empty-string
        // scope count as ours in the terminal classification — same
        // convention as the re-stamp predicate (assertions above). If the
        // empty-string fold is removed from terminal_message, these frames
        // are classified as a "foreign endpoint", the loop does not end, the
        // UI flaps between Error↔Connecting forever, and this pin turns red.
        for code in [
            "endpoint_revoked",
            "invalid_access_token",
            "invalid_desktop_secret",
            "bad_desktop_endpoint_register",
            "unsupported_protocol_version",
        ] {
            let frame = json!({ "type": "error", "code": code, "endpoint_id": "" });
            assert!(
                terminal_message(&frame, "ep_self").is_some(),
                "an empty-scope {code} error must classify terminal for this endpoint"
            );
        }
        // Non-error notices (desktop-side events) always carry an explicit
        // id: an empty string still does not count as ours.
        assert_eq!(
            terminal_message(
                &json!({ "type": "desktop_endpoint_revoked", "endpoint_id": "" }),
                "ep_self"
            ),
            None,
            "an empty-scope notice must not classify terminal"
        );
        // A foreign explicit id still does not end the connection (no
        // regression of the existing semantics).
        assert_eq!(
            terminal_message(
                &json!({ "type": "error", "code": "endpoint_revoked", "endpoint_id": "ep_other" }),
                "ep_self"
            ),
            None
        );

        // The stamp is all-in-one: a matching frame gets this socket's
        // endpoint written, a non-matching frame stays verbatim.
        let mut scoped = json!({
            "type": "error",
            "code": "endpoint_revoked",
            "endpoint_id": ""
        });
        assert!(stamp_unscoped_registration_error(&mut scoped, "ep-mine"));
        assert_eq!(
            scoped.get("endpoint_id").and_then(Value::as_str),
            Some("ep-mine")
        );

        let mut named = json!({
            "type": "error",
            "code": "endpoint_revoked",
            "endpoint_id": "ep_other"
        });
        assert!(!stamp_unscoped_registration_error(&mut named, "ep-mine"));
        assert_eq!(
            named.get("endpoint_id").and_then(Value::as_str),
            Some("ep_other")
        );
    }

    /// `endpoint_not_found` is deliberately absent from the manager list (its
    /// reject frames carry no endpoint_id, and the manager entry drops them
    /// anyway). If the lists drift, run_loop returns no events and the UI
    /// silently freezes on Connecting — worse than the flapping this fixes.
    #[test]
    fn terminal_registration_code_lists_stay_aligned() {
        let mut classified_terminal = vec!["endpoint_revoked"];
        classified_terminal.extend(REGISTER_REJECTED_ERROR_CODES);
        for code in &classified_terminal {
            assert!(
                super::super::manager::TERMINAL_REGISTRATION_ERROR_CODES.contains(code),
                "relay terminal code {code} must be in the manager terminal list"
            );
        }
        for code in super::super::manager::TERMINAL_REGISTRATION_ERROR_CODES {
            let classified =
                terminal_message(&json!({ "type": "error", "code": code }), "ep-cross-pin");
            assert!(
                matches!(
                    classified,
                    Some(RelayTerminal::RegisterRejected | RelayTerminal::Revoked)
                ),
                "manager terminal code {code} must stop the loop in relay_client, got {classified:?}"
            );
        }
        assert!(
            !super::super::manager::TERMINAL_REGISTRATION_ERROR_CODES
                .contains(&"endpoint_not_found")
        );
    }

    /// Wiring pin (Round-15 review MA5): the revocation worker's terminal
    /// gate semantics exist only in this one inline judgment inside
    /// run_revocation_loop — letting `terminal.is_some()` gate alone
    /// (deleting the `|| (scoped_to_self && …endpoint_not_found…)` leg)
    /// resurrects the 12s infinite connect→revoke loop predicted in the
    /// comment, while the terminal_message / terminal_relay_message unit
    /// tests cannot see this call site and the whole suite stays green.
    /// Pins the verbatim shape of the two-leg disjunction and the
    /// endpoint_not_found literal: deleting a leg, regressing to a single
    /// leg, or renaming any of them turns red.
    /// Round-25 review MAJOR-2: pure comment lines are filtered out before
    /// matching (same convention as the manager test module's
    /// comment_filtered; a mention in a trailing `//` or inside a block
    /// comment can still cover).
    fn comment_filtered(source: &str) -> String {
        // Round-27 review m14: the filter primitive moved up to
        // platform::test_support as shared code.
        crate::platform::test_support::comment_filtered(source)
    }

    #[test]
    fn revocation_worker_gate_wires_the_worker_terminal_legs() {
        let source = include_str!("relay_client.rs");
        let start = source
            .find("async fn run_revocation_loop")
            .expect("run_revocation_loop must exist");
        let tail = &source[start..];
        let end = ["\nasync fn ", "\nfn ", "\n#[cfg(test)]", "\nmod "]
            .iter()
            .filter_map(|marker| tail.find(marker))
            .min()
            .expect("run_revocation_loop body must be terminated by the next item");
        let body = comment_filtered(&tail[..end]);
        assert!(
            body.contains(
                "let worker_terminal = terminal.is_some()
                                        || (scoped_to_self"
            ),
            "the revocation worker gate must keep the two-leg disjunction: terminal \
             frames AND self-scoped endpoint_not_found both end the worker; reverting \
             to `terminal.is_some()` alone resurrects the 12s infinite connect→revoke \
             loop after relay state loss"
        );
        assert!(
            body.contains("== Some(\"endpoint_not_found\")"),
            "the worker gate must pin its own endpoint_not_found literal; the shared \
             folding predicate classifies it as recoverable for the register loop"
        );
        // Round-16 follow-up review (M5): the derivation line is part of the
        // gate semantics too — flipping is_none_or → is_some_and (a purely
        // antonymic rewrite) reclassifies a foreign endpoint's
        // endpoint_not_found frame as non-terminal with both existing
        // assertions green, resurrecting the 12s loop. Pins the none-or
        // derivation by its byte-for-byte shape.
        assert!(
            body.contains(".is_none_or(|scope| scope.is_empty() || scope == endpoint_id)"),
            "the scope derivation must stay none-or (missing endpoint_id or an \
             empty/self scope counts as scoped to self); flipping it to is_some_and \
             reopens the infinite connect→revoke loop while both gate-leg pins \
             stay green"
        );
        // Round-18 review: besides the derivation line, two adjacent
        // mutations keep all existing assertions green — (a) deleting the
        // downstream consumption (`if worker_terminal { ... return; }`)
        // leaves the worker never terminating on terminal frames,
        // resurrecting the 12s loop; (b) moving the consumption before the
        // tx_in forward means the manager never sees the terminal frame (the
        // worker exits silently, and a late terminal frame in a rotation
        // race cannot be told apart from frame ownership). Pins both via the
        // "forward before consume" order.
        let forward = body
            .find("if tx_in.send(RelayInbound::Message(text.to_string())).await.is_err()")
            .expect("the terminal frame must still be forwarded to the manager");
        let consume = body.find("if worker_terminal {").expect(
            "the worker_terminal derivation must be consumed by the loop's \
                 terminal return; deleting the consumption lets terminal frames \
                 pass unacted and resurrects the 12s connect→revoke loop",
        );
        assert!(
            forward < consume,
            "the terminal frame must be forwarded to the manager BEFORE the worker \
             terminates; consuming first silently drops the terminal frame"
        );
        // Round-20 review (S04): the mutation deleting the `return;` inside
        // the consumption block (Close still sent, loop keeps running)
        // previously kept every assertion green — the 12s connect→revoke
        // loop resurrects. Pins that the consumption block must still end
        // with return; (slice taken up to the block's first closing brace).
        let consume_slice = &body[consume..];
        let consume_block_end = consume_slice
            .find('}')
            .expect("the worker_terminal consumption block must be closed by its brace");
        assert!(
            consume_slice[..consume_block_end].contains("return;"),
            "the worker_terminal block must still return after sending Close; \
             deleting the return keeps the worker looping on terminal frames \
             and resurrects the 12s connect→revoke loop"
        );
    }

    /// Wiring pin (Round-15 review MA5, lightweight half): run_loop's
    /// registration-class reject frames must go through
    /// stamp_unscoped_registration_error before forwarding — deleting that
    /// call site leaves the helper's unit tests green (the manager's empty-id
    /// defense backstops it, hence only a lightweight pin).
    #[test]
    fn run_loop_stamps_unscoped_registration_errors_before_forwarding() {
        let source = include_str!("relay_client.rs");
        let start = source
            .find("async fn run_loop")
            .expect("run_loop must exist");
        let tail = &source[start..];
        let end = ["\nasync fn ", "\nfn ", "\n#[cfg(test)]", "\nmod "]
            .iter()
            .filter_map(|marker| tail.find(marker))
            .min()
            .expect("run_loop body must be terminated by the next item");
        let body = comment_filtered(&tail[..end]);
        assert!(
            body.contains(
                "if stamp_unscoped_registration_error(
                                            &mut value,
                                            &config.endpoint_id,
                                        )"
            ),
            "run_loop must stamp unscoped registration errors with the endpoint it is \
             registering before forwarding; deleting the call site makes late terminal \
             frames unattributable to the manager's explicit-match gate"
        );
        // Round-18 review: once the stamp goes into `forwarded`, the send arm
        // must use the stamped value — a mutation reverting the send to the
        // raw `text` turns the stamp into a dead computation with both
        // assertions (call-site shape + this one) green. Pins the consumer
        // via the send argument.
        assert!(
            body.contains("RelayInbound::Message(forwarded)"),
            "the forwarded frame must be the stamped `forwarded` value; reverting \
             the send to the raw `text` turns the stamp into a dead computation"
        );
    }
}
