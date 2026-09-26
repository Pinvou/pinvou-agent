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

/// Restores the previous `PINVOU3_HOME` on drop — the same contract as
/// [`HomeGuard`]'s Drop, for tests that set the variable to a non-temp value
/// themselves. The restore must survive a panicking assertion, otherwise the
/// polluted value leaks into every later test in the process.
struct RestoreHome(Option<OsString>);

impl Drop for RestoreHome {
    fn drop(&mut self) {
        match self.0.take() {
            // SAFETY: ENV_LOCK is held by the owning test.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: ENV_LOCK is held by the owning test.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
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

/// Rewrites one seeded message's `role` in place. `role` is a plain `String`
/// on `SavedSession`, so a transcript can carry anything there — including the
/// terminal escapes the body sanitizer exists to stop.
fn set_message_role(id: &str, index: usize, role: &str) {
    let home = PathBuf::from(std::env::var_os("PINVOU3_HOME").unwrap());
    let path = home.join("sessions").join(format!("{id}.json"));
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    value["messages"][index]["role"] = serde_json::json!(role);
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
        vec![
            "pinvou",
            "artifacts",
            "write",
            "s-1",
            "a.md",
            "extra.md",
            "--file",
            "in.md",
        ],
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

/// Round-18 review finding, red-first: `sessions rename <id> see --output
/// json now` used to strip the `--output json` pair out of the MIDDLE of the
/// title (the global scan removed it from anywhere in argv) and store
/// "see now" with exit 0. After the parse_args fix the pair is ordinary
/// family input inside the title, and rename must refuse it as a
/// flag-shaped title: exit 2 and the store untouched. The trailing pair
/// with no garbage after it stays a legal output-mode position.
#[test]
fn rename_trailing_garbage_after_output_json_is_refused_and_leaves_the_store_unchanged() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("rename-output-garbage");
    let id = create_session_fixture();
    // A known title first, so "the store is unchanged" is observable.
    run_json(&["pinvou", "sessions", "rename", &id, "original", "title"]);

    // The pair followed by more title words must be refused at PARSE time
    // (a usage error, exit 2), before any store access.
    let error = parse_args([
        "pinvou", "sessions", "rename", &id, "see", "--output", "json", "now",
    ])
    .expect_err("a global flag pair inside the title must be refused");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(
        error
            .to_string()
            .contains("cannot accept a title containing"),
        "the refusal must name the title-pair rule: {error}"
    );

    // Store unchanged: the persisted title survives the refused rename.
    let value = run_json(&["pinvou", "sessions", "show", &id]);
    assert_eq!(value["title"], "original title");
    assert!(home.sessions_root().join(format!("{id}.json")).is_file());

    // The same pair at the END of the line (legal position) keeps working:
    // mode applied, title made of the words before it.
    let value = run_json(&["pinvou", "sessions", "rename", &id, "plain", "title"]);
    assert_eq!(value["action"], "renamed");
    assert_eq!(value["title"], "plain title");
    let value = run_json(&["pinvou", "sessions", "show", &id]);
    assert_eq!(value["title"], "plain title");
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

/// Stored titles are verbatim and legitimately contain control characters
/// (the GUI's attachment marker embeds "\n\n"): the human TSV row must
/// collapse them so the row stays one line per session, while JSON keeps
/// the real title.
#[test]
fn sessions_list_collapses_control_characters_in_human_titles() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("list-control-chars");
    let id = create_session_fixture();
    let raw_title = "line1\nline2\tcol\x07end";
    run_json(&["pinvou", "sessions", "rename", &id, raw_title]);

    let human = run(&["pinvou", "sessions", "list"]).expect("human list must succeed");
    assert_eq!(human.exit_code, ExitCode::Success);
    // The row is one line: the title's embedded newline must not split it.
    assert_eq!(human.stdout.lines().count(), 1);
    assert!(
        !human.stdout.contains('\x07'),
        "the bell control character must be collapsed: {:?}",
        human.stdout
    );
    assert!(
        human.stdout.contains("line1 line2 col end"),
        "control characters collapse to spaces: {:?}",
        human.stdout
    );
    assert!(
        home.sessions_root().join(format!("{id}.json")).is_file(),
        "the collapse is a rendering choice; the stored title is untouched"
    );

    let listed = run_json(&["pinvou", "--output", "json", "sessions", "list"]);
    assert_eq!(listed["sessions"][0]["title"], serde_json::json!(raw_title));
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

    // --last 1 windows the rendered messages; message_count stays the
    // session total (GUI semantics) with the window size reported separately
    let value = run_json(&["pinvou", "sessions", "show", &id, "--last", "1"]);
    assert_eq!(value["message_count"], 2);
    assert_eq!(value["shown_message_count"], 1);
    assert_eq!(value["messages"][0]["role"], "assistant");
    assert_eq!(value["messages"][0]["text"], "hi there");

    // The human header mirrors the JSON fields: session total plus the
    // windowed count, not the windowed count as the total.
    let outcome =
        run(&["pinvou", "sessions", "show", &id, "--last", "1"]).expect("human show with --last");
    assert!(
        outcome.stdout.contains("messages: 2 (showing 1)"),
        "human show with --last reported the wrong message counts"
    );
    let outcome = run(&["pinvou", "sessions", "show", &id]).expect("human show");
    assert!(
        outcome.stdout.contains("messages: 2 (showing 2)"),
        "human show reported the wrong message counts"
    );

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

    // An existing destination is refused, not overwritten: the transcript
    // store itself is plain files, so a silent overwrite could destroy a
    // stored session with exit 0.
    let error = run(&[
        "pinvou",
        "sessions",
        "export",
        &id,
        "--output",
        destination.to_str().unwrap(),
    ])
    .unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("refusing to overwrite"),
        "{error}"
    );
    // The first export's content survived the refusal untouched.
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), written);
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

/// `SessionStore::set_pinned` / `set_hidden` return `()`: the sidecar layer
/// catches a failed persist, rolls its in-memory cache back to the durable
/// state and only reports on stderr. Without a post-write verification the
/// CLI printed "pinned <id>" and exited 0 with nothing written, so a script
/// on a read-only `~/.pinvou3` recorded a pin (or an archive) that no later
/// run can see. Make the write genuinely impossible and require a non-zero
/// exit plus a message that names the sidecar.
#[cfg(unix)]
#[test]
fn sessions_pin_and_archive_fail_when_the_sidecar_cannot_be_persisted() {
    use std::os::unix::fs::PermissionsExt as _;

    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("sidecar-readonly");
    let id = create_session_fixture();

    // The pinned/hidden registries live next to the transcripts, so a
    // read-only sessions directory is exactly the production failure: the
    // transcript still LOADS (reads are unaffected), only the sidecar write
    // is refused.
    let sessions_root = home.sessions_root();
    let original = std::fs::metadata(&sessions_root).unwrap().permissions();
    std::fs::set_permissions(&sessions_root, std::fs::Permissions::from_mode(0o555)).unwrap();

    let pin = run(&["pinvou", "sessions", "pin", &id]);
    let archive = run(&["pinvou", "sessions", "archive", &id]);

    // Restore before asserting: a failing assertion must not leave an
    // unremovable directory behind for HomeGuard's Drop.
    std::fs::set_permissions(&sessions_root, original).unwrap();

    let pin = pin.expect_err("pin must not report success when nothing was written");
    assert_eq!(pin.exit_code(), ExitCode::Failed);
    assert!(
        pin.to_string().contains("pinned-sessions"),
        "the failure must name the sidecar that did not persist: {pin}"
    );
    let archive = archive.expect_err("archive must not report success when nothing was written");
    assert_eq!(archive.exit_code(), ExitCode::Failed);
    assert!(
        archive.to_string().contains("hidden-sessions"),
        "the failure must name the sidecar that did not persist: {archive}"
    );

    // Nothing was written, and the honest report matches: the session is
    // still unpinned and still visible in the default listing.
    let listed = run_json(&["pinvou", "sessions", "list"]);
    assert_eq!(listed["sessions"][0]["id"], id);
    assert_eq!(listed["sessions"][0]["pinned"], false);
    assert_eq!(listed["sessions"][0]["archived"], false);
}

/// A transcript is model-authored text printed straight to a terminal. In
/// human mode the ESC-driven sequences must not survive (they move the
/// cursor, repaint, rewrite the window title) and neither must CR, which
/// redraws the current line over earlier output. The transcript's own
/// newlines and tabs must survive, because they ARE the output the caller
/// asked for. JSON mode keeps the verbatim bytes.
///
/// The message ROLE is the same surface: it is a plain `String` on the
/// deserialized session, not an enum, so a transcript can put an OSC sequence
/// in the `[n] <role>` header of `sessions show` just as easily as in the body
/// below it. Unlike the body it is a single-line header cell, so it also loses
/// its newlines and tabs — a role that spans lines would push the body out of
/// its own header.
#[test]
fn sessions_show_and_export_strip_terminal_escapes_but_keep_transcript_layout() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("transcript-escapes");
    let id = create_session_fixture();
    let hostile = "line1\n\tindented\x1b[2J\x1b]0;pwned\x07 tail\roverwrite";
    seed_transcript(&id, "hello", hostile);
    let hostile_role = "assistant\x1b]0;pwned\x07\tspoof\nsecond";
    set_message_role(&id, 1, hostile_role);

    for arguments in [
        vec!["pinvou", "sessions", "show", &id],
        vec!["pinvou", "sessions", "export", &id],
    ] {
        let outcome = run(&arguments).unwrap_or_else(|error| panic!("{arguments:?}: {error}"));
        assert!(
            !outcome.stdout.contains('\x1b'),
            "{arguments:?} leaked ESC to the terminal: {:?}",
            outcome.stdout
        );
        assert!(
            !outcome.stdout.contains('\r') && !outcome.stdout.contains('\x07'),
            "{arguments:?} leaked CR/BEL to the terminal: {:?}",
            outcome.stdout
        );
        assert!(
            outcome.stdout.contains("line1\n\tindented"),
            "{arguments:?} destroyed the transcript layout: {:?}",
            outcome.stdout
        );
    }

    // The role header is collapsed as a COLUMN, not as a block: the escape is
    // gone and so are the tab and the newline, so the header stays one line.
    // Each control character becomes its own space (the collapse substitutes
    // one-for-one, it does not squeeze runs), so BEL and TAB leave two.
    let outcome = run(&["pinvou", "sessions", "show", &id]).expect("show must succeed");
    assert!(
        outcome
            .stdout
            .contains("[2] assistant ]0;pwned  spoof second\n"),
        "the role header must be collapsed onto one line: {:?}",
        outcome.stdout
    );

    // JSON mode was never the problem (serde_json escapes everything below
    // 0x20) and must keep reporting the stored bytes.
    let value = run_json(&["pinvou", "--output", "json", "sessions", "show", &id]);
    assert_eq!(value["messages"][1]["text"], serde_json::json!(hostile));
    assert_eq!(
        value["messages"][1]["role"],
        serde_json::json!(hostile_role)
    );
}

/// `sessions export --output` writes the whole conversation — system prompt,
/// every turn, tool calls and their results. A default-umask file (~0644)
/// would hand that to every local user on the machine, so the destination is
/// created 0600 like `code providers export` does.
#[cfg(unix)]
#[test]
fn sessions_export_output_file_is_owner_readable_only() {
    use std::os::unix::fs::PermissionsExt as _;

    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("export-mode");
    let id = create_session_fixture();
    seed_transcript(&id, "hello", "hi there");

    let destination = std::env::temp_dir().join(format!(
        "pinvou-cli-export-mode-{}-{}.md",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    run_json(&[
        "pinvou",
        "sessions",
        "export",
        &id,
        "--output",
        destination.to_str().unwrap(),
    ]);
    let mode = std::fs::metadata(&destination)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    let _ = std::fs::remove_file(&destination);
    assert_eq!(
        mode, 0o600,
        "an exported transcript must not be group/world readable (got {mode:o})"
    );
}

/// `sessions subagents` renders a four-column tab-separated row whose last
/// two cells are untrusted: `objective` is verbatim from the model's own
/// `agent` tool call and `error` is whatever the failing worker reported. A
/// tab would invent a fifth column and a newline would turn one subagent
/// into two rows, silently breaking every consumer that cuts on `\t`.
///
/// The state cell (column 1) is pinned here too. The foundation defines
/// `failed = done && status != Completed`, so one arm covers every terminal
/// non-success and prints the foundation's own status name — this fixture is
/// a `failed` worker WITH a transcript, which is the only shape that reaches
/// that arm (no transcript short-circuits to "queued" first).
#[test]
fn sessions_subagents_human_row_survives_control_characters_in_model_text() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("subagents-columns");
    let id = create_session_fixture();

    // The worker ledger the multiagent transcript listing reads, seeded at
    // the same path the engine writes it to under the session ledger root
    // (`sessions_root/<id>/workspace` for a plain chat session).
    let ledger = home.sessions_root().join(&id).join("workspace");
    let state_dir = ledger.join(".codewhale").join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        state_dir.join("subagents.v1.json"),
        serde_json::json!({
            "schema_version": 1,
            "agents": [],
            "workers": [{
                "spec": {
                    "worker_id": "w-1",
                    "objective": "audit\tthe\nrepo\x1b[31m",
                    "agent_type": "general",
                    "model": "test-model",
                    "workspace": ledger.display().to_string(),
                    "context_mode": "fresh",
                    "fork_context": false,
                    "tool_profile": "inherited",
                    "max_steps": 8,
                    "spawn_depth": 0,
                    "max_spawn_depth": 2,
                },
                "status": "failed",
                "created_at_ms": 1_700_000_000_000_u64,
                "updated_at_ms": 1_700_000_001_000_u64,
                "error": "boom\tdetail\nsecond line",
            }],
        })
        .to_string(),
    )
    .unwrap();
    // The transcript half of the listing: `list` keys the side table off the
    // header line, not off the file name, so a readable header is enough to
    // make `has_transcript` true — which is what lets the row reach the
    // terminal-status arm instead of short-circuiting to "queued".
    let transcripts_dir = state_dir.join("subagent-transcripts");
    std::fs::create_dir_all(&transcripts_dir).unwrap();
    std::fs::write(
        transcripts_dir.join("w-1.jsonl"),
        format!(
            "{}\n",
            serde_json::json!({ "kind": "subagent_transcript_header", "agent_id": "w-1" })
        ),
    )
    .unwrap();

    let outcome = run(&["pinvou", "sessions", "subagents", &id]).expect("subagents must succeed");
    assert_eq!(
        outcome.stdout.lines().count(),
        1,
        "one worker must render as exactly one row: {:?}",
        outcome.stdout
    );
    let columns: Vec<&str> = outcome.stdout.split('\t').collect();
    assert_eq!(
        columns.len(),
        4,
        "the subagents row layout changed: {:?}",
        outcome.stdout
    );
    assert_eq!(columns[0], "w-1");
    assert_eq!(
        columns[1], "failed",
        "a terminal non-success prints the foundation's own status name"
    );
    assert_eq!(columns[2], "audit the repo [31m");
    assert_eq!(columns[3], "boom detail second line");
    assert!(
        !outcome.stdout.contains('\x1b'),
        "ESC must not reach the terminal: {:?}",
        outcome.stdout
    );

    // JSON keeps the verbatim strings: the collapse is a rendering choice.
    let value = run_json(&["pinvou", "--output", "json", "sessions", "subagents", &id]);
    assert_eq!(
        value["subagents"][0]["objective"],
        serde_json::json!("audit\tthe\nrepo\x1b[31m")
    );
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
    // Always present, so a JSON consumer can tell an empty index apart from
    // one truncated by the per-record scan cap.
    assert_eq!(value["skipped_sessions"], serde_json::json!([]));

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

/// `artifacts list` renders a six-column tab-separated row whose untrusted
/// cells come from the record's `storage_path` (the index derives `name`,
/// `ext`, and the whole `path` cell from it) and from the record's
/// `metadata/id` JSON field (`session_id`) — none of which pass the argv
/// validation a `--session` argument gets. A POSIX filename may legally
/// contain `\t`, `\n`, or ESC, and a hand-edited or restored-from-backup
/// record is enough to put either into the row: a tab would invent a seventh
/// column, a newline would turn one deliverable into two rows, and ESC must
/// not reach the terminal — the same contract the `sessions list` tests pin
/// for their titles. The collapse is a rendering choice: JSON keeps the
/// verbatim bytes and the stored bytes are untouched.
#[test]
fn artifacts_list_human_row_survives_control_characters_in_record_fields() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("artifacts-columns");
    let id = create_session_fixture();

    // The artifact file exists on disk under a hostile (but POSIX-legal)
    // filename, and is tracked through the store (the same feature-layer
    // call the GUI `save_session_artifacts` command makes). Every cell of
    // the row is derived from this name: name, ext, path, and via metadata
    // even session_id stays the well-formed id here.
    let workspace_dir = home.sessions_root().join(&id).join("workspace");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    let hostile = workspace_dir.join("poison\trow\njump\x1b]0;pwned\x07.md");
    std::fs::write(&hostile, "# Report\n\nfirst version\n").unwrap();
    // The CLI canonicalizes tracked paths; expectations must use the
    // canonical form (on macOS $TMPDIR canonicalizes to /private/var/...).
    let hostile = std::fs::canonicalize(&hostile).unwrap();

    let store = SessionStore::boot().expect("boot session store");
    store
        .update_artifacts(&id, vec![hostile.to_string_lossy().to_string()])
        .expect("track artifact");
    drop(store);

    let outcome = run(&["pinvou", "artifacts", "list"]).expect("artifacts list must succeed");
    // One artifact renders as exactly one line, whatever its filename says.
    assert_eq!(
        outcome.stdout.lines().count(),
        1,
        "one artifact must render as exactly one row: {:?}",
        outcome.stdout
    );
    let columns: Vec<&str> = outcome.stdout.split('\t').collect();
    assert_eq!(
        columns.len(),
        6,
        "the artifacts row layout changed: {:?}",
        outcome.stdout
    );
    // name, ext, category, size, session_id, path — in column order.
    assert_eq!(columns[0], "poison row jump ]0;pwned .md");
    assert_eq!(columns[1], "md");
    assert_eq!(columns[2], "doc");
    assert_eq!(columns[3], "# Report\n\nfirst version\n".len().to_string());
    assert_eq!(columns[4], id);
    // The path cell: the canonical path minus its four control characters —
    // the same substitution the renderer applies (dir part has none).
    let expected_path = ["\t", "\n", "\x1b", "\x07"]
        .into_iter()
        .fold(hostile.display().to_string(), |acc: String, byte| {
            acc.replace(byte, " ")
        });
    assert_eq!(columns[5], expected_path);

    // None of the row-breaking bytes may survive in the rendered cells. The
    // file name on disk still carries them (the collapse is not a data
    // change); only the rendered line is clean. A tab or newline inside a cell
    // would show up as a seventh column or a second line, both already pinned
    // above — what the count assertions cannot express is ESC, checked here.
    let raw_name = hostile
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap()
        .to_owned();
    assert_eq!(raw_name, "poison\trow\njump\x1b]0;pwned\x07.md");
    assert!(
        !outcome.stdout.contains('\x1b'),
        "ESC must not reach the terminal: {:?}",
        outcome.stdout
    );

    // JSON keeps the verbatim record bytes: the collapse is a per-render
    // choice, not a data change.
    let value = run_json(&["pinvou", "artifacts", "list"]);
    assert_eq!(
        value["artifacts"][0]["name"],
        serde_json::json!("poison\trow\njump\x1b]0;pwned\x07.md")
    );
    assert_eq!(
        value["artifacts"][0]["path"],
        serde_json::json!(hostile.display().to_string())
    );
}

/// The `session_id` cell is read out of the record's `metadata/id` JSON
/// field rather than the filename, so a hand-edited record can poison it
/// independently of every other cell. This pins that the collapse covers
/// it too, and that JSON keeps the verbatim value.
#[test]
fn artifacts_list_collapses_a_hostile_metadata_id() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("artifacts-columns-meta-id");
    let id = create_session_fixture();

    // Hostile record: the artifact filename carries tab+newline+ESC (covered
    // by the sibling test above); this one poisons only metadata/id.
    let workspace_dir = home.sessions_root().join(&id).join("workspace");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    let report = workspace_dir.join("report.md");
    std::fs::write(&report, "# Report\n").unwrap();
    let report = std::fs::canonicalize(&report).unwrap();
    let store = SessionStore::boot().expect("boot session store");
    store
        .update_artifacts(&id, vec![report.to_string_lossy().to_string()])
        .expect("track artifact");
    drop(store);

    let record_path = home.sessions_root().join(format!("{id}.json"));
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&record_path).unwrap()).unwrap();
    value["metadata"]["id"] = serde_json::json!("poison\tid\nnewrow\x1b[2J");
    std::fs::write(&record_path, serde_json::to_vec(&value).unwrap()).unwrap();

    // JSON mode reports /metadata/id verbatim.
    let json = run_json(&["pinvou", "artifacts", "list"]);
    assert_eq!(
        json["artifacts"][0]["session_id"],
        serde_json::json!("poison\tid\nnewrow\x1b[2J")
    );
    // Human mode collapses it onto the one line.
    let outcome = run(&["pinvou", "artifacts", "list"]).expect("artifacts list must succeed");
    assert_eq!(
        outcome.stdout.lines().count(),
        1,
        "one row: {:?}",
        outcome.stdout
    );
    let columns: Vec<&str> = outcome.stdout.split('\t').collect();
    assert_eq!(columns.len(), 6, "row layout: {:?}", outcome.stdout);
    assert_eq!(columns[4], "poison id newrow [2J");
    // The one-row/6-column assertions above already pin that no cell can
    // contain a tab or a newline; what they cannot express is ESC.
    assert!(
        !outcome.stdout.contains('\x1b'),
        "ESC must not reach the terminal: {:?}",
        outcome.stdout
    );
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

/// The session-storage containment is stricter than the GUI's read command,
/// but it says nothing about *what* the file is. An agent can create `.env` or
/// `.ssh/id_rsa` inside its own workspace, and the GUI path policy
/// (`platform::path_policy::check_sensitive_components`) refuses exactly those
/// names — the CLI must too, in both directions, or it becomes the way to lift
/// a credential the other surface will not touch.
#[test]
fn artifacts_refuses_credential_components_inside_the_session_workspace() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("artifacts-sensitive");
    let id = create_session_fixture();

    let workspace_dir = home.sessions_root().join(&id).join("workspace");
    std::fs::create_dir_all(workspace_dir.join(".ssh")).unwrap();
    std::fs::write(workspace_dir.join(".ssh").join("id_rsa"), "PRIVATE KEY").unwrap();
    std::fs::write(workspace_dir.join(".env"), "TOKEN=secret\n").unwrap();
    // An ordinary deliverable whose name merely contains a blacklisted string
    // must keep working: the policy is about path components, not substrings.
    std::fs::write(workspace_dir.join("environment.md"), "# notes\n").unwrap();

    for relative in [".ssh/id_rsa", ".env"] {
        let error = run(&["pinvou", "artifacts", "read", &id, relative])
            .expect_err("a credential component must be refused");
        assert_eq!(error.exit_code(), ExitCode::Failed, "{relative}: {error}");
        assert!(
            error
                .to_string()
                .starts_with("artifact_crosses_sensitive_component"),
            "{relative}: {error}"
        );
    }
    // Writes take the same refusal, and before the markdown-suffix check: the
    // file is untouched.
    let error = run(&["pinvou", "artifacts", "write", &id, ".env", "--stdin"])
        .expect_err("writing a credential component must be refused");
    assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
    assert_eq!(
        std::fs::read_to_string(workspace_dir.join(".env")).unwrap(),
        "TOKEN=secret\n"
    );

    let outcome =
        run(&["pinvou", "artifacts", "read", &id, "environment.md"]).expect("benign name reads");
    assert_eq!(outcome.stdout, "# notes\n");
}

/// Scheduled-run sessions ledger OUTSIDE `sessions_root()` — their
/// workspace lives under `~/.pinvou3/scheduled/<task>/workspace`. Reads
/// must resolve through the session profile's ledger root (the GUI read
/// path reads these fine); writes stay sessions-root confined.
#[test]
fn artifacts_read_reaches_scheduled_run_workspace_artifacts() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("artifacts-sched");
    let session_id = "sched-artifactfix1";
    let task_workspace = home
        .root
        .join("scheduled")
        .join("artifact-task")
        .join("workspace");
    std::fs::create_dir_all(task_workspace.join("artifacts")).unwrap();
    let artifact = task_workspace.join("artifacts").join("weekly.md");
    std::fs::write(&artifact, "# Weekly report\n\nrun output\n").unwrap();

    // The profile registry the GUI writes for every scheduled session; the
    // store boots from it and maps the sched session to its workspace.
    // The store's boot reconciliation prunes profiles whose session JSON is
    // gone, so the transcript file must exist for the profile to survive.
    std::fs::create_dir_all(home.sessions_root()).unwrap();
    std::fs::write(
        home.sessions_root().join(format!("{session_id}.json")),
        "{}",
    )
    .unwrap();

    let profiles = home.root.join("scheduled-runs");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(
        profiles.join("session-profiles.json"),
        serde_json::json!({
            "schema_version": 1,
            "sessions": {
                session_id: {
                    "task_id": "artifact-task",
                    "model": "test-model",
                    "workspace": task_workspace.display().to_string(),
                    "mode": "agent",
                    "allow_shell": true,
                    "trust_mode": true,
                    "auto_approve": true,
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let value = run_json(&[
        "pinvou",
        "artifacts",
        "read",
        session_id,
        "artifacts/weekly.md",
    ]);
    assert_eq!(value["content"], "# Weekly report\n\nrun output\n");

    // Writing back stays refused: the deliverable lives outside
    // sessions_root, exactly like the GUI's write confinement.
    let error = run(&[
        "pinvou",
        "artifacts",
        "write",
        session_id,
        "artifacts/weekly.md",
        "--stdin",
    ])
    .expect_err("scheduled-run artifact write must stay refused");
    assert_eq!(error.exit_code(), ExitCode::Failed);
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
    // Panic-safe restore: the guard captures the previous value before the
    // test overrides it, mirroring HomeGuard's Drop, so a failing assertion
    // cannot leak the relative PINVOU3_HOME into other tests.
    let _restore = RestoreHome(std::env::var_os("PINVOU3_HOME"));
    // A relative PINVOU3_HOME violates the CLI sandbox contract; the shared
    // resolver (support::sandbox_home) must refuse it instead of resolving
    // store paths against the current working directory. The store may
    // create directories under the (relative) root before the refusal, so
    // clean up afterwards (best-effort: a leftover temp directory is
    // harmless, the restored environment is not).
    let relative = format!("pinvou-cli-rel-home-{}-{nonce}", std::process::id());
    // SAFETY: ENV_LOCK is held; env writes are serialized in-process.
    unsafe { std::env::set_var("PINVOU3_HOME", relative.as_str()) };
    let error = run(&["pinvou", "sessions", "folder", "abc"]).expect_err("relative home refused");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("absolute"));
    let _ = std::fs::remove_dir_all(&relative);
}
