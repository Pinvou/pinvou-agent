use std::collections::{HashSet, VecDeque};

use serde_json::{json, Value};

use super::protocol::PROTOCOL_VERSION;
use super::relay_client;

const JOURNAL_CAPACITY: usize = 1_024;
const JOURNAL_BYTES_CAPACITY: usize = 16 * 1024 * 1024;

/// Relay-issued lease IDs are `lease_` plus 24 base64url characters. Use an
/// equal-length placeholder while no browser is connected so a journaled
/// event's complete replay envelope is still sized conservatively.
pub(super) const LEASE_ID_WIRE_PLACEHOLDER: &str = "lease_000000000000000000000000";

#[derive(Debug, Clone)]
struct StreamEvent {
    seq: u64,
    event: String,
    payload: Value,
    wire_bytes: usize,
}

#[derive(Debug)]
pub(super) struct EventStreamState {
    epoch: String,
    seq: u64,
    journal: VecDeque<StreamEvent>,
    journal_bytes: usize,
    subscriptions: HashSet<String>,
}

impl Default for EventStreamState {
    fn default() -> Self {
        Self {
            epoch: new_stream_epoch(),
            seq: 0,
            journal: VecDeque::new(),
            journal_bytes: 0,
            subscriptions: HashSet::new(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum StreamRecordError {
    Serialize(String),
    Oversized { wire_bytes: usize, limit: usize },
}

impl std::fmt::Display for StreamRecordError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialize(error) => write!(formatter, "序列化远程控制事件帧失败：{error}"),
            Self::Oversized { wire_bytes, limit } => write!(
                formatter,
                "远程控制事件帧过大（{wire_bytes} 字节；上限 {limit}）"
            ),
        }
    }
}

impl EventStreamState {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn epoch(&self) -> &str {
        &self.epoch
    }

    pub(super) fn seq(&self) -> u64 {
        self.seq
    }

    pub(super) fn reset(&mut self) {
        self.epoch = new_stream_epoch();
        self.seq = 0;
        self.journal.clear();
        self.journal_bytes = 0;
    }

    pub(super) fn clear_subscriptions(&mut self) {
        self.subscriptions.clear();
    }

    pub(super) fn subscription_names(&self) -> Vec<String> {
        self.subscriptions.iter().cloned().collect()
    }

    pub(super) fn set_subscription(&mut self, event: &str, subscribe: bool) -> bool {
        if subscribe {
            self.subscriptions.insert(event.to_string())
        } else {
            self.subscriptions.remove(event)
        }
    }

    pub(super) fn is_subscribed(&self, event: &str) -> bool {
        self.subscriptions.contains(event)
    }

    pub(super) fn record(
        &mut self,
        endpoint_id: &str,
        lease_id: &str,
        event: String,
        payload: Value,
    ) -> Result<Value, StreamRecordError> {
        self.record_with_limits(
            endpoint_id,
            lease_id,
            event,
            payload,
            relay_client::MAX_RELAY_FRAME_BYTES,
            JOURNAL_CAPACITY,
            JOURNAL_BYTES_CAPACITY,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn record_with_limits(
        &mut self,
        endpoint_id: &str,
        lease_id: &str,
        event: String,
        payload: Value,
        max_wire_bytes: usize,
        journal_capacity: usize,
        journal_bytes_capacity: usize,
    ) -> Result<Value, StreamRecordError> {
        let mut recorded = StreamEvent {
            seq: self.seq.saturating_add(1),
            event,
            payload,
            wire_bytes: 0,
        };
        let message = event_message(endpoint_id, lease_id, &self.epoch, &recorded);
        let wire_bytes = serde_json::to_vec(&message)
            .map_err(|error| StreamRecordError::Serialize(error.to_string()))?
            .len();
        if wire_bytes > max_wire_bytes {
            // The rejected event never advances seq or enters the journal. A
            // fresh epoch makes every existing browser cursor explicitly
            // invalid instead of leaving a permanent, unreplayable hole.
            self.reset();
            return Err(StreamRecordError::Oversized {
                wire_bytes,
                limit: max_wire_bytes,
            });
        }

        recorded.wire_bytes = wire_bytes;
        self.seq = recorded.seq;
        self.journal_bytes = self.journal_bytes.saturating_add(wire_bytes);
        self.journal.push_back(recorded);
        while self.journal.len() > journal_capacity || self.journal_bytes > journal_bytes_capacity {
            let Some(evicted) = self.journal.pop_front() else {
                break;
            };
            self.journal_bytes = self.journal_bytes.saturating_sub(evicted.wire_bytes);
        }
        Ok(message)
    }

    /// `None` means the cursor cannot be satisfied from the bounded journal.
    fn replay_after(&self, after_seq: u64) -> Option<Vec<StreamEvent>> {
        if after_seq > self.seq {
            return None;
        }
        if after_seq == self.seq {
            return Some(Vec::new());
        }
        let oldest = self.journal.front()?.seq;
        if after_seq.saturating_add(1) < oldest {
            return None;
        }
        Some(
            self.journal
                .iter()
                .filter(|entry| entry.seq > after_seq)
                .cloned()
                .collect(),
        )
    }

    pub(super) fn replay_messages_after(
        &self,
        after_seq: u64,
        context: ReplayMessageContext<'_>,
    ) -> Option<Vec<Value>> {
        self.replay_after(after_seq)
            .map(|events| self.subscription_filtered_replay_messages(events, context))
    }

    fn subscription_filtered_replay_messages(
        &self,
        events: Vec<StreamEvent>,
        context: ReplayMessageContext<'_>,
    ) -> Vec<Value> {
        let mut messages = Vec::with_capacity(events.len());
        let mut skipped_through = None;
        for event in events {
            if self.is_subscribed(&event.event) {
                if let Some(seq) = skipped_through.take() {
                    messages.push(snapshot_message(
                        context.endpoint_id,
                        context.lease_id,
                        context.stream_epoch,
                        seq,
                        context.capability_commands,
                        context.capability_events,
                    ));
                }
                messages.push(event_message(
                    context.endpoint_id,
                    context.lease_id,
                    context.stream_epoch,
                    &event,
                ));
            } else {
                skipped_through = Some(event.seq);
            }
        }
        if let Some(seq) = skipped_through {
            messages.push(snapshot_message(
                context.endpoint_id,
                context.lease_id,
                context.stream_epoch,
                seq,
                context.capability_commands,
                context.capability_events,
            ));
        }
        messages
    }

    /// Rebase the journal tail into a fresh epoch after live delivery failed,
    /// preserving the critical final event for the next reconnect.
    pub(super) fn rebase_tail(
        &mut self,
        endpoint_id: &str,
        lease_id: &str,
    ) -> Result<(), StreamRecordError> {
        let Some(failed_event) = self.journal.back().cloned() else {
            return Err(StreamRecordError::Serialize(
                "远程控制事件流末尾数据丢失".to_string(),
            ));
        };
        self.reset();
        self.record(
            endpoint_id,
            lease_id,
            failed_event.event,
            failed_event.payload,
        )?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(super) struct ReplayMessageContext<'a> {
    pub(super) endpoint_id: &'a str,
    pub(super) lease_id: &'a str,
    pub(super) stream_epoch: &'a str,
    pub(super) capability_commands: &'a [String],
    pub(super) capability_events: &'a [String],
}

fn event_message(
    endpoint_id: &str,
    lease_id: &str,
    stream_epoch: &str,
    event: &StreamEvent,
) -> Value {
    json!({
        "v": PROTOCOL_VERSION,
        "type": "event",
        "endpoint_id": endpoint_id,
        "lease_id": lease_id,
        "stream_epoch": stream_epoch,
        "seq": event.seq,
        "event": event.event,
        "payload": event.payload,
    })
}

pub(super) fn stream_reset_message(
    endpoint_id: &str,
    lease_id: &str,
    stream_epoch: &str,
    reason: &str,
) -> Value {
    json!({
        "v": PROTOCOL_VERSION,
        "type": "stream_reset",
        "endpoint_id": endpoint_id,
        "lease_id": lease_id,
        "stream_epoch": stream_epoch,
        "seq": 0,
        "reason": reason,
    })
}

pub(super) fn try_enqueue_message_batch(
    messages: Vec<Value>,
    mut enqueue: impl FnMut(Value) -> bool,
) -> bool {
    for message in messages {
        if !enqueue(message) {
            return false;
        }
    }
    true
}

pub(super) fn snapshot_message(
    endpoint_id: &str,
    lease_id: &str,
    epoch: &str,
    seq: u64,
    commands: &[String],
    events: &[String],
) -> Value {
    json!({
        "v": PROTOCOL_VERSION,
        "type": "desktop_snapshot",
        "endpoint_id": endpoint_id,
        "lease_id": lease_id,
        "stream_epoch": epoch,
        "seq": seq,
        "snapshot": {
            "desktop_connected": true,
            "server_time": chrono::Utc::now().to_rfc3339(),
            "backend_version": env!("CARGO_PKG_VERSION"),
            "capabilities": {
                "protocol_version": PROTOCOL_VERSION,
                "commands": commands,
                "events": events,
            },
        },
    })
}

fn new_stream_epoch() -> String {
    format!("epoch_{}", crate::features::remote_control::short_token(24))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_ENDPOINT_ID: &str = "ep_test";
    const TEST_LEASE_ID: &str = LEASE_ID_WIRE_PLACEHOLDER;

    fn record_stream_event(stream: &mut EventStreamState, event: &str, payload: Value) {
        stream
            .record(TEST_ENDPOINT_ID, TEST_LEASE_ID, event.to_string(), payload)
            .expect("record stream event");
    }

    fn replay_context<'a>(
        stream: &'a EventStreamState,
        commands: &'a [String],
        events: &'a [String],
    ) -> ReplayMessageContext<'a> {
        ReplayMessageContext {
            endpoint_id: TEST_ENDPOINT_ID,
            lease_id: TEST_LEASE_ID,
            stream_epoch: stream.epoch(),
            capability_commands: commands,
            capability_events: events,
        }
    }

    #[test]
    fn event_delivery_requires_current_web_subscription() {
        let mut stream = EventStreamState::new();
        stream.set_subscription("session:deleted", true);

        assert!(stream.is_subscribed("session:deleted"));
        assert!(!stream.is_subscribed("session:list_changed"));
    }

    #[test]
    fn replay_skips_unsubscribed_events_without_creating_sequence_gaps() {
        let mut stream = EventStreamState::new();
        record_stream_event(&mut stream, "session:deleted", json!({ "id": "one" }));
        record_stream_event(&mut stream, "session:list_changed", json!({}));
        record_stream_event(&mut stream, "chat:done", json!({ "id": "one" }));
        stream.set_subscription("session:deleted", true);
        stream.set_subscription("chat:done", true);
        let commands = Vec::new();
        let capabilities = Vec::new();

        let messages = stream
            .replay_messages_after(0, replay_context(&stream, &commands, &capabilities))
            .expect("replay");

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["type"], "event");
        assert_eq!(messages[0]["seq"], 1);
        assert_eq!(messages[0]["event"], "session:deleted");
        assert_eq!(messages[1]["type"], "desktop_snapshot");
        assert_eq!(messages[1]["seq"], 2);
        assert_eq!(messages[2]["type"], "event");
        assert_eq!(messages[2]["seq"], 3);
        assert_eq!(messages[2]["event"], "chat:done");
    }

    #[test]
    fn stream_replays_an_exact_contiguous_suffix() {
        let mut stream = EventStreamState::new();
        record_stream_event(&mut stream, "chat:delta", json!({ "text": "a" }));
        record_stream_event(&mut stream, "chat:delta", json!({ "text": "b" }));
        let replay = stream.replay_after(1).expect("replay");
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].seq, 2);
        assert_eq!(replay[0].payload["text"], "b");
        assert!(stream.replay_after(3).is_none());
    }

    #[test]
    fn fresh_client_baseline_skips_history_but_keeps_ready_window_events() {
        let mut stream = EventStreamState::new();
        record_stream_event(&mut stream, "chat:done", json!({ "turn": "historical" }));
        let baseline_at_join = stream.seq();
        record_stream_event(&mut stream, "chat:turn_started", json!({ "turn": "live" }));

        let replay = stream.replay_after(baseline_at_join).expect("fresh replay");
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].event, "chat:turn_started");
        assert_eq!(replay[0].payload["turn"], "live");
    }

    #[test]
    fn bounded_stream_rejects_a_cursor_older_than_the_journal() {
        let mut stream = EventStreamState::new();
        for seq in 0..=JOURNAL_CAPACITY {
            record_stream_event(&mut stream, "chat:delta", json!(seq));
        }
        assert!(stream.replay_after(0).is_none());
        assert!(stream.replay_after(1).is_some());
    }

    #[test]
    fn oversized_complete_event_frame_rotates_epoch_without_recording_the_event() {
        let mut stream = EventStreamState::new();
        record_stream_event(&mut stream, "chat:delta", json!({ "text": "safe" }));
        let previous_epoch = stream.epoch().to_string();
        let oversized_payload = json!({ "text": "x".repeat(400) });
        let test_wire_limit = 512;
        assert!(serde_json::to_vec(&oversized_payload).unwrap().len() < test_wire_limit);

        let error = stream
            .record_with_limits(
                TEST_ENDPOINT_ID,
                TEST_LEASE_ID,
                "chat:delta".into(),
                oversized_payload,
                test_wire_limit,
                JOURNAL_CAPACITY,
                JOURNAL_BYTES_CAPACITY,
            )
            .expect_err("the complete envelope, not just payload, must be bounded");
        assert!(matches!(
            error,
            StreamRecordError::Oversized {
                wire_bytes,
                limit: 512
            } if wire_bytes > 512
        ));
        assert_ne!(stream.epoch(), previous_epoch);
        assert_eq!(stream.seq(), 0);
        assert!(stream.journal.is_empty());
        assert_eq!(stream.journal_bytes, 0);
        assert!(stream
            .replay_after(0)
            .is_some_and(|events| events.is_empty()));

        stream
            .record_with_limits(
                TEST_ENDPOINT_ID,
                TEST_LEASE_ID,
                "chat:done".into(),
                json!({ "status": "failed" }),
                test_wire_limit,
                JOURNAL_CAPACITY,
                JOURNAL_BYTES_CAPACITY,
            )
            .expect("new epoch remains usable");
        let replay = stream.replay_after(0).expect("reconnect replay");
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].seq, 1);
        assert_eq!(replay[0].event, "chat:done");
    }

    #[test]
    fn stream_journal_evicts_oldest_complete_frames_by_total_bytes() {
        let mut stream = EventStreamState::new();
        stream
            .record_with_limits(
                TEST_ENDPOINT_ID,
                TEST_LEASE_ID,
                "chat:delta".into(),
                json!({ "text": "same-size" }),
                4 * 1024,
                JOURNAL_CAPACITY,
                usize::MAX,
            )
            .unwrap();
        let frame_bytes = stream.journal.back().unwrap().wire_bytes;
        let byte_capacity = frame_bytes * 2;
        for _ in 0..2 {
            stream
                .record_with_limits(
                    TEST_ENDPOINT_ID,
                    TEST_LEASE_ID,
                    "chat:delta".into(),
                    json!({ "text": "same-size" }),
                    4 * 1024,
                    JOURNAL_CAPACITY,
                    byte_capacity,
                )
                .unwrap();
        }

        assert_eq!(stream.seq(), 3);
        assert_eq!(stream.journal.len(), 2);
        assert_eq!(stream.journal.front().map(|event| event.seq), Some(2));
        assert!(stream.journal_bytes <= byte_capacity);
        assert!(stream.replay_after(0).is_none());
        assert_eq!(stream.replay_after(1).unwrap().len(), 2);
    }

    #[test]
    fn failed_live_enqueue_can_rebase_the_critical_tail_for_reconnect() {
        let mut stream = EventStreamState::new();
        record_stream_event(&mut stream, "chat:delta", json!({ "text": "prefix" }));
        record_stream_event(
            &mut stream,
            "chat:user_input_required",
            json!({ "request_id": "request-1" }),
        );
        let previous_epoch = stream.epoch().to_string();

        stream.rebase_tail(TEST_ENDPOINT_ID, TEST_LEASE_ID).unwrap();

        assert_ne!(stream.epoch(), previous_epoch);
        assert_eq!(stream.seq(), 1);
        let replay = stream.replay_after(0).unwrap();
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].event, "chat:user_input_required");
        assert_eq!(replay[0].payload["request_id"], "request-1");
    }

    #[test]
    fn replay_batch_failure_stops_at_the_gap_instead_of_claiming_the_tail() {
        let messages = vec![
            json!({ "seq": 1 }),
            json!({ "seq": 2 }),
            json!({ "seq": 3 }),
        ];
        let mut attempted = Vec::new();
        let complete = try_enqueue_message_batch(messages, |message| {
            let seq = message["seq"].as_u64().unwrap();
            attempted.push(seq);
            seq != 2
        });

        assert!(!complete);
        assert_eq!(attempted, vec![1, 2]);
        let reset = stream_reset_message(
            TEST_ENDPOINT_ID,
            TEST_LEASE_ID,
            "epoch_recovered",
            "replay_enqueue_failed",
        );
        assert_eq!(reset["seq"], 0);
        assert_eq!(reset["reason"], "replay_enqueue_failed");
        assert!(serde_json::to_vec(&reset).unwrap().len() < relay_client::MAX_RELAY_FRAME_BYTES);
    }
}
