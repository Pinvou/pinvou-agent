//! Real independent executable, process death/wait, file custody and actual TLS
//! wait cancellation. The local peer intentionally never completes TLS; it does
//! not bypass production certificate trust or manufacture a successful HTTP body.
#![cfg(target_os = "windows")]
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Read},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
const EXE: &str = env!("CARGO_BIN_EXE_upgrade-download-cache");

struct Fixture {
    root: PathBuf,
    job: PathBuf,
    pin: String,
}
impl Fixture {
    fn new(url: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pinvou-job-process-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let reference = crate_dir
            .ancestors()
            .nth(7)
            .unwrap()
            .join("specs/upgrade/v1/reference/signatures.mjs");
        let output = Command::new("node")
            .arg(crate_dir.join("tests/fixtures.mjs"))
            .arg(reference)
            .arg("abc")
            .arg(url)
            .output()
            .unwrap();
        assert!(output.status.success(), "public signing fixture failed");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        for name in ["root", "package", "expected"] {
            fs::write(
                root.join(format!("{name}.json")),
                value[name].as_str().unwrap(),
            )
            .unwrap();
        }
        fs::write(
            root.join("import.json"),
            serde_json::to_vec(&value["imported"]).unwrap(),
        )
        .unwrap();
        let job = root.join("job");
        let output = Command::new(EXE)
            .arg("create")
            .arg(&job)
            .arg(root.join("root.json"))
            .arg(value["rootSha256"].as_str().unwrap())
            .arg(root.join("package.json"))
            .arg(root.join("expected.json"))
            .arg(root.join("import.json"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "CLI create refused valid signed input: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let created: Value = serde_json::from_slice(&output.stdout).unwrap();
        let pin = created["manifestSha256"].as_str().unwrap().to_owned();
        Self { root, job, pin }
    }
    fn command(&self, action: &str) -> Command {
        let mut cmd = Command::new(EXE);
        cmd.arg(action).arg(&self.job).arg(&self.pin);
        cmd
    }
    fn consent(&self, port: u16) -> PathBuf {
        let path = self.root.join("network.json");
        fs::write(
            &path,
            format!(r#"{{"host":"127.0.0.1","port":{port},"addresses":["127.0.0.1"]}}"#),
        )
        .unwrap();
        path
    }
    fn start(&self, consent: &Path) -> Running {
        let child = self
            .command("run")
            .arg(self.root.join("import.json"))
            .arg(consent)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Running::new(child)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // This exact absolute directory was uniquely created above; no caller
        // input or recursive enumeration selects another deletion target.
        assert!(
            self.root.is_absolute() && self.root.parent() == Some(std::env::temp_dir().as_path())
        );
        if !thread::panicking() {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
}
struct Running {
    child: Child,
    lines: mpsc::Receiver<Value>,
    stdout: Option<thread::JoinHandle<()>>,
    stderr: Option<thread::JoinHandle<Vec<u8>>>,
}
impl Running {
    fn new(mut child: Child) -> Self {
        let (tx, lines) = mpsc::channel();
        let stdout = child.stdout.take().unwrap();
        let stdout = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                if let Ok(value) = serde_json::from_str(&line) {
                    let _ = tx.send(value);
                }
            }
        });
        let mut stderr = child.stderr.take().unwrap();
        let stderr = thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).unwrap();
            bytes
        });
        Self {
            child,
            lines,
            stdout: Some(stdout),
            stderr: Some(stderr),
        }
    }
    fn started(&self) -> String {
        let value = self
            .lines
            .recv_timeout(Duration::from_secs(15))
            .expect("actual child did not publish its run ID");
        assert_eq!(value["kind"], "started");
        value["snapshot"]["runId"].as_str().unwrap().to_owned()
    }
    fn finish(&mut self, kill: bool) -> (bool, Vec<Value>) {
        if kill {
            self.child.kill().unwrap();
        }
        let limit = Instant::now() + Duration::from_secs(15);
        let success = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status.success();
            }
            assert!(Instant::now() < limit, "owned child did not exit");
            thread::sleep(Duration::from_millis(10));
        };
        self.child.wait().unwrap();
        self.stdout.take().unwrap().join().unwrap();
        let _fixed_stderr = self.stderr.take().unwrap().join().unwrap();
        (success, self.lines.try_iter().collect())
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        if self.stdout.is_some() {
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(reader) = self.stdout.take() {
                let _ = reader.join();
            }
            if let Some(reader) = self.stderr.take() {
                let _ = reader.join();
            }
        }
    }
}
fn accept_tls(listener: &TcpListener) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let limit = Instant::now() + Duration::from_secs(10);
    let mut socket = loop {
        match listener.accept() {
            Ok((socket, _)) => break socket,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < limit, "real TLS connection did not arrive");
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => panic!("owned local TLS wait peer failed"),
        }
    };
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut hello = [0u8; 4096];
    assert!(
        socket.read(&mut hello).unwrap() > 0,
        "pause must follow real TLS bytes, not a synthetic pending callback"
    );
    socket
}

#[test]
fn actual_child_kill_wait_reopen_and_new_attempt_preserve_original_bytes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let f = Fixture::new(&format!("https://127.0.0.1:{port}/file"));
    let consent = f.consent(port);
    let mut first = f.start(&consent);
    let _id = first.started();
    let peer = accept_tls(&listener);
    let busy = f.command("status").output().unwrap();
    assert!(!busy.status.success());
    assert_eq!(
        String::from_utf8_lossy(&busy.stderr).trim(),
        "DOWNLOAD_JOB_BUSY"
    );
    let (success, _) = first.finish(true);
    assert!(!success);
    drop(peer);
    let old = f.job.join("attempt-00000000000000000001.bin");
    assert_eq!(fs::read(&old).unwrap(), b"");
    fs::write(&old, b"partial-fixture").unwrap();
    let status = f.command("status").output().unwrap();
    assert!(status.status.success());
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["recordedPhase"], "opened");
    assert_eq!(status["observedBytesMatch"], false);
    let mut second = f.start(&consent);
    let id = second.started();
    let peer = accept_tls(&listener);
    assert_eq!(fs::read(&old).unwrap(), b"partial-fixture");
    assert!(
        f.command("pause")
            .arg(&id)
            .output()
            .unwrap()
            .status
            .success()
    );
    let (success, lines) = second.finish(false);
    assert!(success);
    drop(peer);
    assert!(lines.iter().any(|v| v["recordedPhase"] == "paused"));
    assert_eq!(fs::read(&old).unwrap(), b"partial-fixture");
    assert_eq!(
        fs::read(f.job.join("attempt-00000000000000000002.bin")).unwrap(),
        b""
    );
    assert!(f.command("status").output().unwrap().status.success());
}

#[test]
fn actual_cancel_is_joined_and_complete_orphan_is_checked_without_journal_healing() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let f = Fixture::new(&format!("https://127.0.0.1:{port}/file"));
    let consent = f.consent(port);
    let mut running = f.start(&consent);
    let id = running.started();
    let peer = accept_tls(&listener);
    assert!(
        f.command("cancel")
            .arg(&id)
            .output()
            .unwrap()
            .status
            .success()
    );
    let (success, lines) = running.finish(false);
    assert!(success);
    drop(peer);
    assert!(lines.iter().any(|v| v["recordedPhase"] == "cancelled"));
    let refused = f
        .command("run")
        .arg(f.root.join("import.json"))
        .arg(&consent)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert_eq!(
        String::from_utf8_lossy(&refused.stderr).trim(),
        "DOWNLOAD_JOB_CANCELLED"
    );
    // Test-only file assembly: this proves the real executable's orphan scan,
    // NOT a successful HTTP download or any accepted remote terminal stage.
    let file = f.job.join("attempt-00000000000000000001.bin");
    fs::write(&file, b"abc").unwrap();
    fs::write(f.job.join("journal.jsonl"), b"null\n").unwrap();
    let output = f.command("check-retained").arg("1").output().unwrap();
    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["recordedPhase"], "journalNotConsulted");
    assert_eq!(result["observedBytesMatch"], true);
    assert_eq!(fs::read(f.job.join("journal.jsonl")).unwrap(), b"null\n");
    fs::write(&file, b"axc").unwrap();
    let output = f.command("check-retained").arg("1").output().unwrap();
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim(),
        "DOWNLOAD_JOB_BYTES_MISMATCH"
    );
    fs::write(&file, b"abc").unwrap();
    assert!(
        f.command("check-retained")
            .arg("1")
            .output()
            .unwrap()
            .status
            .success()
    );
}
