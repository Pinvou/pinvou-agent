use crate::{
    tests::{fixture, imported},
    *,
};
use serde_json::Value;
use std::{fs, io::Write, path::PathBuf, process::Command};

fn create_fixture() -> (PathBuf, String) {
    let name = format!(
        "pinvou-download-job-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let path = std::env::temp_dir().join(name);
    let f = fixture();
    let pin = create(
        &path,
        f["root"].as_str().unwrap().into(),
        f["rootSha256"].as_str().unwrap().into(),
        f["package"].as_str().unwrap().into(),
        f["expected"].as_str().unwrap().into(),
        &imported(),
    )
    .unwrap();
    (path, pin)
}

#[test]
fn actual_single_writer_and_held_ancestors_refuse_replacement_until_close() {
    let (path, pin) = create_fixture();
    let job = Job::open(&path, &pin).unwrap();
    assert!(matches!(Job::open(&path, &pin), Err(Error::Busy)));
    let renamed = path.with_extension("moved");
    assert!(fs::rename(&path, &renamed).is_err());
    assert!(fs::remove_file(path.join("writer.lock")).is_err());
    drop(job);
    fs::rename(&path, &renamed).unwrap();
    assert!(Job::open(&renamed, &pin).is_ok());
    fs::remove_dir_all(&renamed).unwrap();
}

#[test]
fn journal_damage_is_not_missing_and_does_not_overwrite_or_heal_original_bytes() {
    let (path, pin) = create_fixture();
    let source = fs::read(path.join("journal.jsonl")).unwrap();
    for fault in [
        b"null\n".as_slice(),
        b"false\n",
        b"0\n",
        b"\"\"\n",
        b"{}\n",
        b"{",
        b"",
    ] {
        fs::write(path.join("journal.jsonl"), fault).unwrap();
        assert!(matches!(Job::open(&path, &pin), Err(Error::JournalDamaged)));
        assert_eq!(fs::read(path.join("journal.jsonl")).unwrap(), fault);
        fs::write(path.join("journal.jsonl"), &source).unwrap();
        assert!(Job::open(&path, &pin).is_ok());
    }
    let mut huge = source.clone();
    huge.extend(vec![b' '; RECORD_LIMIT + 1]);
    fs::write(path.join("journal.jsonl"), &huge).unwrap();
    assert!(matches!(Job::open(&path, &pin), Err(Error::JournalDamaged)));
    assert_eq!(fs::read(path.join("journal.jsonl")).unwrap(), huge);
    fs::write(path.join("journal.jsonl"), source).unwrap();
    assert!(Job::open(&path, &pin).is_ok());
    fs::remove_dir_all(&path).unwrap();
}

#[test]
fn complete_record_requires_real_same_handle_scan_and_rejects_same_size_pollution() {
    let (path, pin) = create_fixture();
    let mut job = Job::open(&path, &pin).unwrap();
    job.journal
        .append(journal::Phase::Running, 1, None)
        .unwrap();
    let mut file = platform::file(&path.join(store::attempt_name(1)), true, true, true).unwrap();
    let id = platform::identity(&file).unwrap();
    job.journal
        .append(journal::Phase::Opened, 1, Some(id))
        .unwrap();
    file.write_all(b"abc").unwrap();
    file.sync_all().unwrap();
    drop(file);
    job.journal
        .append(journal::Phase::BytesMatch, 1, Some(id))
        .unwrap();
    assert!(!job.snapshot().observed_bytes_match);
    assert!(job.check(|| false).unwrap().observed_bytes_match);
    drop(job);
    let original = fs::read(path.join(store::attempt_name(1))).unwrap();
    fs::write(path.join(store::attempt_name(1)), b"axc").unwrap();
    assert!(matches!(
        Job::open(&path, &pin).unwrap().check(|| false),
        Err(Error::ByteMismatch)
    ));
    assert_eq!(fs::read(path.join(store::attempt_name(1))).unwrap(), b"axc");
    fs::write(path.join(store::attempt_name(1)), original).unwrap();
    let mut job = Job::open(&path, &pin).unwrap();
    assert!(job.check(|| false).unwrap().observed_bytes_match);
    assert!(matches!(job.check(|| true), Err(Error::Cancelled)));
    drop(job);
    let damaged = b"null\n";
    fs::write(path.join("journal.jsonl"), damaged).unwrap();
    let recovered = observe_retained(&path, &pin, 1, || false).unwrap();
    assert!(recovered.facts().observed_bytes_match);
    assert_eq!(recovered.facts().recorded_phase, "journalNotConsulted");
    assert_eq!(fs::read(path.join("journal.jsonl")).unwrap(), damaged);
    assert!(matches!(Job::open(&path, &pin), Err(Error::Busy)));
    drop(recovered);
    assert!(matches!(Job::open(&path, &pin), Err(Error::JournalDamaged)));
    fs::remove_dir_all(&path).unwrap();
}

#[test]
fn manifest_pin_and_closed_journal_never_follow_injected_paths() {
    let (path, pin) = create_fixture();
    let raw = fs::read(path.join("journal.jsonl")).unwrap();
    let mut v: Value = serde_json::from_slice(&raw).unwrap();
    v["path"] = Value::String("../foreign".into());
    let mut fault = json(&v).unwrap();
    fault.push(b'\n');
    fs::write(path.join("journal.jsonl"), fault).unwrap();
    assert!(matches!(Job::open(&path, &pin), Err(Error::JournalDamaged)));
    fs::write(path.join("journal.jsonl"), &raw).unwrap();
    assert!(matches!(
        Job::open(&path, &"f".repeat(64)),
        Err(Error::PinMismatch)
    ));
    assert!(Job::open(&path, &pin).is_ok());
    fs::remove_dir_all(&path).unwrap();
}

#[test]
fn actual_replacement_writer_payload_links_and_reparse_ancestors_are_refused() {
    let (path, pin) = create_fixture();
    let lock = path.join("writer.lock");
    let retained_lock = path.join("writer-original.lock");
    fs::rename(&lock, &retained_lock).unwrap();
    fs::write(&lock, b"").unwrap();
    assert!(matches!(Job::open(&path, &pin), Err(Error::UnsafeObject)));
    fs::remove_file(&lock).unwrap();
    fs::rename(&retained_lock, &lock).unwrap();
    assert!(Job::open(&path, &pin).is_ok());

    let payload = path.join(store::attempt_name(1));
    fs::write(&payload, b"abc").unwrap();
    let alias = path.join("payload-alias");
    fs::hard_link(&payload, &alias).unwrap();
    assert!(matches!(
        observe_retained(&path, &pin, 1, || false),
        Err(Error::UnsafeObject)
    ));
    assert_eq!(fs::read(&payload).unwrap(), b"abc");
    fs::remove_file(&alias).unwrap();
    assert!(observe_retained(&path, &pin, 1, || false).is_ok());

    // A junction needs no symlink privilege on the supported Windows fixture.
    // This actual child is joined by output(); only our unique temp names enter
    // its fixed command. No external tree is removed or traversed for cleanup.
    let junction = path.with_extension("junction");
    assert_eq!(junction.parent(), Some(std::env::temp_dir().as_path()));
    let status = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "owned junction fixture creation failed"
    );
    assert!(matches!(
        Job::open(&junction, &pin),
        Err(Error::UnsafeObject)
    ));
    assert!(matches!(
        read_input(&junction.join("job.json"), MANIFEST_LIMIT),
        Err(Error::UnsafeObject)
    ));
    fs::remove_dir(&junction).unwrap();
    assert!(Job::open(&path, &pin).is_ok());
    fs::remove_dir_all(&path).unwrap();
}

#[test]
fn control_damage_and_failed_journal_write_preserve_the_original_source() {
    let (path, pin) = create_fixture();
    assert!(matches!(
        create(
            &path,
            fixture()["root"].as_str().unwrap().into(),
            fixture()["rootSha256"].as_str().unwrap().into(),
            fixture()["package"].as_str().unwrap().into(),
            fixture()["expected"].as_str().unwrap().into(),
            &imported()
        ),
        Err(Error::AlreadyExists)
    ));
    let run_id = "a".repeat(64);
    request_stop(&path, &pin, &run_id, StopRequest::Pause).unwrap();
    request_stop(&path, &pin, &run_id, StopRequest::Pause).unwrap();
    assert!(matches!(
        request_stop(&path, &pin, &run_id, StopRequest::Cancel),
        Err(Error::AlreadyExists)
    ));
    let control = path.join(format!("control-{run_id}.json"));
    let original = fs::read(&control).unwrap();
    fs::write(&control, b"null").unwrap();
    let job = Job::open(&path, &pin).unwrap();
    assert!(matches!(job.control(&run_id), Err(Error::JournalDamaged)));
    assert_eq!(fs::read(&control).unwrap(), b"null");
    drop(job);
    fs::write(&control, &original).unwrap();
    assert!(Job::open(&path, &pin).unwrap().control(&run_id).unwrap() == Some(StopRequest::Pause));

    let journal_path = path.join("journal.jsonl");
    let original = fs::read(&journal_path).unwrap();
    // An actual read-only handle makes the write fail, rather than a mock
    // success. The caller receives IO uncertainty and no appended success.
    let file = platform::file(&journal_path, false, false, true).unwrap();
    let mut journal = journal::Journal::open(file, &pin).unwrap();
    assert!(matches!(
        journal.append(journal::Phase::Running, 1, None),
        Err(Error::IoUnavailable)
    ));
    drop(journal);
    assert_eq!(fs::read(&journal_path).unwrap(), original);
    assert!(Job::open(&path, &pin).is_ok());
    fs::remove_dir_all(&path).unwrap();
}

fn attribute_directory(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    std::fs::OpenOptions::new()
        .access_mode(0x100) // FILE_WRITE_ATTRIBUTES bypasses ordinary share-write denial.
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

// Test-only actual mountpoint conversion. No production control or extra SDK
// feature is exposed. The kernel call requires a writable directory handle.
#[allow(unsafe_code)]
fn mountpoint(path: &std::path::Path, target: Option<&std::path::Path>) -> std::io::Result<()> {
    use std::{
        ffi::c_void,
        os::windows::{ffi::OsStrExt, io::AsRawHandle},
    };
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn DeviceIoControl(
            device: *mut c_void,
            code: u32,
            input: *mut c_void,
            input_size: u32,
            output: *mut c_void,
            output_size: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
    }
    let file = attribute_directory(path)?;
    let mut data = Vec::new();
    if let Some(target) = target {
        let print: Vec<u8> = target
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect();
        let substitute: Vec<u8> = std::ffi::OsString::from(format!("\\??\\{}", target.display()))
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect();
        for value in [
            0,
            substitute.len() as u16,
            substitute.len() as u16 + 2,
            print.len() as u16,
        ] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        data.extend_from_slice(&substitute);
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(&print);
        data.extend_from_slice(&[0, 0]);
    }
    let mut buffer = 0xa0000003u32.to_le_bytes().to_vec();
    buffer.extend_from_slice(&(data.len() as u16).to_le_bytes());
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer.extend(data);
    let mut returned = 0u32;
    // SAFETY: File owns a synchronous, live directory handle. The mountpoint
    // buffer has the documented u32/u16 header and initialized UTF-16 regions;
    // all storage remains live for this call. No output/overlapped is supplied.
    let success = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            if target.is_some() { 0x900a4 } else { 0x900ac },
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if success == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[test]
fn retained_relative_namespace_refuses_actual_write_attributes_retargeting() {
    let root = std::env::temp_dir().join(format!(
        "pinvou-directory-reparse-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    assert!(root.is_absolute() && root.parent() == Some(std::env::temp_dir().as_path()));
    fs::create_dir(&root).unwrap();
    let target = root.join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("marker"), b"target-original").unwrap();
    let empty = root.join("empty");
    fs::create_dir(&empty).unwrap();
    let custody = platform::directory(&empty).unwrap();
    // Ordinary-account minimum FILE_WRITE_ATTRIBUTES can really retarget this
    // same empty physical directory while its read/no-delete handles are live.
    mountpoint(&empty, Some(&target)).unwrap();
    assert_eq!(fs::read(empty.join("marker")).unwrap(), b"target-original");
    assert!(matches!(
        custody.file("escape", true, true, true),
        Err(Error::UnsafeObject)
    ));
    assert!(matches!(
        custody.create_dir(std::ffi::OsStr::new("escape-dir")),
        Err(Error::UnsafeObject)
    ));
    assert!(!target.join("escape").exists() && !target.join("escape-dir").exists());
    mountpoint(&empty, None).unwrap();
    let mut writer = custody.file("normal", true, true, true).unwrap();
    writer.write_all(b"normal-child-write").unwrap();
    writer.sync_all().unwrap();
    custody.publish(&writer, "normal-published").unwrap();
    assert!(matches!(
        custody.publish(&writer, "normal-published"),
        Err(Error::AlreadyExists)
    ));

    let destination = root.join("destination");
    fs::create_dir(&destination).unwrap();
    let destination_custody = platform::directory(&destination).unwrap();
    mountpoint(&destination, Some(&target)).unwrap();
    assert!(matches!(
        destination_custody.publish(&writer, "escape-link"),
        Err(Error::UnsafeObject)
    ));
    assert!(!target.join("escape-link").exists());
    mountpoint(&destination, None).unwrap();
    destination_custody.publish(&writer, "safe-link").unwrap();
    let leaf = empty.join("leaf");
    fs::create_dir(&leaf).unwrap();
    mountpoint(&leaf, Some(&target)).unwrap();
    assert!(matches!(
        custody.file("leaf", false, false, true),
        Err(Error::UnsafeObject)
    ));
    assert!(matches!(
        platform::directory(&leaf),
        Err(Error::UnsafeObject)
    ));
    assert!(matches!(
        custody.publish(&writer, "leaf"),
        Err(Error::AlreadyExists)
    ));
    assert_eq!(fs::read(target.join("marker")).unwrap(), b"target-original");
    mountpoint(&leaf, None).unwrap();
    drop(writer);
    assert_eq!(
        fs::read(destination.join("safe-link")).unwrap(),
        b"normal-child-write"
    );
    assert_eq!(
        fs::read(empty.join("normal-published")).unwrap(),
        b"normal-child-write"
    );
    drop(destination_custody);
    drop(custody);
    assert_eq!(fs::read(target.join("marker")).unwrap(), b"target-original");
    assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
    // Junction tags were explicitly cleared. Only this uniquely created owned
    // tree is selected; the independent reviewer's evidence is never touched.
    fs::remove_dir_all(root).unwrap();
}
