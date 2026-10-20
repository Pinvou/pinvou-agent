//! Actual standalone executable. No fixture image is executed, no test root is
//! installed and no production CA/URL/provider override exists.
#![cfg(target_os = "windows")]
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
const EXE: &str = env!("CARGO_BIN_EXE_upgrade-native-evidence");
struct Fixture {
    root: PathBuf,
    signed: PathBuf,
    unsigned: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "pinvou-native-process-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let signed = root.join("signed.dll");
        let unsigned = root.join("unsigned.exe");
        fs::copy(
            PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/kernel32.dll"),
            &signed,
        )
        .unwrap();
        fs::copy(EXE, &unsigned).unwrap();
        Self {
            root,
            signed,
            unsigned,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn call(path: &Path, size: u64, sha: &str, identity: Option<&str>) -> std::process::Output {
    let mut command = Command::new(EXE);
    command
        .arg("inspect")
        .arg(path)
        .arg(size.to_string())
        .arg(sha);
    if let Some(identity) = identity {
        command.arg(identity);
    }
    command.output().unwrap() // output() waits and drains both pipes.
}
fn report(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap();
    let result = call(path, bytes.len() as u64, &digest(&bytes), None);
    assert!(
        result.status.success(),
        "fixed diagnostic: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["sha256"], digest(&bytes));
    assert_eq!(value["size"], bytes.len() as u64);
    value
}
#[test]
fn real_cli_reports_signed_unsigned_rehashed_tamper_and_fixed_refusals() {
    let f = Fixture::new();
    let original = fs::read(&f.signed).unwrap();
    let sha = digest(&original);
    let value = report(&f.signed);
    for field in [
        "publisherQualification",
        "currentQualification",
        "installationRights",
        "verifiedExecutionObject",
        "trustedDeadlineEvidence",
    ] {
        assert_eq!(value[field], false);
    }
    assert!(!value["signatures"].as_array().unwrap().is_empty());
    let identity = value["identitySha256"].as_str().unwrap();
    let identity_positive = call(&f.signed, original.len() as u64, &sha, Some(identity));
    assert!(identity_positive.status.success());
    for (size, hash, identity, error) in [
        (
            original.len() as u64 + 1,
            sha.clone(),
            None,
            "NATIVE_EVIDENCE_BYTES_MISMATCH",
        ),
        (
            original.len() as u64,
            "0".repeat(64),
            None,
            "NATIVE_EVIDENCE_BYTES_MISMATCH",
        ),
        (
            original.len() as u64,
            sha.clone(),
            Some("0".repeat(64)),
            "NATIVE_EVIDENCE_IDENTITY_MISMATCH",
        ),
    ] {
        let output = call(&f.signed, size, &hash, identity.as_deref());
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(String::from_utf8(output.stderr).unwrap().trim(), error);
    }
    let unsigned = report(&f.unsigned);
    assert_eq!(unsigned["signatures"][0]["verification"], "unsigned");
    assert_eq!(unsigned["signatures"][0]["nativeStatus"], "800b0100");
    let mut changed = original.clone();
    changed[0x1000] ^= 1;
    fs::write(&f.signed, &changed).unwrap();
    let damaged = report(&f.signed);
    assert_eq!(
        damaged["signatures"].as_array().unwrap().len(),
        value["signatures"].as_array().unwrap().len()
    );
    for s in damaged["signatures"].as_array().unwrap() {
        assert_eq!(s["verification"], "invalidSignature");
        assert_eq!(s["nativeStatus"], "80096010");
    }
    fs::write(&f.signed, &original).unwrap();
    assert_eq!(report(&f.signed)["sha256"], sha);
}

struct Joined(Child);
impl Drop for Joined {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[allow(unsafe_code)]
fn process_has_file(file: &fs::File, pid: u32) -> bool {
    use std::{mem::size_of, os::windows::io::AsRawHandle};
    use windows_sys::{
        Wdk::Storage::FileSystem::{FileProcessIdsUsingFileInformation, NtQueryInformationFile},
        Win32::System::IO::IO_STATUS_BLOCK,
    };
    // FILE_PROCESS_IDS_USING_FILE_INFORMATION: aligned count and ULONG_PTR tail.
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
    // SAFETY: exact initialized native result buffer, live owned fixture File,
    // synchronous query. This observes handles without opening a rival writer.
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
        "owned-file process observation unavailable"
    );
    ids.ids[..ids.count as usize].contains(&(pid as usize))
}
#[test]
fn real_cli_kill_after_actual_lock_is_joined_and_releases_handle() {
    let f = Fixture::new();
    let large = f.root.join("large.dll");
    let mut out = fs::File::create(&large).unwrap();
    out.write_all(&fs::read(&f.signed).unwrap()).unwrap();
    out.set_len(128 * 1024 * 1024).unwrap();
    out.sync_all().unwrap();
    drop(out);
    let bytes = fs::read(&large).unwrap();
    let sha = digest(&bytes);
    drop(bytes);
    let child = Command::new(EXE)
        .arg("inspect")
        .arg(&large)
        .arg((128u64 * 1024 * 1024).to_string())
        .arg(sha)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = Joined(child);
    let observer = fs::File::open(&large).unwrap();
    let start = Instant::now();
    let mut locked = false;
    while start.elapsed() < Duration::from_secs(10) {
        if process_has_file(&observer, child.0.id()) {
            // Only now attempt a conflicting writer. Probing a writer before
            // acquisition itself races and can correctly make the CLI refuse.
            locked = fs::OpenOptions::new()
                .write(true)
                .share_mode(7)
                .open(&large)
                .is_err_and(|e| e.raw_os_error() == Some(32));
            break;
        }
        if child.0.try_wait().unwrap().is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    child.0.kill().unwrap();
    let status = child.0.wait().unwrap();
    drop(observer);
    assert!(
        locked,
        "child must actually own the read reservation before kill"
    );
    assert!(!status.success());
    assert!(fs::OpenOptions::new().write(true).open(&large).is_ok());
    let renamed = f.root.join("released.dll");
    fs::rename(&large, &renamed).unwrap();
}
