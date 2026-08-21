use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use pinvou_controller::{ControllerWal, IngestOutcome, WalError};
use pinvou_protocol::RuntimeEventEnvelope;

fn temp_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "pinvou-controller-wal-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn event(stream: &str, seq: u64) -> RuntimeEventEnvelope {
    let mut value: serde_json::Value = serde_json::from_str(include_str!(
        "../../protocol/tests/fixtures/events/text.delta.json"
    ))
    .unwrap();
    value["node_id"] = "node-a".into();
    value["attachment_id"] = "att-a".into();
    value["seq"] = seq.into();
    if stream == "control" {
        value["stream_id"] = "control".into();
        value["rate_class"] = "R0".into();
        value["kind"] = "turn.ended".into();
        value["source_span"] = serde_json::Value::Null;
        value["payload"] = serde_json::json!({"end_reason":"completed","error":null});
    }
    RuntimeEventEnvelope::from_value(value).unwrap()
}

#[test]
fn wal_deduplicates_replay_but_rejects_transport_gaps_per_stream() {
    let directory = temp_dir("sequence");
    let mut wal = ControllerWal::open(&directory).unwrap();
    assert!(matches!(
        wal.ingest(event("main", 1), Duration::ZERO).unwrap(),
        IngestOutcome::Pending
    ));
    assert!(matches!(
        wal.ingest(event("main", 1), Duration::ZERO).unwrap(),
        IngestOutcome::Duplicate
    ));
    let mut conflicting = serde_json::to_value(event("main", 1)).unwrap();
    conflicting["payload"]["content"] = "different".into();
    assert!(matches!(
        wal.ingest(
            RuntimeEventEnvelope::from_value(conflicting).unwrap(),
            Duration::ZERO
        ),
        Err(WalError::ConflictingDuplicate { seq: 1 })
    ));
    assert!(matches!(
        wal.ingest(event("main", 3), Duration::ZERO),
        Err(WalError::SequenceGap {
            expected: 2,
            actual: 3,
            ..
        })
    ));
    assert!(matches!(
        wal.ingest(event("control", 1), Duration::ZERO).unwrap(),
        IngestOutcome::Pending
    ));
    let acks = wal.flush().unwrap();
    assert_eq!(acks.len(), 1);
    assert_eq!(acks[0].control, Some(1));
    assert_eq!(acks[0].main, Some(1));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn wal_group_commits_at_sixteen_or_five_milliseconds_and_recovers_replay() {
    let directory = temp_dir("group");
    let mut wal = ControllerWal::open(&directory).unwrap();
    for seq in 1..16 {
        assert!(matches!(
            wal.ingest(event("main", seq), Duration::ZERO).unwrap(),
            IngestOutcome::Pending
        ));
    }
    assert!(wal.flush_due(Duration::from_millis(4)).unwrap().is_empty());
    let outcome = wal
        .ingest(event("main", 16), Duration::from_millis(4))
        .unwrap();
    assert!(matches!(outcome, IngestOutcome::Committed(_)));
    assert_eq!(
        wal.durable_watermark("node-a", "att-a", pinvou_protocol::StreamId::Main),
        Some(16)
    );
    drop(wal);

    let mut reopened = ControllerWal::open(&directory).unwrap();
    assert_eq!(
        reopened
            .replay("node-a", "att-a", pinvou_protocol::StreamId::Main, 14)
            .unwrap()
            .len(),
        3
    );
    assert!(matches!(
        reopened
            .ingest(event("control", 1), Duration::from_millis(10))
            .unwrap(),
        IngestOutcome::Pending
    ));
    let acks = reopened.flush_due(Duration::from_millis(15)).unwrap();
    assert_eq!(acks[0].control, Some(1));
    assert_eq!(acks[0].main, Some(16));
    fs::remove_dir_all(directory).unwrap();
}
