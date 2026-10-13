use super::*;
use crate::tests::{ABC_HASH, OwnedFile, target};
use pinvou_upgrade_transfer::{
    Artifact, PrefixState, ResponseDecision, ResponseHeaders, SourceContinuity, StrongEtag,
    plan_request, plan_response,
};
use std::io::{self, Read};

fn plan(target: Artifact<'static>, range: Option<(u64, u64)>) -> BodyPlan<'static> {
    let prefix = range.map(|(offset, _)| {
        PrefixState::new(
            Some(target),
            offset,
            Some(StrongEtag::parse(b"\"one\"").unwrap()),
        )
        .unwrap()
    });
    let request = plan_request(target, prefix, SourceContinuity::SameImmutableEntity);
    let mut fields: Vec<(&str, &[u8])> = vec![];
    let content_range;
    if let Some((offset, end)) = range {
        content_range = format!("bytes {offset}-{end}/{}", target.size());
        fields.push(("Content-Range", content_range.as_bytes()));
        fields.push(("ETag", b"\"one\""));
    }
    match plan_response(
        request,
        if range.is_some() { 206 } else { 200 },
        ResponseHeaders::from_fields(fields).unwrap(),
    )
    .unwrap()
    {
        ResponseDecision::Transfer(plan) => plan,
        _ => panic!("expected transfer"),
    }
}
fn receipt(plan: BodyPlan<'static>) -> SegmentReceipt<'static> {
    let mut progress = plan.receive();
    progress
        .observe_bytes(plan.expected_response_length())
        .unwrap();
    progress.finish().unwrap()
}
fn assert_error<T>(result: Result<T, SegmentWriteError>, error: SegmentWriteError) {
    assert!(matches!(result, Err(actual) if actual == error));
}
fn contents(file: &mut File) -> Vec<u8> {
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    bytes
}
#[test]
fn actual_replacement_requires_separate_permission_and_checks_complete_bytes() {
    let mut owned = OwnedFile::new();
    owned.file().write_all(b"older bytes").unwrap();
    let plan = plan(target(3, ABC_HASH), None);
    assert_error(
        FileSegmentWriter::start(
            owned.file(),
            plan,
            CacheWritePermission::AppendExisting,
            || false,
        ),
        SegmentWriteError::WritePermissionMismatch,
    );
    assert_eq!(contents(owned.file()), b"older bytes");
    assert_error(
        FileSegmentWriter::start(
            owned.file(),
            plan,
            CacheWritePermission::ReplaceExisting,
            || true,
        ),
        SegmentWriteError::File(ByteCheckError::Cancelled),
    );
    assert_eq!(contents(owned.file()), b"older bytes");
    let mut sink = FileSegmentWriter::start(
        owned.file(),
        plan,
        CacheWritePermission::ReplaceExisting,
        || false,
    )
    .unwrap();
    sink.write(b"a", || false).unwrap();
    sink.write(b"bc", || false).unwrap();
    let done = sink
        .finish_after_network_eof(receipt(plan), || false)
        .unwrap();
    assert!(done.target() == target(3, ABC_HASH));
    assert!(done.has_full_byte_count());
    assert_eq!(contents(owned.file()), b"abc");
    assert!(crate::check_file(owned.file(), done.target(), || false).is_ok());
}
#[test]
fn actual_short_segments_resume_only_at_the_observed_current_length() {
    let mut owned = OwnedFile::new();
    owned.file().write_all(b"a").unwrap();
    let first = plan(target(3, ABC_HASH), Some((1, 1)));
    assert_error(
        FileSegmentWriter::start(
            owned.file(),
            first,
            CacheWritePermission::ReplaceExisting,
            || false,
        ),
        SegmentWriteError::WritePermissionMismatch,
    );
    let wrong = plan(target(3, ABC_HASH), Some((2, 2)));
    assert_error(
        FileSegmentWriter::start(
            owned.file(),
            wrong,
            CacheWritePermission::AppendExisting,
            || false,
        ),
        SegmentWriteError::PrefixLengthMismatch,
    );
    assert_eq!(contents(owned.file()), b"a");
    let mut sink = FileSegmentWriter::start(
        owned.file(),
        first,
        CacheWritePermission::AppendExisting,
        || false,
    )
    .unwrap();
    sink.write(b"b", || false).unwrap();
    let partial = sink
        .finish_after_network_eof(receipt(first), || false)
        .unwrap();
    assert_eq!(partial.next_offset(), 2);
    assert!(!partial.has_full_byte_count());
    let mut sink = FileSegmentWriter::start(
        owned.file(),
        wrong,
        CacheWritePermission::AppendExisting,
        || false,
    )
    .unwrap();
    sink.write(b"c", || false).unwrap();
    sink.finish_after_network_eof(receipt(wrong), || false)
        .unwrap();
    assert!(crate::check_file(owned.file(), target(3, ABC_HASH), || false).is_ok());
    owned.file().seek(SeekFrom::Start(0)).unwrap();
    owned.file().write_all(b"x").unwrap();
    assert!(matches!(
        crate::check_file(owned.file(), target(3, ABC_HASH), || false),
        Err(ByteCheckError::HashMismatch)
    ));
}
#[test]
fn excess_chunk_truncation_and_cancellation_latch_without_recovery() {
    for (bytes, expected) in [
        (vec![b'x'; 65537], SegmentWriteError::ChunkAboveLimit),
        (
            b"abcd".to_vec(),
            SegmentWriteError::Transfer(TransferError::BodyTooLong),
        ),
    ] {
        let mut owned = OwnedFile::new();
        let plan = plan(target(3, ABC_HASH), None);
        let mut sink = FileSegmentWriter::start(
            owned.file(),
            plan,
            CacheWritePermission::ReplaceExisting,
            || false,
        )
        .unwrap();
        assert_error(sink.write(&bytes, || false), expected);
        assert_error(sink.write(b"abc", || false), expected);
        assert_error(
            sink.finish_after_network_eof(receipt(plan), || false),
            expected,
        );
        assert_eq!(owned.file().metadata().unwrap().len(), 0);
    }
    let mut owned = OwnedFile::new();
    let plan = plan(target(3, ABC_HASH), None);
    let mut sink = FileSegmentWriter::start(
        owned.file(),
        plan,
        CacheWritePermission::ReplaceExisting,
        || false,
    )
    .unwrap();
    sink.write(b"a", || false).unwrap();
    assert_error(
        sink.finish_after_network_eof(receipt(plan), || false),
        SegmentWriteError::Transfer(TransferError::BodyTruncated),
    );
    let mut sink = FileSegmentWriter::start(
        owned.file(),
        plan,
        CacheWritePermission::ReplaceExisting,
        || false,
    )
    .unwrap();
    assert_error(
        sink.write(b"abc", || true),
        SegmentWriteError::File(ByteCheckError::Cancelled),
    );
    assert_error(
        sink.write(b"abc", || false),
        SegmentWriteError::File(ByteCheckError::Cancelled),
    );
    assert_error(
        sink.finish_after_network_eof(receipt(plan), || false),
        SegmentWriteError::File(ByteCheckError::Cancelled),
    );
}
#[test]
fn foreign_transport_receipts_and_visible_length_changes_refuse() {
    for mode in 0..4 {
        let mut owned = OwnedFile::new();
        let alias = owned.file().try_clone().unwrap();
        let original = plan(target(3, ABC_HASH), None);
        let mut sink = FileSegmentWriter::start(
            owned.file(),
            original,
            CacheWritePermission::ReplaceExisting,
            || false,
        )
        .unwrap();
        sink.write(b"abc", || false).unwrap();
        if mode == 0 {
            let foreign = plan(Artifact::new("other", 3, ABC_HASH).unwrap(), None);
            assert_error(
                sink.finish_after_network_eof(receipt(foreign), || false),
                SegmentWriteError::ReceiptMismatch,
            );
        } else if mode == 1 {
            alias.set_len(4).unwrap();
            assert_error(
                sink.finish_after_network_eof(receipt(original), || false),
                SegmentWriteError::DestinationLengthMismatch,
            );
        } else if mode == 3 {
            let short = plan(target(3, ABC_HASH), Some((1, 1)));
            assert_error(
                sink.finish_after_network_eof(receipt(short), || false),
                SegmentWriteError::ReceiptMismatch,
            );
        } else {
            assert_error(
                sink.finish_with_sync(receipt(original), &mut || false, &mut |_| {
                    alias.set_len(2).unwrap();
                    Ok(())
                }),
                SegmentWriteError::DestinationLengthMismatch,
            );
        }
    }
}
#[test]
fn actual_read_only_writer_and_private_sync_failure_leave_no_receipt() {
    let mut owned = OwnedFile::new();
    owned.file().write_all(b"a").unwrap();
    let mut read_only = File::open(&owned.path).unwrap();
    let plan = plan(target(3, ABC_HASH), Some((1, 2)));
    let mut sink = FileSegmentWriter::start(
        &mut read_only,
        plan,
        CacheWritePermission::AppendExisting,
        || false,
    )
    .unwrap();
    assert_error(sink.write(b"bc", || false), io_error());
    assert_error(sink.write(b"bc", || false), io_error());
    assert_error(
        sink.finish_after_network_eof(receipt(plan), || false),
        io_error(),
    );
    assert_eq!(contents(owned.file()), b"a");
    let mut sink = FileSegmentWriter::start(
        owned.file(),
        plan,
        CacheWritePermission::AppendExisting,
        || false,
    )
    .unwrap();
    sink.write(b"bc", || false).unwrap();
    assert_error(
        sink.finish_with_sync(receipt(plan), &mut || false, &mut |_| Err(io_error())),
        io_error(),
    );
    assert_eq!(contents(owned.file()), b"abc");
}
#[test]
fn every_observed_start_write_and_finish_cancellation_poll_refuses() {
    let plan = plan(target(3, ABC_HASH), None);
    let mut baseline = OwnedFile::new();
    let mut count = 0;
    let mut counter = || {
        count += 1;
        false
    };
    let mut sink = FileSegmentWriter::start(
        baseline.file(),
        plan,
        CacheWritePermission::ReplaceExisting,
        &mut counter,
    )
    .unwrap();
    sink.write(b"abc", &mut counter).unwrap();
    sink.finish_after_network_eof(receipt(plan), &mut counter)
        .unwrap();
    for stop in 1..=count {
        let mut owned = OwnedFile::new();
        let mut current = 0;
        let mut cancel = || {
            current += 1;
            current == stop
        };
        let result = (|| {
            let mut sink = FileSegmentWriter::start(
                owned.file(),
                plan,
                CacheWritePermission::ReplaceExisting,
                &mut cancel,
            )?;
            sink.write(b"abc", &mut cancel)?;
            sink.finish_after_network_eof(receipt(plan), &mut cancel)
        })();
        assert_error(result, SegmentWriteError::File(ByteCheckError::Cancelled));
    }
}
#[test]
fn actual_33_mib_transfer_is_not_capped_by_per_chunk_size() {
    let mut owned = OwnedFile::new();
    let target = target(
        33 * 1024 * 1024,
        "7d641a2a7b1c449f586b444bd47a2005d83be0e142596f6d1a52da9c2256c96b",
    );
    let plan = plan(target, None);
    let mut sink = FileSegmentWriter::start(
        owned.file(),
        plan,
        CacheWritePermission::ReplaceExisting,
        || false,
    )
    .unwrap();
    for _ in 0..528 {
        sink.write(&[b'x'; 65536], || false).unwrap();
    }
    sink.finish_after_network_eof(receipt(plan), || false)
        .unwrap();
    assert!(crate::check_file(owned.file(), target, || false).is_ok());
}
#[test]
fn private_writer_faults_cover_partial_interrupted_zero_and_mid_write_cancel() {
    struct Writer {
        bytes: Vec<u8>,
        calls: usize,
        fail: bool,
    }
    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.calls == 1 {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if self.fail && self.calls == 3 {
                return Ok(0);
            }
            self.bytes.push(bytes[0]);
            Ok(1)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer {
        bytes: vec![],
        calls: 0,
        fail: false,
    };
    write_bytes(&mut writer, b"abc", &mut || false).unwrap();
    assert_eq!(writer.bytes, b"abc");
    writer = Writer {
        bytes: vec![],
        calls: 0,
        fail: true,
    };
    assert_error(write_bytes(&mut writer, b"abc", &mut || false), io_error());
    assert_eq!(writer.bytes, b"a");
    writer = Writer {
        bytes: vec![],
        calls: 0,
        fail: false,
    };
    let mut polls = 0;
    assert_error(
        write_bytes(&mut writer, b"abc", &mut || {
            polls += 1;
            polls == 3
        }),
        SegmentWriteError::File(ByteCheckError::Cancelled),
    );
    assert_eq!(writer.bytes, b"a");
}

#[test]
fn private_failure_after_real_partial_write_permanently_latches_the_session() {
    for failure in [
        io_error(),
        SegmentWriteError::File(ByteCheckError::Cancelled),
    ] {
        let mut owned = OwnedFile::new();
        let plan = plan(target(3, ABC_HASH), None);
        let mut sink = FileSegmentWriter::start(
            owned.file(),
            plan,
            CacheWritePermission::ReplaceExisting,
            || false,
        )
        .unwrap();
        assert_error(
            sink.write_with(b"abc", &mut || false, |file, bytes, _| {
                file.write_all(&bytes[..1]).unwrap();
                Err(failure)
            }),
            failure,
        );
        assert_error(sink.write(b"bc", || false), failure);
        assert_error(
            sink.finish_after_network_eof(receipt(plan), || false),
            failure,
        );
        assert_eq!(contents(owned.file()), b"a");
    }
}

#[test]
fn actual_append_mode_and_empty_calls_preserve_segment_boundaries() {
    let mut owned = OwnedFile::new();
    owned.file().write_all(b"a").unwrap();
    let mut append = std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(&owned.path)
        .unwrap();
    let plan = plan(target(3, ABC_HASH), Some((1, 2)));
    let mut sink = FileSegmentWriter::start(
        &mut append,
        plan,
        CacheWritePermission::AppendExisting,
        || false,
    )
    .unwrap();
    sink.write(b"", || false).unwrap();
    sink.write(b"bc", || false).unwrap();
    sink.finish_after_network_eof(receipt(plan), || false)
        .unwrap();
    assert!(crate::check_file(owned.file(), target(3, ABC_HASH), || false).is_ok());
}
#[test]
fn actual_append_offset_shorter_or_longer_refuses_without_changing_bytes_or_cursor() {
    for length in [0, 2] {
        let mut owned = OwnedFile::new();
        owned.file().write_all(&vec![b'x'; length]).unwrap();
        owned.file().seek(SeekFrom::Start(7)).unwrap();
        let plan = plan(target(3, ABC_HASH), Some((1, 2)));
        assert_error(
            FileSegmentWriter::start(
                owned.file(),
                plan,
                CacheWritePermission::AppendExisting,
                || false,
            ),
            SegmentWriteError::PrefixLengthMismatch,
        );
        assert_eq!(owned.file().stream_position().unwrap(), 7);
        assert_eq!(contents(owned.file()), vec![b'x'; length]);
    }
}
