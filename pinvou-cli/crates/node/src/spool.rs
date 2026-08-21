use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use pinvou_protocol::{SourceSpan, StreamId};
use pinvou_seglog::{AckRange, Config, Cursor, RecoveryIssue, SegmentLog};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransportRecord {
    pub seq: u64,
    pub source_span: SourceSpan,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawSpoolRecord {
    pub source_seq: u64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SpoolRecovery {
    pub control_raw: Option<RecoveryIssue>,
    pub main_raw: Option<RecoveryIssue>,
    pub control_mapping: Option<RecoveryIssue>,
    pub main_mapping: Option<RecoveryIssue>,
}

#[derive(Debug, Error)]
pub enum SpoolError {
    #[error("node spool storage failed: {0}")]
    Storage(#[from] pinvou_seglog::Error),
    #[error("node spool metadata is corrupt: {0}")]
    CorruptMetadata(#[from] serde_json::Error),
    #[error(
        "source span is not the next complete raw range: expected start {expected}, got {actual}"
    )]
    SourceGap { expected: u64, actual: u64 },
    #[error("source span end {end} exceeds durable raw source {durable}")]
    SourceBeyondDurable { end: u64, durable: u64 },
    #[error("ACK {requested} exceeds last sent transport sequence {sent}")]
    AckBeyondSent { requested: u64, sent: u64 },
    #[error("transport mapping is not contiguous: expected {expected}, got {actual}")]
    CorruptMapping { expected: u64, actual: u64 },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum MappingRecord {
    Transport(TransportRecord),
    Ack { seq: u64, source: u64 },
}

#[derive(Debug)]
struct StreamSpool {
    raw: SegmentLog,
    mapping: SegmentLog,
    transports: BTreeMap<u64, TransportRecord>,
    ack: u64,
    source_ack: u64,
    last_source_mapped: u64,
}

#[derive(Debug)]
pub struct NodeSpool {
    control: StreamSpool,
    main: StreamSpool,
    recovery: SpoolRecovery,
}

impl NodeSpool {
    pub fn open(
        directory: impl AsRef<Path>,
        node_id: &str,
        attachment_id: &str,
    ) -> Result<Self, SpoolError> {
        let root = directory.as_ref();
        let (control, control_raw, control_mapping) = StreamSpool::open(
            root.join("control"),
            node_id,
            attachment_id,
            StreamId::Control,
        )?;
        let (main, main_raw, main_mapping) =
            StreamSpool::open(root.join("main"), node_id, attachment_id, StreamId::Main)?;
        Ok(Self {
            control,
            main,
            recovery: SpoolRecovery {
                control_raw,
                main_raw,
                control_mapping,
                main_mapping,
            },
        })
    }

    pub fn recovery(&self) -> &SpoolRecovery {
        &self.recovery
    }

    pub fn append_raw_batch<I, B>(
        &mut self,
        stream_id: StreamId,
        payloads: I,
    ) -> Result<RangeInclusive<u64>, SpoolError>
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        let stream = self.stream_mut(stream_id);
        let cursors = stream.raw.append_batch(payloads)?;
        stream.raw.durable_barrier()?;
        Ok(cursors.start().get()..=cursors.end().get())
    }

    pub fn prepare_transport(
        &mut self,
        stream_id: StreamId,
        source_span: SourceSpan,
        payload: &[u8],
    ) -> Result<TransportRecord, SpoolError> {
        let stream = self.stream_mut(stream_id);
        let expected = stream.last_source_mapped.saturating_add(1);
        if source_span.start != expected || source_span.start > source_span.end {
            return Err(SpoolError::SourceGap {
                expected,
                actual: source_span.start,
            });
        }
        let durable = stream.raw.durable_cursor().map(Cursor::get).unwrap_or(0);
        if source_span.end > durable {
            return Err(SpoolError::SourceBeyondDurable {
                end: source_span.end,
                durable,
            });
        }
        let seq = stream
            .transports
            .last_key_value()
            .map_or(1, |(seq, _)| seq.saturating_add(1));
        let transport = TransportRecord {
            seq,
            source_span,
            payload: payload.to_vec(),
        };
        // The mapping barrier precedes returning the record to the sender.
        let encoded = serde_json::to_vec(&MappingRecord::Transport(transport.clone()))?;
        stream.mapping.append_batch([encoded.as_slice()])?;
        stream.mapping.durable_barrier()?;
        stream.last_source_mapped = source_span.end;
        stream.transports.insert(seq, transport.clone());
        Ok(transport)
    }

    pub fn apply_ack(&mut self, stream_id: StreamId, ack: u64) -> Result<(), SpoolError> {
        let stream = self.stream_mut(stream_id);
        if ack <= stream.ack {
            return Ok(());
        }
        let sent = stream
            .transports
            .last_key_value()
            .map_or(0, |(seq, _)| *seq);
        if ack > sent {
            return Err(SpoolError::AckBeyondSent {
                requested: ack,
                sent,
            });
        }
        let source = stream
            .transports
            .get(&ack)
            .expect("contiguous mapping checked while opening")
            .source_span
            .end;
        let encoded = serde_json::to_vec(&MappingRecord::Ack { seq: ack, source })?;
        stream.mapping.append_batch([encoded.as_slice()])?;
        stream.mapping.durable_barrier()?;
        stream.restore_raw_ack(source)?;
        stream.ack = ack;
        stream.source_ack = source;
        stream.raw.reclaim_acknowledged()?;
        Ok(())
    }

    pub fn apply_batch_ack(
        &mut self,
        control: Option<u64>,
        main: Option<u64>,
    ) -> Result<(), SpoolError> {
        if let Some(ack) = control {
            self.apply_ack(StreamId::Control, ack)?;
        }
        if let Some(ack) = main {
            self.apply_ack(StreamId::Main, ack)?;
        }
        Ok(())
    }

    pub fn replay_unacked(&self, stream_id: StreamId) -> Result<Vec<TransportRecord>, SpoolError> {
        let stream = self.stream(stream_id);
        Ok(stream
            .transports
            .range(stream.ack.saturating_add(1)..)
            .map(|(_, transport)| transport.clone())
            .collect())
    }

    /// Durable raw records that still need a transport mapping after recovery.
    pub fn replay_unmapped_raw(
        &self,
        stream_id: StreamId,
    ) -> Result<Vec<RawSpoolRecord>, SpoolError> {
        let stream = self.stream(stream_id);
        Ok(stream
            .raw
            .replay_from(Cursor::new(stream.last_source_mapped.saturating_add(1)))?
            .into_iter()
            .map(|record| RawSpoolRecord {
                source_seq: record.cursor.get(),
                payload: record.payload,
            })
            .collect())
    }

    pub fn durable_source_watermark(&self, stream_id: StreamId) -> u64 {
        self.stream(stream_id)
            .raw
            .durable_cursor()
            .map(Cursor::get)
            .unwrap_or(0)
    }

    pub fn ack_watermark(&self, stream_id: StreamId) -> u64 {
        self.stream(stream_id).ack
    }

    pub fn source_ack_watermark(&self, stream_id: StreamId) -> u64 {
        self.stream(stream_id).source_ack
    }

    fn stream(&self, stream_id: StreamId) -> &StreamSpool {
        match stream_id {
            StreamId::Control => &self.control,
            StreamId::Main => &self.main,
        }
    }

    fn stream_mut(&mut self, stream_id: StreamId) -> &mut StreamSpool {
        match stream_id {
            StreamId::Control => &mut self.control,
            StreamId::Main => &mut self.main,
        }
    }
}

impl StreamSpool {
    fn open(
        directory: PathBuf,
        node_id: &str,
        attachment_id: &str,
        stream_id: StreamId,
    ) -> Result<(Self, Option<RecoveryIssue>, Option<RecoveryIssue>), SpoolError> {
        let metadata =
            format!("node={node_id}\nattachment={attachment_id}\nstream={stream_id:?}\n");
        let raw_opened = SegmentLog::open(
            Config::new(directory.join("raw"))
                .with_stream_metadata(format!("raw-v1\n{metadata}").into_bytes()),
        )?;
        let mapping_opened = SegmentLog::open(
            Config::new(directory.join("mapping"))
                .with_stream_metadata(format!("mapping-v1\n{metadata}").into_bytes()),
        )?;
        let raw_issue = raw_opened.recovery.issue.clone();
        let mapping_issue = mapping_opened.recovery.issue.clone();
        let mut transports = BTreeMap::new();
        let mut ack = 0;
        let mut source_ack = 0;
        let mut last_source_mapped = 0;
        for record in mapping_opened.log.replay_from(Cursor::new(1))? {
            match serde_json::from_slice::<MappingRecord>(&record.payload)? {
                MappingRecord::Transport(transport) => {
                    let expected = transports.last_key_value().map_or(1, |(seq, _)| *seq + 1);
                    if transport.seq != expected {
                        return Err(SpoolError::CorruptMapping {
                            expected,
                            actual: transport.seq,
                        });
                    }
                    let expected_source = last_source_mapped + 1;
                    if transport.source_span.start != expected_source
                        || transport.source_span.start > transport.source_span.end
                    {
                        return Err(SpoolError::SourceGap {
                            expected: expected_source,
                            actual: transport.source_span.start,
                        });
                    }
                    last_source_mapped = transport.source_span.end;
                    transports.insert(transport.seq, transport);
                }
                MappingRecord::Ack { seq, source } => {
                    let sent = transports.last_key_value().map_or(0, |(seq, _)| *seq);
                    if seq < ack || seq > sent {
                        return Err(SpoolError::AckBeyondSent {
                            requested: seq,
                            sent,
                        });
                    }
                    let expected_source = transports.get(&seq).map_or(0, |tx| tx.source_span.end);
                    if source != expected_source {
                        return Err(SpoolError::CorruptMapping {
                            expected: expected_source,
                            actual: source,
                        });
                    }
                    ack = seq;
                    source_ack = source;
                }
            }
        }
        let mut stream = Self {
            raw: raw_opened.log,
            mapping: mapping_opened.log,
            transports,
            ack,
            source_ack,
            last_source_mapped,
        };
        let raw_durable = stream.raw.durable_cursor().map(Cursor::get).unwrap_or(0);
        if last_source_mapped > raw_durable {
            return Err(SpoolError::SourceBeyondDurable {
                end: last_source_mapped,
                durable: raw_durable,
            });
        }
        if source_ack != 0 {
            stream.restore_raw_ack(source_ack)?;
            stream.raw.reclaim_acknowledged()?;
        }
        Ok((stream, raw_issue, mapping_issue))
    }

    fn restore_raw_ack(&mut self, source: u64) -> Result<(), SpoolError> {
        let current = self.raw.acknowledged_cursor().map(Cursor::get).unwrap_or(0);
        if source > current {
            self.raw.acknowledge(AckRange::new(
                Cursor::new(current + 1),
                Cursor::new(source),
            )?)?;
        }
        Ok(())
    }
}
