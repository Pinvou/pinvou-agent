use super::{Fixture, HASH, full, range};
use crate::{CacheReceiveError, FetchResult, FileHttpError, receive_cache_body_on_worker};
use pinvou_upgrade_byte_check::{
    ByteCheckError, CacheWritePermission, SegmentWriteError, check_file,
};
use pinvou_upgrade_transfer::{Artifact, RequestPlan, SegmentReceipt};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct CacheFile {
    path: PathBuf,
    file: Option<File>,
}
impl CacheFile {
    fn new(bytes: &[u8]) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pinvou-https-cache-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.write_all(bytes).unwrap();
        Self {
            path,
            file: Some(file),
        }
    }
    fn file(&mut self) -> &mut File {
        self.file.as_mut().unwrap()
    }
    fn bytes(&mut self) -> Vec<u8> {
        self.file().seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = vec![];
        self.file().read_to_end(&mut bytes).unwrap();
        bytes
    }
}
impl Drop for CacheFile {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}
async fn receive(
    fixture: &Fixture,
    path: &str,
    request: RequestPlan<'static>,
    mut owned: CacheFile,
    permission: CacheWritePermission,
    cancelled: impl FnMut() -> bool + Send + 'static,
) -> (
    CacheFile,
    Result<SegmentReceipt<'static>, CacheReceiveError>,
) {
    let FetchResult::Transfer(body) = fixture.get(path, request).await.unwrap() else {
        panic!("expected actual HTTP body");
    };
    let runtime = tokio::runtime::Handle::current();
    // The current-thread test keeps its Runtime::block_on alive by awaiting
    // this worker. Its driver can therefore service network and timer events.
    tokio::task::spawn_blocking(move || {
        let result =
            receive_cache_body_on_worker(&runtime, *body, owned.file(), permission, cancelled);
        (owned, result)
    })
    .await
    .unwrap()
}
#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_https_cache_replacement_and_short_range_continuation_use_actual_eof() {
    let fixture = Fixture::load();
    let (mut owned, result) = receive(
        &fixture,
        "/ignored-range",
        range(4),
        CacheFile::new(b"old cache bytes"),
        CacheWritePermission::AppendExisting,
        || false,
    )
    .await;
    assert!(matches!(
        result,
        Err(CacheReceiveError::Write(
            SegmentWriteError::WritePermissionMismatch
        ))
    ));
    assert_eq!(owned.bytes(), b"old cache bytes");
    let (mut owned, result) = receive(
        &fixture,
        "/ignored-range",
        range(4),
        owned,
        CacheWritePermission::ReplaceExisting,
        || false,
    )
    .await;
    let receipt = result.unwrap();
    assert!(receipt.has_full_byte_count());
    assert!(check_file(owned.file(), receipt.target(), || false).is_ok());
    assert_eq!(owned.bytes(), b"0123456789");
    let (mut prefix, result) = receive(
        &fixture,
        "/short-segment",
        range(4),
        CacheFile::new(b"0123"),
        CacheWritePermission::AppendExisting,
        || false,
    )
    .await;
    let receipt = result.unwrap();
    assert_eq!(receipt.next_offset(), 7);
    assert!(!receipt.has_full_byte_count());
    assert_eq!(prefix.bytes(), b"0123456");
    let (mut prefix, result) = receive(
        &fixture,
        "/partial",
        range(7),
        prefix,
        CacheWritePermission::AppendExisting,
        || false,
    )
    .await;
    let receipt = result.unwrap();
    assert!(receipt.target() == Artifact::new("package-1", 10, HASH).unwrap());
    assert!(check_file(prefix.file(), receipt.target(), || false).is_ok());
}
#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_https_network_and_file_failures_cannot_finish_cache_writes() {
    let fixture = Fixture::load();
    for path in ["/truncated", "/overlong", "/network-error"] {
        let (mut owned, result) = receive(
            &fixture,
            path,
            full(10),
            CacheFile::new(b"old"),
            CacheWritePermission::ReplaceExisting,
            || false,
        )
        .await;
        assert!(matches!(result, Err(CacheReceiveError::Http(_))));
        assert!(owned.file().metadata().unwrap().len() <= 10);
    }
    let mut owned = CacheFile::new(b"0123");
    drop(owned.file.take());
    owned.file = Some(File::open(&owned.path).unwrap());
    let (mut owned, result) = receive(
        &fixture,
        "/partial",
        range(4),
        owned,
        CacheWritePermission::AppendExisting,
        || false,
    )
    .await;
    assert!(matches!(
        result,
        Err(CacheReceiveError::Write(SegmentWriteError::File(
            ByteCheckError::IoUnavailable
        )))
    ));
    assert_eq!(owned.bytes(), b"0123");
    let (mut owned, result) = receive(
        &fixture,
        "/partial",
        range(4),
        CacheFile::new(b"012"),
        CacheWritePermission::AppendExisting,
        || false,
    )
    .await;
    assert!(matches!(
        result,
        Err(CacheReceiveError::Write(
            SegmentWriteError::PrefixLengthMismatch
        ))
    ));
    assert_eq!(owned.bytes(), b"012");
}
#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_https_pending_read_cancellation_retains_untrusted_cache_without_receipt() {
    let fixture = Fixture::load();
    let started = std::time::Instant::now();
    let mut first_poll = None;
    let mut polls = 0;
    let first_true = std::sync::Arc::new(AtomicU64::new(0));
    let observed_true = first_true.clone();
    let (mut owned, result) = receive(
        &fixture,
        "/cache-cancel-wait",
        full(10),
        CacheFile::new(b"old"),
        CacheWritePermission::ReplaceExisting,
        move || {
            polls += 1;
            let stop = first_poll
                .get_or_insert_with(std::time::Instant::now)
                .elapsed()
                >= std::time::Duration::from_millis(120);
            if stop {
                observed_true.store(polls, Ordering::SeqCst);
            }
            stop
        },
    )
    .await;
    assert!(matches!(
        result,
        Err(CacheReceiveError::Write(SegmentWriteError::File(
            ByteCheckError::Cancelled
        )))
    ));
    assert_eq!(owned.bytes(), b"");
    // All three writer-start polls must have passed. With no data/EOF from
    // this peer, the first true poll must occur in the pinned network await.
    assert!(first_true.load(Ordering::SeqCst) > 3);
    // Test-only allowance; the product cancellation callback decides when to stop.
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    // With no owner cancellation the same no-data exchange stays pending until
    // the fixture's independent read-idle limit, never a success receipt.
    let (mut owned, result) = receive(
        &fixture,
        "/cache-cancel-wait",
        full(10),
        CacheFile::new(b""),
        CacheWritePermission::ReplaceExisting,
        || false,
    )
    .await;
    assert!(matches!(
        result,
        Err(CacheReceiveError::Http(FileHttpError::RequestTimeout))
    ));
    assert_eq!(owned.bytes(), b"");
}
#[tokio::test]
#[ignore = "requires isolated HTTPS fixture"]
async fn real_https_progress_and_large_chunks_write_without_a_total_limit() {
    let fixture = Fixture::load();
    let (mut owned, result) = receive(
        &fixture,
        "/slow-progress",
        full(10),
        CacheFile::new(b""),
        CacheWritePermission::ReplaceExisting,
        || false,
    )
    .await;
    let receipt = result.unwrap();
    assert!(check_file(owned.file(), receipt.target(), || false).is_ok());
    let (mut owned, result) = receive(
        &fixture,
        "/large-chunk",
        full(131073),
        CacheFile::new(b""),
        CacheWritePermission::ReplaceExisting,
        || false,
    )
    .await;
    let receipt = result.unwrap();
    assert_eq!(receipt.next_offset(), 131073);
    assert_eq!(owned.bytes(), vec![b'x'; 131073]);
    // The synthetic large response's request digest belongs to a different fixture;
    // exact transfer count must never become final hash verification.
    assert!(matches!(
        check_file(owned.file(), receipt.target(), || false),
        Err(ByteCheckError::HashMismatch)
    ));
}

mod attempt_tests;
mod observer_tests;
