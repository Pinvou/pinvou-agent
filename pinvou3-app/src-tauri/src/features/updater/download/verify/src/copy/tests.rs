use super::*;
use crate::tests::{ABC_HASH, OwnedFile, error, target};
use std::io::{self, Read};

fn source() -> OwnedFile {
    let mut source = OwnedFile::new();
    source.file().write_all(b"abc").unwrap();
    source
}

#[test]
fn actual_copy_starts_both_handles_at_zero_and_checks_large_destination_bytes() {
    for (size, digest) in [
        (
            65_537_u64,
            "1abe08ebecf1c18cab71f6fe28aaddf20268f85bad78bb9a72f88ca47c874662",
        ),
        (
            33 * 1024 * 1024,
            "7d641a2a7b1c449f586b444bd47a2005d83be0e142596f6d1a52da9c2256c96b",
        ),
    ] {
        let mut source = OwnedFile::new();
        let mut destination = OwnedFile::new();
        let buffer = vec![b'x'; 65_536];
        let mut remaining = size;
        while remaining != 0 {
            let count = remaining.min(buffer.len() as u64) as usize;
            source.file().write_all(&buffer[..count]).unwrap();
            remaining -= count as u64;
        }
        source.file().seek(SeekFrom::Start(1)).unwrap();
        destination.file().seek(SeekFrom::Start(5)).unwrap();
        let original = target(size, digest);
        let matched =
            copy_and_check(source.file(), destination.file(), original, || false).unwrap();
        assert!(matched.target() == original);
        assert_eq!(destination.file().metadata().unwrap().len(), size);
        assert_eq!(destination.file().stream_position().unwrap(), size);
    }
}

#[test]
fn actual_nonempty_destination_and_same_file_clone_refuse_without_overwrite() {
    let mut source = source();
    let mut destination = OwnedFile::new();
    destination.file().write_all(b"keep").unwrap();
    error(
        copy_and_check(
            source.file(),
            destination.file(),
            target(3, ABC_HASH),
            || false,
        ),
        ByteCheckError::DestinationNotEmpty,
    );
    destination.file().seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = Vec::new();
    destination.file().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"keep");
    let mut alias = source.file().try_clone().unwrap();
    error(
        copy_and_check(source.file(), &mut alias, target(3, ABC_HASH), || false),
        ByteCheckError::DestinationNotEmpty,
    );
    assert!(crate::check_file(source.file(), target(3, ABC_HASH), || false).is_ok());
}

#[test]
fn actual_source_pollution_after_an_old_match_is_rechecked_and_not_promoted() {
    let mut source = source();
    assert!(crate::check_file(source.file(), target(3, ABC_HASH), || false).is_ok());
    source.file().seek(SeekFrom::Start(1)).unwrap();
    source.file().write_all(b"x").unwrap();
    let mut destination = OwnedFile::new();
    error(
        copy_and_check(
            source.file(),
            destination.file(),
            target(3, ABC_HASH),
            || false,
        ),
        ByteCheckError::HashMismatch,
    );
    // Failure deliberately leaves untrusted copied bytes for owner cleanup.
    assert_eq!(destination.file().metadata().unwrap().len(), 3);
    destination.file().seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = Vec::new();
    destination.file().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"axc");
}

#[test]
fn actual_read_write_permission_failures_never_return_a_destination_match() {
    let mut source = source();
    let mut destination = OwnedFile::new();
    let mut read_only = File::open(&destination.path).unwrap();
    error(
        copy_and_check(source.file(), &mut read_only, target(3, ABC_HASH), || false),
        ByteCheckError::IoUnavailable,
    );
    let mut source_write_only = std::fs::OpenOptions::new()
        .write(true)
        .open(&source.path)
        .unwrap();
    error(
        copy_and_check(
            &mut source_write_only,
            destination.file(),
            target(3, ABC_HASH),
            || false,
        ),
        ByteCheckError::IoUnavailable,
    );
    assert_eq!(destination.file().metadata().unwrap().len(), 0);
    let mut destination_write_only = std::fs::OpenOptions::new()
        .write(true)
        .open(&destination.path)
        .unwrap();
    error(
        copy_and_check(
            source.file(),
            &mut destination_write_only,
            target(3, ABC_HASH),
            || false,
        ),
        ByteCheckError::IoUnavailable,
    );
    assert_eq!(destination.file().metadata().unwrap().len(), 3);
}

#[test]
fn private_sync_failure_and_visible_destination_pollution_are_not_success() {
    for polluted in [false, true] {
        let mut source = source();
        let mut destination = OwnedFile::new();
        let mut synced = false;
        let result = copy_with_sync(
            source.file(),
            destination.file(),
            target(3, ABC_HASH),
            &mut || false,
            &mut |file| {
                synced = true;
                if !polluted {
                    return Err(ByteCheckError::IoUnavailable);
                }
                let mut alias = file.try_clone().unwrap();
                alias.seek(SeekFrom::Start(1)).unwrap();
                alias.write_all(b"x").unwrap();
                file.sync_all().unwrap();
                Ok(())
            },
        );
        assert!(synced);
        error(
            result,
            if polluted {
                ByteCheckError::HashMismatch
            } else {
                ByteCheckError::IoUnavailable
            },
        );
        assert_eq!(destination.file().metadata().unwrap().len(), 3);
    }
}

#[test]
fn actual_cancellation_at_copy_sync_and_destination_eof_boundaries_never_matches() {
    // Exact polling positions for a single three-byte chunk. The final polls
    // cover source EOF, before sync, after sync and destination EOF.
    for cancel_at in [1, 4, 6, 7, 8, 11] {
        let mut source = source();
        let mut destination = OwnedFile::new();
        let mut polls = 0;
        error(
            copy_and_check(
                source.file(),
                destination.file(),
                target(3, ABC_HASH),
                || {
                    polls += 1;
                    polls == cancel_at
                },
            ),
            ByteCheckError::Cancelled,
        );
        assert_eq!(polls, cancel_at);
    }
}

#[test]
fn private_partial_writes_interruption_zero_and_between_write_cancel_are_bounded() {
    struct Partial {
        bytes: Vec<u8>,
        interrupted: bool,
    }
    impl Write for Partial {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.bytes.push(bytes[0]);
            Ok(1)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut partial = Partial {
        bytes: Vec::new(),
        interrupted: false,
    };
    write_chunk(&mut partial, b"abc", &mut || false).unwrap();
    assert_eq!(partial.bytes, b"abc");
    let mut partial = Partial {
        bytes: Vec::new(),
        interrupted: true,
    };
    let mut polls = 0;
    assert_eq!(
        write_chunk(&mut partial, b"abc", &mut || {
            polls += 1;
            polls == 2
        }),
        Err(ByteCheckError::Cancelled)
    );
    assert_eq!(partial.bytes, b"a");
    struct Zero;
    impl Write for Zero {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Ok(0)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    assert_eq!(
        write_chunk(&mut Zero, b"abc", &mut || false),
        Err(ByteCheckError::IoUnavailable)
    );
}
