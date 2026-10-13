use super::*;
use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
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
            "pinvou-zip-check-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
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
pub(super) fn hex(raw: &str) -> Vec<u8> {
    raw.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
pub(super) fn vectors() -> Vec<Value> {
    serde_json::from_str(include_str!("../tests/fixtures/zip-vectors.json")).unwrap()
}
pub(super) fn limits() -> ArchiveLimits {
    ArchiveLimits::new(
        64 * 1024 * 1024,
        128 * 1024 * 1024,
        100,
        1024 * 1024,
        1024 * 1024,
        1_000_000,
    )
    .unwrap()
}
pub(super) fn fixture(value: &Value) -> OwnedFile {
    let mut owned = OwnedFile::new();
    if let Some(raw) = value["hex"].as_str() {
        owned.file().write_all(&hex(raw)).unwrap();
    } else {
        owned
            .file()
            .write_all(&hex(value["headerHex"].as_str().unwrap()))
            .unwrap();
        let bytes = vec![b'x'; 65_536];
        let mut remaining = value["repeat"].as_u64().unwrap();
        while remaining != 0 {
            let count = remaining.min(bytes.len() as u64) as usize;
            owned.file().write_all(&bytes[..count]).unwrap();
            remaining -= count as u64;
        }
        owned
            .file()
            .write_all(&hex(value["tailHex"].as_str().unwrap()))
            .unwrap();
    }
    owned
}
pub(super) fn target(file: &mut File, value: &Value) -> Artifact<'static> {
    Artifact::new(
        "package-1",
        file.metadata().unwrap().len(),
        value["sha256"].as_str().unwrap(),
    )
    .unwrap()
}

#[test]
fn independent_zip32_vectors_validate_complete_inventory_or_fixed_failure() {
    for value in vectors() {
        let name = value["case"].as_str().unwrap();
        let mut owned = fixture(&value);
        let original = target(owned.file(), &value);
        let result = inspect_file(owned.file(), original, limits(), || false);
        if let Some(expected) = value["error"].as_str() {
            match result {
                Err(error) => assert_eq!(error.to_string(), expected, "{name}"),
                Ok(_) => panic!("unexpected acceptance: {name}"),
            }
        } else {
            let inventory = result.unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(inventory.target() == original);
            let expected = value["members"].as_array().unwrap();
            assert_eq!(inventory.members().len(), expected.len(), "{name}");
            for member in inventory.members() {
                let expected = expected
                    .iter()
                    .find(|e| e["name"].as_str().unwrap() == member.name())
                    .unwrap();
                assert_eq!(member.size(), expected["size"].as_u64().unwrap(), "{name}");
                assert_eq!(
                    member.sha256().as_slice(),
                    hex(expected["sha256"].as_str().unwrap()),
                    "{name}"
                );
            }
        }
    }
}

#[test]
fn resource_limits_are_explicit_and_cover_metadata_paths_entries_and_expansion() {
    for invalid in [
        ArchiveLimits::new(0, 1, 1, 1, 1, 1),
        ArchiveLimits::new(1, 0, 1, 1, 1, 1),
        ArchiveLimits::new(1, 1, 0, 1, 1, 1),
        ArchiveLimits::new(1, 1, 4097, 1, 1, 1),
        ArchiveLimits::new(1, 1, 1, MAX_METADATA + 1, 1, 1),
        ArchiveLimits::new(1, 1, 1, 1, MAX_METADATA + 1, 1),
        ArchiveLimits::new(SAFE_SIZE + 1, 1, 1, 1, 1, 1),
        ArchiveLimits::new(1, 1, 1, 1, 1, 0),
    ] {
        assert!(matches!(invalid, Err(ArchiveError::InvalidLimits)));
    }
    let all = vectors();
    let value = all.iter().find(|v| v["case"] == "store").unwrap();
    let mut file = fixture(value);
    let original = target(file.file(), value);
    for limited in [
        ArchiveLimits::new(2, 100, 100, 10000, 10000, 100).unwrap(),
        ArchiveLimits::new(100, 2, 100, 10000, 10000, 100).unwrap(),
        ArchiveLimits::new(100, 100, 100, 1, 10000, 100).unwrap(),
        ArchiveLimits::new(100, 100, 100, 10000, 1, 100).unwrap(),
    ] {
        assert!(matches!(
            inspect_file(file.file(), original, limited, || false),
            Err(ArchiveError::ResourceLimit)
        ));
    }
    let value = all.iter().find(|v| v["case"] == "central-order").unwrap();
    let mut file = fixture(value);
    let original = target(file.file(), value);
    assert!(matches!(
        inspect_file(
            file.file(),
            original,
            ArchiveLimits::new(100, 100, 1, 10000, 10000, 100).unwrap(),
            || false
        ),
        Err(ArchiveError::ResourceLimit)
    ));
    let value = all.iter().find(|v| v["case"] == "deflate").unwrap();
    let mut file = fixture(value);
    let original = target(file.file(), value);
    assert!(matches!(
        inspect_file(
            file.file(),
            original,
            ArchiveLimits::new(100000, 100000, 100, 10000, 10000, 1).unwrap(),
            || false
        ),
        Err(ArchiveError::ResourceLimit)
    ));
}

#[test]
fn original_package_hash_io_and_every_cancellation_boundary_return_no_inventory() {
    let all = vectors();
    let value = all.iter().find(|v| v["case"] == "store").unwrap();
    let mut file = fixture(value);
    let bad = Artifact::new(
        "package-1",
        file.file().metadata().unwrap().len(),
        &"a".repeat(64),
    )
    .unwrap();
    assert!(matches!(
        inspect_file(file.file(), bad, limits(), || false),
        Err(ArchiveError::PackageBytes(
            pinvou_upgrade_byte_check::ByteCheckError::HashMismatch
        ))
    ));
    let original = target(file.file(), value);
    for kind in ["store", "deflate"] {
        let value = all.iter().find(|v| v["case"] == kind).unwrap();
        let mut owned = fixture(value);
        let target = target(owned.file(), value);
        let mut polls = 0;
        assert!(
            inspect_file(owned.file(), target, limits(), || {
                polls += 1;
                false
            })
            .is_ok()
        );
        for boundary in 1..=polls {
            let mut now = 0;
            assert!(
                matches!(
                    inspect_file(owned.file(), target, limits(), || {
                        now += 1;
                        now == boundary
                    }),
                    Err(ArchiveError::Cancelled)
                ),
                "{kind} cancel poll {boundary}"
            );
        }
    }
    let mut write_only = std::fs::OpenOptions::new()
        .write(true)
        .open(&file.path)
        .unwrap();
    assert!(matches!(
        inspect_file(&mut write_only, original, limits(), || false),
        Err(ArchiveError::PackageBytes(
            pinvou_upgrade_byte_check::ByteCheckError::IoUnavailable
        ))
    ));
    assert!(std::error::Error::source(&ArchiveError::IoUnavailable).is_none());
}
