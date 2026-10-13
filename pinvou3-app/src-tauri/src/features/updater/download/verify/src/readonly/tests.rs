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

fn destination(fixture: &Fixture) -> pinvou_upgrade_cache_object::ExclusiveCacheFile {
    create_exclusive_cache(&fixture.path).unwrap()
}
fn assert_released(fixture: &Fixture) {
    assert!(
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fixture.path)
            .is_ok()
    );
    let renamed = fixture.path.with_extension("released");
    fs::rename(&fixture.path, &renamed).unwrap();
    fs::rename(&renamed, &fixture.path).unwrap();
}
#[test]
fn actual_read_only_copy_rescans_same_exclusive_destination_at_eof() {
    for (bytes, hash) in [
        (b"0123456789".to_vec(), HASH),
        (
            vec![b'x'; 65_537],
            "1abe08ebecf1c18cab71f6fe28aaddf20268f85bad78bb9a72f88ca47c874662",
        ),
    ] {
        let source_fixture = Fixture::new();
        let mut source = match create_exclusive_cache(&source_fixture.path) {
            Ok(source) => source,
            Err(CacheObjectError::Unsupported) => {
                assert!(!source_fixture.path.exists());
                eprintln!("read-only copy unavailable: no physical copy controls executed");
                return;
            }
            Err(_) => panic!("test source creation unavailable"),
        };
        source.file_mut().write_all(&bytes).unwrap();
        let view = source.read_only_duplicate().unwrap();
        drop(source);
        let destination_fixture = Fixture::new();
        let mut output = destination(&destination_fixture);
        let expected = target(bytes.len() as u64, hash);
        let matched = copy_read_only_cache(&view, &mut output, expected, || false).unwrap();
        assert!(matched.target() == expected);
        assert_eq!(
            output.file_mut().stream_position().unwrap(),
            expected.size()
        );
        assert_eq!(output.file_mut().metadata().unwrap().len(), expected.size());
        assert!(
            fs::OpenOptions::new()
                .write(true)
                .open(&destination_fixture.path)
                .is_err()
        );
        assert!(
            fs::OpenOptions::new()
                .write(true)
                .open(&source_fixture.path)
                .is_err()
        );
        drop(output);
        drop(view);
        assert_eq!(fs::read(&destination_fixture.path).unwrap(), bytes);
        assert_eq!(fs::read(&source_fixture.path).unwrap(), bytes);
        assert_released(&source_fixture);
        assert_released(&destination_fixture);
    }
}
#[test]
fn actual_read_only_copy_refuses_wrong_target_nonempty_and_same_object_without_promoting_output() {
    for (size, hash, expected_error, expected_output) in [
        (9, HASH, ByteCheckError::TooLong, &b""[..]),
        (11, HASH, ByteCheckError::TooShort, &b"0123456789"[..]),
        (
            10,
            "0000000000000000000000000000000000000000000000000000000000000000",
            ByteCheckError::HashMismatch,
            &b"0123456789"[..],
        ),
    ] {
        let input = Fixture::new();
        let Some(view) = input.view() else {
            return;
        };
        let output_fixture = Fixture::new();
        let mut output = destination(&output_fixture);
        let result = copy_read_only_cache(&view, &mut output, target(size, hash), || false);
        assert!(matches!(result,Err(actual) if actual == expected_error));
        drop(output);
        drop(view);
        assert_eq!(fs::read(&input.path).unwrap(), b"0123456789");
        // Source mismatch may have written all untrusted bytes before refusing.
        assert_eq!(fs::read(&output_fixture.path).unwrap(), expected_output);
        assert_released(&input);
        assert_released(&output_fixture);
    }
    let input = Fixture::new();
    let Some(view) = input.view() else {
        return;
    };
    let output_fixture = Fixture::new();
    let mut output = destination(&output_fixture);
    output.file_mut().write_all(b"keep").unwrap();
    let mut input_alias = view.file();
    input_alias.seek(SeekFrom::Start(7)).unwrap();
    output.file_mut().seek(SeekFrom::Start(2)).unwrap();
    assert!(matches!(
        copy_read_only_cache(&view, &mut output, target(10, HASH), || false),
        Err(ByteCheckError::DestinationNotEmpty)
    ));
    assert_eq!(input_alias.stream_position().unwrap(), 7);
    assert_eq!(output.file_mut().stream_position().unwrap(), 2);
    drop(output);
    drop(view);
    assert_eq!(fs::read(&output_fixture.path).unwrap(), b"keep");
    assert_released(&input);
    assert_released(&output_fixture);
    let alias_fixture = Fixture::new();
    let mut owner = destination(&alias_fixture);
    owner.file_mut().write_all(b"0123456789").unwrap();
    let alias = owner.read_only_duplicate().unwrap();
    assert!(matches!(
        copy_read_only_cache(&alias, &mut owner, target(10, HASH), || false),
        Err(ByteCheckError::DestinationNotEmpty)
    ));
    drop(owner);
    drop(alias);
    assert_eq!(fs::read(&alias_fixture.path).unwrap(), b"0123456789");
    assert_released(&alias_fixture);
}
#[test]
fn actual_read_only_copy_cancels_all_clone_copy_sync_rescan_and_final_hash_boundaries() {
    for cancel_at in 1..=13 {
        let input = Fixture::new();
        let Some(view) = input.view() else {
            return;
        };
        let output_fixture = Fixture::new();
        let mut output = destination(&output_fixture);
        let mut alias = view.file();
        alias.seek(SeekFrom::Start(7)).unwrap();
        output.file_mut().seek(SeekFrom::Start(5)).unwrap();
        let mut calls = 0;
        let result = copy_read_only_cache(&view, &mut output, target(10, HASH), || {
            calls += 1;
            calls == cancel_at
        });
        assert!(matches!(result, Err(ByteCheckError::Cancelled)));
        assert_eq!(calls, cancel_at);
        if cancel_at <= 3 {
            assert_eq!(alias.stream_position().unwrap(), 7);
            assert_eq!(output.file_mut().stream_position().unwrap(), 5);
        }
        drop(output);
        drop(view);
        assert_eq!(fs::read(&input.path).unwrap(), b"0123456789");
        let expected = if cancel_at <= 5 {
            &b""[..]
        } else {
            &b"0123456789"[..]
        };
        assert_eq!(fs::read(&output_fixture.path).unwrap(), expected);
        assert_released(&input);
        assert_released(&output_fixture);
    }
}
