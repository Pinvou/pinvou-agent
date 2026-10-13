use super::*;
use crate::tests::{OwnedFile, fixture, hex, limits, target, vectors};
use std::io::{self, Read};

fn expected(value: &serde_json::Value) -> ExpectedMember<'_> {
    let member = &value["members"][0];
    ExpectedMember::new(
        member["name"].as_str().unwrap(),
        member["size"].as_u64().unwrap(),
        hex(member["sha256"].as_str().unwrap()).try_into().unwrap(),
    )
    .unwrap()
}
fn error(result: Result<MaterializedMember<'_>, Error>, expected: Error) {
    match result {
        Err(actual) => assert_eq!(actual, expected),
        Ok(_) => panic!("unexpected materialized member"),
    }
}

#[test]
fn actual_store_deflate_descriptor_large_and_empty_members_rescan_the_destination() {
    let all = vectors();
    for case in [
        "store",
        "deflate",
        "descriptor-12",
        "descriptor-16",
        "large-deflate",
        "empty-file",
    ] {
        let value = all.iter().find(|v| v["case"] == case).unwrap();
        let mut source = fixture(value);
        let mut destination = OwnedFile::new();
        destination.file().seek(SeekFrom::Start(9)).unwrap();
        let package = target(source.file(), value);
        let result = materialize_member(
            source.file(),
            destination.file(),
            package,
            expected(value),
            limits(),
            || false,
        )
        .unwrap();
        assert!(result.package() == package);
        assert_eq!(
            result.member().name(),
            value["members"][0]["name"].as_str().unwrap()
        );
        assert_eq!(
            result.member().size(),
            value["members"][0]["size"].as_u64().unwrap()
        );
        assert_eq!(
            destination.file().metadata().unwrap().len(),
            result.member().size()
        );
        assert_eq!(
            destination.file().stream_position().unwrap(),
            result.member().size()
        );
    }
}

#[test]
fn expected_locator_size_hash_directory_and_shape_fail_without_writes() {
    assert!(matches!(
        ExpectedMember::new("a/", 0, [0; 32]),
        Err(Error::InvalidExpectedMember)
    ));
    assert!(matches!(
        ExpectedMember::new("../a", 0, [0; 32]),
        Err(Error::InvalidExpectedMember)
    ));
    assert!(matches!(
        ExpectedMember::new("CON", 0, [0; 32]),
        Err(Error::InvalidExpectedMember)
    ));
    assert!(matches!(
        ExpectedMember::new("a", SAFE_SIZE + 1, [0; 32]),
        Err(Error::InvalidExpectedMember)
    ));
    let all = vectors();
    let value = all.iter().find(|v| v["case"] == "store").unwrap();
    let mut source = fixture(value);
    let package = target(source.file(), value);
    let mut destination = OwnedFile::new();
    for expected in [
        ExpectedMember::new("other", 3, expected(value).sha256).unwrap(),
        ExpectedMember::new(expected(value).name, 2, expected(value).sha256).unwrap(),
        ExpectedMember::new(expected(value).name, 3, [0; 32]).unwrap(),
    ] {
        error(
            materialize_member(
                source.file(),
                destination.file(),
                package,
                expected,
                limits(),
                || false,
            ),
            Error::MemberMismatch,
        );
        assert_eq!(destination.file().metadata().unwrap().len(), 0);
    }
    let value = all
        .iter()
        .find(|v| v["case"] == "directory-after-child")
        .unwrap();
    let mut source = fixture(value);
    let package = target(source.file(), value);
    error(
        materialize_member(
            source.file(),
            destination.file(),
            package,
            ExpectedMember::new("bin", 0, [0; 32]).unwrap(),
            limits(),
            || false,
        ),
        Error::MemberMismatch,
    );
    assert_eq!(destination.file().metadata().unwrap().len(), 0);
}

#[test]
fn actual_nonempty_alias_and_access_failures_leave_no_success() {
    let all = vectors();
    let value = all.iter().find(|v| v["case"] == "store").unwrap();
    let mut source = fixture(value);
    let package = target(source.file(), value);
    let mut destination = OwnedFile::new();
    destination.file().write_all(b"keep").unwrap();
    error(
        materialize_member(
            source.file(),
            destination.file(),
            package,
            expected(value),
            limits(),
            || false,
        ),
        Error::DestinationNotEmpty,
    );
    destination.file().seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = Vec::new();
    destination.file().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"keep");
    let mut alias = source.file().try_clone().unwrap();
    error(
        materialize_member(
            source.file(),
            &mut alias,
            package,
            expected(value),
            limits(),
            || false,
        ),
        Error::DestinationNotEmpty,
    );
    assert!(inspect_file(source.file(), package, limits(), || false).is_ok());
    let mut destination = OwnedFile::new();
    let mut read_only = File::open(&destination.path).unwrap();
    error(
        materialize_member(
            source.file(),
            &mut read_only,
            package,
            expected(value),
            limits(),
            || false,
        ),
        Error::IoUnavailable,
    );
    let mut write_only = std::fs::OpenOptions::new()
        .write(true)
        .open(&destination.path)
        .unwrap();
    error(
        materialize_member(
            source.file(),
            &mut write_only,
            package,
            expected(value),
            limits(),
            || false,
        ),
        Error::InvalidEntryBytes,
    );
    assert_eq!(destination.file().metadata().unwrap().len(), 3);
    let mut empty_destination = OwnedFile::new();
    let mut source_write_only = std::fs::OpenOptions::new()
        .write(true)
        .open(&source.path)
        .unwrap();
    error(
        materialize_member(
            &mut source_write_only,
            empty_destination.file(),
            package,
            expected(value),
            limits(),
            || false,
        ),
        Error::PackageBytes(pinvou_upgrade_byte_check::ByteCheckError::IoUnavailable),
    );
    assert_eq!(empty_destination.file().metadata().unwrap().len(), 0);
}

#[test]
fn actual_visible_source_changes_after_inspection_are_rechecked() {
    let all = vectors();
    let value = all.iter().find(|v| v["case"] == "store").unwrap();
    for payload_change in [false, true] {
        let mut source = fixture(value);
        let package = target(source.file(), value);
        let mut inspect_polls = 0;
        assert!(
            inspect_file(source.file(), package, limits(), || {
                inspect_polls += 1;
                false
            })
            .is_ok()
        );
        let raw = hex(value["hex"].as_str().unwrap());
        let offset = if payload_change {
            30 + "bin/installer.exe".len()
        } else {
            raw.windows(4).position(|v| v == b"PK\x01\x02").unwrap() + 38
        };
        let mut alias = source.file().try_clone().unwrap();
        let mut destination = OwnedFile::new();
        let mut polls = 0;
        let result = materialize_member(
            source.file(),
            destination.file(),
            package,
            expected(value),
            limits(),
            || {
                polls += 1;
                if polls == inspect_polls + 2 {
                    alias.seek(SeekFrom::Start(offset as u64)).unwrap();
                    alias
                        .write_all(if payload_change { b"x" } else { b"\x20" })
                        .unwrap();
                }
                false
            },
        );
        error(
            result,
            if payload_change {
                Error::InvalidEntryBytes
            } else {
                Error::PackageBytes(pinvou_upgrade_byte_check::ByteCheckError::HashMismatch)
            },
        );
        assert_eq!(destination.file().metadata().unwrap().len(), 3);
    }
}

#[test]
fn private_sync_failure_and_visible_destination_pollution_return_no_materialization() {
    let all = vectors();
    let value = all.iter().find(|v| v["case"] == "store").unwrap();
    for polluted in [false, true] {
        let mut source = fixture(value);
        let package = target(source.file(), value);
        let mut destination = OwnedFile::new();
        let mut synced = false;
        let result = materialize_with_sync(
            source.file(),
            destination.file(),
            package,
            expected(value),
            limits(),
            &mut || false,
            &mut |file| {
                synced = true;
                if !polluted {
                    return Err(Error::IoUnavailable);
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
                Error::InvalidEntryBytes
            } else {
                Error::IoUnavailable
            },
        );
        assert_eq!(destination.file().metadata().unwrap().len(), 3);
    }
}

#[test]
fn every_store_deflate_cancel_poll_and_later_corrupt_entry_leave_no_result() {
    let all = vectors();
    for case in ["store", "deflate"] {
        let value = all.iter().find(|v| v["case"] == case).unwrap();
        let mut source = fixture(value);
        let package = target(source.file(), value);
        let mut destination = OwnedFile::new();
        let mut total = 0;
        assert!(
            materialize_member(
                source.file(),
                destination.file(),
                package,
                expected(value),
                limits(),
                || {
                    total += 1;
                    false
                }
            )
            .is_ok()
        );
        for boundary in 1..=total {
            let mut destination = OwnedFile::new();
            let mut polls = 0;
            error(
                materialize_member(
                    source.file(),
                    destination.file(),
                    package,
                    expected(value),
                    limits(),
                    || {
                        polls += 1;
                        polls == boundary
                    },
                ),
                Error::Cancelled,
            );
        }
    }
    let value = all.iter().find(|v| v["case"] == "later-bad-entry").unwrap();
    let mut source = fixture(value);
    let package = target(source.file(), value);
    let mut destination = OwnedFile::new();
    error(
        materialize_member(
            source.file(),
            destination.file(),
            package,
            expected(value),
            limits(),
            || false,
        ),
        Error::InvalidEntryBytes,
    );
    assert_eq!(destination.file().metadata().unwrap().len(), 0);
}

#[test]
fn private_partial_interrupted_zero_and_cancelled_writes_are_exact() {
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
    let mut writer = Partial {
        bytes: Vec::new(),
        interrupted: false,
    };
    write_chunk(&mut writer, b"abc", &mut || false).unwrap();
    assert_eq!(writer.bytes, b"abc");
    let mut writer = Partial {
        bytes: Vec::new(),
        interrupted: true,
    };
    let mut polls = 0;
    assert_eq!(
        write_chunk(&mut writer, b"abc", &mut || {
            polls += 1;
            polls == 2
        }),
        Err(Error::Cancelled)
    );
    assert_eq!(writer.bytes, b"a");
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
        Err(Error::IoUnavailable)
    );
}
