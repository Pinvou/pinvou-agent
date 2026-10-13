use super::*;
use crate::create_exclusive_cache;
use std::{
    fs,
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    directory: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "pinvou-exclusive-cache-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        Self { directory }
    }
    fn path(&self, name: &str) -> std::path::PathBuf {
        self.directory.join(name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Only exact names created by this owned fixture; never recursive cleanup
        // through a link or an unrelated directory. Live handles may defer cleanup.
        for name in ["fresh", "renamed", "existing", "alias", "target", "link"] {
            let _ = fs::remove_file(self.path(name));
        }
        let _ = fs::remove_dir(self.path("directory"));
        let _ = fs::remove_dir(&self.directory);
    }
}
#[test]
fn actual_new_cache_denies_data_opens_delete_rename_then_releases_when_all_handles_close() {
    let fixture = Fixture::new();
    let path = fixture.path("fresh");
    let mut cache = create_exclusive_cache(&path).unwrap();
    assert_eq!(cache.file_mut().metadata().unwrap().len(), 0);
    cache.file_mut().write_all(b"cache bytes").unwrap();
    cache.file_mut().sync_all().unwrap();
    assert!(OpenOptions::new().read(true).open(&path).is_err());
    assert!(OpenOptions::new().write(true).open(&path).is_err());
    assert!(fs::remove_file(&path).is_err());
    assert!(fs::rename(&path, fixture.path("renamed")).is_err());
    drop(cache);
    assert_eq!(fs::read(&path).unwrap(), b"cache bytes");
    let mut writer = OpenOptions::new().append(true).open(&path).unwrap();
    writer.write_all(b"!").unwrap();
    drop(writer);
    assert_eq!(fs::read(&path).unwrap(), b"cache bytes!");
    fs::rename(&path, fixture.path("renamed")).unwrap();
    fs::remove_file(fixture.path("renamed")).unwrap();
}
#[test]
fn explicit_trusted_handle_duplicate_keeps_sharing_restriction_after_wrapper_drop() {
    let fixture = Fixture::new();
    let path = fixture.path("fresh");
    let mut cache = create_exclusive_cache(&path).unwrap();
    let duplicate = cache.file_mut().try_clone().unwrap();
    drop(cache);
    assert!(OpenOptions::new().read(true).open(&path).is_err());
    assert!(fs::remove_file(&path).is_err());
    drop(duplicate);
    assert!(OpenOptions::new().read(true).open(&path).is_ok());
    fs::remove_file(&path).unwrap();
}
#[test]
fn existing_regular_hardlink_and_directory_paths_are_never_truncated_or_deleted() {
    let fixture = Fixture::new();
    let path = fixture.path("existing");
    fs::write(&path, b"original").unwrap();
    assert!(matches!(
        create_exclusive_cache(&path),
        Err(CacheObjectError::AlreadyExists)
    ));
    fs::hard_link(&path, fixture.path("alias")).unwrap();
    assert!(matches!(
        create_exclusive_cache(&fixture.path("alias")),
        Err(CacheObjectError::AlreadyExists)
    ));
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert_eq!(fs::read(fixture.path("alias")).unwrap(), b"original");
    fs::create_dir(fixture.path("directory")).unwrap();
    assert!(create_exclusive_cache(&fixture.path("directory")).is_err());
    assert!(fixture.path("directory").is_dir());
}
#[test]
fn actual_handle_checker_refuses_multiple_links_nonempty_file_and_directory() {
    let fixture = Fixture::new();
    let path = fixture.path("existing");
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    assert_eq!(validate(&file), Ok(()));
    fs::hard_link(&path, fixture.path("alias")).unwrap();
    assert_eq!(validate(&file), Err(CacheObjectError::InvalidObject));
    fs::remove_file(fixture.path("alias")).unwrap();
    assert_eq!(validate(&file), Ok(()));
    file.write_all(b"x").unwrap();
    assert_eq!(validate(&file), Err(CacheObjectError::InvalidObject));
    drop(file);
    fs::create_dir(fixture.path("directory")).unwrap();
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(
            windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS
                | FILE_FLAG_OPEN_REPARSE_POINT,
        )
        .open(fixture.path("directory"))
        .unwrap();
    assert_eq!(validate(&directory), Err(CacheObjectError::InvalidObject));
}
#[test]
#[ignore = "requires Windows symlink privilege; run explicitly on an eligible host"]
fn actual_final_symlink_refuses_creation_and_same_handle_checker_refuses_reparse() {
    let fixture = Fixture::new();
    let target = fixture.path("target");
    let link = fixture.path("link");
    fs::write(&target, b"original").unwrap();
    std::os::windows::fs::symlink_file(&target, &link).unwrap();
    assert!(create_exclusive_cache(&link).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"original");
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&link)
        .unwrap();
    assert_eq!(validate(&file), Err(CacheObjectError::InvalidObject));
}

#[test]
fn alternate_data_stream_is_not_a_fresh_physical_cache_file() {
    let fixture = Fixture::new();
    let base = fixture.path("existing");
    let stream = fixture.path("existing:cache");
    fs::write(&base, b"original").unwrap();
    assert!(matches!(
        create_exclusive_cache(&stream),
        Err(CacheObjectError::InvalidPath)
    ));
    assert_eq!(fs::read(&base).unwrap(), b"original");
    assert!(OpenOptions::new().read(true).open(&stream).is_err());
}

#[test]
fn stream_syntax_is_refused_in_all_components_without_rejecting_drive_prefixes() {
    for path in [
        r"C:\fresh",
        r"C:relative",
        r"\\?\C:\fresh",
        r"\\server\share\fresh",
        r"\\?\UNC\server\share\fresh",
    ] {
        assert_eq!(validate_path(Path::new(path)), Ok(()));
    }
    for path in [
        r"fresh:stream",
        r"C:\base:stream",
        r"C:base:stream",
        r"\\?\C:\base:stream",
        r"\\server\share\base:stream",
        r"C:\base::$DATA",
        r"C:\folder:stream\fresh",
        r"\\?\name:stream",
        r"\\.\name:stream",
    ] {
        assert_eq!(
            validate_path(Path::new(path)),
            Err(CacheObjectError::InvalidPath)
        );
    }
    assert_eq!(
        CacheObjectError::InvalidPath.to_string(),
        "CACHE_OBJECT_PATH_INVALID"
    );
}

#[test]
fn actual_read_only_duplicate_same_object_denies_writes_and_shares_cursor_not_sealed_bytes() {
    use std::io::{Read, Seek, SeekFrom};
    let fixture = Fixture::new();
    let mut source = create_exclusive_cache(&fixture.path("fresh")).unwrap();
    source.file_mut().write_all(b"first").unwrap();
    source.file_mut().seek(SeekFrom::Start(0)).unwrap();
    let read_only = source.read_only_duplicate().unwrap();
    let original = observe(source.file_mut()).unwrap();
    let duplicated = observe(read_only.file()).unwrap();
    assert_eq!(
        (
            original.dwVolumeSerialNumber,
            original.nFileIndexHigh,
            original.nFileIndexLow
        ),
        (
            duplicated.dwVolumeSerialNumber,
            duplicated.nFileIndexHigh,
            duplicated.nFileIndexLow
        )
    );
    let mut view = read_only.file();
    let mut bytes = [0; 5];
    view.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"first");
    assert_eq!(source.file_mut().stream_position().unwrap(), 5);
    assert!(view.write_all(b"wrong").is_err());
    assert!(read_only.file().set_len(0).is_err());
    assert!(
        read_only
            .file()
            .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
            .is_err()
    );
    view.seek(SeekFrom::Start(0)).unwrap();
    view.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"first");
    // The original writer still works and its changes are visible through the
    // duplicate: this proves handle permissions, never immutable/sealed content.
    source.file_mut().seek(SeekFrom::Start(0)).unwrap();
    source.file_mut().write_all(b"other").unwrap();
    view.seek(SeekFrom::Start(0)).unwrap();
    view.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"other");
}
#[test]
fn read_only_duplicate_and_explicit_clone_keep_original_share_rules_until_last_close() {
    use std::io::{Read, Seek, SeekFrom};
    let fixture = Fixture::new();
    let path = fixture.path("fresh");
    let mut source = create_exclusive_cache(&path).unwrap();
    source.file_mut().write_all(b"payload").unwrap();
    let read_only = source.read_only_duplicate().unwrap();
    let mut clone = read_only.file().try_clone().unwrap();
    drop(source);
    assert!(OpenOptions::new().read(true).open(&path).is_err());
    assert!(OpenOptions::new().write(true).open(&path).is_err());
    assert!(fs::remove_file(&path).is_err());
    assert!(fs::rename(&path, fixture.path("renamed")).is_err());
    assert!(clone.write_all(b"wrong").is_err());
    assert!(clone.set_len(0).is_err());
    clone.seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = vec![];
    clone.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"payload");
    drop(read_only);
    assert!(OpenOptions::new().read(true).open(&path).is_err());
    drop(clone);
    assert_eq!(fs::read(&path).unwrap(), b"payload");
    let mut writer = OpenOptions::new().append(true).open(&path).unwrap();
    writer.write_all(b"!").unwrap();
    drop(writer);
    fs::rename(&path, fixture.path("renamed")).unwrap();
    assert_eq!(fs::read(fixture.path("renamed")).unwrap(), b"payload!");
    fs::remove_file(fixture.path("renamed")).unwrap();
}
