use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};
struct Fixture {
    directory: PathBuf,
    pin: String,
    now: i64,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "Failed isolated QA fixture retained for diagnosis: {}",
                self.directory.display()
            );
            return;
        }
        // Only remove this test's newly created absolute, unique temp root.
        // Successful call sites have already joined children/drains and closed
        // owner DB connections. Failure paths preserve the actual evidence.
        let root = self
            .directory
            .canonicalize()
            .expect("owned QA root must exist");
        let temp = std::env::temp_dir().canonicalize().unwrap();
        assert_eq!(root.parent(), Some(temp.as_path()));
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("pinvou-t27-qa-{}-", std::process::id()))
        );
        assert!(
            !fs::symlink_metadata(&self.directory)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(&root).expect("owned QA fixture cleanup must complete");
    }
}
impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-t27-qa-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let reference = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(7)
            .unwrap()
            .join("specs/upgrade/v1/reference/signatures.mjs");
        let out = Command::new("node")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures.mjs"))
            .arg(reference)
            .arg(&directory)
            .output()
            .expect("Node fixture process must start");
        assert!(out.status.success(), "QA signature fixture failed");
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        Self {
            directory,
            pin: v["pin"].as_str().unwrap().to_owned(),
            now: v["now"].as_i64().unwrap(),
        }
    }
    fn store(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }
    fn command(&self, action: &str, store: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pinvou-upgrade-metadata-qa"));
        cmd.env_remove("PINVOU_METADATA_QA_CHECKPOINT")
            .env_remove("PINVOU_METADATA_QA_TRACE_WRITES");
        cmd.arg(action).arg(self.store(store));
        if action == "init" {
            cmd.arg(self.directory.join("anchor.json"))
                .arg(&self.pin)
                .arg(self.now.to_string());
        } else {
            cmd.arg(&self.pin);
        }
        cmd
    }
    fn init(&self, name: &str) {
        assert_success(self.command("init", name).output().unwrap());
    }
    fn accept(&self, name: &str, bundle: &str) -> Output {
        self.command("accept", name)
            .arg(self.directory.join(format!("{bundle}.json")))
            .output()
            .unwrap()
    }
    fn inspect(&self, name: &str) -> Value {
        assert_success(self.command("inspect", name).output().unwrap())
    }
}

#[cfg(feature = "fault-tests")]
fn checkpoint_child(mut cmd: Command, stage: &str, lose_ack: bool) -> Output {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        sync::mpsc,
        thread,
        time::Duration,
    };
    cmd.env("PINVOU_METADATA_QA_CHECKPOINT", stage)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let drain = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let result = reader.read_line(&mut line);
        let _ = tx.send((result.is_ok(), line));
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        tail
    });
    let errors = thread::spawn(move || {
        let mut stream = stderr;
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let reached = rx.recv_timeout(Duration::from_secs(60));
    let mut input_error = false;
    if lose_ack && reached.is_ok() {
        input_error = child
            .stdin
            .take()
            .unwrap()
            .write_all(b"lose-ack\n")
            .is_err();
    } else if child.try_wait().unwrap().is_none() {
        child.kill().unwrap();
    }
    let status = child.wait().unwrap();
    let tail = drain.join().unwrap();
    let stderr = errors.join().unwrap();
    // Always join before assertions, including a missing checkpoint/early exit.
    let (read, line) = reached.expect("actual SQL checkpoint must be reached");
    assert!(read);
    assert_eq!(line.trim(), format!("QA_CHECKPOINT {stage}"));
    assert!(!input_error);
    Output {
        status,
        stdout: tail,
        stderr,
    }
}

#[cfg(feature = "fault-tests")]
#[test]
fn actual_each_accept_persistent_statement_kill_and_postcommit_ack_loss() {
    let f = Fixture::new();
    f.init("trace");
    assert_success(f.accept("trace", "v1"));
    let mut trace = f.command("accept", "trace");
    trace
        .arg(f.directory.join("root-chain.json"))
        .env("PINVOU_METADATA_QA_TRACE_WRITES", "1");
    let traced = trace.output().unwrap();
    assert!(traced.status.success());
    let writes: Vec<usize> = String::from_utf8(traced.stderr)
        .unwrap()
        .lines()
        .map(|line| line.strip_prefix("QA_SQL_WRITE ").unwrap().parse().unwrap())
        .collect();
    assert!(writes.len() > 10);
    eprintln!("QA acceptance persistent statement count: {}", writes.len());
    assert_eq!(writes, (1..=writes.len()).collect::<Vec<_>>());
    for index in writes {
        let name = format!("write-{index}");
        f.init(&name);
        assert_success(f.accept(&name, "v1"));
        let before = f.inspect(&name);
        let bytes = fs::read(f.store(&name).join("metadata-qa.sqlite")).unwrap();
        let mut cmd = f.command("accept", &name);
        cmd.arg(f.directory.join("root-chain.json"));
        let killed = checkpoint_child(cmd, &format!("sql-write-{index}"), false);
        assert!(!killed.status.success());
        assert!(killed.stderr.is_empty());
        assert_eq!(f.inspect(&name), before);
        assert_eq!(
            fs::read(f.store(&name).join("metadata-qa.sqlite")).unwrap(),
            bytes
        );
        assert_eq!(
            assert_success(f.accept(&name, "root-chain"))["generation"],
            3
        );
    }
    f.init("ack-loss");
    assert_success(f.accept("ack-loss", "v1"));
    let mut cmd = f.command("accept", "ack-loss");
    cmd.arg(f.directory.join("root-chain.json"));
    let lost = checkpoint_child(cmd, "after-commit", true);
    assert!(!lost.status.success());
    assert_eq!(
        String::from_utf8(lost.stderr).unwrap().trim(),
        "METADATA_QA_COMMIT_UNCERTAIN"
    );
    assert_eq!(f.inspect("ack-loss")["generation"], 3);
    assert_eq!(
        assert_success(f.accept("ack-loss", "root-chain"))["generation"],
        3
    );
}
fn assert_success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "child refused: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn actual_restart_high_water_originals_and_failed_set_are_atomic() {
    let f = Fixture::new();
    f.init("state");
    assert_eq!(f.inspect("state")["generation"], 1);
    let first = assert_success(f.accept("state", "v1"));
    assert_eq!(first["generation"], 2);
    assert_eq!(assert_success(f.accept("state", "v1"))["generation"], 2);
    assert_eq!(assert_success(f.accept("state", "v2"))["generation"], 3);
    let mut reordered: Value =
        serde_json::from_slice(&fs::read(f.directory.join("v2.json")).unwrap()).unwrap();
    reordered["members"].as_array_mut().unwrap().reverse();
    fs::write(
        f.directory.join("reordered.json"),
        serde_json::to_vec(&reordered).unwrap(),
    )
    .unwrap();
    assert_eq!(
        assert_success(f.accept("state", "reordered"))["generation"],
        3
    );
    let before = fs::read(f.store("state").join("metadata-qa.sqlite")).unwrap();
    for (name, code) in [
        ("v1", "METADATA_ROLLBACK"),
        ("equiv2", "METADATA_EQUIVOCATION"),
        ("missing", "METADATA_QA_VERIFICATION_FAILED"),
        ("corrupt", "METADATA_QA_VERIFICATION_FAILED"),
    ] {
        let failed = f.accept("state", name);
        assert!(!failed.status.success());
        assert_eq!(String::from_utf8(failed.stderr).unwrap().trim(), code);
        assert_eq!(f.inspect("state")["generation"], 3);
        assert_eq!(
            fs::read(f.store("state").join("metadata-qa.sqlite")).unwrap(),
            before
        );
    }
    let mut after = String::new();
    let mut found = 0;
    loop {
        let value = assert_success(
            f.command("original-after", "state")
                .arg(&after)
                .arg("3")
                .output()
                .unwrap(),
        );
        if value.is_null() {
            break;
        }
        assert!(!value["originalUtf8"].as_str().unwrap().is_empty());
        let raw = value["originalUtf8"].as_str().unwrap().as_bytes();
        let hash: String = {
            use sha2::{Digest, Sha256};
            Sha256::digest(raw)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        assert_eq!(hash, value["envelopeSha256"]);
        after = String::from_utf8(
            pinvou_upgrade_codec::Document::parse(
                serde_json::to_string(&value["context"]).unwrap().as_bytes(),
                pinvou_upgrade_codec::InputKind::Metadata,
            )
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        )
        .unwrap();
        found += 1;
    }
    assert_eq!(found, 5);
    assert_eq!(f.inspect("state")["currentQualificationChecked"], false);
}

#[test]
fn actual_committed_unvisited_island_cannot_be_inspected_or_exported() {
    use sha2::{Digest, Sha256};
    let f = Fixture::new();
    f.init("state");
    let expected = f.inspect("state");
    let raw = fs::read(f.directory.join("v1-package.json")).unwrap();
    let body: Value = serde_json::from_slice(&raw).unwrap();
    let old = body["signatures"][0]["signature"].as_str().unwrap();
    let new = format!(
        "{}{}",
        if old.starts_with('A') { "B" } else { "A" },
        &old[1..]
    );
    let forged = String::from_utf8(raw)
        .unwrap()
        .replace(old, &new)
        .into_bytes();
    let hash: String = Sha256::digest(&forged)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut db = rusqlite::Connection::open(f.store("state").join("metadata-qa.sqlite")).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    // The corrected schema independently rejects the original ordinary SQL attack.
    assert!(
        db.execute(
            "INSERT INTO batches VALUES(0,?,?,?,'app',?)",
            rusqlite::params![f.pin, f.pin, f.now, b"{}".as_slice()]
        )
        .is_err()
    );
    // An explicit owner-adversarial fault bypasses CHECK only to retain the
    // original committed-island negative. No production bypass is provided.
    db.execute_batch("PRAGMA ignore_check_constraints=ON;")
        .unwrap();
    let tx = db.transaction().unwrap();
    tx.execute(
        "INSERT INTO objects VALUES(?,?)",
        rusqlite::params![hash, forged],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO batches VALUES(0,?,?,?,'app',?)",
        rusqlite::params![f.pin, f.pin, f.now, b"{}".as_slice()],
    )
    .unwrap();
    tx.execute("INSERT INTO root_steps VALUES(0,0,?)", [&hash])
        .unwrap();
    tx.commit().unwrap();
    assert!(
        !f.command("inspect", "state")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        !f.command("original", "state")
            .arg(&hash)
            .arg("1")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM batches WHERE generation=0", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let tx = db.transaction().unwrap();
    tx.execute("DELETE FROM root_steps WHERE generation=0", [])
        .unwrap();
    tx.execute("DELETE FROM batches WHERE generation=0", [])
        .unwrap();
    tx.execute("DELETE FROM objects WHERE hash=?", [&hash])
        .unwrap();
    tx.commit().unwrap();
    drop(db);
    assert_eq!(f.inspect("state"), expected);
}
#[test]
fn actual_root_continuity_and_complete_other_component_inventory() {
    let f = Fixture::new();
    f.init("state");
    assert_success(f.accept("state", "v1"));
    assert_success(f.accept("state", "engine"));
    let before = f.inspect("state");
    assert!(!f.accept("state", "root-no-engine").status.success());
    assert_eq!(f.inspect("state"), before);
    let accepted = assert_success(f.accept("state", "root-chain"));
    assert_eq!(accepted["generation"], 4);
    assert_eq!(
        assert_success(f.accept("state", "root-chain"))["generation"],
        4
    );
    assert!(!f.accept("state", "v1").status.success());
    assert_eq!(f.inspect("state")["generation"], 4);
}
#[test]
fn actual_distinct_root_rotation_retries_bind_bytes_not_descriptor_spelling() {
    let f = Fixture::new();
    f.init("state");
    assert_success(f.accept("state", "v1"));
    let before = f.inspect("state");
    assert!(!f.accept("state", "rotation-invalid").status.success());
    assert_eq!(f.inspect("state"), before);
    assert_eq!(
        assert_success(f.accept("state", "rotated"))["generation"],
        3
    );
    let original: Value =
        serde_json::from_slice(&fs::read(f.directory.join("rotated.json")).unwrap()).unwrap();
    assert_eq!(
        assert_success(f.accept("state", "rotated"))["generation"],
        3
    );
    let mut varied = original.clone();
    varied["suppliedNowMs"] = Value::from(f.now + 1001);
    varied["members"].as_array_mut().unwrap().reverse();
    for field in ["timestamp", "snapshot"] {
        let old = varied[field].as_str().unwrap();
        let new = format!("alias-{old}");
        fs::copy(f.directory.join(old), f.directory.join(&new)).unwrap();
        varied[field] = new.into();
    }
    for field in ["members", "roots"] {
        for name in varied[field].as_array_mut().unwrap() {
            let old = name.as_str().unwrap();
            let new = format!("alias-{old}");
            fs::copy(f.directory.join(old), f.directory.join(&new)).unwrap();
            *name = new.into();
        }
    }
    fs::write(
        f.directory.join("varied.json"),
        serde_json::to_vec(&varied).unwrap(),
    )
    .unwrap();
    assert_eq!(assert_success(f.accept("state", "varied"))["generation"], 3);
    // Both repeated and new-deny requests use actual current inputs, not the
    // retained historical time/deny facts from the first successful commit.
    let root: Value =
        serde_json::from_slice(&fs::read(f.directory.join("anchor.json")).unwrap()).unwrap();
    varied["deniedByRole"] = serde_json::json!({"root":[root["signed"]["roles"][0]["keyIds"][0]]});
    fs::write(
        f.directory.join("denied.json"),
        serde_json::to_vec(&varied).unwrap(),
    )
    .unwrap();
    assert!(!f.accept("state", "denied").status.success());
    varied["deniedByRole"] = serde_json::json!({});
    varied["suppliedNowMs"] = Value::from(f.now + 366 * 86400000);
    fs::write(
        f.directory.join("expired.json"),
        serde_json::to_vec(&varied).unwrap(),
    )
    .unwrap();
    assert!(!f.accept("state", "expired").status.success());
    assert_eq!(f.inspect("state")["generation"], 3);
}

#[test]
fn actual_identical_signed_body_with_other_valid_signature_is_equivocation() {
    let f = Fixture::new();
    f.init("state");
    assert_success(f.accept("state", "v1"));
    let mut bundle: Value =
        serde_json::from_slice(&fs::read(f.directory.join("v1.json")).unwrap()).unwrap();
    let actual: Value =
        serde_json::from_slice(&fs::read(f.directory.join("v1-timestamp.json")).unwrap()).unwrap();
    let alternate: Value = serde_json::from_slice(
        &fs::read(f.directory.join("same-body-other-signature.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(actual["signed"], alternate["signed"]);
    assert_ne!(actual["signatures"], alternate["signatures"]);
    bundle["timestamp"] = "same-body-other-signature.json".into();
    fs::write(
        f.directory.join("signature-equiv.json"),
        serde_json::to_vec(&bundle).unwrap(),
    )
    .unwrap();
    let before = fs::read(f.store("state").join("metadata-qa.sqlite")).unwrap();
    let refused = f.accept("state", "signature-equiv");
    assert!(!refused.status.success());
    assert_eq!(
        String::from_utf8(refused.stderr).unwrap().trim(),
        "METADATA_EQUIVOCATION"
    );
    assert_eq!(
        fs::read(f.store("state").join("metadata-qa.sqlite")).unwrap(),
        before
    );
}
#[test]
fn actual_corrupt_store_is_refused_without_repair_and_exact_restore_recovers() {
    let f = Fixture::new();
    f.init("state");
    assert_success(f.accept("state", "v1"));
    let expected = f.inspect("state");
    let db = rusqlite::Connection::open(f.store("state").join("metadata-qa.sqlite")).unwrap();
    let row: (String, i64, String) = db
        .query_row(
            "SELECT context,version,hash FROM waters WHERE context LIKE '%timestamp%'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    db.execute("DELETE FROM waters WHERE context=?", [&row.0])
        .unwrap();
    assert!(
        !f.command("inspect", "state")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM waters WHERE context=?",
            [&row.0],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    db.execute(
        "INSERT INTO waters VALUES(?,?,?)",
        rusqlite::params![row.0, row.1, row.2],
    )
    .unwrap();
    drop(db);
    assert_eq!(f.inspect("state"), expected);
}
#[test]
fn actual_schema_and_each_persistent_field_are_bounded_before_materialization() {
    let f = Fixture::new();
    f.init("state");
    assert_success(f.accept("state", "v1"));
    let expected = f.inspect("state");
    let path = f.store("state").join("metadata-qa.sqlite");
    let original = fs::read(&path).unwrap();
    {
        let mut store =
            pinvou_upgrade_metadata_qa::QaStore::open_existing(&f.store("state"), &f.pin).unwrap();
        assert!(matches!(
            store.original(&"a".repeat(1_048_577), 2),
            Err(pinvou_upgrade_metadata_qa::Error::InvalidInput)
        ));
        assert!(matches!(
            store.original_after(&"x".repeat(1_048_577), 2),
            Err(pinvou_upgrade_metadata_qa::Error::InvalidInput)
        ));
        assert!(store.original(&f.pin, 2).is_ok());
    }
    let huge = "x".repeat(1_048_577);
    // Each corruption is an explicit isolated owner COMMIT; the real CLI must
    // reject it without healing, then reopen the exact restored original bytes.
    let statements = [
        "CREATE TABLE unexpected(value TEXT)",
        "UPDATE store SET product=?",
        "UPDATE store SET root_hash=?",
        "UPDATE waters SET context=? WHERE context LIKE '%timestamp%'",
        "UPDATE batches SET component=?",
        "UPDATE sets SET timestamp_hash=?",
        "UPDATE members SET hash=? WHERE ordinal=0",
        "UPDATE active SET component=?",
        "UPDATE objects SET hash=? WHERE hash=(SELECT anchor_hash FROM store)",
        "UPDATE batches SET evidence=CAST(? AS BLOB)",
        "UPDATE objects SET bytes=CAST(? AS BLOB) WHERE hash=(SELECT anchor_hash FROM store)",
    ];
    for statement in statements {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("PRAGMA foreign_keys=OFF;PRAGMA ignore_check_constraints=ON;")
            .unwrap();
        if statement.contains('?') {
            db.execute(statement, [&huge]).unwrap();
        } else {
            db.execute_batch(statement).unwrap();
        }
        drop(db);
        let corrupt = fs::read(&path).unwrap();
        let out = f.command("inspect", "state").output().unwrap();
        assert!(!out.status.success(), "{statement}");
        assert_eq!(
            String::from_utf8(out.stderr).unwrap().trim(),
            "METADATA_QA_STORE_INVALID",
            "{statement}"
        );
        assert_eq!(fs::read(&path).unwrap(), corrupt);
        fs::write(&path, &original).unwrap();
        assert_eq!(f.inspect("state"), expected);
    }
    // Object count is unchanged here: the known definition's byte-length gate
    // itself must reject a huge valid SQL comment before fetching its text.
    let db = rusqlite::Connection::open(&path).unwrap();
    let definition: String = db
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='objects'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    db.execute_batch("PRAGMA writable_schema=ON;").unwrap();
    db.execute(
        "UPDATE sqlite_schema SET sql=? WHERE name='objects'",
        [format!("{definition} /*{huge}*/")],
    )
    .unwrap();
    db.execute_batch("PRAGMA schema_version=900;PRAGMA writable_schema=OFF;")
        .unwrap();
    drop(db);
    let corrupt = fs::read(&path).unwrap();
    let out = f.command("inspect", "state").output().unwrap();
    assert!(!out.status.success());
    assert_eq!(
        String::from_utf8(out.stderr).unwrap().trim(),
        "METADATA_QA_STORE_INVALID"
    );
    assert_eq!(fs::read(&path).unwrap(), corrupt);
    fs::write(&path, &original).unwrap();
    assert_eq!(f.inspect("state"), expected);
}
#[test]
fn actual_independent_concurrent_processes_serialize_without_lost_waters() {
    let f = Fixture::new();
    f.init("state");
    assert_success(f.accept("state", "v1"));
    let mut left = f.command("accept", "state");
    left.arg(f.directory.join("v2.json"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut right = f.command("accept", "state");
    right
        .arg(f.directory.join("equiv2.json"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let left = left.spawn().unwrap();
    let right = right.spawn().unwrap();
    let a = left.wait_with_output().unwrap();
    let b = right.wait_with_output().unwrap();
    assert_ne!(a.status.success(), b.status.success());
    assert_eq!(f.inspect("state")["generation"], 3);
    let refused = if a.status.success() { b } else { a };
    assert_eq!(
        String::from_utf8(refused.stderr).unwrap().trim(),
        "METADATA_EQUIVOCATION"
    );
}

#[test]
fn actual_closed_stdout_after_init_and_accept_is_uncertain_but_complete() {
    fn closed_pipe(mut cmd: Command) -> Output {
        cmd.env_remove("PINVOU_METADATA_QA_CHECKPOINT")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        // Close the actual OS read end immediately; no synthetic fault flag,
        // replacement stdout writer or post-COMMIT adapter is used here.
        drop(child.stdout.take());
        child.wait_with_output().unwrap()
    }
    let f = Fixture::new();
    let init = closed_pipe(f.command("init", "state"));
    assert!(!init.status.success());
    assert_eq!(
        String::from_utf8(init.stderr).unwrap().trim(),
        "METADATA_QA_COMMIT_UNCERTAIN"
    );
    assert_eq!(f.inspect("state")["generation"], 1);
    let mut command = f.command("accept", "state");
    command.arg(f.directory.join("v1.json"));
    let accept = closed_pipe(command);
    assert!(!accept.status.success());
    assert_eq!(
        String::from_utf8(accept.stderr).unwrap().trim(),
        "METADATA_QA_COMMIT_UNCERTAIN"
    );
    assert_eq!(f.inspect("state")["generation"], 2);
    assert_eq!(assert_success(f.accept("state", "v1"))["generation"], 2);
}

#[cfg(feature = "fault-tests")]
#[test]
fn actual_child_kill_before_and_after_sql_commit_reopens_whole_old_or_new_set() {
    use std::{
        io::{BufRead, BufReader, Read},
        sync::mpsc,
        thread,
        time::Duration,
    };
    let f = Fixture::new();
    for stage in [
        "roots-written",
        "objects-written",
        "head-written",
        "before-commit",
        "after-commit",
    ] {
        let name = stage;
        f.init(name);
        assert_success(f.accept(name, "v1"));
        let mut cmd = f.command("accept", name);
        cmd.arg(f.directory.join("v2.json"))
            .env("PINVOU_METADATA_QA_CHECKPOINT", stage)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let drain = thread::spawn(move || {
            let mut read = BufReader::new(stdout);
            let mut line = String::new();
            let result = read.read_line(&mut line);
            let _ = tx.send((result.is_ok(), line));
            let mut tail = String::new();
            read.read_to_string(&mut tail).unwrap();
        });
        let errors = thread::spawn(move || {
            let mut bytes = Vec::new();
            let mut stream = stderr;
            stream.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let reached = rx.recv_timeout(Duration::from_secs(60));
        child.kill().unwrap();
        child.wait().unwrap();
        drain.join().unwrap();
        let errors = errors.join().unwrap();
        let (read, line) = reached.expect("actual SQL checkpoint must be reached before kill");
        assert!(read);
        assert_eq!(line.trim(), format!("QA_CHECKPOINT {stage}"));
        assert!(errors.is_empty());
        assert_eq!(
            f.inspect(name)["generation"],
            if stage == "after-commit" { 3 } else { 2 }
        );
        assert_success(f.accept(name, "v2"));
        assert_eq!(f.inspect(name)["generation"], 3);
    }
}

#[test]
fn actual_current_context_expiry_and_role_deny_refuse_without_new_acceptance() {
    let f = Fixture::new();
    f.init("current-inputs");
    assert_eq!(
        assert_success(f.accept("current-inputs", "v1"))["generation"],
        2
    );
    let before = f.inspect("current-inputs");
    let database = f.store("current-inputs").join("metadata-qa.sqlite");
    let original_database = fs::read(&database).unwrap();
    let original: Value =
        serde_json::from_slice(&fs::read(f.directory.join("v2.json")).unwrap()).unwrap();
    let root: Value =
        serde_json::from_slice(&fs::read(f.directory.join("anchor.json")).unwrap()).unwrap();
    let timestamp: Value = serde_json::from_slice(
        &fs::read(f.directory.join(original["timestamp"].as_str().unwrap())).unwrap(),
    )
    .unwrap();
    let timestamp_key = timestamp["signatures"][0]["keyId"].as_str().unwrap();
    let policy = root["signed"]["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|role| role["role"] == "timestamp" && role["component"] == "app")
        .unwrap();
    assert!(
        policy["keyIds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == timestamp_key)
    );
    assert!(
        root["signed"]["roles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|role| role["role"] == "timestamp" && role["component"] == "engine")
    );
    let expired_at = f.now + 86_400_000 + 1000;
    assert!(timestamp["signed"]["expiresAt"].as_i64().unwrap() < expired_at);
    assert!(root["signed"]["expiresAt"].as_i64().unwrap() > expired_at);

    for name in ["wrong-context", "expired-window", "denied-role-key"] {
        let mut changed = original.clone();
        match name {
            "wrong-context" => changed["component"] = Value::from("engine"),
            "expired-window" => changed["suppliedNowMs"] = Value::from(expired_at),
            "denied-role-key" => {
                changed["deniedByRole"] = serde_json::json!({"timestamp":[timestamp_key]})
            }
            _ => unreachable!(),
        }
        fs::write(
            f.directory.join(format!("{name}.json")),
            serde_json::to_vec(&changed).unwrap(),
        )
        .unwrap();
        let output = f.accept("current-inputs", name);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert_eq!(error.trim(), "METADATA_QA_VERIFICATION_FAILED");
        eprintln!("QA current-input case {name}: {}", error.trim());
        assert_eq!(f.inspect("current-inputs"), before);
        assert_eq!(fs::read(&database).unwrap(), original_database);
    }
    assert_eq!(
        assert_success(f.accept("current-inputs", "v2"))["generation"],
        3
    );
    assert_eq!(f.inspect("current-inputs")["generation"], 3);
}
