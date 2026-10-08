//! Execute-level contract tests for the `artifacts` family
//! (`crates/cli/src/artifacts.rs`), complementing the parse-shape coverage in
//! `sessions_contract.rs` and the `#[cfg(test)]` unit tests inside the module.
//!
//! Scope: the behavioral lanes of `artifacts list|read|write` — the JSON
//! payload shapes, the stable error codes scripts key on, the 10 MiB
//! save-cap refusal and its ordering, the credential-source gate on
//! `--file`, the session-storage containment codes, and the
//! `skipped_sessions` disclosure. Every default test runs against a
//! throwaway `PINVOU3_HOME` (ENV_LOCK serialization, same pattern as
//! `sessions_contract.rs`) and touches no network, model endpoint, display,
//! or real `$HOME`. The one process-global lane, `--stdin`, cannot be driven
//! in-process (execute reads the test runner's own stdin), so it runs the
//! real binary with a piped stdin — the house subprocess pattern from
//! `scheduled_contract.rs`.
//!
//! Deliberately NOT duplicated here (each lane is pinned elsewhere):
//! - argv shapes and exit-2 usage errors: `sessions_contract.rs`
//!   (`every_artifacts_subcommand_parses_and_unknown_flags_exit_two`) and
//!   the in-module unit tests of `artifacts.rs`;
//! - the human-renderer column collapse, the aux-stem index skip, the
//!   non-markdown overwrite refusal, the scheduled-run write refusal, and
//!   the scheduled-run read allowance: the `artifacts_*` tests in
//!   `sessions_contract.rs`;
//! - the relative-`PINVOU3_HOME` sandbox refusal: `dispatch_contract.rs`
//!   and `projects_contract.rs`;
//! - the blocklist mirror itself (every component and every system prefix):
//!   the `sensitive_path_mirror_matches_the_upstream_blacklists` unit test.
//!
//! The two byte caps are pinned by value — the save cap (10 MiB, the GUI
//! `MAX_EDITABLE_MARKDOWN_BYTES` this lane mirrors) and the index scan cap
//! (32 MiB). The crate-private constants are not importable from an
//! integration test, and a silent cap change is exactly what these tests
//! must catch. No `#[ignore]` opt-in tests are needed: nothing here needs a
//! display host, a model, or a live endpoint.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use pinvou_cli::{CliError, CliOutcome, ExitCode, execute, parse_args};
use pinvou3_lib::features::sessions::SessionStore;

/// The GUI artifact-editor save cap (`MAX_EDITABLE_MARKDOWN_BYTES`) that the
/// write lane mirrors. Integration tests cannot import the crate-private
/// constant, so the documented number is pinned here.
const SAVE_CAP_BYTES: usize = 10 * 1024 * 1024;

/// The per-record index scan cap (`MAX_LIST_SCAN_BYTES` in `artifacts.rs`),
/// pinned by value for the same reason.
const SCAN_CAP_BYTES: u64 = 32 * 1024 * 1024;

/// Serialises tests that mutate the process-global `PINVOU3_HOME` environment
/// variable, preventing data races when the parallel test runner executes them
/// concurrently.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Sets `PINVOU3_HOME` to a fresh throwaway directory for the duration of
/// the test and restores the previous value on drop.
struct HomeGuard {
    previous: Option<OsString>,
    root: PathBuf,
}

impl HomeGuard {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pinvou-cli-artifacts-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let previous = std::env::var_os("PINVOU3_HOME");
        // SAFETY: the caller holds ENV_LOCK for the whole test, so env writes
        // are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
        Self { previous, root }
    }

    /// `~/.pinvou3/sessions` under the sandbox home.
    fn sessions_root(&self) -> PathBuf {
        self.root.join("sessions")
    }

    /// The ledger workspace relative artifact paths resolve against
    /// (`sessions_root/<id>/workspace` for a plain chat session fixture).
    fn workspace_of(&self, id: &str) -> PathBuf {
        self.sessions_root().join(id).join("workspace")
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            // SAFETY: ENV_LOCK is held by the owning test.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: ENV_LOCK is held by the owning test.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn run(arguments: &[&str]) -> Result<CliOutcome, CliError> {
    let parsed = parse_args(arguments).expect("arguments must parse");
    execute(parsed)
}

fn run_json(arguments: &[&str]) -> serde_json::Value {
    let mut owned = arguments.to_vec();
    owned.extend(["--output", "json"]);
    let outcome = run(&owned).expect("execute must succeed");
    serde_json::from_str(&outcome.stdout).expect("json output must be a single serde_json line")
}

/// Runs the command and asserts the exit-1 (host failure) class, returning
/// the error for message assertions.
fn expect_failed(arguments: &[&str]) -> CliError {
    match run(arguments) {
        Ok(outcome) => panic!(
            "expected a failed execution, got exit {:?}: {}",
            outcome.exit_code, outcome.stdout
        ),
        Err(error) => {
            assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
            error
        }
    }
}

/// Creates one empty chat session through `SessionStore` (the exact path the
/// CLI's `open_store` boots) and returns its id.
fn create_session_fixture() -> String {
    let store = SessionStore::boot().expect("boot session store");
    let session = store
        .create_new("test-model".to_owned(), None, std::env::temp_dir())
        .expect("create session");
    session.metadata.id
}

/// Drives the real binary with `stdin` piped to it (the `--stdin` write lane
/// is process-global, so it cannot be exercised through in-process dispatch).
fn drive_binary(home: &HomeGuard, arguments: &[&str], stdin: &[u8]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pinvou"))
        .args(arguments)
        .env("PINVOU3_HOME", &home.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the pinvou binary must spawn");
    let mut pipe = child.stdin.take().expect("stdin must be piped");
    // The child drains the pipe while reading; if a mutation makes it exit
    // early, this write fails with EPIPE and the status assertions below
    // surface the changed behavior instead of the test hanging.
    let _ = pipe.write_all(stdin);
    drop(pipe);
    child
        .wait_with_output()
        .expect("the pinvou binary must run to completion")
}

// ── happy paths: the JSON contract of every subcommand ─────────────────────

/// One fixture session with a tracked workspace deliverable pins every field
/// of the three success payloads: the list row (`DeliverableItem` shape,
/// including `source` from the record title, numeric `mtime`/`size`, and the
/// always-present `skipped_sessions`), the read payload, and the write
/// payload — across both writable areas (`workspace/` and `artifacts/`).
/// A mutation to any field name, shape, or the read-back content fails here.
#[test]
fn artifacts_list_read_write_happy_paths_pin_the_json_contract() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("happy");
    let id = create_session_fixture();

    let workspace = home.workspace_of(&id);
    std::fs::create_dir_all(&workspace).unwrap();
    let report = workspace.join("report.md");
    std::fs::write(&report, "# Report\n\nfirst version\n").unwrap();
    // The index reports canonical paths; on macOS $TMPDIR (/var/folders/…)
    // canonicalizes to /private/var/…, so expectations use the canonical form.
    let report = std::fs::canonicalize(&report).unwrap();
    let content_len = std::fs::metadata(&report).unwrap().len();

    let store = SessionStore::boot().expect("boot session store");
    // A fixture-chosen title makes the row's `source` field deterministic
    // here instead of pinning the foundation's new-chat default.
    store
        .set_title(&id, "Quarterly report".to_owned())
        .expect("set title");
    store
        .update_artifacts(&id, vec![report.to_string_lossy().to_string()])
        .expect("track artifact");
    drop(store);

    // list: exactly one row with the full DeliverableItem shape.
    let value = run_json(&["pinvou", "artifacts", "list"]);
    let rows = value["artifacts"].as_array().expect("artifacts array");
    assert_eq!(rows.len(), 1, "{value:?}");
    assert_eq!(rows[0]["name"], "report.md");
    assert_eq!(rows[0]["ext"], "md");
    assert_eq!(rows[0]["category"], "doc");
    assert_eq!(rows[0]["session_id"], id);
    assert_eq!(rows[0]["source"], "Quarterly report");
    assert_eq!(rows[0]["path"], report.display().to_string());
    let mtime = rows[0]["mtime"].as_i64().expect("numeric mtime");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(mtime > 0 && mtime <= now + 5, "mtime {mtime} vs now {now}");
    assert_eq!(rows[0]["size"], content_len);
    // Always present and empty on the clean day, so a consumer can tell
    // "no deliverables" from "index incomplete".
    assert_eq!(value["skipped_sessions"], serde_json::json!([]));

    // read: the JSON payload names the session and the validated path, and
    // carries the verbatim content; the human lane prints the content body.
    let value = run_json(&["pinvou", "artifacts", "read", &id, "report.md"]);
    assert_eq!(value["session_id"], id);
    assert_eq!(value["path"], report.display().to_string());
    assert_eq!(value["content"], "# Report\n\nfirst version\n");
    let outcome =
        run(&["pinvou", "artifacts", "read", &id, "report.md"]).expect("human read");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert_eq!(outcome.stdout, "# Report\n\nfirst version\n");

    // write --file overwrites the existing workspace artifact in place and
    // reports the byte count; a follow-up read observes the new content.
    let source = home.root.join("source.md");
    std::fs::write(&source, "# Report\n\nsecond version\n").unwrap();
    let value = run_json(&[
        "pinvou",
        "artifacts",
        "write",
        &id,
        "report.md",
        "--file",
        source.to_str().unwrap(),
    ]);
    assert_eq!(value["session_id"], id);
    assert_eq!(value["path"], report.display().to_string());
    assert_eq!(value["bytes"], "# Report\n\nsecond version\n".len());
    let outcome = run(&["pinvou", "artifacts", "read", &id, "report.md"])
        .expect("read after write");
    assert_eq!(outcome.stdout, "# Report\n\nsecond version\n");

    // The second writable area: an existing file under the session's
    // artifacts/ directory, addressed relative to the ledger workspace.
    // Relative paths resolve against workspace/, so ../artifacts/… is the
    // spelling that reaches sessions_root/<id>/artifacts.
    let artifacts_dir = home.sessions_root().join(&id).join("artifacts");
    std::fs::create_dir_all(&artifacts_dir).unwrap();
    std::fs::write(artifacts_dir.join("notes.md"), "old notes\n").unwrap();
    let value = run_json(&[
        "pinvou",
        "artifacts",
        "write",
        &id,
        "../artifacts/notes.md",
        "--file",
        source.to_str().unwrap(),
    ]);
    assert_eq!(value["bytes"], "# Report\n\nsecond version\n".len());
    let outcome = run(&["pinvou", "artifacts", "read", &id, "../artifacts/notes.md"])
        .expect("read artifacts-area file");
    assert_eq!(outcome.stdout, "# Report\n\nsecond version\n");
}

// ── write: the save cap and the credential-source gate ─────────────────────

/// The 10 MiB save cap is enforced with the GUI wire code
/// `markdown_artifact_is_too_large_to_save`, and it fires BEFORE the store
/// boot and target resolution: `write()` stats the source and refuses before
/// `open_store()`/`resolve_session_artifact` ever run. Both properties are
/// pinned — the over-cap file carries invalid-UTF-8 bytes, so an
/// implementation that read the source first would fail with the reader's
/// UTF-8 error instead of the stable cap code, and the ordering probe passes
/// a session id that could never resolve, so a cap check moved after
/// resolution would come back as `artifact_not_found` rather than the cap
/// code. The boundary is sharp: exactly the cap passes (the probe is
/// strictly greater-than) and lands whole.
#[test]
fn artifacts_write_refuses_over_cap_sources_before_any_store_boot() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("over-cap");
    let id = create_session_fixture();

    let workspace = home.workspace_of(&id);
    std::fs::create_dir_all(&workspace).unwrap();
    let report = workspace.join("report.md");
    std::fs::write(&report, "old\n").unwrap();
    let report = std::fs::canonicalize(&report).unwrap();

    // One byte over the cap: refused with the exact stable wire code, and
    // the target is untouched. 0xFF bytes are not valid UTF-8 on purpose —
    // see the test's doc comment.
    let over_cap = home.root.join("over-cap.md");
    std::fs::write(&over_cap, vec![0xFF_u8; SAVE_CAP_BYTES + 1]).unwrap();
    let error = expect_failed(&[
        "pinvou",
        "artifacts",
        "write",
        &id,
        "report.md",
        "--file",
        over_cap.to_str().unwrap(),
    ]);
    assert_eq!(
        error.to_string(),
        "markdown_artifact_is_too_large_to_save",
        "the over-cap refusal must carry the exact GUI wire code"
    );
    assert_eq!(
        std::fs::read_to_string(&report).unwrap(),
        "old\n",
        "a refused write must not touch the target"
    );

    // Ordering: the same refusal comes back for a session id that can never
    // resolve — the cap check precedes the store boot and the target
    // resolution, so the cap code (not `artifact_not_found` or a store
    // error) is what a script sees.
    let error = expect_failed(&[
        "pinvou",
        "artifacts",
        "write",
        "no-such-session",
        "report.md",
        "--file",
        over_cap.to_str().unwrap(),
    ]);
    assert_eq!(
        error.to_string(),
        "markdown_artifact_is_too_large_to_save",
        "the cap must be enforced before any session/store work: {error}"
    );

    // Boundary: exactly the cap is accepted (the probe is strictly `>`) and
    // the full payload lands.
    let at_cap = home.root.join("at-cap.md");
    std::fs::write(&at_cap, vec![b'b'; SAVE_CAP_BYTES]).unwrap();
    let value = run_json(&[
        "pinvou",
        "artifacts",
        "write",
        &id,
        "report.md",
        "--file",
        at_cap.to_str().unwrap(),
    ]);
    assert_eq!(value["bytes"], SAVE_CAP_BYTES);
    assert_eq!(
        std::fs::metadata(&report).unwrap().len(),
        SAVE_CAP_BYTES as u64,
        "the at-cap write must land in full"
    );
}

/// The `--file` source takes the credential-path policy: a source whose
/// canonical path crosses a blocked component (`.ssh`, `.env`) is refused
/// with the `artifacts write: refusing --file:` prefix, before anything is
/// written. The fixtures live under the SANDBOX home, not the real `$HOME` —
/// the gate inspects path components, so a sandbox-relative fake is refused
/// identically without touching machine state. The gate also precedes the
/// store/resolution work (probed with an unresolvable session id), and it is
/// about components, not substrings: a source merely NAMED like a blocked
/// key (`id_rsa_notes.md`) is an ordinary file and must write.
#[test]
fn artifacts_write_file_gate_refuses_credential_source_paths() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("credential-gate");
    let id = create_session_fixture();

    let workspace = home.workspace_of(&id);
    std::fs::create_dir_all(&workspace).unwrap();
    let report = workspace.join("report.md");
    std::fs::write(&report, "old\n").unwrap();
    let report = std::fs::canonicalize(&report).unwrap();

    let ssh_key = home.root.join(".ssh").join("id_rsa");
    std::fs::create_dir_all(ssh_key.parent().unwrap()).unwrap();
    std::fs::write(&ssh_key, "PRIVATE KEY\n").unwrap();
    let dotenv = home.root.join(".env");
    std::fs::write(&dotenv, "TOKEN=secret\n").unwrap();

    for source in [&ssh_key, &dotenv] {
        let error = expect_failed(&[
            "pinvou",
            "artifacts",
            "write",
            &id,
            "report.md",
            "--file",
            source.to_str().unwrap(),
        ]);
        let message = error.to_string();
        assert!(
            message.starts_with("artifacts write: refusing --file:"),
            "{source:?}: {message}"
        );
        assert!(
            message.contains("crosses the credential path component"),
            "{source:?}: {message}"
        );
        assert_eq!(
            std::fs::read_to_string(&report).unwrap(),
            "old\n",
            "a refused source must not reach the target"
        );
    }

    // Ordering: the gate fires before the store/resolution work too — the
    // unresolvable session id must not turn the refusal into
    // `artifact_not_found`.
    let error = expect_failed(&[
        "pinvou",
        "artifacts",
        "write",
        "no-such-session",
        "report.md",
        "--file",
        ssh_key.to_str().unwrap(),
    ]);
    assert!(
        error.to_string().starts_with("artifacts write: refusing --file:"),
        "the source gate must precede the session resolution: {error}"
    );

    // Components, not substrings: the blocked names are exact path
    // components, so a file whose name merely contains one stays writable.
    let benign = home.root.join("id_rsa_notes.md");
    std::fs::write(&benign, "# notes on key handling\n").unwrap();
    let value = run_json(&[
        "pinvou",
        "artifacts",
        "write",
        &id,
        "report.md",
        "--file",
        benign.to_str().unwrap(),
    ]);
    assert_eq!(value["bytes"], "# notes on key handling\n".len());
    assert_eq!(
        std::fs::read_to_string(&report).unwrap(),
        "# notes on key handling\n"
    );
}

// ── write/read: the stable error codes of the failure lanes ────────────────

/// Write failures carry distinct stable prefixes a script can key on: a
/// source that cannot be resolved reports `artifacts write: cannot resolve`,
/// while a target that does not exist reports `artifact_not_found` — and the
/// write lane is overwrite-only, so a missing target must NOT be created as
/// a side effect. (The non-markdown suffix refusal and the scheduled-run
/// write refusal are pinned in `sessions_contract.rs`.)
#[test]
fn artifacts_write_error_lanes_pin_stable_codes() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("write-errors");
    let id = create_session_fixture();

    // A --file that does not exist: the source prefix, distinct from the
    // target lanes below.
    let absent = home.root.join("absent-source.md");
    let error = expect_failed(&[
        "pinvou",
        "artifacts",
        "write",
        &id,
        "report.md",
        "--file",
        absent.to_str().unwrap(),
    ]);
    assert!(
        error
            .to_string()
            .starts_with("artifacts write: cannot resolve"),
        "{error}"
    );

    // A real source but a missing target: `artifact_not_found`, and the
    // overwrite-only rule means the failed write must not create the file.
    let source = home.root.join("plain.md");
    std::fs::write(&source, "replacement\n").unwrap();
    let target = home.workspace_of(&id).join("new.md");
    let error = expect_failed(&[
        "pinvou",
        "artifacts",
        "write",
        &id,
        "new.md",
        "--file",
        source.to_str().unwrap(),
    ]);
    assert!(
        error.to_string().starts_with("artifact_not_found"),
        "{error}"
    );
    assert!(
        !target.exists(),
        "write is overwrite-only: a missing target must not be created"
    );
}

/// The read containment codes beyond plain not-found: a relative path may
/// not reach ANOTHER session's storage (`artifact_session_mismatch`), a
/// leading-underscore directory is not an editable session
/// (`artifact_outside_editable_session`), and a file outside sessions
/// storage entirely — and outside this session's ledger tree, so the
/// scheduled-run read allowance cannot rescue it — is
/// `artifact_outside_session_storage`. A directory target is classified as
/// not-found ("is not a file"), not a crash.
#[test]
fn artifacts_read_error_lanes_pin_containment_codes() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("read-lanes");
    let id_a = create_session_fixture();
    let id_b = create_session_fixture();

    // Both sessions need an existing workspace: the canonicalize of the
    // requested path must reach the file for the containment codes to be
    // the thing under test (a missing intermediate would short-circuit to
    // plain `artifact_not_found`).
    let workspace_a = home.workspace_of(&id_a);
    std::fs::create_dir_all(&workspace_a).unwrap();
    let workspace_b = home.workspace_of(&id_b);
    std::fs::create_dir_all(&workspace_b).unwrap();
    std::fs::write(workspace_b.join("report.md"), "# B\n").unwrap();

    // Session mismatch: B's deliverable exists, but A must not reach it by
    // spelling a relative path through the shared sessions root.
    // Two levels up: the read base is the session's own workspace, so the
    // hop to the sessions root (where the sibling session lives) is `../..`.
    let escape = format!("../../{id_b}/workspace/report.md");
    let error = expect_failed(&["pinvou", "artifacts", "read", &id_a, &escape]);
    let message = error.to_string();
    assert!(
        message.starts_with("artifact_session_mismatch:"),
        "{message}"
    );
    assert!(message.contains(&id_b), "the owning session is named: {message}");

    // Leading-underscore directory under the sessions root: inside storage,
    // but never an editable session.
    let hidden = home.sessions_root().join("_private").join("artifacts");
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(hidden.join("a.md"), "secret\n").unwrap();
    let error = expect_failed(&[
        "pinvou",
        "artifacts",
        "read",
        &id_a,
        "../../_private/artifacts/a.md",
    ]);
    assert_eq!(error.to_string(), "artifact_outside_editable_session");

    // Outside sessions storage entirely (home root), and outside A's ledger
    // tree, so the writable=false ledger fallback cannot rescue it.
    std::fs::write(home.root.join("outside.md"), "nope\n").unwrap();
    let error = expect_failed(&["pinvou", "artifacts", "read", &id_a, "../../../outside.md"]);
    assert_eq!(error.to_string(), "artifact_outside_session_storage");

    // A directory target exists but is not a file: the not-found code with
    // the explicit "is not a file" classification.
    std::fs::create_dir_all(workspace_a.join("bundle")).unwrap();
    let error = expect_failed(&["pinvou", "artifacts", "read", &id_a, "bundle"]);
    let message = error.to_string();
    assert!(message.starts_with("artifact_not_found:"), "{message}");
    assert!(message.contains("is not a file"), "{message}");
}

// ── list: the skipped_sessions disclosure and the sessions-root states ─────

/// A session record the index scan cannot use must be DISCLOSED in
/// `skipped_sessions`, never silently dropped — an incomplete listing must
/// stay distinguishable from an empty one. Two unscannable shapes are
/// covered: a record that is not JSON at all (it reaches the parse and fails
/// there), and a record over the 32 MiB scan cap (skipped by the size probe
/// before any read; `set_len` keeps the fixture sparse since the probe only
/// stats). The single-session filter, by contrast, skips foreign stems by
/// FILENAME before any read or parse, so a filtered listing has nothing
/// unscannable left to disclose — `skipped_sessions` comes back empty.
#[test]
fn artifacts_list_reports_unscannable_records_in_skipped_sessions() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("skipped");
    let id = create_session_fixture();

    let workspace = home.workspace_of(&id);
    std::fs::create_dir_all(&workspace).unwrap();
    let report = workspace.join("report.md");
    std::fs::write(&report, "# Report\n").unwrap();
    let report = std::fs::canonicalize(&report).unwrap();
    let store = SessionStore::boot().expect("boot session store");
    store
        .update_artifacts(&id, vec![report.to_string_lossy().to_string()])
        .expect("track artifact");
    drop(store);

    let sessions = home.sessions_root();
    // Not JSON and carrying neither the `artifacts` nor the `metadata` key,
    // so it reaches the serde parse and must be disclosed on failure.
    std::fs::write(sessions.join("junk.json"), b"not a session record").unwrap();
    // Over the scan cap: the stat probe skips it without reading. `set_len`
    // keeps the file sparse — the probe only stats the size.
    let over_scan = std::fs::File::create(sessions.join("huge.json")).unwrap();
    over_scan.set_len(SCAN_CAP_BYTES + 1).unwrap();
    drop(over_scan);

    let value = run_json(&["pinvou", "artifacts", "list"]);
    assert_eq!(value["artifacts"].as_array().unwrap().len(), 1);
    assert_eq!(
        value["skipped_sessions"],
        serde_json::json!(["huge", "junk"]),
        "both unscannable records must be disclosed, sorted"
    );

    // The --session filter short-circuits on the file name before any read
    // or parse, so the foreign unscannable stems never enter this scan and
    // nothing is left to disclose.
    let value = run_json(&["pinvou", "artifacts", "list", "--session", &id]);
    assert_eq!(value["artifacts"].as_array().unwrap().len(), 1);
    assert_eq!(value["skipped_sessions"], serde_json::json!([]));
}

/// The two sessions-root states the list lane distinguishes: a MISSING root
/// is a genuinely empty store and must succeed with an empty index, while a
/// root shadowed by a regular file is an undecidable store (`read_dir`
/// fails with a non-NotFound error) and must fail loudly instead of
/// rendering the truncation as "no deliverables".
#[test]
fn artifacts_list_empty_store_succeeds_and_unreadable_root_fails_loudly() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("root-states");

    // Missing sessions root: the empty-index shape, not an error.
    let value = run_json(&["pinvou", "artifacts", "list"]);
    assert_eq!(value["artifacts"].as_array().unwrap().len(), 0);
    assert_eq!(value["skipped_sessions"], serde_json::json!([]));

    // `sessions` shadowed by a regular file: read_dir fails with an error
    // that is not NotFound — the empty everything is the one shape that
    // reads as a clean bill, so this must not render as empty.
    std::fs::write(home.sessions_root(), b"not a directory").unwrap();
    let error = expect_failed(&["pinvou", "artifacts", "list"]);
    assert!(
        error.to_string().contains("cannot read the sessions root"),
        "{error}"
    );
}

// ── write --stdin: the process-global lane, via the real binary ────────────

/// The `--stdin` write lane round-trips through the real binary with a piped
/// stdin (in-process dispatch cannot redirect the runner's own stdin), and
/// the same 10 MiB cap applies with the same stable code: cap+1 bytes on
/// stdin are refused and the target keeps its previous content. The child
/// drains the pipe while reading (bounded `Read::take`), so feeding it
/// cap+1 bytes cannot deadlock either direction.
#[test]
fn artifacts_write_stdin_lane_round_trips_and_refuses_over_cap_input() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("stdin");
    let id = create_session_fixture();

    let workspace = home.workspace_of(&id);
    std::fs::create_dir_all(&workspace).unwrap();
    let report = workspace.join("report.md");
    std::fs::write(&report, "old\n").unwrap();
    let report = std::fs::canonicalize(&report).unwrap();

    let arguments = [
        "artifacts",
        "write",
        id.as_str(),
        "report.md",
        "--stdin",
        "--output",
        "json",
    ];

    // Happy path: stdin content lands, the JSON reports the byte count.
    let output = drive_binary(&home, &arguments, b"# replaced\n");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json output on stdout");
    assert_eq!(value["bytes"], "# replaced\n".len());
    assert_eq!(std::fs::read_to_string(&report).unwrap(), "# replaced\n");

    // Over cap: refused with the shared wire code, target unchanged.
    let output = drive_binary(&home, &arguments, &vec![b'x'; SAVE_CAP_BYTES + 1]);
    assert!(
        !output.status.success(),
        "an over-cap stdin write must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("markdown_artifact_is_too_large_to_save"),
        "{stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&report).unwrap(),
        "# replaced\n",
        "a refused stdin write must not touch the target"
    );
}
