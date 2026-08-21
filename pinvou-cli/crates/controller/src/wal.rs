use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use pinvou_protocol::{RuntimeEventEnvelope, StreamId};
use pinvou_seglog::{Config, Cursor, RecoveryIssue, SegmentLog};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const GROUP_COMMIT_MAX_EVENTS: usize = 16;
const GROUP_COMMIT_WINDOW: Duration = Duration::from_millis(5);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct StreamKey {
    node_id: String,
    attachment_id: String,
    stream_slot: u8,
}

#[derive(Debug, Default)]
struct StreamState {
    accepted: u64,
    durable: u64,
    events: BTreeMap<u64, Vec<u8>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BatchAck {
    pub node_id: String,
    pub attachment_id: String,
    pub control: Option<u64>,
    pub main: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IngestOutcome {
    Pending,
    Duplicate,
    Committed(Vec<BatchAck>),
}

#[derive(Debug, Error)]
pub enum WalError {
    #[error("controller WAL storage failed: {0}")]
    Storage(#[from] pinvou_seglog::Error),
    #[error("controller WAL event encoding failed: {0}")]
    Event(#[from] pinvou_protocol::EventSchemaError),
    #[error("transport sequence gap for {stream_id:?}: expected {expected}, got {actual}")]
    SequenceGap {
        stream_id: StreamId,
        expected: u64,
        actual: u64,
    },
    #[error("duplicate transport sequence {seq} has different event bytes")]
    ConflictingDuplicate { seq: u64 },
    #[error("controller WAL contains an invalid persisted event: {0}")]
    CorruptEvent(pinvou_protocol::EventSchemaError),
}

/// Policy layer over one physical seglog. Logical streams retain independent
/// sequence and ACK watermarks even though their bytes share a WAL.
#[derive(Debug)]
pub struct ControllerWal {
    log: SegmentLog,
    streams: BTreeMap<StreamKey, StreamState>,
    pending_count: usize,
    pending_since: Option<Duration>,
    pending_attachments: BTreeSet<(String, String)>,
    recovery_issue: Option<RecoveryIssue>,
}

impl ControllerWal {
    pub fn open(directory: impl AsRef<Path>) -> Result<Self, WalError> {
        let opened = SegmentLog::open(
            Config::new(directory.as_ref()).with_stream_metadata(b"controller-wal-v1".to_vec()),
        )?;
        let recovery_issue = opened.recovery.issue.clone();
        let mut streams: BTreeMap<StreamKey, StreamState> = BTreeMap::new();
        for record in opened.log.replay_from(Cursor::new(1))? {
            let event = RuntimeEventEnvelope::from_json_slice(&record.payload)
                .map_err(WalError::CorruptEvent)?;
            let key = event_key(&event);
            let state = streams.entry(key).or_default();
            let expected = state.durable.saturating_add(1);
            if event.seq() != expected {
                return Err(WalError::SequenceGap {
                    stream_id: event.stream_id(),
                    expected,
                    actual: event.seq(),
                });
            }
            state.events.insert(event.seq(), record.payload);
            state.accepted = event.seq();
            state.durable = event.seq();
        }
        Ok(Self {
            log: opened.log,
            streams,
            pending_count: 0,
            pending_since: None,
            pending_attachments: BTreeSet::new(),
            recovery_issue,
        })
    }

    pub fn ingest(
        &mut self,
        event: RuntimeEventEnvelope,
        now: Duration,
    ) -> Result<IngestOutcome, WalError> {
        let key = event_key(&event);
        let bytes = event.to_json_vec()?;
        let state = self.streams.entry(key.clone()).or_default();
        if event.seq() <= state.accepted {
            return if state.events.get(&event.seq()) == Some(&bytes) {
                Ok(IngestOutcome::Duplicate)
            } else {
                Err(WalError::ConflictingDuplicate { seq: event.seq() })
            };
        }
        let expected = state.accepted.saturating_add(1);
        if event.seq() != expected {
            return Err(WalError::SequenceGap {
                stream_id: event.stream_id(),
                expected,
                actual: event.seq(),
            });
        }
        self.log.append_batch([bytes.as_slice()])?;
        state.events.insert(event.seq(), bytes);
        state.accepted = event.seq();
        self.pending_count += 1;
        self.pending_since.get_or_insert(now);
        self.pending_attachments
            .insert((key.node_id, key.attachment_id));
        if self.pending_count >= GROUP_COMMIT_MAX_EVENTS {
            Ok(IngestOutcome::Committed(self.flush()?))
        } else {
            Ok(IngestOutcome::Pending)
        }
    }

    pub fn flush_due(&mut self, now: Duration) -> Result<Vec<BatchAck>, WalError> {
        let due = self
            .pending_since
            .is_some_and(|since| now.saturating_sub(since) >= GROUP_COMMIT_WINDOW);
        if due { self.flush() } else { Ok(Vec::new()) }
    }

    pub fn flush(&mut self) -> Result<Vec<BatchAck>, WalError> {
        if self.pending_count == 0 {
            return Ok(Vec::new());
        }
        self.log.durable_barrier()?;
        for state in self.streams.values_mut() {
            state.durable = state.accepted;
        }
        let attachments = std::mem::take(&mut self.pending_attachments);
        self.pending_count = 0;
        self.pending_since = None;
        Ok(attachments
            .into_iter()
            .map(|(node_id, attachment_id)| self.batch_ack(&node_id, &attachment_id))
            .collect())
    }

    pub fn durable_watermark(
        &self,
        node_id: &str,
        attachment_id: &str,
        stream_id: StreamId,
    ) -> Option<u64> {
        self.streams
            .get(&StreamKey {
                node_id: node_id.to_owned(),
                attachment_id: attachment_id.to_owned(),
                stream_slot: stream_slot(stream_id),
            })
            .and_then(|state| (state.durable != 0).then_some(state.durable))
    }

    pub fn recovery_issue(&self) -> Option<&RecoveryIssue> {
        self.recovery_issue.as_ref()
    }

    pub fn replay(
        &self,
        node_id: &str,
        attachment_id: &str,
        stream_id: StreamId,
        from_seq: u64,
    ) -> Result<Vec<RuntimeEventEnvelope>, WalError> {
        let Some(state) = self.streams.get(&StreamKey {
            node_id: node_id.to_owned(),
            attachment_id: attachment_id.to_owned(),
            stream_slot: stream_slot(stream_id),
        }) else {
            return Ok(Vec::new());
        };
        state
            .events
            .range(from_seq..=state.durable)
            .map(|(_, bytes)| {
                RuntimeEventEnvelope::from_json_slice(bytes).map_err(WalError::CorruptEvent)
            })
            .collect()
    }

    fn batch_ack(&self, node_id: &str, attachment_id: &str) -> BatchAck {
        BatchAck {
            node_id: node_id.to_owned(),
            attachment_id: attachment_id.to_owned(),
            control: self.durable_watermark(node_id, attachment_id, StreamId::Control),
            main: self.durable_watermark(node_id, attachment_id, StreamId::Main),
        }
    }
}

fn event_key(event: &RuntimeEventEnvelope) -> StreamKey {
    StreamKey {
        node_id: event.node_id().to_owned(),
        attachment_id: event.attachment_id().to_owned(),
        stream_slot: stream_slot(event.stream_id()),
    }
}

const fn stream_slot(stream_id: StreamId) -> u8 {
    match stream_id {
        StreamId::Control => 0,
        StreamId::Main => 1,
    }
}
