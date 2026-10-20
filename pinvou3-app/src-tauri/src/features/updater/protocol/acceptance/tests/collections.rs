//! Real CLI/SQLite collection boundaries using independently signed QA graphs.
//! These ordinary files and supplied times never qualify protected production state.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const OBJECT_BYTES: usize = 1_048_576;
const OUTPUT_BYTES: u64 = (2 * OBJECT_BYTES + 4097) as u64;

// Drain both bounded pipes while waiting. A timeout kills and joins the actual
// child and both readers; it is a test budget, never an artifact download policy.
fn child(mut command: Command) -> Output {
    let mut running = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("collection test child must start");
    let stdout = running.stdout.take().unwrap();
    let stderr = running.stderr.take().unwrap();
    let output = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(OUTPUT_BYTES + 1)
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    });
    let errors = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(4097).read_to_end(&mut bytes).unwrap();
        bytes
    });
    let started = Instant::now();
    let mut timed_out = false;
    let mut wait_failed = false;
    let status = loop {
        match running.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(_) => {
                wait_failed = true;
                let _ = running.kill();
                break running.wait();
            }
        }
        if started.elapsed() >= Duration::from_secs(360) {
            timed_out = true;
            let _ = running.kill();
            break running.wait();
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = output.join();
    let stderr = errors.join();
    let stdout = stdout.expect("collection stdout reader must join");
    let stderr = stderr.expect("collection stderr reader must join");
    let status = status.expect("collection child must join");
    assert!(!wait_failed, "collection child status observation failed");
    assert!(!timed_out, "collection test subprocess exceeded its budget");
    assert!(
        stdout.len() as u64 <= OUTPUT_BYTES && stderr.len() <= 4096,
        "collection test child exceeded its bounded diagnostic framing"
    );
    Output {
        status,
        stdout,
        stderr,
    }
}

struct Fixture {
    directory: PathBuf,
    temporary_parent: PathBuf,
    pin: String,
    now: i64,
    details: Value,
}
impl Fixture {
    fn new(scenario: &str) -> Self {
        let temporary_parent = std::env::temp_dir().canonicalize().unwrap();
        let directory = temporary_parent.join(format!(
            "pinvou-t27-collections-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(!directory.exists());
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let reference = manifest
            .ancestors()
            .nth(7)
            .unwrap()
            .join("specs/upgrade/v1/reference/signatures.mjs");
        let mut command = Command::new("node");
        command
            .arg(manifest.join("tests/collection_fixtures.mjs"))
            .arg(reference)
            .arg(&directory)
            .arg(scenario);
        let output = child(command);
        assert!(
            output.status.success(),
            "independent collection signature/reference fixture failed"
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        Self {
            directory,
            temporary_parent,
            pin: result["pin"].as_str().unwrap().to_owned(),
            now: result["now"].as_i64().unwrap(),
            details: result["details"].clone(),
        }
    }
    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }
    fn command(&self, action: &str, store: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_pinvou-upgrade-metadata-qa"));
        command.arg(action).arg(self.path(store));
        if action == "init" {
            command
                .arg(self.path("anchor.json"))
                .arg(&self.pin)
                .arg(self.now.to_string());
        } else {
            command.arg(&self.pin);
        }
        command
    }
    fn init(&self, store: &str) {
        success(child(self.command("init", store)));
    }
    fn accept(&self, store: &str, bundle: &str) -> Output {
        let mut command = self.command("accept", store);
        command.arg(self.path(&format!("{bundle}.json")));
        child(command)
    }
    fn inspect(&self, store: &str) -> Value {
        success(child(self.command("inspect", store)))
    }
    fn original(&self, store: &str, hash: &str, generation: u64) -> Value {
        let mut command = self.command("original", store);
        command.arg(hash).arg(generation.to_string());
        success(child(command))
    }
    fn database(&self, store: &str) -> PathBuf {
        self.path(store).join("metadata-qa.sqlite")
    }
    fn unchanged(&self, store: &str, before: &Value, fingerprint: &(u64, Vec<u8>)) {
        assert_eq!(&self.inspect(store), before);
        assert_eq!(&file_identity(&self.database(store)), fingerprint);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Failed tests retain their own signed public inputs for private diagnosis.
        // Delete only this unique absolute directory, after resolving it again.
        if thread::panicking() {
            return;
        }
        let metadata =
            fs::symlink_metadata(&self.directory).expect("owned fixture directory remains present");
        let resolved = self
            .directory
            .canonicalize()
            .expect("owned fixture path resolves");
        assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
        assert_eq!(resolved.parent(), Some(self.temporary_parent.as_path()));
        assert_eq!(resolved, self.directory);
        assert!(
            resolved
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("pinvou-t27-collections-")
        );
        fs::remove_dir_all(&resolved).expect("successful owned fixture cleanup");
    }
}
fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "actual collection CLI refused a verified positive input"
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}
fn refusal(output: Output, code: &str) {
    assert!(!output.status.success(), "invalid collection was accepted");
    assert!(
        output.stdout.is_empty(),
        "a refusal returned uncommitted acceptance bytes"
    );
    let error = String::from_utf8(output.stderr).unwrap();
    assert_eq!(error.trim(), code);
    eprintln!("QA collection actual refusal: {}", error.trim());
}
fn file_identity(path: &Path) -> (u64, Vec<u8>) {
    let mut file = File::open(path).unwrap();
    let length = file.metadata().unwrap().len();
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    (length, hash.finalize().to_vec())
}

#[test]
fn genuine_committed_root_batches_exceed_lifetime4096_and_reopen_expired_history() {
    let f = Fixture::new("roots");
    assert_eq!(
        f.details["committedSuccessors"],
        serde_json::json!([4096, 1])
    );
    assert!(
        f.details["firstTailExpiresAt"].as_i64().unwrap()
            < f.details["secondObservedAt"].as_i64().unwrap()
    );
    f.init("overflow-state");
    let original = f.inspect("overflow-state");
    let before = file_identity(&f.database("overflow-state"));
    refusal(
        f.accept("overflow-state", "overflow"),
        "METADATA_QA_INPUT_INVALID",
    );
    f.unchanged("overflow-state", &original, &before);
    assert_eq!(f.details["expiredNewBatchReferenceRefused"], true);
    refusal(
        f.accept("overflow-state", "expired-prefix"),
        "METADATA_QA_VERIFICATION_FAILED",
    );
    f.unchanged("overflow-state", &original, &before);
    f.init("state");
    assert_eq!(success(f.accept("state", "first"))["generation"], 2);
    assert_eq!(f.inspect("state")["rootSha256"], f.details["firstTailHash"]);
    assert_eq!(success(f.accept("state", "second"))["generation"], 3);
    let reopened = f.inspect("state");
    assert_eq!(reopened["rootSha256"], f.details["secondRootHash"]);
    assert_eq!(reopened["generation"], 3);
    assert_eq!(reopened["currentQualificationChecked"], false);
    let db = rusqlite::Connection::open_with_flags(
        f.database("state"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let batches: Vec<i64> = db
        .prepare("SELECT count(*) FROM root_steps GROUP BY generation ORDER BY generation")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(batches, vec![4096, 1]);
    drop(db);
    for (name, version) in [
        ("root-2.json", 2),
        ("root-4097.json", 4097),
        ("later-root-4098.json", 4098),
    ] {
        let expected = fs::read(f.path(name)).unwrap();
        let hash: String = Sha256::digest(&expected)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let retained = f.original("state", &hash, 3);
        assert_eq!(
            retained["originalUtf8"].as_str().unwrap().as_bytes(),
            expected
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&expected).unwrap()["signed"]["version"],
            version
        );
    }
}

#[test]
fn maximum_snapshot_inventory_requires_exact_last_member_without_partial_acceptance() {
    let f = Fixture::new("snapshot");
    assert_eq!(f.details["snapshotMembers"], 4096);
    assert_eq!(f.details["releases"], 4092);
    assert!(f.details["snapshotBytes"].as_u64().unwrap() <= OBJECT_BYTES as u64);
    assert!(f.details["largestTargetBytes"].as_u64().unwrap() <= OBJECT_BYTES as u64);
    assert_eq!(f.details["extraControlReferencedMembers"], 4095);
    assert_eq!(f.details["extraControlSuppliedMembers"], 4096);
    f.init("state");
    assert_eq!(success(f.accept("state", "maximum"))["generation"], 2);
    let before = f.inspect("state");
    let fingerprint = file_identity(&f.database("state"));
    let db = rusqlite::Connection::open_with_flags(
        f.database("state"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM members WHERE set_id=(SELECT set_id FROM active WHERE component='app')", [], |row| row.get::<_, i64>(0)).unwrap(), 4096);
    drop(db);
    // The duplicate descriptor really repeats the first original member at
    // the final ordinal. Prove that exact constraint on the actual accepted
    // schema, rather than treating any storage failure as a valid graph test.
    let duplicate: Value =
        serde_json::from_slice(&fs::read(f.path("duplicate-final.json")).unwrap()).unwrap();
    let names = duplicate["members"].as_array().unwrap();
    assert_eq!(names.len(), 4096);
    let first = fs::read(f.path(names[0].as_str().unwrap())).unwrap();
    let repeated = fs::read(f.path(names[4095].as_str().unwrap())).unwrap();
    assert_eq!(first, repeated);
    let repeated_hash: String = Sha256::digest(&repeated)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mut db = rusqlite::Connection::open_with_flags(
        f.database("state"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    let transaction = db.transaction().unwrap();
    let set_id: i64 = transaction
        .query_row(
            "SELECT set_id FROM active WHERE component='app'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let first_hash: String = transaction
        .query_row(
            "SELECT hash FROM members WHERE set_id=?1 AND ordinal=0",
            [set_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(first_hash, repeated_hash);
    assert_eq!(
        transaction
            .execute(
                "DELETE FROM members WHERE set_id=?1 AND ordinal=4095",
                [set_id]
            )
            .unwrap(),
        1
    );
    let error = transaction
        .execute(
            "INSERT INTO members(set_id,ordinal,hash) VALUES(?1,4095,?2)",
            rusqlite::params![set_id, repeated_hash],
        )
        .unwrap_err();
    assert!(matches!(error,
        rusqlite::Error::SqliteFailure(ref code, ref message)
            if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                && message.as_deref() == Some("UNIQUE constraint failed: members.set_id, members.hash")
    ));
    eprintln!("QA duplicate-final actual SQLite source: UNIQUE2067 members.set_id,members.hash");
    transaction.rollback().unwrap();
    drop(db);
    f.unchanged("state", &before, &fingerprint);

    for (bundle, code) in [
        ("missing-final", "METADATA_QA_VERIFICATION_FAILED"),
        ("duplicate-final", "METADATA_QA_STORAGE_UNAVAILABLE"),
        ("extra-final", "METADATA_QA_VERIFICATION_FAILED"),
    ] {
        eprintln!("QA collection case: {bundle}");
        refusal(f.accept("state", bundle), code);
        // FULL + DELETE has no separate WAL. Every refused child has exited,
        // and these complete bytes equal the fully revalidated baseline. The
        // final original/idempotent/fresh-inspect checks below remain actual.
        assert_eq!(file_identity(&f.database("state")), fingerprint);
    }
    let last = f.original("state", f.details["lastOriginalHash"].as_str().unwrap(), 2);
    assert_eq!(
        last["originalUtf8"].as_str().unwrap().as_bytes(),
        fs::read(f.path(f.details["lastOriginalName"].as_str().unwrap())).unwrap()
    );
    assert_eq!(success(f.accept("state", "maximum"))["generation"], 2);
    f.unchanged("state", &before, &fingerprint);
}

#[test]
fn actual_bundle_reader_accepts_exact_limit_and_rejects_one_extra_byte() {
    let f = Fixture::new("bounds");
    let compact = fs::read(f.path("complete.json")).unwrap();
    assert!(pinvou_upgrade_metadata_qa::Bundle::parse(&compact).is_ok());
    let exact = pinvou_upgrade_metadata_qa::read_bounded(&f.path("exact.json")).unwrap();
    assert_eq!(exact.len(), OBJECT_BYTES);
    assert_eq!(exact, fs::read(f.path("exact.json")).unwrap());
    assert_eq!(
        pinvou_upgrade_metadata_qa::read_bounded(&f.path("over.json")),
        Err(pinvou_upgrade_metadata_qa::Error::InvalidInput)
    );
    assert_eq!(
        fs::metadata(f.path("over.json")).unwrap().len(),
        OBJECT_BYTES as u64 + 1
    );
    f.init("state");
    assert_eq!(success(f.accept("state", "exact"))["generation"], 2);
    let before = f.inspect("state");
    let fingerprint = file_identity(&f.database("state"));
    refusal(f.accept("state", "over"), "METADATA_QA_INPUT_INVALID");
    f.unchanged("state", &before, &fingerprint);
    assert_eq!(success(f.accept("state", "complete"))["generation"], 2);
}

#[test]
fn release_id_waters_survive_membership_removal_and_ignore_shared_app_version() {
    let f = Fixture::new("release");
    assert_eq!(f.details["sameAppVersion"], true);
    f.init("state");
    assert_eq!(success(f.accept("state", "r1"))["generation"], 2);
    assert_eq!(success(f.accept("state", "r2"))["generation"], 3);
    let db = rusqlite::Connection::open_with_flags(
        f.database("state"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut statement = db
        .prepare("SELECT context,version,hash FROM waters ORDER BY context")
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap();
    let mut releases = std::collections::BTreeMap::new();
    for row in rows {
        let (context, version, hash) = row.unwrap();
        let context: Value = serde_json::from_str(&context).unwrap();
        if context["role"] == "release" {
            releases.insert(
                context["scope"]["releaseId"].as_str().unwrap().to_owned(),
                (version, hash),
            );
        }
    }
    assert_eq!(releases.len(), 2);
    assert_eq!(
        releases["r1"],
        (2, f.details["r1Hash"].as_str().unwrap().to_owned())
    );
    assert_eq!(
        releases["r2"],
        (1, f.details["r2Hash"].as_str().unwrap().to_owned())
    );
    drop(statement);
    drop(db);
    let before = f.inspect("state");
    let fingerprint = file_identity(&f.database("state"));
    refusal(f.accept("state", "rollback-r1"), "METADATA_ROLLBACK");
    f.unchanged("state", &before, &fingerprint);
    let original = f.original("state", f.details["r1Hash"].as_str().unwrap(), 3);
    assert_eq!(original["version"], 2);
    assert_eq!(success(f.accept("state", "r2"))["generation"], 3);
    f.unchanged("state", &before, &fingerprint);
}
