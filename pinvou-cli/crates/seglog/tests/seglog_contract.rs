use pinvou_seglog::{
    AckRange, Config, Cursor, Error, FaultInjector, FaultPoint, InjectedFailure, RecoveryIssue,
    SegmentLog,
};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("pinvou-seglog-{name}-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn config(dir: &TestDir, segment_target_bytes: u64) -> Config {
    Config::new(dir.path())
        .with_segment_target_bytes(segment_target_bytes)
        .with_stream_metadata(b"attachment=att-1;stream=main".to_vec())
}

fn segment_files(dir: &TestDir) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("pseg"))
        .collect();
    paths.sort();
    paths
}

#[derive(Debug)]
struct FailOnce {
    point: FaultPoint,
    after_bytes: usize,
    remaining: AtomicU64,
}

impl FailOnce {
    fn at(point: FaultPoint, after_bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            point,
            after_bytes,
            remaining: AtomicU64::new(1),
        })
    }
}

impl FaultInjector for FailOnce {
    fn failure(&self, point: FaultPoint, _requested_bytes: usize) -> Option<InjectedFailure> {
        (point == self.point && self.remaining.fetch_sub(1, Ordering::Relaxed) == 1)
            .then_some(InjectedFailure::after_bytes(self.after_bytes))
    }
}

#[derive(Debug)]
struct FailAtCall {
    point: FaultPoint,
    call: AtomicU64,
}

impl FailAtCall {
    fn new(point: FaultPoint, call: u64) -> Arc<Self> {
        Arc::new(Self {
            point,
            call: AtomicU64::new(call),
        })
    }
}

impl FaultInjector for FailAtCall {
    fn failure(&self, point: FaultPoint, _requested_bytes: usize) -> Option<InjectedFailure> {
        if point != self.point {
            return None;
        }
        (self.call.fetch_sub(1, Ordering::Relaxed) == 1).then_some(InjectedFailure::after_bytes(0))
    }
}

#[test]
fn append_batch_becomes_replayable_only_after_durable_barrier() {
    let dir = TestDir::new("barrier");
    let opened = SegmentLog::open(config(&dir, 4096)).unwrap();
    let mut log = opened.log;

    let appended = log
        .append_batch([b"one".as_slice(), b"two".as_slice()])
        .unwrap();
    assert_eq!(appended, Cursor::new(1)..=Cursor::new(2));
    assert_eq!(log.durable_cursor(), None);
    assert!(log.replay_from(Cursor::new(1)).unwrap().is_empty());

    assert_eq!(log.durable_barrier().unwrap(), Some(Cursor::new(2)));
    let replayed = log.replay_from(Cursor::new(1)).unwrap();
    assert_eq!(
        replayed
            .iter()
            .map(|record| (record.cursor, record.payload.as_slice()))
            .collect::<Vec<_>>(),
        vec![
            (Cursor::new(1), b"one".as_slice()),
            (Cursor::new(2), b"two".as_slice())
        ]
    );
}

#[test]
fn recovery_discards_and_reports_a_valid_but_uncommitted_tail() {
    let dir = TestDir::new("uncommitted-tail");
    {
        let mut log = SegmentLog::open(config(&dir, 4096)).unwrap().log;
        log.append_batch([b"durable".as_slice()]).unwrap();
        log.durable_barrier().unwrap();
        log.append_batch([b"not-durable".as_slice()]).unwrap();
    }

    let reopened = SegmentLog::open(config(&dir, 4096)).unwrap();
    assert!(matches!(
        reopened.recovery.issue,
        Some(RecoveryIssue::UncommittedTail { .. })
    ));
    assert_eq!(reopened.recovery.durable_through, Some(Cursor::new(1)));
    assert_eq!(
        reopened
            .log
            .replay_from(Cursor::new(1))
            .unwrap()
            .iter()
            .map(|record| record.payload.as_slice())
            .collect::<Vec<_>>(),
        vec![b"durable".as_slice()]
    );
}

#[test]
fn recovery_stops_at_a_truncated_frame_and_keeps_the_committed_prefix() {
    let dir = TestDir::new("truncated");
    {
        let mut log = SegmentLog::open(config(&dir, 4096)).unwrap().log;
        log.append_batch([b"prefix".as_slice()]).unwrap();
        log.durable_barrier().unwrap();
        log.append_batch([b"truncated-record".as_slice()]).unwrap();
    }
    let path = segment_files(&dir).pop().unwrap();
    let len = fs::metadata(&path).unwrap().len();
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(len - 3)
        .unwrap();

    let reopened = SegmentLog::open(config(&dir, 4096)).unwrap();
    assert!(matches!(
        reopened.recovery.issue,
        Some(RecoveryIssue::TruncatedRecord { .. })
    ));
    assert_eq!(
        reopened.log.replay_from(Cursor::new(1)).unwrap()[0].payload,
        b"prefix"
    );
}

#[test]
fn crc_damage_is_explicit_and_recovery_never_reads_past_it() {
    let dir = TestDir::new("crc");
    {
        let mut log = SegmentLog::open(config(&dir, 4096)).unwrap().log;
        log.append_batch([b"first".as_slice()]).unwrap();
        log.durable_barrier().unwrap();
        log.append_batch([b"second-payload".as_slice()]).unwrap();
        log.durable_barrier().unwrap();
    }

    let path = segment_files(&dir).pop().unwrap();
    let mut bytes = Vec::new();
    OpenOptions::new()
        .read(true)
        .open(&path)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    let needle = b"second-payload";
    let payload_offset = bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .unwrap();
    let mut file = OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(payload_offset as u64 + 2))
        .unwrap();
    file.write_all(&[b'X']).unwrap();
    file.sync_all().unwrap();

    let reopened = SegmentLog::open(config(&dir, 4096)).unwrap();
    assert!(matches!(
        reopened.recovery.issue,
        Some(RecoveryIssue::ChecksumMismatch { .. })
    ));
    let replayed = reopened.log.replay_from(Cursor::new(1)).unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].payload, b"first");
}

#[test]
fn records_replay_in_cursor_order_across_segments() {
    let dir = TestDir::new("cross-segment");
    {
        let mut log = SegmentLog::open(config(&dir, 96)).unwrap().log;
        for value in 0_u8..12 {
            log.append_batch([vec![value; 24]]).unwrap();
        }
        log.durable_barrier().unwrap();
    }
    assert!(segment_files(&dir).len() > 1);

    let reopened = SegmentLog::open(config(&dir, 96)).unwrap();
    let replayed = reopened.log.replay_from(Cursor::new(1)).unwrap();
    assert_eq!(replayed.len(), 12);
    for (index, record) in replayed.iter().enumerate() {
        assert_eq!(record.cursor, Cursor::new(index as u64 + 1));
        assert_eq!(record.payload, vec![index as u8; 24]);
    }
}

#[test]
fn only_fully_acknowledged_closed_segments_are_reclaimed() {
    let dir = TestDir::new("reclaim");
    let mut log = SegmentLog::open(config(&dir, 96)).unwrap().log;
    for value in 0_u8..12 {
        log.append_batch([vec![value; 24]]).unwrap();
    }
    log.durable_barrier().unwrap();
    let before = segment_files(&dir).len();
    assert!(before > 2);

    assert_eq!(
        log.acknowledge(AckRange::new(Cursor::new(1), Cursor::new(6)).unwrap())
            .unwrap(),
        Cursor::new(6)
    );
    let reclaimed = log.reclaim_acknowledged().unwrap();
    assert!(!reclaimed.is_empty());
    assert!(segment_files(&dir).len() < before);
    assert!(
        log.replay_from(Cursor::new(7))
            .unwrap()
            .iter()
            .all(|record| record.cursor >= Cursor::new(7))
    );

    drop(log);
    let reopened = SegmentLog::open(config(&dir, 96)).unwrap();
    let replayed = reopened.log.replay_from(Cursor::new(7)).unwrap();
    assert_eq!(replayed.first().unwrap().cursor, Cursor::new(7));
    assert_eq!(replayed.last().unwrap().cursor, Cursor::new(12));
}

#[test]
fn acknowledgements_must_be_contiguous_and_cannot_pass_the_durable_cursor() {
    let dir = TestDir::new("ack-range");
    let mut log = SegmentLog::open(config(&dir, 4096)).unwrap().log;
    log.append_batch([b"one".as_slice(), b"two".as_slice()])
        .unwrap();
    log.durable_barrier().unwrap();

    assert!(
        log.acknowledge(AckRange::new(Cursor::new(2), Cursor::new(2)).unwrap())
            .is_err()
    );
    assert!(
        log.acknowledge(AckRange::new(Cursor::new(1), Cursor::new(3)).unwrap())
            .is_err()
    );
    assert_eq!(
        log.acknowledge(AckRange::new(Cursor::new(1), Cursor::new(1)).unwrap())
            .unwrap(),
        Cursor::new(1)
    );
    assert_eq!(
        log.acknowledge(AckRange::new(Cursor::new(2), Cursor::new(2)).unwrap())
            .unwrap(),
        Cursor::new(2)
    );
}

#[test]
fn a_higher_segment_format_version_is_rejected_without_mutating_the_file() {
    let dir = TestDir::new("future-format");
    {
        let mut log = SegmentLog::open(config(&dir, 4096)).unwrap().log;
        log.append_batch([b"one".as_slice()]).unwrap();
        log.durable_barrier().unwrap();
    }
    let path = segment_files(&dir).pop().unwrap();
    let before = fs::read(&path).unwrap();
    let mut file = OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(8)).unwrap();
    file.write_all(&u16::MAX.to_le_bytes()).unwrap();
    file.sync_all().unwrap();

    let error = SegmentLog::open(config(&dir, 4096)).unwrap_err();
    assert!(matches!(
        error,
        Error::UnsupportedFormat {
            found: u16::MAX,
            ..
        }
    ));
    let after = fs::read(&path).unwrap();
    assert_eq!(&after[..8], &before[..8]);
    assert_eq!(&after[10..], &before[10..]);
}

#[test]
fn replay_reports_concurrent_disk_damage_instead_of_panicking() {
    let dir = TestDir::new("replay-damage");
    let mut log = SegmentLog::open(config(&dir, 4096)).unwrap().log;
    log.append_batch([b"payload".as_slice()]).unwrap();
    log.durable_barrier().unwrap();

    let path = segment_files(&dir).pop().unwrap();
    let len = fs::metadata(&path).unwrap().len();
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(len - 2)
        .unwrap();

    assert!(log.replay_from(Cursor::new(1)).is_err());
}

#[test]
fn durable_summary_recovers_a_barrier_removed_at_an_exact_frame_boundary() {
    let dir = TestDir::new("durable-summary");
    let data_end;
    {
        let mut log = SegmentLog::open(config(&dir, 4096)).unwrap().log;
        log.append_batch([b"one".as_slice(), b"two".as_slice()])
            .unwrap();
        data_end = fs::metadata(segment_files(&dir).pop().unwrap())
            .unwrap()
            .len();
        log.durable_barrier().unwrap();
    }
    let path = segment_files(&dir).pop().unwrap();
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(data_end)
        .unwrap();

    let reopened = SegmentLog::open(config(&dir, 4096)).unwrap();
    assert!(matches!(
        reopened.recovery.issue,
        Some(RecoveryIssue::DurableSummaryRepaired { .. })
    ));
    assert_eq!(reopened.recovery.durable_through, Some(Cursor::new(2)));
    assert_eq!(reopened.log.replay_from(Cursor::new(1)).unwrap().len(), 2);
}

#[test]
fn partial_append_failure_poisons_the_instance_until_reopen() {
    let dir = TestDir::new("append-poison");
    let injector = FailOnce::at(FaultPoint::AppendFrame, 5);
    let mut log = SegmentLog::open(config(&dir, 4096).with_fault_injector(injector))
        .unwrap()
        .log;

    assert!(log.append_batch([b"partial".as_slice()]).is_err());
    assert!(matches!(
        log.append_batch([b"must-reopen".as_slice()]),
        Err(Error::Poisoned { .. })
    ));
    assert!(matches!(log.durable_barrier(), Err(Error::Poisoned { .. })));
    drop(log);

    let reopened = SegmentLog::open(config(&dir, 4096)).unwrap();
    assert!(matches!(
        reopened.recovery.issue,
        Some(RecoveryIssue::TruncatedRecord { .. })
    ));
}

#[test]
fn failed_reclaim_keeps_memory_consistent_and_can_be_retried() {
    let dir = TestDir::new("reclaim-failure");
    let injector = FailOnce::at(FaultPoint::ReclaimDelete, 0);
    let mut log = SegmentLog::open(config(&dir, 96).with_fault_injector(injector))
        .unwrap()
        .log;
    for value in 0_u8..12 {
        log.append_batch([vec![value; 24]]).unwrap();
    }
    log.durable_barrier().unwrap();
    log.acknowledge(AckRange::new(Cursor::new(1), Cursor::new(8)).unwrap())
        .unwrap();

    assert!(log.reclaim_acknowledged().is_err());
    assert_eq!(log.replay_from(Cursor::new(9)).unwrap().len(), 4);
    assert!(!log.reclaim_acknowledged().unwrap().is_empty());
    assert_eq!(log.replay_from(Cursor::new(9)).unwrap().len(), 4);
}

#[test]
fn partial_barrier_failure_poisons_and_reopen_reports_the_tail() {
    let dir = TestDir::new("barrier-poison");
    let injector = FailOnce::at(FaultPoint::BarrierFrame, 5);
    let mut log = SegmentLog::open(config(&dir, 4096).with_fault_injector(injector))
        .unwrap()
        .log;
    log.append_batch([b"written".as_slice()]).unwrap();

    assert!(log.durable_barrier().is_err());
    assert!(matches!(log.durable_barrier(), Err(Error::Poisoned { .. })));
    drop(log);
    let reopened = SegmentLog::open(config(&dir, 4096)).unwrap();
    assert!(matches!(
        reopened.recovery.issue,
        Some(RecoveryIssue::TruncatedRecord { .. })
    ));
}

#[test]
fn same_record_version_preserves_opaque_trailing_extensions() {
    let dir = TestDir::new("record-extension");
    let payload = b"known-fields\0opaque-future-extension";
    {
        let mut log = SegmentLog::open(config(&dir, 4096)).unwrap().log;
        log.append_batch([payload.as_slice()]).unwrap();
        log.durable_barrier().unwrap();
    }
    let reopened = SegmentLog::open(config(&dir, 4096)).unwrap();
    assert_eq!(
        reopened.log.replay_from(Cursor::new(1)).unwrap()[0].payload,
        payload
    );
    assert!(dir.path().join("durable.summary").is_file());
}

#[test]
fn reclaiming_the_segment_named_by_durable_summary_reopens_at_the_retained_floor() {
    let dir = TestDir::new("reclaim-summary-floor");
    let mut log = SegmentLog::open(config(&dir, 96)).unwrap().log;
    for value in 0_u8..8 {
        log.append_batch([vec![value; 24]]).unwrap();
    }
    log.durable_barrier().unwrap();
    log.append_batch([vec![9_u8; 24]]).unwrap();
    log.acknowledge(AckRange::new(Cursor::new(1), Cursor::new(8)).unwrap())
        .unwrap();
    assert!(!log.reclaim_acknowledged().unwrap().is_empty());
    drop(log);

    let reopened = SegmentLog::open(config(&dir, 96)).unwrap();
    assert!(matches!(
        reopened.recovery.issue,
        Some(RecoveryIssue::UncommittedTail { .. })
    ));
    assert_eq!(reopened.recovery.durable_through, None);
}

#[test]
fn reclaim_journal_finishes_a_partial_delete_on_reopen_without_stale_summary() {
    let dir = TestDir::new("partial-reclaim-summary-floor");
    let injector = FailAtCall::new(FaultPoint::ReclaimDelete, 2);
    let mut log = SegmentLog::open(config(&dir, 96).with_fault_injector(injector))
        .unwrap()
        .log;
    for value in 0_u8..8 {
        log.append_batch([vec![value; 24]]).unwrap();
    }
    log.durable_barrier().unwrap();
    log.append_batch([vec![9_u8; 24]]).unwrap();
    log.acknowledge(AckRange::new(Cursor::new(1), Cursor::new(8)).unwrap())
        .unwrap();
    assert!(log.reclaim_acknowledged().is_err());
    drop(log);

    let reopened = SegmentLog::open(config(&dir, 96)).unwrap();
    assert!(matches!(
        reopened.recovery.issue,
        Some(RecoveryIssue::UncommittedTail { .. })
    ));
    assert_eq!(reopened.recovery.durable_through, None);
}

#[test]
fn a_new_barrier_after_floor_reclaim_does_not_reference_deleted_dirty_segments() {
    let dir = TestDir::new("barrier-after-floor-reclaim");
    let mut log = SegmentLog::open(config(&dir, 96)).unwrap().log;
    for value in 0_u8..8 {
        log.append_batch([vec![value; 24]]).unwrap();
    }
    log.durable_barrier().unwrap();
    log.append_batch([vec![9_u8; 24]]).unwrap();
    log.acknowledge(AckRange::new(Cursor::new(1), Cursor::new(8)).unwrap())
        .unwrap();
    log.reclaim_acknowledged().unwrap();

    assert_eq!(log.durable_barrier().unwrap(), Some(Cursor::new(9)));
    drop(log);
    let reopened = SegmentLog::open(config(&dir, 96)).unwrap();
    assert_eq!(reopened.recovery.durable_through, Some(Cursor::new(9)));
    assert_eq!(reopened.log.replay_from(Cursor::new(9)).unwrap().len(), 1);
}

#[test]
fn partial_durable_summary_write_poisons_but_synced_barrier_recovers_on_reopen() {
    let dir = TestDir::new("partial-durable-summary");
    let injector = FailOnce::at(FaultPoint::DurableSummary, 11);
    let mut log = SegmentLog::open(config(&dir, 4096).with_fault_injector(injector))
        .unwrap()
        .log;
    log.append_batch([b"durable".as_slice()]).unwrap();
    assert!(log.durable_barrier().is_err());
    assert!(matches!(
        log.append_batch([b"poisoned".as_slice()]),
        Err(Error::Poisoned { .. })
    ));
    assert!(fs::read_dir(dir.path()).unwrap().any(|entry| {
        let entry = entry.unwrap();
        entry
            .file_name()
            .to_string_lossy()
            .contains("durable-summary.tmp")
            && entry.metadata().unwrap().len() == 11
    }));
    drop(log);

    let reopened = SegmentLog::open(config(&dir, 4096)).unwrap();
    assert_eq!(reopened.recovery.durable_through, Some(Cursor::new(1)));
    assert_eq!(reopened.log.replay_from(Cursor::new(1)).unwrap().len(), 1);
}

#[test]
fn durable_summary_repairs_a_short_or_crc_damaged_barrier_tail() {
    for (name, damage) in [("short", 0_u8), ("crc", 1_u8)] {
        let dir = TestDir::new(name);
        let data_end;
        {
            let mut log = SegmentLog::open(config(&dir, 4096)).unwrap().log;
            log.append_batch([b"durable".as_slice()]).unwrap();
            data_end = fs::metadata(segment_files(&dir).pop().unwrap())
                .unwrap()
                .len();
            log.durable_barrier().unwrap();
        }
        let path = segment_files(&dir).pop().unwrap();
        if damage == 0 {
            OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(data_end + 5)
                .unwrap();
        } else {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            file.seek(SeekFrom::Start(data_end + 5)).unwrap();
            file.write_all(&[0xff]).unwrap();
            file.sync_all().unwrap();
        }

        let reopened = SegmentLog::open(config(&dir, 4096)).unwrap();
        assert!(matches!(
            reopened.recovery.issue,
            Some(RecoveryIssue::DurableSummaryRepaired { .. })
        ));
        assert_eq!(reopened.recovery.durable_through, Some(Cursor::new(1)));
        assert_eq!(reopened.log.replay_from(Cursor::new(1)).unwrap().len(), 1);
    }
}
