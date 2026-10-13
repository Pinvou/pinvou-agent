use super::*;
use std::io::{self, Cursor, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) const ABC_HASH: &str =
    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const TEN_HASH: &str = "84d89877f0d4041efb6bf91a16f0248f2fd573e6af05c19f96bedb9f882f7882";
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

pub(super) struct OwnedFile {
    pub(super) path: PathBuf,
    file: Option<File>,
}
impl OwnedFile {
    pub(super) fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pinvou-byte-check-{}-{nonce}-{}",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        Self {
            path,
            file: Some(file),
        }
    }
    pub(super) fn file(&mut self) -> &mut File {
        self.file.as_mut().unwrap()
    }
}
impl Drop for OwnedFile {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}
pub(super) fn target(size: u64, digest: &str) -> Artifact<'static> {
    Artifact::new("package-1", size, digest).unwrap()
}
pub(super) fn error(result: Result<ByteMatch<'_>, ByteCheckError>, expected: ByteCheckError) {
    match result {
        Err(actual) => assert_eq!(actual, expected),
        Ok(_) => panic!("unexpected byte match"),
    }
}

#[test]
fn actual_file_starts_at_zero_and_binds_the_original_target() {
    let mut owned = OwnedFile::new();
    owned.file().write_all(b"0123456789").unwrap();
    owned.file().seek(SeekFrom::Start(7)).unwrap();
    let original = target(10, TEN_HASH);
    let result = check_file(owned.file(), original, || false).unwrap();
    assert!(result.target() == original);
    assert_eq!(owned.file().stream_position().unwrap(), 10);
}

#[test]
fn actual_short_appended_and_equal_length_polluted_bytes_refuse() {
    for (bytes, expected) in [
        (b"ab".as_slice(), ByteCheckError::TooShort),
        (b"abcd".as_slice(), ByteCheckError::TooLong),
        (b"abd".as_slice(), ByteCheckError::HashMismatch),
        (b"".as_slice(), ByteCheckError::TooShort),
    ] {
        let mut owned = OwnedFile::new();
        owned.file().write_all(bytes).unwrap();
        error(
            check_file(owned.file(), target(3, ABC_HASH), || false),
            expected,
        );
    }
}

#[test]
fn actual_chunk_boundary_and_more_than_32_mib_have_no_artifact_cap() {
    // Expected digests independently generated with Python hashlib, not this crate.
    for (length, digest) in [
        (
            1_u64,
            "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881",
        ),
        (
            65_535,
            "09ab7495d3e61a76f0deb12cb0306f0696cbb17ffc12131368c7a939f12f56d3",
        ),
        (
            65_536,
            "1f8745f0d2d1387ec1af2211a3cf417b2e9e885e853472649c1d979d0e9370e3",
        ),
        (
            65_537_u64,
            "1abe08ebecf1c18cab71f6fe28aaddf20268f85bad78bb9a72f88ca47c874662",
        ),
        (
            33 * 1024 * 1024,
            "7d641a2a7b1c449f586b444bd47a2005d83be0e142596f6d1a52da9c2256c96b",
        ),
    ] {
        let mut owned = OwnedFile::new();
        let block = vec![b'x'; BUFFER_BYTES];
        let mut remaining = length;
        while remaining != 0 {
            let count = remaining.min(BUFFER_BYTES as u64) as usize;
            owned.file().write_all(&block[..count]).unwrap();
            remaining -= count as u64;
        }
        owned.file().sync_all().unwrap();
        let result = check_file(owned.file(), target(length, digest), || false).unwrap();
        assert_eq!(result.target().size(), length);
    }
}

#[test]
fn actual_cancel_before_seek_and_after_eof_cannot_return_a_match() {
    let mut owned = OwnedFile::new();
    owned.file().write_all(b"abc").unwrap();
    error(
        check_file(owned.file(), target(3, ABC_HASH), || true),
        ByteCheckError::Cancelled,
    );
    assert_eq!(owned.file().stream_position().unwrap(), 3);
    let mut polls = 0;
    // Before seek, before first read, before EOF read, then after EOF.
    error(
        check_file(owned.file(), target(3, ABC_HASH), || {
            polls += 1;
            polls == 4
        }),
        ByteCheckError::Cancelled,
    );
    assert_eq!(polls, 4);
    assert_eq!(owned.file().stream_position().unwrap(), 3);
    let mut large = OwnedFile::new();
    large.file().write_all(&vec![b'x'; 65_537]).unwrap();
    let mut polls = 0;
    error(
        check_file(
            large.file(),
            target(
                65_537,
                "1abe08ebecf1c18cab71f6fe28aaddf20268f85bad78bb9a72f88ca47c874662",
            ),
            || {
                polls += 1;
                polls == 3
            },
        ),
        ByteCheckError::Cancelled,
    );
    assert_eq!(large.file().stream_position().unwrap(), 65_536);
}

#[test]
fn actual_unreadable_handle_fails_without_io_source_or_path() {
    let mut owned = OwnedFile::new();
    owned.file().write_all(b"abc").unwrap();
    let mut write_only = std::fs::OpenOptions::new()
        .write(true)
        .open(&owned.path)
        .unwrap();
    error(
        check_file(&mut write_only, target(3, ABC_HASH), || false),
        ByteCheckError::IoUnavailable,
    );
    let failure = ByteCheckError::IoUnavailable;
    assert!(std::error::Error::source(&failure).is_none());
    assert_eq!(failure.to_string(), "FILE_CHECK_IO_UNAVAILABLE");
}

#[test]
fn actual_pre_scan_truncate_append_and_first_middle_tail_pollution_refuse() {
    for offset in [0, 5, 9] {
        let mut owned = OwnedFile::new();
        owned.file().write_all(b"0123456789").unwrap();
        owned.file().seek(SeekFrom::Start(offset)).unwrap();
        owned.file().write_all(b"x").unwrap();
        error(
            check_file(owned.file(), target(10, TEN_HASH), || false),
            ByteCheckError::HashMismatch,
        );
    }
    let mut owned = OwnedFile::new();
    owned.file().write_all(b"0123456789").unwrap();
    owned.file().set_len(9).unwrap();
    error(
        check_file(owned.file(), target(10, TEN_HASH), || false),
        ByteCheckError::TooShort,
    );
    owned.file().seek(SeekFrom::End(0)).unwrap();
    owned.file().write_all(b"9x").unwrap();
    error(
        check_file(owned.file(), target(10, TEN_HASH), || false),
        ByteCheckError::TooLong,
    );
}

#[test]
fn private_seek_failure_and_repeated_interruption_never_return_a_match() {
    struct Unseekable;
    impl Read for Unseekable {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("read after failed seek")
        }
    }
    impl Seek for Unseekable {
        fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
            Err(io::Error::other("private handle detail"))
        }
    }
    error(
        check_seekable(&mut Unseekable, target(3, ABC_HASH), &mut || false),
        ByteCheckError::IoUnavailable,
    );
    struct Interrupted;
    impl Read for Interrupted {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::Interrupted.into())
        }
    }
    let mut polls = 0;
    error(
        check_reader(&mut Interrupted, target(3, ABC_HASH), &mut || {
            polls += 1;
            polls == 3
        }),
        ByteCheckError::Cancelled,
    );
    assert_eq!(polls, 3);
}

#[test]
fn interrupted_read_retries_and_polling_allows_cancel_or_io_failure() {
    struct Interrupted {
        data: Cursor<Vec<u8>>,
        once: bool,
    }
    impl Read for Interrupted {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if !self.once {
                self.once = true;
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.data.read(buffer)
        }
    }
    let mut reader = Interrupted {
        data: Cursor::new(b"abc".to_vec()),
        once: false,
    };
    assert!(check_reader(&mut reader, target(3, ABC_HASH), &mut || false).is_ok());
    struct Fault {
        bytes: bool,
    }
    impl Read for Fault {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if !self.bytes {
                self.bytes = true;
                buffer[..3].copy_from_slice(b"abc");
                return Ok(3);
            }
            Err(io::Error::other("private source detail"))
        }
    }
    error(
        check_reader(
            &mut Fault { bytes: false },
            target(3, ABC_HASH),
            &mut || false,
        ),
        ByteCheckError::IoUnavailable,
    );
    let mut reader = Interrupted {
        data: Cursor::new(b"abc".to_vec()),
        once: false,
    };
    let mut polls = 0;
    error(
        check_reader(&mut reader, target(3, ABC_HASH), &mut || {
            polls += 1;
            polls == 2
        }),
        ByteCheckError::Cancelled,
    );
}
