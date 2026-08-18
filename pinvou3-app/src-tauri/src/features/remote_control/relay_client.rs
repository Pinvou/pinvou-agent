use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use rand::Rng;
use serde_json::{json, Value};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
};
use tokio_util::sync::CancellationToken;

use super::protocol::{WebAccessConfig, PROTOCOL_VERSION};

const RECONNECT_BASE_DELAY_MS: u64 = 500;
const RECONNECT_MAX_DELAY_MS: u64 = 10_000;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(25);
const REVOKE_ACK_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_PENDING_MESSAGES: usize = 2_048;
const OUTBOUND_CHANNEL_CAPACITY: usize = 2_048;
// RelayInbound queues raw JSON text, so 32 slots at the 2 MiB frame ceiling
// bound retained wire payload to roughly 64 MiB. Only the current frame is
// parsed at either side of the queue.
const INBOUND_CHANNEL_CAPACITY: usize = 32;
const MAX_INBOUND_TEXT_BYTES: usize = 2 * 1024 * 1024;
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

struct OutboundFrame {
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
    worker_starts: u64,
}

enum QueuedOutbound {
    Message(OutboundFrame),
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
    tx: mpsc::Sender<QueuedOutbound>,
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
        let queued = QueuedOutbound::Message(OutboundFrame {
            text: frame.text,
            _byte_permit: byte_permit,
        });
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
        let queued = QueuedOutbound::Message(OutboundFrame {
            text: frame.text,
            _byte_permit: byte_permit,
        });
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
        if matches!(&outbound, RelayOutbound::Shutdown) {
            // Control-plane shutdown must remain deliverable even when every
            // data slot and byte permit is occupied.
            self.cancel();
            return Ok(());
        }
        let frame = match outbound {
            RelayOutbound::Message(value) => self.serialize_message(value)?,
            RelayOutbound::Shutdown => unreachable!("shutdown returned above"),
        };
        self.try_send_serialized(frame)
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
                state.worker_starts = state.worker_starts.saturating_add(1);
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

fn outbound_channel(
    channel_capacity: usize,
    byte_budget: usize,
    max_frame_bytes: usize,
) -> (RelaySender, mpsc::Receiver<QueuedOutbound>) {
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
                                    drop(value);
                                    if tx_in.send(RelayInbound::Message(text.to_string())).await.is_err() {
                                        return;
                                    }
                                    if terminal.is_some() {
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

        if tx_in
            .send(RelayInbound::Connection {
                endpoint_id: endpoint_id.clone(),
                connected: false,
                error: Some("relay revocation connection lost; reconnecting".to_string()),
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
    mut rx_out: mpsc::Receiver<QueuedOutbound>,
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
                        Some(QueuedOutbound::Message(frame)) => {
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
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    return;
                }
                outbound = rx_out.recv() => {
                    match outbound {
                        Some(QueuedOutbound::Message(frame)) => {
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
                                Ok(value) => {
                                    let terminal = terminal_message(&value, &config.endpoint_id);
                                    if registration_acknowledges_endpoint(&value, &config.endpoint_id) {
                                        reconnect_backoff.reset();
                                    }
                                    drop(value);
                                    if !send_inbound_until_shutdown(
                                        &tx_in,
                                        RelayInbound::Message(text.to_string()),
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
    Missing,
}

fn terminal_message(value: &Value, endpoint_id: &str) -> Option<RelayTerminal> {
    let message_endpoint = value.get("endpoint_id").and_then(Value::as_str);
    if value.get("type").and_then(Value::as_str) == Some("error")
        && value.get("code").and_then(Value::as_str) == Some("endpoint_not_found")
        && message_endpoint.is_none_or(|value| value == endpoint_id)
    {
        return Some(RelayTerminal::Missing);
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
        }
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
            let QueuedOutbound::Message(frame) = receiver.try_recv().unwrap();
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
            let QueuedOutbound::Message(frame) = receiver.try_recv().unwrap();
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
        let QueuedOutbound::Message(frame) = receiver.try_recv().unwrap();
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
            assert_eq!(state.worker_starts, 1);
            let latest = state
                .latest
                .as_ref()
                .expect("latest reset must be retained");
            let latest: Value = serde_json::from_str(&latest.text).unwrap();
            assert_eq!(latest["lease_id"], "C");
        }

        let QueuedOutbound::Message(blocker) = receiver.recv().await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&blocker.text).unwrap()["kind"],
            "blocker"
        );
        drop(blocker);

        let QueuedOutbound::Message(first) = receiver.recv().await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&first.text).unwrap()["lease_id"],
            "A"
        );
        drop(first);
        let QueuedOutbound::Message(latest) = receiver.recv().await.unwrap();
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
        assert_eq!(sender.coalesced_control.lock().worker_starts, 1);
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
            terminal_message(
                &json!({
                    "type": "error",
                    "code": "endpoint_not_found"
                }),
                "ep_1"
            ),
            Some(RelayTerminal::Missing)
        );
        // 终结判定只对已注册 endpoint 生效:他端点消息不得终结本连接
        // (原 terminal_messages_are_scoped_to_the_registered_endpoint)。
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
}
