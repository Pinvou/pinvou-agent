//! A single-stream append-only segmented log.
//!
//! This crate deliberately owns only storage mechanics: record framing and CRC,
//! batched appends, durable barriers, prefix recovery, cursors, and reclamation
//! of fully acknowledged closed segments. Spool/WAL policy belongs to callers.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
pub const FORMAT_VERSION: u16 = 1;
pub const RECORD_VERSION: u16 = 1;

const MAGIC: &[u8; 8] = b"PSEGLOG\0";
const HEADER_PREFIX_LEN: usize = 34;
const DATA_KIND: u8 = 1;
const BARRIER_KIND: u8 = 2;
const SEGMENT_SUMMARY_KIND: u8 = 3;
const SUMMARY_PAYLOAD_LEN: usize = 32;
const DURABLE_SUMMARY_MAGIC: &[u8; 8] = b"PDSUM\0\0\0";
const DURABLE_SUMMARY_FILE: &str = "durable.summary";
const DURABLE_SUMMARY_BYTES: usize = 54;
const RECLAIM_JOURNAL_MAGIC: &[u8; 8] = b"PRECLM\0\0";
const RECLAIM_JOURNAL_FILE: &str = "reclaim.journal";
const FRAME_PREFIX_LEN: usize = 4;
const FRAME_CRC_LEN: usize = 4;
const FRAME_COMMON_BODY_LEN: usize = 11;
const MAX_METADATA_BYTES: usize = 64 * 1024;
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Cursor(u64);

impl Cursor {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AckRange {
    pub start: Cursor,
    pub end: Cursor,
}

impl AckRange {
    pub fn new(start: Cursor, end: Cursor) -> Result<Self, Error> {
        if start > end {
            return Err(Error::InvalidAckRange { start, end });
        }
        Ok(Self { start, end })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultPoint {
    AppendFrame,
    BarrierFrame,
    DurableSummary,
    ReclaimDelete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InjectedFailure {
    after_bytes: usize,
}

impl InjectedFailure {
    pub const fn after_bytes(after_bytes: usize) -> Self {
        Self { after_bytes }
    }
}

/// Per-instance I/O failure seam used by deterministic storage contract tests.
pub trait FaultInjector: fmt::Debug + Send + Sync {
    fn failure(&self, point: FaultPoint, requested_bytes: usize) -> Option<InjectedFailure>;
}

#[derive(Debug)]
struct NoFaults;

impl FaultInjector for NoFaults {
    fn failure(&self, _point: FaultPoint, _requested_bytes: usize) -> Option<InjectedFailure> {
        None
    }
}

#[derive(Clone)]
pub struct Config {
    directory: PathBuf,
    segment_target_bytes: u64,
    stream_metadata: Vec<u8>,
    fault_injector: Arc<dyn FaultInjector>,
}

impl fmt::Debug for Config {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Config")
            .field("directory", &self.directory)
            .field("segment_target_bytes", &self.segment_target_bytes)
            .field("stream_metadata", &self.stream_metadata)
            .finish_non_exhaustive()
    }
}

impl Config {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            segment_target_bytes: 8 * 1024 * 1024,
            stream_metadata: Vec::new(),
            fault_injector: Arc::new(NoFaults),
        }
    }

    pub fn with_segment_target_bytes(mut self, bytes: u64) -> Self {
        self.segment_target_bytes = bytes;
        self
    }

    pub fn with_stream_metadata(mut self, metadata: Vec<u8>) -> Self {
        self.stream_metadata = metadata;
        self
    }

    #[doc(hidden)]
    pub fn with_fault_injector(mut self, injector: Arc<dyn FaultInjector>) -> Self {
        self.fault_injector = injector;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Record {
    pub cursor: Cursor,
    pub record_version: u16,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryIssue {
    DurableSummaryRepaired {
        path: PathBuf,
        durable_through: Cursor,
    },
    UncommittedTail {
        first_cursor: Cursor,
        last_cursor: Cursor,
    },
    TruncatedRecord {
        path: PathBuf,
        offset: u64,
    },
    ChecksumMismatch {
        path: PathBuf,
        offset: u64,
    },
    InvalidRecord {
        path: PathBuf,
        offset: u64,
        detail: &'static str,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecoveryReport {
    pub issue: Option<RecoveryIssue>,
    pub durable_through: Option<Cursor>,
}

pub struct OpenedLog {
    pub log: SegmentLog,
    pub recovery: RecoveryReport,
}

impl fmt::Debug for OpenedLog {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenedLog")
            .field("log", &self.log)
            .field("recovery", &self.recovery)
            .finish()
    }
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    InvalidConfig(&'static str),
    InvalidHeader {
        path: PathBuf,
        detail: &'static str,
    },
    CorruptRecord {
        path: PathBuf,
        offset: u64,
        detail: &'static str,
    },
    UnsupportedFormat {
        path: PathBuf,
        found: u16,
        supported: u16,
    },
    UnsupportedRecordVersion {
        path: PathBuf,
        offset: u64,
        found: u16,
        supported: u16,
    },
    MetadataMismatch {
        path: PathBuf,
    },
    InvalidAckRange {
        start: Cursor,
        end: Cursor,
    },
    NonContiguousAck {
        expected: Cursor,
        actual: Cursor,
    },
    AckBeyondDurable {
        requested: Cursor,
        durable: Option<Cursor>,
    },
    CursorExhausted,
    PayloadTooLarge {
        bytes: usize,
    },
    Poisoned {
        operation: &'static str,
    },
    PoisonedIo {
        operation: &'static str,
        source: std::io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "seglog I/O error: {error}"),
            Self::InvalidConfig(detail) => write!(formatter, "invalid seglog config: {detail}"),
            Self::InvalidHeader { path, detail } => {
                write!(
                    formatter,
                    "invalid segment header {}: {detail}",
                    path.display()
                )
            }
            Self::CorruptRecord {
                path,
                offset,
                detail,
            } => write!(
                formatter,
                "corrupt segment record {} at byte {offset}: {detail}",
                path.display()
            ),
            Self::UnsupportedFormat {
                path,
                found,
                supported,
            } => write!(
                formatter,
                "unsupported segment format {found} in {}; reader supports {supported}",
                path.display()
            ),
            Self::UnsupportedRecordVersion {
                path,
                offset,
                found,
                supported,
            } => write!(
                formatter,
                "unsupported record version {found} in {} at byte {offset}; reader supports {supported}",
                path.display()
            ),
            Self::MetadataMismatch { path } => write!(
                formatter,
                "segment stream metadata does not match config: {}",
                path.display()
            ),
            Self::InvalidAckRange { start, end } => write!(
                formatter,
                "invalid acknowledgement range {}..={}",
                start.get(),
                end.get()
            ),
            Self::NonContiguousAck { expected, actual } => write!(
                formatter,
                "non-contiguous acknowledgement: expected {}, got {}",
                expected.get(),
                actual.get()
            ),
            Self::AckBeyondDurable { requested, durable } => write!(
                formatter,
                "acknowledgement {} exceeds durable cursor {:?}",
                requested.get(),
                durable.map(Cursor::get)
            ),
            Self::CursorExhausted => formatter.write_str("seglog cursor space exhausted"),
            Self::PayloadTooLarge { bytes } => {
                write!(
                    formatter,
                    "seglog record payload is too large: {bytes} bytes"
                )
            }
            Self::Poisoned { operation } => write!(
                formatter,
                "seglog instance is poisoned after failed {operation}; reopen required"
            ),
            Self::PoisonedIo { operation, source } => write!(
                formatter,
                "seglog {operation} failed and poisoned the instance: {source}"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::PoisonedIo { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug)]
pub struct SegmentLog {
    config: Config,
    segments: Vec<SegmentInfo>,
    active: File,
    next_cursor: u64,
    last_appended: Option<Cursor>,
    durable_cursor: Option<Cursor>,
    acknowledged_cursor: u64,
    dirty_segments: BTreeSet<PathBuf>,
    poisoned: Option<&'static str>,
}

#[derive(Clone, Debug)]
struct SegmentInfo {
    id: u64,
    path: PathBuf,
    header_len: u64,
    file_len: u64,
    first_cursor: u64,
    max_cursor: Option<Cursor>,
    record_count: u64,
}

#[derive(Debug)]
struct Header {
    segment_id: u64,
    first_cursor: u64,
    metadata: Vec<u8>,
    encoded_len: u64,
}

#[derive(Debug)]
struct ScanResult {
    segment: SegmentInfo,
    data_records: Vec<RecordLocation>,
    barriers: Vec<BarrierLocation>,
    issue: Option<RecoveryIssue>,
}

#[derive(Clone, Copy, Debug)]
struct RecordLocation {
    cursor: Cursor,
}

#[derive(Clone, Debug)]
struct BarrierLocation {
    cursor: Option<Cursor>,
    path: PathBuf,
    end_offset: u64,
    durable_summary: DurableSummary,
}

#[derive(Clone, Copy, Debug)]
struct DurableSummary {
    segment_id: u64,
    segment_len: u64,
    data_end: u64,
    durable_cursor: Cursor,
    record_count: u64,
}

impl SegmentLog {
    pub fn open(config: Config) -> Result<OpenedLog, Error> {
        validate_config(&config)?;
        fs::create_dir_all(&config.directory)?;
        apply_reclaim_journal(&config.directory)?;
        let mut paths = segment_paths(&config.directory)?;
        if paths.is_empty() {
            let path = create_segment(&config, 0, 1)?;
            paths.push((0, path));
        }

        let mut scans = Vec::with_capacity(paths.len());
        let mut first_issue = None;
        let mut expected_cursor = None;
        let mut last_barrier = None;
        let mut highest_data_cursor = None;
        for (position, (expected_id, path)) in paths.iter().enumerate() {
            let header = read_header(path)?;
            if header.segment_id != *expected_id {
                return Err(Error::InvalidHeader {
                    path: path.clone(),
                    detail: "segment id does not match file name",
                });
            }
            if header.metadata != config.stream_metadata {
                return Err(Error::MetadataMismatch { path: path.clone() });
            }
            if position == 0 {
                expected_cursor = Some(header.first_cursor);
            } else if expected_cursor != Some(header.first_cursor) {
                return Err(Error::InvalidHeader {
                    path: path.clone(),
                    detail: "segment first cursor is not contiguous",
                });
            }
            let scan = scan_segment(path, &header, expected_cursor.unwrap())?;
            for record in &scan.data_records {
                highest_data_cursor = Some(record.cursor);
                expected_cursor = Some(
                    record
                        .cursor
                        .get()
                        .checked_add(1)
                        .ok_or(Error::CursorExhausted)?,
                );
            }
            if let Some(barrier) = scan.barriers.last() {
                last_barrier = Some(barrier.clone());
            }
            if first_issue.is_none() {
                first_issue = scan.issue.clone();
            }
            scans.push(scan);
            if first_issue.is_some() {
                break;
            }
        }

        if let Some(summary) = read_durable_summary(&config.directory)?
            && summary.durable_cursor.get() != 0
        {
            let observed_durable = last_barrier.as_ref().and_then(|barrier| barrier.cursor);
            if observed_durable.is_none_or(|cursor| cursor < summary.durable_cursor) {
                let summary_scan = scans
                    .iter()
                    .find(|scan| scan.segment.id == summary.segment_id);
                let clean_boundary = first_issue.is_none()
                    && summary_scan.is_some_and(|scan| scan.segment.file_len == summary.data_end);
                let damaged_barrier_boundary = first_issue.as_ref().is_some_and(|issue| {
                    recovery_issue_at(
                        issue,
                        summary_scan.map(|scan| scan.segment.path.as_path()),
                        summary.data_end,
                    )
                });
                let recoverable = highest_data_cursor == Some(summary.durable_cursor)
                    && summary_scan.is_some_and(|scan| {
                        scan.segment.record_count == summary.record_count
                            && (clean_boundary || damaged_barrier_boundary)
                    });
                if !recoverable && first_issue.is_none() {
                    return Err(Error::CorruptRecord {
                        path: config.directory.join(DURABLE_SUMMARY_FILE),
                        offset: 0,
                        detail: "durable summary references missing or corrupt data",
                    });
                }
                if recoverable {
                    let scan = summary_scan.expect("recoverable summary segment exists");
                    let payload = encode_segment_summary(&scan.segment, summary.data_end);
                    let frame = encode_frame(BARRIER_KIND, 0, summary.durable_cursor, &payload)?;
                    if summary.data_end + frame.len() as u64 != summary.segment_len {
                        return Err(Error::CorruptRecord {
                            path: config.directory.join(DURABLE_SUMMARY_FILE),
                            offset: 0,
                            detail: "durable summary length does not match barrier framing",
                        });
                    }
                    let truncate_file = OpenOptions::new().write(true).open(&scan.segment.path)?;
                    truncate_file.set_len(summary.data_end)?;
                    truncate_file.sync_all()?;
                    drop(truncate_file);
                    let mut file = OpenOptions::new().append(true).open(&scan.segment.path)?;
                    file.write_all(&frame)?;
                    file.sync_all()?;
                    let repaired_path = scan.segment.path.clone();
                    let mut reopened = Self::open(config)?;
                    reopened.recovery.issue = Some(RecoveryIssue::DurableSummaryRepaired {
                        path: repaired_path,
                        durable_through: summary.durable_cursor,
                    });
                    return Ok(reopened);
                }
            }
        }

        let durable_cursor = last_barrier.as_ref().and_then(|barrier| barrier.cursor);
        let uncommitted = match (highest_data_cursor, durable_cursor) {
            (Some(highest), Some(durable)) if highest > durable => {
                Some((Cursor::new(durable.get() + 1), highest))
            }
            (Some(highest), None) => {
                let first = scans
                    .first()
                    .map(|scan| Cursor::new(scan.segment.first_cursor))
                    .unwrap_or(Cursor::new(1));
                Some((first, highest))
            }
            _ => None,
        };
        let recovery_issue = first_issue.or_else(|| {
            uncommitted.map(
                |(first_cursor, last_cursor)| RecoveryIssue::UncommittedTail {
                    first_cursor,
                    last_cursor,
                },
            )
        });
        if recovery_issue.is_some() || scans.len() < paths.len() {
            repair_to_last_barrier(&paths, &scans, last_barrier.as_ref())?;
            let repaired_summary = last_barrier
                .as_ref()
                .map(|barrier| barrier.durable_summary)
                .unwrap_or_else(|| {
                    let first = &scans[0].segment;
                    DurableSummary {
                        segment_id: first.id,
                        segment_len: first.header_len,
                        data_end: first.header_len,
                        durable_cursor: Cursor::new(0),
                        record_count: 0,
                    }
                });
            persist_durable_summary(&config.directory, repaired_summary)?;
        } else if let Some(barrier) = &last_barrier {
            let persisted = read_durable_summary(&config.directory)?;
            if persisted.map(|summary| summary.durable_cursor) != barrier.cursor {
                persist_durable_summary(&config.directory, barrier.durable_summary)?;
            }
        }

        let paths = segment_paths(&config.directory)?;
        let mut segments = Vec::with_capacity(paths.len());
        for (_, path) in &paths {
            let header = read_header(path)?;
            let scan = scan_segment(path, &header, header.first_cursor)?;
            segments.push(scan.segment);
        }
        let active_path = segments
            .last()
            .expect("opening always creates or retains a segment")
            .path
            .clone();
        let active = OpenOptions::new()
            .read(true)
            .append(true)
            .open(active_path)?;
        let first_retained = segments
            .first()
            .map(|segment| segment.first_cursor)
            .unwrap_or(1);
        let next_cursor = durable_cursor
            .map(|cursor| cursor.get().checked_add(1).ok_or(Error::CursorExhausted))
            .transpose()?
            .unwrap_or(first_retained);

        Ok(OpenedLog {
            log: Self {
                config,
                segments,
                active,
                next_cursor,
                last_appended: durable_cursor,
                durable_cursor,
                acknowledged_cursor: first_retained.saturating_sub(1),
                dirty_segments: BTreeSet::new(),
                poisoned: None,
            },
            recovery: RecoveryReport {
                issue: recovery_issue,
                durable_through: durable_cursor,
            },
        })
    }

    pub fn append_batch<I, B>(&mut self, payloads: I) -> Result<RangeInclusive<Cursor>, Error>
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        self.ensure_healthy()?;
        let mut first = None;
        let mut last = None;
        for payload in payloads {
            let payload = payload.as_ref();
            let cursor = Cursor::new(self.next_cursor);
            let frame = encode_frame(DATA_KIND, RECORD_VERSION, cursor, payload)?;
            self.rotate_if_needed(frame.len() as u64)?;
            self.write_active_frame(FaultPoint::AppendFrame, &frame, "append")?;
            let segment = self.segments.last_mut().expect("an active segment exists");
            segment.file_len += frame.len() as u64;
            segment.max_cursor = Some(cursor);
            segment.record_count += 1;
            self.dirty_segments.insert(segment.path.clone());
            self.next_cursor = self
                .next_cursor
                .checked_add(1)
                .ok_or(Error::CursorExhausted)?;
            self.last_appended = Some(cursor);
            first.get_or_insert(cursor);
            last = Some(cursor);
        }
        match (first, last) {
            (Some(first), Some(last)) => Ok(first..=last),
            _ => Err(Error::InvalidConfig(
                "append_batch requires at least one record",
            )),
        }
    }

    pub fn durable_barrier(&mut self) -> Result<Option<Cursor>, Error> {
        self.ensure_healthy()?;
        if self.dirty_segments.is_empty() {
            return Ok(self.durable_cursor);
        }
        for path in self.dirty_segments.clone() {
            let result = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .and_then(|file| file.sync_all());
            if let Err(error) = result {
                return Err(self.poison_io("barrier sync", error));
            }
        }
        let cursor = self.last_appended;
        let barrier_cursor = cursor.unwrap_or(Cursor::new(0));
        let active = self.segments.last().expect("an active segment exists");
        let data_end = active.file_len;
        let payload = encode_segment_summary(active, data_end);
        let frame = encode_frame(BARRIER_KIND, 0, barrier_cursor, &payload)?;
        self.write_active_frame(FaultPoint::BarrierFrame, &frame, "barrier write")?;
        if let Err(error) = self.active.sync_all() {
            return Err(self.poison_io("barrier sync", error));
        }
        let active = self.segments.last_mut().expect("an active segment exists");
        active.file_len += frame.len() as u64;
        let summary = DurableSummary {
            segment_id: active.id,
            segment_len: active.file_len,
            data_end,
            durable_cursor: barrier_cursor,
            record_count: active.record_count,
        };
        let injected_failure = self
            .config
            .fault_injector
            .failure(FaultPoint::DurableSummary, DURABLE_SUMMARY_BYTES);
        if let Err(error) =
            persist_durable_summary_with_failure(&self.config.directory, summary, injected_failure)
        {
            return Err(self.poison_io("durable summary", error));
        }
        self.dirty_segments.clear();
        self.durable_cursor = cursor;
        Ok(cursor)
    }

    pub fn durable_cursor(&self) -> Option<Cursor> {
        self.durable_cursor
    }

    pub fn replay_from(&self, start: Cursor) -> Result<Vec<Record>, Error> {
        let Some(durable) = self.durable_cursor else {
            return Ok(Vec::new());
        };
        if start > durable {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        for segment in &self.segments {
            read_records(&segment.path, start, durable, &mut records)?;
        }
        Ok(records)
    }

    pub fn acknowledge(&mut self, range: AckRange) -> Result<Cursor, Error> {
        let expected = Cursor::new(
            self.acknowledged_cursor
                .checked_add(1)
                .ok_or(Error::CursorExhausted)?,
        );
        if range.start != expected {
            return Err(Error::NonContiguousAck {
                expected,
                actual: range.start,
            });
        }
        if self
            .durable_cursor
            .is_none_or(|durable| range.end > durable)
        {
            return Err(Error::AckBeyondDurable {
                requested: range.end,
                durable: self.durable_cursor,
            });
        }
        self.acknowledged_cursor = range.end.get();
        Ok(range.end)
    }

    pub fn acknowledged_cursor(&self) -> Option<Cursor> {
        (self.acknowledged_cursor != 0).then(|| Cursor::new(self.acknowledged_cursor))
    }

    pub fn reclaim_acknowledged(&mut self) -> Result<Vec<PathBuf>, Error> {
        let active_id = self.segments.last().expect("an active segment exists").id;
        let mut reclaimed = Vec::new();
        let candidates: Vec<_> = self
            .segments
            .iter()
            .filter(|segment| {
                segment.id != active_id
                    && segment
                        .max_cursor
                        .is_some_and(|cursor| cursor.get() <= self.acknowledged_cursor)
            })
            .map(|segment| segment.path.clone())
            .collect();
        if candidates.is_empty() {
            return Ok(reclaimed);
        }
        let retained_floor_segment = self
            .segments
            .iter()
            .filter(|segment| !candidates.contains(&segment.path))
            .min_by_key(|segment| segment.id)
            .expect("the active segment is always retained");
        if read_durable_summary(&self.config.directory)?.is_some_and(|summary| {
            candidates
                .iter()
                .any(|path| *path == segment_path(&self.config.directory, summary.segment_id))
        }) {
            // Commit the retained-floor interpretation before the deletion journal.
            // If the process stops between these two publishes, reopening can still
            // rediscover the old barrier and recreate its summary. The reverse order
            // could leave a durable summary pointing at a segment the journal deletes.
            persist_durable_summary(
                &self.config.directory,
                DurableSummary {
                    segment_id: retained_floor_segment.id,
                    segment_len: retained_floor_segment.header_len,
                    data_end: retained_floor_segment.header_len,
                    durable_cursor: Cursor::new(0),
                    record_count: 0,
                },
            )?;
        }
        persist_reclaim_journal(&self.config.directory, retained_floor_segment.id)?;
        for path in candidates {
            if self
                .config
                .fault_injector
                .failure(FaultPoint::ReclaimDelete, 0)
                .is_some()
            {
                return Err(Error::Io(std::io::Error::other(
                    "injected reclaim delete failure",
                )));
            }
            fs::remove_file(&path)?;
            self.segments.retain(|segment| segment.path != path);
            self.dirty_segments.remove(&path);
            reclaimed.push(path);
        }
        Ok(reclaimed)
    }

    fn ensure_healthy(&self) -> Result<(), Error> {
        match self.poisoned {
            Some(operation) => Err(Error::Poisoned { operation }),
            None => Ok(()),
        }
    }

    fn poison_io(&mut self, operation: &'static str, source: std::io::Error) -> Error {
        self.poisoned = Some(operation);
        Error::PoisonedIo { operation, source }
    }

    fn write_active_frame(
        &mut self,
        point: FaultPoint,
        frame: &[u8],
        operation: &'static str,
    ) -> Result<(), Error> {
        if let Some(failure) = self.config.fault_injector.failure(point, frame.len()) {
            let prefix = failure.after_bytes.min(frame.len());
            if prefix > 0 {
                if let Err(error) = self.active.write_all(&frame[..prefix]) {
                    return Err(self.poison_io(operation, error));
                }
            }
            return Err(self.poison_io(operation, std::io::Error::other("injected I/O failure")));
        }
        if let Err(error) = self.active.write_all(frame) {
            return Err(self.poison_io(operation, error));
        }
        Ok(())
    }

    fn rotate_if_needed(&mut self, incoming_bytes: u64) -> Result<(), Error> {
        let active = self.segments.last().expect("an active segment exists");
        if active.max_cursor.is_none()
            || active.file_len + incoming_bytes <= self.config.segment_target_bytes
        {
            return Ok(());
        }
        let next_id = active.id.checked_add(1).ok_or(Error::CursorExhausted)?;
        let active_snapshot = active.clone();
        let summary_payload = encode_segment_summary(&active_snapshot, active_snapshot.file_len);
        let summary_cursor = active_snapshot.max_cursor.unwrap_or(Cursor::new(0));
        let summary_frame =
            encode_frame(SEGMENT_SUMMARY_KIND, 0, summary_cursor, &summary_payload)?;
        self.write_active_frame(FaultPoint::AppendFrame, &summary_frame, "segment summary")?;
        self.segments
            .last_mut()
            .expect("an active segment exists")
            .file_len += summary_frame.len() as u64;
        self.dirty_segments.insert(active_snapshot.path.clone());
        let path = match create_segment(&self.config, next_id, self.next_cursor) {
            Ok(path) => path,
            Err(error) => {
                self.poisoned = Some("segment rotation");
                return Err(error);
            }
        };
        self.active = match OpenOptions::new().read(true).append(true).open(&path) {
            Ok(file) => file,
            Err(error) => return Err(self.poison_io("segment rotation", error)),
        };
        let header = read_header(&path)?;
        self.segments.push(SegmentInfo {
            id: next_id,
            path,
            header_len: header.encoded_len,
            file_len: header.encoded_len,
            first_cursor: self.next_cursor,
            max_cursor: None,
            record_count: 0,
        });
        Ok(())
    }
}

fn validate_config(config: &Config) -> Result<(), Error> {
    if config.segment_target_bytes == 0 {
        return Err(Error::InvalidConfig(
            "segment_target_bytes must be non-zero",
        ));
    }
    if config.stream_metadata.len() > MAX_METADATA_BYTES {
        return Err(Error::InvalidConfig("stream metadata exceeds 64 KiB"));
    }
    Ok(())
}

fn segment_paths(directory: &Path) -> Result<Vec<(u64, PathBuf)>, Error> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("pseg") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let Ok(id) = stem.parse::<u64>() else {
            continue;
        };
        paths.push((id, path));
    }
    paths.sort_by_key(|(id, _)| *id);
    Ok(paths)
}

fn segment_path(directory: &Path, id: u64) -> PathBuf {
    directory.join(format!("{id:020}.pseg"))
}

fn create_segment(config: &Config, id: u64, first_cursor: u64) -> Result<PathBuf, Error> {
    let path = segment_path(&config.directory, id);
    if path.exists() {
        return Err(Error::InvalidConfig(
            "refusing to replace an existing segment",
        ));
    }
    let temporary = temporary_path(&config.directory, &format!("segment-{id:020}"));
    let bytes = encode_header(id, first_cursor, &config.stream_metadata)?;
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    atomic_publish(&temporary, &path, false)?;
    Ok(path)
}

fn encode_segment_summary(segment: &SegmentInfo, data_end: u64) -> [u8; SUMMARY_PAYLOAD_LEN] {
    let mut payload = [0_u8; SUMMARY_PAYLOAD_LEN];
    payload[0..8].copy_from_slice(&segment.record_count.to_le_bytes());
    payload[8..16].copy_from_slice(&segment.first_cursor.to_le_bytes());
    payload[16..24].copy_from_slice(
        &segment
            .max_cursor
            .map(Cursor::get)
            .unwrap_or(0)
            .to_le_bytes(),
    );
    payload[24..32].copy_from_slice(&data_end.to_le_bytes());
    payload
}

fn persist_durable_summary(directory: &Path, summary: DurableSummary) -> std::io::Result<()> {
    persist_durable_summary_with_failure(directory, summary, None)
}

fn persist_durable_summary_with_failure(
    directory: &Path,
    summary: DurableSummary,
    injected_failure: Option<InjectedFailure>,
) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(DURABLE_SUMMARY_BYTES);
    bytes.extend_from_slice(DURABLE_SUMMARY_MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&summary.segment_id.to_le_bytes());
    bytes.extend_from_slice(&summary.segment_len.to_le_bytes());
    bytes.extend_from_slice(&summary.data_end.to_le_bytes());
    bytes.extend_from_slice(&summary.durable_cursor.get().to_le_bytes());
    bytes.extend_from_slice(&summary.record_count.to_le_bytes());
    let checksum = crc32(&bytes);
    bytes.extend_from_slice(&checksum.to_le_bytes());

    let target = directory.join(DURABLE_SUMMARY_FILE);
    let temporary = temporary_path(directory, "durable-summary");
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        if let Some(failure) = injected_failure {
            let prefix = failure.after_bytes.min(bytes.len());
            file.write_all(&bytes[..prefix])?;
            file.sync_all()?;
            return Err(std::io::Error::other(
                "injected partial durable summary write",
            ));
        }
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    atomic_publish(&temporary, &target, true)
}

fn read_durable_summary(directory: &Path) -> Result<Option<DurableSummary>, Error> {
    let path = directory.join(DURABLE_SUMMARY_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::Io(error)),
    };
    if bytes.len() != 54 || &bytes[..8] != DURABLE_SUMMARY_MAGIC {
        return Err(Error::InvalidHeader {
            path,
            detail: "invalid durable summary framing",
        });
    }
    let version = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
    if version != FORMAT_VERSION {
        return Err(Error::UnsupportedFormat {
            path,
            found: version,
            supported: FORMAT_VERSION,
        });
    }
    let stored_crc = u32::from_le_bytes(bytes[50..54].try_into().unwrap());
    if crc32(&bytes[..50]) != stored_crc {
        return Err(Error::InvalidHeader {
            path,
            detail: "durable summary checksum mismatch",
        });
    }
    Ok(Some(DurableSummary {
        segment_id: u64::from_le_bytes(bytes[10..18].try_into().unwrap()),
        segment_len: u64::from_le_bytes(bytes[18..26].try_into().unwrap()),
        data_end: u64::from_le_bytes(bytes[26..34].try_into().unwrap()),
        durable_cursor: Cursor::new(u64::from_le_bytes(bytes[34..42].try_into().unwrap())),
        record_count: u64::from_le_bytes(bytes[42..50].try_into().unwrap()),
    }))
}

fn persist_reclaim_journal(directory: &Path, retained_floor: u64) -> Result<(), Error> {
    let mut bytes = Vec::with_capacity(22);
    bytes.extend_from_slice(RECLAIM_JOURNAL_MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&retained_floor.to_le_bytes());
    let checksum = crc32(&bytes);
    bytes.extend_from_slice(&checksum.to_le_bytes());
    let target = directory.join(RECLAIM_JOURNAL_FILE);
    let temporary = temporary_path(directory, "reclaim-journal");
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    atomic_publish(&temporary, &target, true)?;
    Ok(())
}

fn apply_reclaim_journal(directory: &Path) -> Result<(), Error> {
    let journal = directory.join(RECLAIM_JOURNAL_FILE);
    let bytes = match fs::read(&journal) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(Error::Io(error)),
    };
    if bytes.len() != 22 || &bytes[..8] != RECLAIM_JOURNAL_MAGIC {
        return Err(Error::InvalidHeader {
            path: journal,
            detail: "invalid reclaim journal framing",
        });
    }
    let version = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
    if version != FORMAT_VERSION {
        return Err(Error::UnsupportedFormat {
            path: journal,
            found: version,
            supported: FORMAT_VERSION,
        });
    }
    let stored_crc = u32::from_le_bytes(bytes[18..22].try_into().unwrap());
    if crc32(&bytes[..18]) != stored_crc {
        return Err(Error::InvalidHeader {
            path: journal,
            detail: "reclaim journal checksum mismatch",
        });
    }
    let retained_floor = u64::from_le_bytes(bytes[10..18].try_into().unwrap());
    for (id, path) in segment_paths(directory)? {
        if id < retained_floor {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(Error::Io(error)),
            }
        }
    }
    Ok(())
}

fn temporary_path(directory: &Path, label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    directory.join(format!(".{label}.tmp-{}-{nonce}", std::process::id()))
}

#[cfg(unix)]
fn atomic_publish(temporary: &Path, target: &Path, _replace: bool) -> std::io::Result<()> {
    fs::rename(temporary, target)?;
    File::open(target.parent().expect("published path has a parent"))?.sync_all()
}

#[cfg(windows)]
fn atomic_publish(temporary: &Path, target: &Path, replace: bool) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let flags = MOVEFILE_WRITE_THROUGH
        | if replace {
            MOVEFILE_REPLACE_EXISTING
        } else {
            0
        };
    // SAFETY: both paths are valid NUL-terminated UTF-16 buffers for the call duration.
    let moved = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn encode_header(id: u64, first_cursor: u64, metadata: &[u8]) -> Result<Vec<u8>, Error> {
    let metadata_len = u32::try_from(metadata.len())
        .map_err(|_| Error::InvalidConfig("stream metadata length does not fit u32"))?;
    let total_len = HEADER_PREFIX_LEN
        .checked_add(metadata.len())
        .and_then(|value| value.checked_add(4))
        .ok_or(Error::InvalidConfig("segment header length overflow"))?;
    let mut bytes = Vec::with_capacity(total_len);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(total_len as u32).to_le_bytes());
    bytes.extend_from_slice(&id.to_le_bytes());
    bytes.extend_from_slice(&first_cursor.to_le_bytes());
    bytes.extend_from_slice(&metadata_len.to_le_bytes());
    bytes.extend_from_slice(metadata);
    let checksum = crc32(&bytes);
    bytes.extend_from_slice(&checksum.to_le_bytes());
    Ok(bytes)
}

fn read_header(path: &Path) -> Result<Header, Error> {
    let mut file = File::open(path)?;
    let mut prefix = [0_u8; HEADER_PREFIX_LEN];
    file.read_exact(&mut prefix)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::UnexpectedEof => Error::InvalidHeader {
                path: path.to_path_buf(),
                detail: "truncated header",
            },
            _ => Error::Io(error),
        })?;
    if &prefix[..8] != MAGIC {
        return Err(Error::InvalidHeader {
            path: path.to_path_buf(),
            detail: "magic mismatch",
        });
    }
    let format_version = u16::from_le_bytes(prefix[8..10].try_into().unwrap());
    if format_version != FORMAT_VERSION {
        return Err(Error::UnsupportedFormat {
            path: path.to_path_buf(),
            found: format_version,
            supported: FORMAT_VERSION,
        });
    }
    let header_len = u32::from_le_bytes(prefix[10..14].try_into().unwrap()) as usize;
    let metadata_len = u32::from_le_bytes(prefix[30..34].try_into().unwrap()) as usize;
    let expected_len = HEADER_PREFIX_LEN
        .checked_add(metadata_len)
        .and_then(|value| value.checked_add(4))
        .ok_or(Error::InvalidHeader {
            path: path.to_path_buf(),
            detail: "header length overflow",
        })?;
    if header_len != expected_len || metadata_len > MAX_METADATA_BYTES {
        return Err(Error::InvalidHeader {
            path: path.to_path_buf(),
            detail: "invalid encoded header length",
        });
    }
    let mut remainder = vec![0_u8; metadata_len + 4];
    file.read_exact(&mut remainder)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::UnexpectedEof => Error::InvalidHeader {
                path: path.to_path_buf(),
                detail: "truncated metadata or header checksum",
            },
            _ => Error::Io(error),
        })?;
    let mut checksummed = prefix.to_vec();
    checksummed.extend_from_slice(&remainder[..metadata_len]);
    let expected_crc = u32::from_le_bytes(remainder[metadata_len..].try_into().unwrap());
    if crc32(&checksummed) != expected_crc {
        return Err(Error::InvalidHeader {
            path: path.to_path_buf(),
            detail: "header checksum mismatch",
        });
    }
    Ok(Header {
        segment_id: u64::from_le_bytes(prefix[14..22].try_into().unwrap()),
        first_cursor: u64::from_le_bytes(prefix[22..30].try_into().unwrap()),
        metadata: remainder[..metadata_len].to_vec(),
        encoded_len: header_len as u64,
    })
}

fn encode_frame(
    kind: u8,
    record_version: u16,
    cursor: Cursor,
    payload: &[u8],
) -> Result<Vec<u8>, Error> {
    let body_len =
        FRAME_COMMON_BODY_LEN
            .checked_add(payload.len())
            .ok_or(Error::PayloadTooLarge {
                bytes: payload.len(),
            })?;
    if body_len > MAX_FRAME_BYTES || u32::try_from(body_len).is_err() {
        return Err(Error::PayloadTooLarge {
            bytes: payload.len(),
        });
    }
    let mut bytes = Vec::with_capacity(FRAME_PREFIX_LEN + body_len + FRAME_CRC_LEN);
    bytes.extend_from_slice(&(body_len as u32).to_le_bytes());
    let body_start = bytes.len();
    bytes.push(kind);
    bytes.extend_from_slice(&record_version.to_le_bytes());
    bytes.extend_from_slice(&cursor.get().to_le_bytes());
    bytes.extend_from_slice(payload);
    let checksum = crc32(&bytes[body_start..]);
    bytes.extend_from_slice(&checksum.to_le_bytes());
    Ok(bytes)
}

fn scan_segment(
    path: &Path,
    header: &Header,
    mut expected_cursor: u64,
) -> Result<ScanResult, Error> {
    let bytes = fs::read(path)?;
    let mut offset = header.encoded_len as usize;
    let mut records = Vec::new();
    let mut barriers = Vec::new();
    let mut issue = None;
    while offset < bytes.len() {
        let frame_offset = offset;
        if bytes.len() - offset < FRAME_PREFIX_LEN {
            issue = Some(RecoveryIssue::TruncatedRecord {
                path: path.to_path_buf(),
                offset: frame_offset as u64,
            });
            break;
        }
        let body_len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += FRAME_PREFIX_LEN;
        if !(FRAME_COMMON_BODY_LEN..=MAX_FRAME_BYTES).contains(&body_len) {
            issue = Some(RecoveryIssue::InvalidRecord {
                path: path.to_path_buf(),
                offset: frame_offset as u64,
                detail: "invalid frame length",
            });
            break;
        }
        let frame_end = match offset
            .checked_add(body_len)
            .and_then(|value| value.checked_add(FRAME_CRC_LEN))
        {
            Some(end) if end <= bytes.len() => end,
            _ => {
                issue = Some(RecoveryIssue::TruncatedRecord {
                    path: path.to_path_buf(),
                    offset: frame_offset as u64,
                });
                break;
            }
        };
        let body = &bytes[offset..offset + body_len];
        let stored_crc =
            u32::from_le_bytes(bytes[offset + body_len..frame_end].try_into().unwrap());
        if crc32(body) != stored_crc {
            issue = Some(RecoveryIssue::ChecksumMismatch {
                path: path.to_path_buf(),
                offset: frame_offset as u64,
            });
            break;
        }
        let kind = body[0];
        let version = u16::from_le_bytes(body[1..3].try_into().unwrap());
        let cursor = Cursor::new(u64::from_le_bytes(body[3..11].try_into().unwrap()));
        match kind {
            DATA_KIND if version == RECORD_VERSION && cursor.get() == expected_cursor => {
                records.push(RecordLocation { cursor });
                expected_cursor = expected_cursor
                    .checked_add(1)
                    .ok_or(Error::CursorExhausted)?;
            }
            DATA_KIND if version != RECORD_VERSION => {
                return Err(Error::UnsupportedRecordVersion {
                    path: path.to_path_buf(),
                    offset: frame_offset as u64,
                    found: version,
                    supported: RECORD_VERSION,
                });
            }
            DATA_KIND => {
                issue = Some(RecoveryIssue::InvalidRecord {
                    path: path.to_path_buf(),
                    offset: frame_offset as u64,
                    detail: "non-contiguous record cursor",
                });
                break;
            }
            BARRIER_KIND
                if version == 0 && body_len == FRAME_COMMON_BODY_LEN + SUMMARY_PAYLOAD_LEN =>
            {
                let cursor = (cursor.get() != 0).then_some(cursor);
                if cursor.map(Cursor::get)
                    != expected_cursor.checked_sub(1).filter(|value| *value != 0)
                    || !summary_payload_matches(
                        &body[FRAME_COMMON_BODY_LEN..],
                        &records,
                        header.first_cursor,
                        frame_offset as u64,
                    )
                {
                    issue = Some(RecoveryIssue::InvalidRecord {
                        path: path.to_path_buf(),
                        offset: frame_offset as u64,
                        detail: "barrier summary does not match appended prefix",
                    });
                    break;
                }
                barriers.push(BarrierLocation {
                    cursor,
                    path: path.to_path_buf(),
                    end_offset: frame_end as u64,
                    durable_summary: DurableSummary {
                        segment_id: header.segment_id,
                        segment_len: frame_end as u64,
                        data_end: u64::from_le_bytes(
                            body[FRAME_COMMON_BODY_LEN + 24..FRAME_COMMON_BODY_LEN + 32]
                                .try_into()
                                .unwrap(),
                        ),
                        durable_cursor: cursor.unwrap_or(Cursor::new(0)),
                        record_count: records.len() as u64,
                    },
                });
            }
            SEGMENT_SUMMARY_KIND
                if version == 0
                    && body_len == FRAME_COMMON_BODY_LEN + SUMMARY_PAYLOAD_LEN
                    && frame_end == bytes.len()
                    && summary_payload_matches(
                        &body[FRAME_COMMON_BODY_LEN..],
                        &records,
                        header.first_cursor,
                        frame_offset as u64,
                    ) => {}
            _ => {
                issue = Some(RecoveryIssue::InvalidRecord {
                    path: path.to_path_buf(),
                    offset: frame_offset as u64,
                    detail: "unknown record kind",
                });
                break;
            }
        }
        offset = frame_end;
    }
    Ok(ScanResult {
        segment: SegmentInfo {
            id: header.segment_id,
            path: path.to_path_buf(),
            header_len: header.encoded_len,
            file_len: bytes.len() as u64,
            first_cursor: header.first_cursor,
            max_cursor: records.last().map(|record| record.cursor),
            record_count: records.len() as u64,
        },
        data_records: records,
        barriers,
        issue,
    })
}

fn summary_payload_matches(
    payload: &[u8],
    records: &[RecordLocation],
    first_cursor: u64,
    data_end: u64,
) -> bool {
    let count = u64::from_le_bytes(payload[0..8].try_into().unwrap());
    let encoded_first = u64::from_le_bytes(payload[8..16].try_into().unwrap());
    let encoded_last = u64::from_le_bytes(payload[16..24].try_into().unwrap());
    let encoded_data_end = u64::from_le_bytes(payload[24..32].try_into().unwrap());
    count == records.len() as u64
        && encoded_first == if records.is_empty() { 0 } else { first_cursor }
        && encoded_last
            == records
                .last()
                .map(|record| record.cursor.get())
                .unwrap_or(0)
        && encoded_data_end == data_end
}

fn repair_to_last_barrier(
    paths: &[(u64, PathBuf)],
    scans: &[ScanResult],
    last_barrier: Option<&BarrierLocation>,
) -> Result<(), Error> {
    let (keep_path, keep_len) = if let Some(barrier) = last_barrier {
        (barrier.path.clone(), barrier.end_offset)
    } else {
        let first = scans
            .first()
            .ok_or(Error::InvalidConfig("no segment to recover"))?;
        (first.segment.path.clone(), first.segment.header_len)
    };
    let keep_file = OpenOptions::new().write(true).open(&keep_path)?;
    keep_file.set_len(keep_len)?;
    keep_file.sync_all()?;
    for (_, path) in paths {
        if path > &keep_path {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn recovery_issue_at(issue: &RecoveryIssue, path: Option<&Path>, offset: u64) -> bool {
    let Some(expected_path) = path else {
        return false;
    };
    match issue {
        RecoveryIssue::TruncatedRecord {
            path,
            offset: actual,
        }
        | RecoveryIssue::ChecksumMismatch {
            path,
            offset: actual,
        }
        | RecoveryIssue::InvalidRecord {
            path,
            offset: actual,
            ..
        } => path == expected_path && *actual == offset,
        RecoveryIssue::DurableSummaryRepaired { .. } | RecoveryIssue::UncommittedTail { .. } => {
            false
        }
    }
}

fn read_records(
    path: &Path,
    start: Cursor,
    durable: Cursor,
    output: &mut Vec<Record>,
) -> Result<(), Error> {
    let header = read_header(path)?;
    let bytes = fs::read(path)?;
    let mut offset = header.encoded_len as usize;
    while offset < bytes.len() {
        let frame_offset = offset;
        if bytes.len() - offset < FRAME_PREFIX_LEN {
            return corrupt_read(path, frame_offset, "truncated frame length");
        }
        let body_len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += FRAME_PREFIX_LEN;
        if !(FRAME_COMMON_BODY_LEN..=MAX_FRAME_BYTES).contains(&body_len) {
            return corrupt_read(path, frame_offset, "invalid frame length");
        }
        let Some(frame_end) = offset
            .checked_add(body_len)
            .and_then(|value| value.checked_add(FRAME_CRC_LEN))
        else {
            return corrupt_read(path, frame_offset, "frame length overflow");
        };
        if frame_end > bytes.len() {
            return corrupt_read(path, frame_offset, "truncated frame");
        }
        let body = &bytes[offset..offset + body_len];
        let stored_crc =
            u32::from_le_bytes(bytes[offset + body_len..frame_end].try_into().unwrap());
        if crc32(body) != stored_crc {
            return corrupt_read(path, frame_offset, "checksum mismatch");
        }
        let kind = body[0];
        let version = u16::from_le_bytes(body[1..3].try_into().unwrap());
        let cursor = Cursor::new(u64::from_le_bytes(body[3..11].try_into().unwrap()));
        if kind != DATA_KIND && kind != BARRIER_KIND && kind != SEGMENT_SUMMARY_KIND {
            return corrupt_read(path, frame_offset, "unknown record kind");
        }
        if kind == DATA_KIND && version != RECORD_VERSION {
            return corrupt_read(path, frame_offset, "unsupported record version");
        }
        if kind == DATA_KIND && cursor >= start && cursor <= durable {
            output.push(Record {
                cursor,
                record_version: version,
                payload: body[FRAME_COMMON_BODY_LEN..].to_vec(),
            });
        }
        offset = frame_end;
    }
    Ok(())
}

fn corrupt_read<T>(path: &Path, offset: usize, detail: &'static str) -> Result<T, Error> {
    Err(Error::CorruptRecord {
        path: path.to_path_buf(),
        offset: offset as u64,
        detail,
    })
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_identity_is_stable() {
        assert_eq!(CRATE_NAME, "pinvou-seglog");
    }

    #[test]
    fn crc32_matches_the_standard_check_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn unknown_record_version_is_rejected_without_truncating_the_segment() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-seglog-record-version-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let config = Config::new(&directory);
        {
            let mut log = SegmentLog::open(config.clone()).unwrap().log;
            log.append_batch([b"payload".as_slice()]).unwrap();
            log.durable_barrier().unwrap();
        }
        let path = segment_path(&directory, 0);
        let header = read_header(&path).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        let frame = header.encoded_len as usize;
        let body_len = u32::from_le_bytes(bytes[frame..frame + 4].try_into().unwrap()) as usize;
        let body_start = frame + FRAME_PREFIX_LEN;
        bytes[body_start + 1..body_start + 3].copy_from_slice(&2_u16.to_le_bytes());
        let checksum = crc32(&bytes[body_start..body_start + body_len]);
        bytes[body_start + body_len..body_start + body_len + 4]
            .copy_from_slice(&checksum.to_le_bytes());
        {
            let mut file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            file.write_all(&bytes).unwrap();
            file.sync_all().unwrap();
        }
        let before = fs::read(&path).unwrap();

        assert!(matches!(
            SegmentLog::open(config),
            Err(Error::UnsupportedRecordVersion { found: 2, .. })
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
        let _ = fs::remove_dir_all(directory);
    }
}
