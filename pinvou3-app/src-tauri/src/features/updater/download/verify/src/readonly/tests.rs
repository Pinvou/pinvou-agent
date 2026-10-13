use super::*;
use pinvou_upgrade_cache_object::{CacheObjectError, create_exclusive_cache};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
const HASH: &str = "84d89877f0d4041efb6bf91a16f0248f2fd573e6af05c19f96bedb9f882f7882";
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    path: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self {
            path: std::env::temp_dir().join(format!(
                "pinvou-readonly-scan-{}-{nonce}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )),
        }
    }
    fn view(&self) -> Option<ReadOnlyCacheHandle> {
        let mut source = match create_exclusive_cache(&self.path) {
            Ok(source) => source,
            Err(CacheObjectError::Unsupported) => {
                assert!(!self.path.exists());
                eprintln!(
                    "read-only cache primitive unavailable: no physical scan controls executed"
                );
                return None;
            }
            Err(_) => panic!("test cache creation unavailable"),
        };
        source.file_mut().write_all(b"0123456789").unwrap();
        let view = source.read_only_duplicate().unwrap();
        drop(source);
        Some(view)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
fn target(size: u64, hash: &str) -> Artifact<'static> {
    Artifact::new("package-1", size, hash).unwrap()
}
#[test]
fn actual_read_only_scan_matches_original_target_at_real_eof_and_releases_internal_clone() {
    let fixture = Fixture::new();
    let Some(view) = fixture.view() else {
        return;
    };
    let mut alias = view.file();
    alias.seek(SeekFrom::Start(7)).unwrap();
    let expected = target(10, HASH);
    let matched = check_read_only_cache(&view, expected, || false).unwrap();
    assert!(matched.target() == expected);
    assert_eq!(alias.stream_position().unwrap(), 10);
    assert!(alias.write_all(b"wrong").is_err());
    assert!(
        fs::OpenOptions::new()
            .write(true)
            .open(&fixture.path)
            .is_err()
    );
    drop(view);
    assert_eq!(fs::read(&fixture.path).unwrap(), b"0123456789");
    let mut writer = fs::OpenOptions::new()
        .append(true)
        .open(&fixture.path)
        .unwrap();
    writer.write_all(b"!").unwrap();
    drop(writer);
    assert_eq!(fs::read(&fixture.path).unwrap(), b"0123456789!");
}
#[test]
fn actual_read_only_scan_wrong_original_size_hash_never_match_or_leave_a_clone() {
    for (size, hash, error) in [
        (9, HASH, ByteCheckError::TooLong),
        (11, HASH, ByteCheckError::TooShort),
        (
            10,
            "0000000000000000000000000000000000000000000000000000000000000000",
            ByteCheckError::HashMismatch,
        ),
    ] {
        let fixture = Fixture::new();
        let Some(view) = fixture.view() else {
            return;
        };
        assert!(
            matches!(check_read_only_cache(&view,target(size,hash),||false),Err(actual) if actual==error)
        );
        assert!(
            fs::OpenOptions::new()
                .write(true)
                .open(&fixture.path)
                .is_err()
        );
        drop(view);
        assert_eq!(fs::read(&fixture.path).unwrap(), b"0123456789");
        assert!(
            fs::OpenOptions::new()
                .write(true)
                .open(&fixture.path)
                .is_ok()
        );
    }
}
#[test]
fn actual_read_only_scan_cancellation_before_clone_through_after_hash_never_survives() {
    for cancel_at in 1..=6 {
        let fixture = Fixture::new();
        let Some(view) = fixture.view() else {
            return;
        };
        let mut alias = view.file();
        alias.seek(SeekFrom::Start(7)).unwrap();
        let mut calls = 0;
        let result = check_read_only_cache(&view, target(10, HASH), || {
            calls += 1;
            calls == cancel_at
        });
        assert!(matches!(result, Err(ByteCheckError::Cancelled)));
        assert_eq!(calls, cancel_at);
        if cancel_at <= 2 {
            assert_eq!(alias.stream_position().unwrap(), 7);
        }
        alias.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = vec![];
        alias.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"0123456789");
        drop(view);
        assert!(
            fs::OpenOptions::new()
                .write(true)
                .open(&fixture.path)
                .is_ok()
        );
    }
}
