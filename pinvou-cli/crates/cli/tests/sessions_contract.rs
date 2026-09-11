//! Contract tests for the `sessions` and `artifacts` families (GUI-parity
//! project). Parse-level coverage runs against the typed command tree;
//! execute-level coverage only touches pure-storage paths: every test runs
//! against a throwaway `PINVOU3_HOME` and boots `SessionStore` directly in
//! the test process — the same standalone constructor the CLI uses. No
//! engine, network, model, or display is involved anywhere in this file, so
//! no `#[ignore]` opt-in tests are needed.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use pinvou_cli::{CliError, CliOutcome, ExitCode, OutputMode, execute, parse_args};
use pinvou3_lib::features::sessions::SessionStore;

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
            "pinvou-cli-sessions-{label}-{}-{nonce}",
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

/// Creates one empty chat session through `SessionStore` (the exact path the
/// CLI's `SessionStore::boot()` uses) and returns its id.
fn create_session_fixture() -> String {
    let store = SessionStore::boot().expect("boot session store");
    let session = store
        .create_new("test-model".to_owned(), None, std::env::temp_dir())
        .expect("create session");
    session.metadata.id
}

/// Seeds a two-message transcript by editing the persisted `SavedSession`
/// JSON (the CLI reads transcripts from the same store file).
fn seed_transcript(id: &str, user_text: &str, assistant_text: &str) {
    let home = PathBuf::from(std::env::var_os("PINVOU3_HOME").unwrap());
    let path = home.join("sessions").join(format!("{id}.json"));
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    value["messages"] = serde_json::json!([
        {"role": "user", "content": [{"type": "text", "text": user_text}]},
        {"role": "assistant", "content": [{"type": "text", "text": assistant_text}]},
    ]);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
}

// ── parse-level coverage ────────────────────────────────────────────────────

#[test]
fn every_sessions_subcommand_parses_and_unknown_flags_exit_two() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let valid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "sessions", "list"],
        vec!["pinvou", "sessions", "list", "--archived"],
        vec!["pinvou", "sessions", "list", "--limit", "5"],
        vec!["pinvou", "sessions", "show", "s-1"],
        vec!["pinvou", "sessions", "show", "s-1", "--last", "2", "--full"],
        vec!["pinvou", "sessions", "rename", "s-1", "new", "title"],
        vec!["pinvou", "sessions", "pin", "s-1"],
        vec!["pinvou", "sessions", "unpin", "s-1"],
        vec!["pinvou", "sessions", "archive", "s-1"],
        vec!["pinvou", "sessions", "restore", "s-1"],
        vec!["pinvou", "sessions", "delete", "s-1", "--yes"],
        vec!["pinvou", "sessions", "delete", "s-1"],
        vec!["pinvou", "sessions", "export", "s-1"],
        vec!["pinvou", "sessions", "export", "s-1", "--format", "json"],
        vec!["pinvou", "sessions", "export", "s-1", "--output", "out.md"],
        vec!["pinvou", "sessions", "timeline", "s-1"],
        vec!["pinvou", "sessions", "subagents", "s-1"],
        vec!["pinvou", "sessions", "folder", "s-1"],
    ];
    for arguments in &valid {
        let parsed = parse_args(arguments.clone())
            .unwrap_or_else(|error| panic!("{arguments:?} must parse: {error}"));
        // The Sessions command tree is dispatched exclusively through the
        // Sessions variant; the type is not nameable from integration tests,
        // so assert family + variant through the derived Debug form.
        let debug = format!("{:?}", parsed.command());
        let variant = match arguments[2] {
            "list" => "List",
            "show" => "Show",
            "rename" => "Rename",
            "pin" => "Pin",
            "unpin" => "Unpin",
            "archive" => "Archive",
            "restore" => "Restore",
            "delete" => "Delete",
            "export" => "Export",
            "timeline" => "Timeline",
            "subagents" => "Subagents",
            "folder" => "Folder",
            other => panic!("unmapped subcommand {other}"),
        };
        assert!(debug.starts_with("Sessions("), "{arguments:?} -> {debug}");
        assert!(
            debug.contains(&format!("{variant} ")) || debug.ends_with(variant),
            "{arguments:?} -> {debug}"
        );
    }

    let invalid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "sessions"],
        vec!["pinvou", "sessions", "bogus"],
        vec!["pinvou", "sessions", "list", "--bogus"],
        vec!["pinvou", "sessions", "list", "--limit", "0"],
        vec!["pinvou", "sessions", "list", "--limit", "-1"],
        vec!["pinvou", "sessions", "list", "--archived", "--archived"],
        vec!["pinvou", "sessions", "show"],
        vec!["pinvou", "sessions", "show", "s-1", "--nope"],
        vec!["pinvou", "sessions", "rename", "s-1"],
        vec!["pinvou", "sessions", "pin"],
        vec!["pinvou", "sessions", "pin", "s-1", "--extra"],
        vec!["pinvou", "sessions", "delete", "s-1", "--nope"],
        vec!["pinvou", "sessions", "export", "s-1", "--format", "html"],
        vec!["pinvou", "sessions", "export", "s-1", "--format"],
        vec!["pinvou", "sessions", "timeline"],
        vec!["pinvou", "sessions", "timeline", "s-1", "--full"],
    ];
    for arguments in &invalid {
        let error = parse_args(arguments.clone()).expect_err(&arguments.join(" "));
        assert_eq!(error.exit_code(), ExitCode::Usage, "{arguments:?}");
    }
}

#[test]
fn every_artifacts_subcommand_parses_and_unknown_flags_exit_two() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let valid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "artifacts", "list"],
        vec!["pinvou", "artifacts", "list", "--session", "s-1"],
        vec!["pinvou", "artifacts", "read", "s-1", "artifacts/a.md"],
        vec![
            "pinvou",
            "artifacts",
            "write",
            "s-1",
            "artifacts/a.md",
            "--file",
            "in.md",
        ],
        vec![
            "pinvou",
            "artifacts",
            "write",
            "s-1",
            "artifacts/a.md",
            "--stdin",
        ],
    ];
    for arguments in &valid {
        let parsed = parse_args(arguments.clone())
            .unwrap_or_else(|error| panic!("{arguments:?} must parse: {error}"));
        let debug = format!("{:?}", parsed.command());
        assert!(debug.starts_with("Artifacts("), "{arguments:?} -> {debug}");
    }

    let invalid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "artifacts"],
        vec!["pinvou", "artifacts", "bogus"],
        vec!["pinvou", "artifacts", "list", "--nope"],
        vec!["pinvou", "artifacts", "list", "--session"],
        vec!["pinvou", "artifacts", "read"],
        vec!["pinvou", "artifacts", "read", "s-1"],
        vec!["pinvou", "artifacts", "read", "s-1", "a.md", "--extra"],
        vec!["pinvou", "artifacts", "write", "s-1"],
        vec!["pinvou", "artifacts", "write", "s-1", "a.md"],
        vec![
            "pinvou",
            "artifacts",
            "write",
            "s-1",
            "a.md",
            "--file",
            "a.md",
            "--stdin",
        ],
        vec!["pinvou", "artifacts", "write", "s-1", "a.md", "--file"],
        vec!["pinvou", "artifacts", "write", "s-1", "a.md", "--bogus"],
    ];
    for arguments in &invalid {
        let error = parse_args(arguments.clone()).expect_err(&arguments.join(" "));
        assert_eq!(error.exit_code(), ExitCode::Usage, "{arguments:?}");
    }
}

#[test]
fn sessions_delete_without_yes_is_a_usage_error() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("delete-needs-yes");
    let id = create_session_fixture();
    let parsed = parse_args(["pinvou", "sessions", "delete", &id]).unwrap();
    let error = execute(parsed).expect_err("delete without --yes must refuse");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(error.to_string().contains("--yes"));
    // The session must still exist.
    assert!(home.sessions_root().join(format!("{id}.json")).is_file());
}

// ── execute-level coverage (pure storage, temp PINVOU3_HOME) ───────────────

#[test]
fn sessions_list_on_empty_store_returns_empty_output() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("list-empty");
    let outcome = run(&["pinvou", "sessions", "list"]).expect("list must succeed");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert_eq!(outcome.stdout, "");

    let parsed = parse_args(["pinvou", "--output", "json", "sessions", "list"]).unwrap();
    let outcome = execute(parsed).expect("json list must succeed");
    assert_eq!(outcome.stdout, r#"{"sessions":[]}"#);
}

#[test]
fn sessions_metadata_round_trip_updates_list_and_state() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("round-trip");
    let id = create_session_fixture();
    seed_transcript(&id, "hello", "hi there");

    // rename
    let value = run_json(&["pinvou", "sessions", "rename", &id, "new", "title"]);
    assert_eq!(value["action"], "renamed");
    assert_eq!(value["title"], "new title");
    let value = run_json(&["pinvou", "sessions", "show", &id]);
    assert_eq!(value["title"], "new title");
    assert_eq!(value["kind"], "chat");
    assert_eq!(value["message_count"], 2);

    // pin / unpin
    let value = run_json(&["pinvou", "sessions", "pin", &id]);
    assert_eq!(value["action"], "pinned");
    let listed = run_json(&["pinvou", "sessions", "list"]);
    assert_eq!(listed["sessions"][0]["pinned"], true);
    run_json(&["pinvou", "sessions", "unpin", &id]);
    let listed = run_json(&["pinvou", "sessions", "list"]);
    assert_eq!(listed["sessions"][0]["pinned"], false);

    // archive hides from the default list and shows under --archived
    run_json(&["pinvou", "sessions", "archive", &id]);
    let listed = run_json(&["pinvou", "sessions", "list"]);
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 0);
    let archived = run_json(&["pinvou", "sessions", "list", "--archived"]);
    assert_eq!(archived["sessions"][0]["id"], id);
    assert_eq!(archived["sessions"][0]["archived"], true);

    // restore brings it back
    run_json(&["pinvou", "sessions", "restore", &id]);
    let listed = run_json(&["pinvou", "sessions", "list"]);
    assert_eq!(listed["sessions"][0]["id"], id);
    assert_eq!(listed["sessions"][0]["archived"], false);

    // human list renders one stable column-ish line
    let outcome = run(&["pinvou", "sessions", "list"]).expect("human list");
    let line = outcome.stdout.lines().next().unwrap_or_default();
    let columns: Vec<&str> = line.split('\t').collect();
    assert_eq!(columns[0], id);
    assert_eq!(columns[2], "-");
    assert_eq!(columns[3], "chat");

    // delete --yes removes the session from the store
    let value = run_json(&["pinvou", "sessions", "delete", &id, "--yes"]);
    assert_eq!(value["action"], "deleted");
    assert!(!home.sessions_root().join(format!("{id}.json")).exists());
    let listed = run_json(&["pinvou", "sessions", "list"]);
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 0);
}

#[test]
fn sessions_show_limits_and_exports_transcript() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("show-export");
    let id = create_session_fixture();
    seed_transcript(&id, "hello", "hi there");

    // --last 1 keeps only the assistant message
    let value = run_json(&["pinvou", "sessions", "show", &id, "--last", "1"]);
    assert_eq!(value["message_count"], 1);
    assert_eq!(value["messages"][0]["role"], "assistant");
    assert_eq!(value["messages"][0]["text"], "hi there");

    // markdown export is a human-readable transcript with role headers
    let outcome = run(&["pinvou", "sessions", "export", &id]).expect("markdown export");
    assert!(outcome.stdout.contains("## user"));
    assert!(outcome.stdout.contains("## assistant"));
    assert!(outcome.stdout.contains("hello"));
    assert!(outcome.stdout.contains("hi there"));

    // json export carries the SavedSession content
    let value = run_json(&["pinvou", "sessions", "export", &id, "--format", "json"]);
    let saved: serde_json::Value =
        serde_json::from_str(value["content"].as_str().unwrap()).unwrap();
    assert_eq!(saved["metadata"]["id"], id);
    assert_eq!(saved["messages"][0]["role"], "user");

    // --output writes the export to a file instead of stdout
    let destination = std::env::temp_dir().join(format!(
        "pinvou-cli-export-{}-{}.md",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let value = run_json(&[
        "pinvou",
        "sessions",
        "export",
        &id,
        "--output",
        destination.to_str().unwrap(),
    ]);
    assert_eq!(value["format"], "markdown");
    assert_eq!(value["output"], destination.display().to_string());
    let written = std::fs::read_to_string(&destination).unwrap();
    assert!(written.contains("## user"));
    assert!(written.contains("hello"));
    std::fs::remove_file(&destination).unwrap();

    // exporting an unknown session fails at host level (exit 1)
    let error = run(&["pinvou", "sessions", "export", "missing-session"]).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Failed);
}

#[test]
fn sessions_timeline_reads_timing_events_and_tolerates_missing_file() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("timeline");
    let id = create_session_fixture();

    // No timing sidecar yet: empty output, no error.
    let outcome = run(&["pinvou", "sessions", "timeline", &id]).expect("empty timeline");
    assert_eq!(outcome.stdout, "");

    let session_dir = home.sessions_root().join(&id);
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("timing_events.jsonl"),
        format!(
            "{{\"event\":\"user_start\",\"session_id\":\"{id}\",\"turn_id\":\"t2\",\"timestamp\":2000,\"ts\":\"t2-start\"}}\n\
             {{\"event\":\"assistant_done\",\"session_id\":\"{id}\",\"turn_id\":\"t2\",\"timestamp\":3000,\"ts\":\"t2-done\",\"status\":\"ok\"}}\n\
             {{\"event\":\"user_start\",\"session_id\":\"{id}\",\"turn_id\":\"t1\",\"timestamp\":1000,\"ts\":\"t1-start\"}}\n\
             not-json-at-all\n"
        ),
    )
    .unwrap();

    // Events come back ascending by timestamp; the corrupt line is skipped.
    let value = run_json(&["pinvou", "sessions", "timeline", &id]);
    let events = value["events"].as_array().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["turn_id"], "t1");
    assert_eq!(events[2]["event"], "assistant_done");
    assert_eq!(events[2]["status"], "ok");

    let outcome = run(&["pinvou", "sessions", "timeline", &id]).expect("human timeline");
    let lines: Vec<&str> = outcome.stdout.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("t1-start"));
}

#[test]
fn sessions_subagents_lists_read_only_and_folder_prints_session_path() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("subagents-folder");
    let id = create_session_fixture();

    // A fresh session has dispatched no subagents: empty read-only listing.
    let outcome = run(&["pinvou", "sessions", "subagents", &id]).expect("empty subagents");
    assert_eq!(outcome.stdout, "");
    let value = run_json(&["pinvou", "sessions", "subagents", &id]);
    assert_eq!(value["subagents"].as_array().unwrap().len(), 0);

    let value = run_json(&["pinvou", "sessions", "folder", &id]);
    let expected = home.sessions_root().join(&id);
    assert_eq!(value["path"], expected.display().to_string());
    let outcome = run(&["pinvou", "sessions", "folder", &id]).expect("human folder");
    assert_eq!(outcome.stdout, expected.display().to_string());
}

#[test]
fn artifacts_list_read_write_round_trip_with_fixture_session() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("artifacts-round-trip");
    let id = create_session_fixture();

    // Track the artifact on the session (same feature-layer call the GUI
    // `save_session_artifacts` command makes), then index it cross-session.
    // Relative artifact paths resolve against the session ledger workspace
    // (`sessions_root/<id>/workspace`), the same base the GUI uses.
    let workspace_dir = home.sessions_root().join(&id).join("workspace");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    let report = workspace_dir.join("report.md");
    std::fs::write(&report, "# Report\n\nfirst version\n").unwrap();
    // The CLI canonicalizes stored artifact paths (session containment
    // check); on macOS $TMPDIR (/var/folders/...) canonicalizes to
    // /private/var/..., so expectations must use the canonical form.
    let report = std::fs::canonicalize(&report).unwrap();

    let store = SessionStore::boot().expect("boot session store");
    store
        .update_artifacts(&id, vec![report.to_string_lossy().to_string()])
        .expect("track artifact");
    drop(store);

    let value = run_json(&["pinvou", "artifacts", "list"]);
    let rows = value["artifacts"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["session_id"], id);
    assert_eq!(rows[0]["name"], "report.md");
    assert_eq!(rows[0]["ext"], "md");
    assert_eq!(rows[0]["category"], "doc");
    assert_eq!(rows[0]["path"], report.display().to_string());

    // Filtered index by --session.
    let value = run_json(&["pinvou", "artifacts", "list", "--session", &id]);
    assert_eq!(value["artifacts"].as_array().unwrap().len(), 1);
    let value = run_json(&["pinvou", "artifacts", "list", "--session", "other"]);
    assert_eq!(value["artifacts"].as_array().unwrap().len(), 0);

    // read returns the validated text content
    let value = run_json(&["pinvou", "artifacts", "read", &id, "report.md"]);
    assert_eq!(value["content"], "# Report\n\nfirst version\n");
    assert_eq!(value["path"], report.display().to_string());
    let outcome = run(&["pinvou", "artifacts", "read", &id, "report.md"]).expect("human read");
    assert_eq!(outcome.stdout, "# Report\n\nfirst version\n");

    // write --file overwrites the markdown artifact in place
    let source = std::env::temp_dir().join(format!(
        "pinvou-cli-artifact-source-{}-{}.md",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
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
    assert_eq!(value["bytes"], "# Report\n\nsecond version\n".len());
    std::fs::remove_file(&source).unwrap();
    let outcome =
        run(&["pinvou", "artifacts", "read", &id, "report.md"]).expect("read after write");
    assert_eq!(outcome.stdout, "# Report\n\nsecond version\n");
}

#[test]
fn artifacts_write_rejects_non_markdown_suffix() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("artifacts-md-only");
    let id = create_session_fixture();

    let workspace_dir = home.sessions_root().join(&id).join("workspace");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    std::fs::write(workspace_dir.join("notes.txt"), "plain").unwrap();

    let error = run(&["pinvou", "artifacts", "write", &id, "notes.txt", "--stdin"])
        .expect_err("non-markdown artifact must be rejected");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error
            .to_string()
            .contains("only_markdown_artifacts_can_be_edited")
    );
    // The target file is untouched.
    assert_eq!(
        std::fs::read_to_string(workspace_dir.join("notes.txt")).unwrap(),
        "plain"
    );
}

#[test]
fn artifacts_read_rejects_escape_outside_the_session() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("artifacts-containment");
    let id = create_session_fixture();

    // A session id is required to exist and the resolved path must stay
    // inside sessions_root/<id>/{artifacts,workspace}.
    let error = run(&["pinvou", "artifacts", "read", &id, "../../etc/passwd"])
        .expect_err("path escape must be rejected");
    assert_eq!(error.exit_code(), ExitCode::Failed);

    let error = run(&["pinvou", "artifacts", "read", &id, "artifacts/missing.md"])
        .expect_err("missing artifact must be rejected");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().starts_with("artifact_not_found"));

    // Session ids are restricted to [A-Za-z0-9_-] before any path join.
    let error = run(&["pinvou", "artifacts", "read", "../escape", "a.md"])
        .expect_err("invalid session id must be a usage error");
    assert_eq!(error.exit_code(), ExitCode::Usage);
}

#[test]
fn json_output_mode_flows_through_every_session_subcommand() {
    // Parse-level assertion only: no PINVOU3_HOME mutation needed here.
    for arguments in [
        vec!["pinvou", "sessions", "list"],
        vec!["pinvou", "sessions", "show", "s-1"],
        vec!["pinvou", "sessions", "timeline", "s-1"],
        vec!["pinvou", "sessions", "subagents", "s-1"],
        vec!["pinvou", "sessions", "folder", "s-1"],
        vec!["pinvou", "artifacts", "list"],
    ] {
        let mut owned = arguments.clone();
        owned.push("--output");
        owned.push("json");
        let parsed = parse_args(owned).unwrap();
        assert_eq!(parsed.output(), OutputMode::Json, "{arguments:?}");
    }
}

#[test]
fn relative_pinvou3_home_is_rejected_before_any_store_access() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let previous = std::env::var_os("PINVOU3_HOME");
    // A relative PINVOU3_HOME violates the CLI sandbox contract; the shared
    // resolver (support::sandbox_home) must refuse it instead of resolving
    // store paths against the current working directory. The store may
    // create directories under the (relative) root before the refusal, so
    // clean up afterwards.
    let relative = format!("pinvou-cli-rel-home-{}-{nonce}", std::process::id());
    // SAFETY: ENV_LOCK is held; env writes are serialized in-process.
    unsafe { std::env::set_var("PINVOU3_HOME", relative.as_str()) };
    let error = run(&["pinvou", "sessions", "folder", "abc"]).expect_err("relative home refused");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("absolute"));
    match previous {
        // SAFETY: ENV_LOCK is held.
        Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
        // SAFETY: ENV_LOCK is held.
        None => unsafe { std::env::remove_var("PINVOU3_HOME") },
    }
    let _ = std::fs::remove_dir_all(relative);
}
