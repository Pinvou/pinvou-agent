#![cfg(target_os = "windows")]
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command, sync::atomic::AtomicBool};
const EXE: &str = env!("CARGO_BIN_EXE_upgrade-inspect-retained-native");
const JOB_EXE: &str = env!("CARGO_BIN_EXE_upgrade-download-cache");
struct Fixture {
    root: PathBuf,
    job: PathBuf,
    output: PathBuf,
    pin: String,
}
impl Fixture {
    fn new(kind: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pinvou-native-job-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let native = root.join("input.exe");
        if kind == "unsigned" {
            fs::copy(EXE, &native).unwrap();
        } else {
            fs::copy(
                PathBuf::from(std::env::var_os("SystemRoot").unwrap())
                    .join("System32/kernel32.dll"),
                &native,
            )
            .unwrap();
            if kind == "tampered" {
                let mut bytes = fs::read(&native).unwrap();
                // DOS unused header byte: keeps PE structure while changing the
                // Authenticode-covered image. Metadata signs the changed hash.
                bytes[32] ^= 1;
                fs::write(&native, bytes).unwrap();
            }
            if kind == "large" {
                fs::OpenOptions::new()
                    .write(true)
                    .open(&native)
                    .unwrap()
                    .set_len(4 * 1024 * 1024)
                    .unwrap();
            }
        }
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let zip = root.join("fullpack.zip");
        let made = Command::new("python")
            .arg(crate_dir.join("tests/fullpack.py"))
            .arg(&native)
            .arg(&zip)
            .output()
            .unwrap();
        assert!(made.status.success(), "ZIP fixture generation refused");
        let fixture = Command::new("node")
            .arg(crate_dir.join("tests/fixtures.mjs"))
            .arg(
                crate_dir
                    .ancestors()
                    .nth(7)
                    .unwrap()
                    .join("specs/upgrade/v1/reference/signatures.mjs"),
            )
            .arg(&zip)
            .arg(&native)
            .arg(if kind == "linux" {
                "linux-x86_64-deb-perMachine"
            } else {
                "windows-x86_64-nsis-perMachine"
            })
            .arg(kind)
            .output()
            .unwrap();
        assert!(
            fixture.status.success(),
            "signed fixture failed: {}",
            String::from_utf8_lossy(&fixture.stderr)
        );
        let f: Value = serde_json::from_slice(&fixture.stdout).unwrap();
        for name in ["root", "package", "expected"] {
            fs::write(root.join(format!("{name}.json")), f[name].as_str().unwrap()).unwrap();
        }
        fs::write(
            root.join("import.json"),
            serde_json::to_vec(&f["imported"]).unwrap(),
        )
        .unwrap();
        let job = root.join("job");
        let create = Command::new(JOB_EXE)
            .arg("create")
            .arg(&job)
            .arg(root.join("root.json"))
            .arg(f["rootSha256"].as_str().unwrap())
            .arg(root.join("package.json"))
            .arg(root.join("expected.json"))
            .arg(root.join("import.json"))
            .output()
            .unwrap();
        assert!(
            create.status.success(),
            "actual legacy create: {}",
            String::from_utf8_lossy(&create.stderr)
        );
        let made: Value = serde_json::from_slice(&create.stdout).unwrap();
        // Fixture populates retained bytes, never invents a successful journal.
        fs::copy(zip, job.join("attempt-00000000000000000001.bin")).unwrap();
        let output = root.join("output");
        fs::create_dir(&output).unwrap();
        fs::write(root.join("limits.json"),r#"{"fileBytes":134217728,"expandedBytes":134217728,"entries":64,"metadataBytes":1048576,"pathStateBytes":1048576,"ratio":100}"#).unwrap();
        Self {
            root,
            job,
            output,
            pin: made["manifestSha256"].as_str().unwrap().into(),
        }
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(EXE);
        cmd.arg("inspect")
            .arg(&self.job)
            .arg(&self.pin)
            .arg("1")
            .arg(&self.output)
            .arg("payload/installer.exe")
            .arg(self.root.join("limits.json"));
        cmd
    }
    fn collect(&self) -> Value {
        let result = self.command().output().unwrap();
        assert!(
            result.status.success(),
            "actual inspection failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["fileKind"], "FullPack");
        assert_eq!(value["retained"]["manifestSha256"], self.pin);
        assert_eq!(value["memberName"], "payload/installer.exe");
        for key in [
            "publisherQualification",
            "currentQualification",
            "installationRights",
            "verifiedExecutionObject",
            "trustedDeadlineEvidence",
        ] {
            assert_eq!(value[key], false);
            assert_eq!(value["native"][key], false);
        }
        assert_eq!(value["declaredNative"]["sha256"], value["native"]["sha256"]);
        assert_eq!(value["declaredNative"]["size"], value["native"]["size"]);
        value
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.root.parent(), Some(std::env::temp_dir().as_path()));
        if !std::thread::panicking() {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
}
#[test]
fn actual_signed_fullpack_legacy_job_bad_journal_and_cli_refusals() {
    let f = Fixture::new("signed");
    let result = f.collect();
    assert!(
        !result["native"]["signatures"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    fs::write(f.job.join("journal.jsonl"), b"bad journal").unwrap();
    let journal = Command::new(JOB_EXE)
        .arg("status")
        .arg(&f.job)
        .arg(&f.pin)
        .output()
        .unwrap();
    assert!(
        !journal.status.success(),
        "the actual original journal is damaged"
    );
    let bad = f.collect();
    assert_eq!(bad["native"]["sha256"], result["native"]["sha256"]);
    let mut wrong = f.command();
    let args: Vec<_> = wrong.get_args().map(|x| x.to_owned()).collect();
    wrong = Command::new(EXE);
    for (i, arg) in args.iter().enumerate() {
        wrong.arg(if i == 5 {
            std::ffi::OsStr::new("helper/helper.exe")
        } else {
            arg
        });
    }
    // Same bytes in helper still bind the finalInstaller's byte identity; only
    // exact identity is a claim, names do not carry a publisher/role authority.
    let alias = wrong.output().unwrap();
    assert!(alias.status.success());
    let mut cmd = f.command();
    cmd.arg("extra");
    assert!(!cmd.output().unwrap().status.success());
    let limits: pinvou_upgrade_native_inspection::Limits =
        serde_json::from_slice(&fs::read(f.root.join("limits.json")).unwrap()).unwrap();
    assert!(matches!(
        pinvou_upgrade_native_inspection::inspect(
            &f.job,
            &f.pin,
            1,
            &f.output,
            "missing",
            &limits,
            &AtomicBool::new(false)
        ),
        Err(pinvou_upgrade_native_inspection::Error::Package)
    ));
    assert!(matches!(
        pinvou_upgrade_native_inspection::inspect(
            &f.job,
            &f.pin,
            1,
            &f.output,
            "payload/installer.exe",
            &limits,
            &AtomicBool::new(true)
        ),
        Err(pinvou_upgrade_native_inspection::Error::Cancelled)
    ));
    fs::write(f.job.join("attempt-00000000000000000001.bin"), b"bad").unwrap();
    assert!(!f.command().output().unwrap().status.success());
}
#[test]
fn actual_unsigned_and_rehashed_signature_tamper_reach_native_verifier() {
    for kind in ["unsigned", "tampered"] {
        let f = Fixture::new(kind);
        let result = f.collect();
        assert!(
            result["native"]["signatures"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["verification"] != "verified")
        );
        if kind == "unsigned" {
            assert!(
                result["native"]["signatures"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s["verification"] == "unsigned")
            );
        } else {
            let signatures = result["native"]["signatures"].as_array().unwrap();
            assert!(!signatures.is_empty());
            assert!(signatures.iter().all(
                |s| s["verification"] == "invalidSignature" && s["nativeStatus"] == "80096010"
            ));
        }
    }
}
#[test]
fn actual_original_writer_identity_wrong_native_claim_and_other_target_refuse() {
    for kind in ["wrong-native", "linux"] {
        let f = Fixture::new(kind);
        let result = f.command().output().unwrap();
        assert!(!result.status.success());
        assert!(fs::read_dir(&f.output).unwrap().next().is_none());
        assert_eq!(
            String::from_utf8(result.stderr).unwrap().trim(),
            if kind == "linux" {
                "RETAINED_NATIVE_UNSUPPORTED"
            } else {
                "RETAINED_NATIVE_PACKAGE_REFUSED"
            }
        );
    }
    let f = Fixture::new("signed");
    fs::rename(
        f.job.join("writer.lock"),
        f.job.join("original-writer.lock"),
    )
    .unwrap();
    fs::write(f.job.join("writer.lock"), b"").unwrap();
    assert!(!f.command().output().unwrap().status.success());
    assert!(fs::read_dir(&f.output).unwrap().next().is_none());
    fs::remove_file(f.job.join("writer.lock")).unwrap();
    fs::rename(
        f.job.join("original-writer.lock"),
        f.job.join("writer.lock"),
    )
    .unwrap();
    f.collect();
}
#[test]
fn actual_library_custody_releases_output_only_after_observation_drop() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new("signed");
    let limits = serde_json::from_slice(&fs::read(f.root.join("limits.json")).unwrap()).unwrap();
    let observed = pinvou_upgrade_native_inspection::inspect(
        &f.job,
        &f.pin,
        1,
        &f.output,
        "payload/installer.exe",
        &limits,
        &AtomicBool::new(false),
    )
    .unwrap();
    let report = serde_json::to_value(observed.report()).unwrap();
    let output = f.output.join(report["destinationName"].as_str().unwrap());
    assert!(
        fs::OpenOptions::new()
            .write(true)
            .share_mode(7)
            .open(&output)
            .is_err()
    );
    assert!(fs::rename(&output, f.output.join("forbidden.exe")).is_err());
    let blocked = Command::new(JOB_EXE)
        .arg("status")
        .arg(&f.job)
        .arg(&f.pin)
        .output()
        .unwrap();
    assert!(!blocked.status.success());
    drop(observed);
    assert!(
        fs::OpenOptions::new()
            .write(true)
            .share_mode(7)
            .open(&output)
            .is_ok()
    );
    fs::rename(output, f.output.join("released.exe")).unwrap();
    let released = Command::new(JOB_EXE)
        .arg("status")
        .arg(&f.job)
        .arg(&f.pin)
        .output()
        .unwrap();
    assert!(released.status.success());
}
#[allow(unsafe_code)]
fn process_has_file(file: &fs::File, pid: u32) -> bool {
    use std::{mem::size_of, os::windows::io::AsRawHandle};
    use windows_sys::{
        Wdk::Storage::FileSystem::{FileProcessIdsUsingFileInformation, NtQueryInformationFile},
        Win32::System::IO::IO_STATUS_BLOCK,
    };
    #[repr(C)]
    struct Ids {
        count: u32,
        ids: [usize; 64],
    }
    let mut ids = Ids {
        count: 0,
        ids: [0; 64],
    };
    let mut io = IO_STATUS_BLOCK::default();
    // SAFETY: exact native layout and initialized aligned writable storage;
    // synchronous call borrows the live attribute-only observation handle only.
    let status = unsafe {
        NtQueryInformationFile(
            file.as_raw_handle(),
            &mut io,
            (&mut ids as *mut Ids).cast(),
            size_of::<Ids>() as u32,
            FileProcessIdsUsingFileInformation,
        )
    };
    assert!(
        status >= 0 && ids.count <= 64,
        "actual file-process query must be available"
    );
    ids.ids[..ids.count as usize].contains(&(pid as usize))
}
struct Joined(std::process::Child);
impl Drop for Joined {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn actual_cli_kill_wait_releases_original_job_and_cache_custody() {
    use std::{
        os::windows::fs::OpenOptionsExt,
        process::Stdio,
        time::{Duration, Instant},
    };
    let f = Fixture::new("large");
    let path = f.job.join("attempt-00000000000000000001.bin");
    // An attribute-only, fully shared observation does not race the child's read
    // acquisition with a conflicting probe writer or duplicate read capability.
    let observer = fs::OpenOptions::new()
        .read(true)
        .access_mode(windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES)
        .share_mode(7)
        .open(&path)
        .unwrap();
    let child = f
        .command()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = Joined(child);
    let start = Instant::now();
    let mut owned = false;
    while start.elapsed() < Duration::from_secs(20) {
        if process_has_file(&observer, child.0.id()) {
            owned = true;
            break;
        }
        if child.0.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    if owned {
        child.0.kill().unwrap();
    }
    let status = child.0.wait().unwrap();
    drop(observer);
    assert!(
        owned,
        "kill occurs only after real original retained-file ownership"
    );
    assert!(!status.success());
    assert!(
        fs::OpenOptions::new()
            .write(true)
            .share_mode(7)
            .open(&path)
            .is_ok()
    );
    let result = Command::new(JOB_EXE)
        .arg("status")
        .arg(&f.job)
        .arg(&f.pin)
        .output()
        .unwrap();
    assert!(result.status.success());
    let renamed = f.job.join("released.bin");
    fs::rename(&path, &renamed).unwrap();
    fs::rename(renamed, &path).unwrap();
}
#[test]
fn actual_copy_cancellation_joins_before_releasing_partial_output() {
    use std::{
        os::windows::fs::OpenOptionsExt,
        sync::{Arc, atomic::Ordering},
        time::{Duration, Instant},
    };
    let f = Fixture::new("large");
    let cancel = Arc::new(AtomicBool::new(false));
    let watcher_cancel = cancel.clone();
    let output = f.output.clone();
    let native_bytes = fs::read(f.root.join("input.exe")).unwrap();
    let expected_size = native_bytes.len() as u64;
    let watcher = std::thread::spawn(move || {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(60) {
            for entry in fs::read_dir(&output).unwrap() {
                let entry = entry.unwrap();
                // FindFirstFile directory sizes can stay stale until close.
                // Attribute-only access does not conflict with share-mode-zero
                // data access. Poll the actual live file, not directory cache.
                let file = fs::OpenOptions::new()
                    .read(true)
                    .access_mode(windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES)
                    .share_mode(7)
                    .open(entry.path())
                    .unwrap();
                while start.elapsed() < Duration::from_secs(60) {
                    let copied = file.metadata().unwrap().len();
                    if copied > 0 && copied < expected_size {
                        watcher_cancel.store(true, Ordering::Release);
                        return Some((entry.path(), copied));
                    }
                    if copied >= expected_size {
                        return None;
                    }
                    // Do not impose a scheduler tick between short real writes.
                    std::thread::yield_now();
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        None
    });
    let limits = serde_json::from_slice(&fs::read(f.root.join("limits.json")).unwrap()).unwrap();
    let result = pinvou_upgrade_native_inspection::inspect(
        &f.job,
        &f.pin,
        1,
        &f.output,
        "payload/installer.exe",
        &limits,
        &cancel,
    );
    let (partial, observed_size) = watcher
        .join()
        .unwrap()
        .expect("actual nonempty incomplete destination precedes cancellation");
    assert!(matches!(
        result,
        Err(pinvou_upgrade_native_inspection::Error::Cancelled)
    ));
    let bytes = fs::read(&partial).unwrap();
    assert!(
        bytes.len() as u64 >= observed_size && (bytes.len() as u64) < expected_size,
        "the joined outcome must still be a genuine incomplete copy"
    );
    assert_eq!(
        bytes,
        native_bytes[..bytes.len()],
        "actual native prefix was copied"
    );
    for entry in fs::read_dir(&f.output).unwrap() {
        assert!(
            fs::OpenOptions::new()
                .write(true)
                .share_mode(7)
                .open(entry.unwrap().path())
                .is_ok()
        );
    }
    let result = Command::new(JOB_EXE)
        .arg("status")
        .arg(&f.job)
        .arg(&f.pin)
        .output()
        .unwrap();
    assert!(result.status.success());
}
#[test]
#[ignore = "requires existing complete cache-only chains/timestamp for actual system PE"]
fn cache_only_strict_fullpack_signature_positive() {
    let f = Fixture::new("signed");
    let result = f.collect();
    assert!(
        result["native"]["signatures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["verification"] == "verified"
                && !s["timestamps"].as_array().unwrap().is_empty())
    );
}
