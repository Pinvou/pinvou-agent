use std::fs;
use std::path::PathBuf;

use pinvou_node::{NodeSpool, SpoolError};
use pinvou_protocol::{SourceSpan, StreamId};

fn temp_dir(name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("pinvou-node-spool-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn mapping_is_durable_before_send_and_ack_reclaims_only_complete_source_spans() {
    let directory = temp_dir("mapping");
    let mut spool = NodeSpool::open(&directory, "node-a", "att-a").unwrap();
    let raw = spool
        .append_raw_batch(StreamId::Main, [b"a".as_slice(), b"b", b"c"])
        .unwrap();
    assert_eq!((*raw.start(), *raw.end()), (1, 3));
    let tx = spool
        .prepare_transport(StreamId::Main, SourceSpan { start: 1, end: 3 }, b"merged")
        .unwrap();
    assert_eq!(tx.seq, 1);
    drop(spool);

    let mut reopened = NodeSpool::open(&directory, "node-a", "att-a").unwrap();
    assert_eq!(reopened.replay_unacked(StreamId::Main).unwrap(), vec![tx]);
    reopened.apply_ack(StreamId::Main, 1).unwrap();
    assert_eq!(reopened.ack_watermark(StreamId::Main), 1);
    assert_eq!(reopened.source_ack_watermark(StreamId::Main), 3);
    assert!(reopened.replay_unacked(StreamId::Main).unwrap().is_empty());
    drop(reopened);
    let recovered_ack = NodeSpool::open(&directory, "node-a", "att-a").unwrap();
    assert_eq!(recovered_ack.ack_watermark(StreamId::Main), 1);
    assert_eq!(recovered_ack.source_ack_watermark(StreamId::Main), 3);
    assert!(
        recovered_ack
            .replay_unacked(StreamId::Main)
            .unwrap()
            .is_empty()
    );
    drop(recovered_ack);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn transport_mapping_and_ack_gaps_are_rejected_independently_per_stream() {
    let directory = temp_dir("gaps");
    let mut spool = NodeSpool::open(&directory, "node-a", "att-a").unwrap();
    spool
        .append_raw_batch(StreamId::Main, [b"m".as_slice()])
        .unwrap();
    assert!(matches!(
        spool.prepare_transport(StreamId::Main, SourceSpan { start: 2, end: 2 }, b"bad"),
        Err(SpoolError::SourceGap { .. })
    ));
    spool
        .append_raw_batch(StreamId::Control, [b"c".as_slice()])
        .unwrap();
    let control = spool
        .prepare_transport(
            StreamId::Control,
            SourceSpan { start: 1, end: 1 },
            b"control",
        )
        .unwrap();
    assert_eq!(control.seq, 1);
    assert!(matches!(
        spool.apply_ack(StreamId::Control, 2),
        Err(SpoolError::AckBeyondSent { .. })
    ));
    spool.apply_ack(StreamId::Control, 1).unwrap();
    assert_eq!(spool.ack_watermark(StreamId::Main), 0);
    assert_eq!(spool.ack_watermark(StreamId::Control), 1);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn durable_raw_without_a_mapping_is_replayed_after_restart() {
    let directory = temp_dir("raw-recovery");
    let mut spool = NodeSpool::open(&directory, "node-a", "att-a").unwrap();
    spool
        .append_raw_batch(StreamId::Main, [b"unmapped".as_slice()])
        .unwrap();
    drop(spool);

    let reopened = NodeSpool::open(&directory, "node-a", "att-a").unwrap();
    let raw = reopened.replay_unmapped_raw(StreamId::Main).unwrap();
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].source_seq, 1);
    assert_eq!(raw[0].payload, b"unmapped");
    drop(reopened);
    fs::remove_dir_all(directory).unwrap();
}
