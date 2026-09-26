//! Contract tests for the `memory` family (`crates/cli/src/memory.rs`).
//!
//! Parse-level tests cover every subcommand plus the invalid shapes that must
//! map to exit-code 2 usage errors. Execute-level tests run against a temp
//! `PINVOU3_HOME` (serialized through ENV_LOCK, following cli_contract.rs) and
//! assert through the same `pinvou3_lib::features::memory` io functions the
//! GUI uses; they never touch the network or a model (AGENTS.md rule). The
//! only host/model path, `memory organize`, is covered by an `#[ignore]`d
//! documentation test.

use pinvou_cli::{CliCommand, ExitCode, OutputMode, execute, parse_args};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Serialises tests that mutate the process-global `PINVOU3_HOME` environment
/// variable, preventing data races when the parallel test runner executes them
/// concurrently.
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct TempHome {
    root: PathBuf,
    previous: Option<std::ffi::OsString>,
}

impl TempHome {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pinvou-cli-memory-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let previous = std::env::var_os("PINVOU3_HOME");
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
        Self { root, previous }
    }

    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        // Best-effort cleanup: a leftover temp directory must never turn an
        // assertion failure into a panic raised from inside Drop.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn run_ok(arguments: &[&str]) -> String {
    let parsed = parse_args(arguments.to_vec()).expect("valid memory command");
    let outcome = execute(parsed).expect("successful memory command");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    outcome.stdout
}

/// Usage errors may surface from `parse` or from `execute`; both must carry
/// the exit-code 2 usage marker.
fn expect_usage_error(arguments: &[&str]) -> pinvou_cli::CliError {
    match parse_args(arguments.to_vec()) {
        Err(error) => error,
        Ok(parsed) => execute(parsed).expect_err("expected usage error"),
    }
}

fn assert_usage(arguments: &[&str]) {
    let error = expect_usage_error(arguments);
    assert_eq!(error.exit_code(), ExitCode::Usage, "{error}");
}

fn enqueue_fixture(kind: &str, content: &str) -> pinvou3_lib::features::memory::PendingMemoryItem {
    pinvou3_lib::features::memory::enqueue_memory_candidate(
        pinvou3_lib::features::memory::MemorySuggestion {
            kind: kind.to_owned(),
            topic: String::new(),
            content: content.to_owned(),
            source: "contract-test".to_owned(),
        },
    )
    .expect("fixture pending entry")
}

// ---- parse-level coverage ----

/// The family command types are private to the crate (lib.rs only re-exports
/// `AgentCommand`), so parse-level assertions freeze the derived Debug shape of
/// the parsed command instead of naming the variants directly.
fn parsed_debug(arguments: &[&str]) -> String {
    format!(
        "{:?}",
        parse_args(arguments.to_vec())
            .expect("valid memory command")
            .command()
    )
}

#[test]
fn memory_parses_every_subcommand() {
    let cases = [
        (&["pinvou", "memory", "overview"][..], "Memory(Overview)"),
        (
            &["pinvou", "memory", "profile", "get"][..],
            "Memory(ProfileGet)",
        ),
        (
            &[
                "pinvou",
                "memory",
                "profile",
                "set",
                "--call-name",
                "Alice",
                "--assistant-alias",
                "Pin",
            ][..],
            r#"Memory(ProfileSet { call_name: Some("Alice"), assistant_alias: Some("Pin") })"#,
        ),
        (
            &["pinvou", "memory", "list"][..],
            "Memory(List { store: None })",
        ),
        (
            &["pinvou", "memory", "list", "--store", "pending"][..],
            "Memory(List { store: Some(Pending) })",
        ),
        (
            &["pinvou", "memory", "list", "--store", "work_context"][..],
            "Memory(List { store: Some(WorkContext) })",
        ),
        (
            &[
                "pinvou",
                "memory",
                "add",
                "preference",
                "--content",
                "Prefer concise answers",
            ][..],
            r#"Memory(Add { kind: Preference, source: Inline("Prefer concise answers") })"#,
        ),
        (
            &[
                "pinvou",
                "memory",
                "add",
                "work-context",
                "ship",
                "the",
                "cli",
            ][..],
            r#"Memory(Add { kind: WorkContext, source: Inline("ship the cli") })"#,
        ),
        (
            &[
                "pinvou",
                "memory",
                "update",
                "work_context",
                "ctx-1",
                "--content",
                "new text",
            ][..],
            r#"Memory(Update { store: WorkContext, id: "ctx-1", content: "new text" })"#,
        ),
        (
            &[
                "pinvou",
                "memory",
                "delete",
                "preferences",
                "pref-1",
                "--yes",
            ][..],
            r#"Memory(Delete { store: Preferences, id: "pref-1", confirmed: true })"#,
        ),
        (
            &["pinvou", "memory", "archive", "rw-1"][..],
            r#"Memory(Archive { id: "rw-1" })"#,
        ),
        (
            &["pinvou", "memory", "pending", "confirm", "p-1"][..],
            r#"Memory(Pending { action: Confirm, id: "p-1", reason: None })"#,
        ),
        (
            &[
                "pinvou",
                "memory",
                "pending",
                "never",
                "p-1",
                "--reason",
                "sensitive",
            ][..],
            r#"Memory(Pending { action: Never, id: "p-1", reason: Some("sensitive") })"#,
        ),
        (
            &["pinvou", "memory", "organize", "--yes"][..],
            "Memory(Organize { confirmed: true })",
        ),
        (
            &["pinvou", "memory", "organize-history"][..],
            "Memory(OrganizeHistory)",
        ),
    ];
    for (arguments, expected) in cases {
        assert_eq!(parsed_debug(arguments), expected, "{arguments:?}");
        assert!(matches!(
            parse_args(arguments.to_vec()).unwrap().command(),
            CliCommand::Memory(_)
        ));
    }
}

#[test]
fn memory_rejects_invalid_usage_with_exit_code_two() {
    // unknown subcommand / missing subcommand
    assert_usage(&["pinvou", "memory", "bogus"]);
    assert_usage(&["pinvou", "memory"]);
    // unknown store value names the valid options
    let error = expect_usage_error(&["pinvou", "memory", "list", "--store", "bogus"]);
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(error.to_string().contains("preferences"), "{error}");
    assert!(error.to_string().contains("recent-work"), "{error}");
    assert_usage(&[
        "pinvou",
        "memory",
        "update",
        "bogus",
        "id-1",
        "--content",
        "x",
    ]);
    // recent-work is archive-only, pending resolves through `memory pending`
    assert_usage(&[
        "pinvou",
        "memory",
        "update",
        "recent-work",
        "id-1",
        "--content",
        "x",
    ]);
    assert_usage(&["pinvou", "memory", "delete", "pending", "id-1", "--yes"]);
    // add without content
    assert_usage(&["pinvou", "memory", "add", "preference"]);
    assert_usage(&["pinvou", "memory", "add", "work-context"]);
    assert_usage(&["pinvou", "memory", "add", "bogus-kind", "--content", "x"]);
    assert_usage(&[
        "pinvou",
        "memory",
        "add",
        "preference",
        "--content",
        "a",
        "--file",
        "f",
    ]);
    // update/delete require the id and --content
    assert_usage(&["pinvou", "memory", "update", "preferences"]);
    assert_usage(&["pinvou", "memory", "update", "preferences", "id-1"]);
    // pending without id
    assert_usage(&["pinvou", "memory", "pending", "confirm"]);
    assert_usage(&["pinvou", "memory", "pending", "bogus", "id-1"]);
    // profile set without any field, unknown profile action
    assert_usage(&["pinvou", "memory", "profile", "set"]);
    assert_usage(&["pinvou", "memory", "profile", "bogus"]);
    // unknown options and unexpected trailing arguments
    assert_usage(&["pinvou", "memory", "overview", "--json"]);
    assert_usage(&["pinvou", "memory", "organize", "extra"]);
    // Stray positionals on the option-only commands: each of these used to
    // parse and run with the extra token silently dropped, answering a
    // different command than the one typed — `list preferences` dumped all six
    // stores, `pending confirm ID oops` ignored `oops`, `profile set
    // --call-name X junk` wrote only X, and `add --file P extra words` stored
    // the file and discarded the words.
    for arguments in [
        &["pinvou", "memory", "list", "preferences"][..],
        &["pinvou", "memory", "pending", "confirm", "id-1", "oops"][..],
        &[
            "pinvou",
            "memory",
            "profile",
            "set",
            "--call-name",
            "Alice",
            "junk",
        ][..],
        &[
            "pinvou",
            "memory",
            "add",
            "preference",
            "--file",
            "p.txt",
            "extra",
            "words",
        ][..],
    ] {
        let error = expect_usage_error(arguments);
        assert_eq!(error.exit_code(), ExitCode::Usage, "{arguments:?}: {error}");
        assert!(
            error.to_string().contains("no positional arguments"),
            "{arguments:?}: {error}"
        );
    }

    // organize without --yes is rejected at execute time, like delete: the
    // LLM-driven store rewrite is destructive
    let error = expect_usage_error(&["pinvou", "memory", "organize"]);
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(error.to_string().contains("--yes"), "{error}");
    assert_usage(&["pinvou", "memory", "list", "--bogus", "x"]);

    // delete without --yes is rejected at execute time
    let error = expect_usage_error(&["pinvou", "memory", "delete", "preferences", "id-1"]);
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(error.to_string().contains("--yes"), "{error}");
}

#[test]
fn memory_global_output_flag_still_applies() {
    let parsed = parse_args(["pinvou", "--output", "json", "memory", "list"]).unwrap();
    assert_eq!(parsed.output(), OutputMode::Json);
    assert_eq!(
        format!("{:?}", parsed.command()),
        "Memory(List { store: None })"
    );
}

// ---- execute-level coverage (temp PINVOU3_HOME, no host/model) ----

#[test]
fn memory_profile_set_get_round_trips_through_feature_io() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("profile");

    let human = run_ok(&[
        "pinvou",
        "memory",
        "profile",
        "set",
        "--call-name",
        "Alice",
        "--assistant-alias",
        "Pin",
    ]);
    assert!(
        human.contains("Alice"),
        "profile set output should confirm the call name"
    );
    assert!(
        human.contains("Pin"),
        "profile set output should confirm the assistant alias"
    );

    // assert through the same feature io the GUI reads
    let profile = pinvou3_lib::features::memory::load_profile().unwrap();
    assert_eq!(profile.identity.call_name, "Alice");
    assert_eq!(profile.identity.assistant_alias, "Pin");

    let json = run_ok(&["pinvou", "memory", "profile", "get", "--output", "json"]);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["identity"]["call_name"], "Alice");
    assert_eq!(value["identity"]["assistant_alias"], "Pin");
}

#[test]
fn memory_add_preference_shows_up_in_list_and_supports_update_delete() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("preference-roundtrip");

    run_ok(&[
        "pinvou",
        "memory",
        "add",
        "preference",
        "--content",
        "Prefer concise answers",
    ]);

    let json = run_ok(&[
        "pinvou",
        "memory",
        "list",
        "--store",
        "preferences",
        "--output",
        "json",
    ]);
    let envelope: serde_json::Value = serde_json::from_str(&json).unwrap();
    // Every `list --store` JSON shape is the same {items, cleanup_warnings}
    // envelope; preferences may carry a cleanup warning, the others always
    // report an empty array.
    assert_eq!(envelope["cleanup_warnings"], serde_json::json!([]));
    let items = envelope["items"]
        .as_array()
        .expect("json array of preferences");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["text"], "Prefer concise answers");
    let id = items[0]["id"].as_str().unwrap().to_owned();

    let stored = pinvou3_lib::features::memory::list_preferences().unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].id, id);

    run_ok(&[
        "pinvou",
        "memory",
        "update",
        "preferences",
        &id,
        "--content",
        "Prefer bullet answers",
    ]);
    let stored = pinvou3_lib::features::memory::list_preferences().unwrap();
    assert_eq!(stored[0].text, "Prefer bullet answers");

    // delete requires --yes
    assert_usage(&["pinvou", "memory", "delete", "preferences", &id]);
    run_ok(&["pinvou", "memory", "delete", "preferences", &id, "--yes"]);
    assert!(
        pinvou3_lib::features::memory::list_preferences()
            .unwrap()
            .is_empty()
    );

    // deleting the same id again is a host failure, not a silent success
    let error = expect_usage_error(&["pinvou", "memory", "delete", "preferences", &id, "--yes"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("preferences_not_found"),
        "{error}"
    );
}

#[test]
fn memory_add_work_context_from_file_and_positional_arguments() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("work-context");

    let file = home.path().join("context.txt");
    std::fs::write(&file, "Shipping the pinvou CLI memory family\n").unwrap();
    run_ok(&[
        "pinvou",
        "memory",
        "add",
        "work-context",
        "--file",
        file.to_str().unwrap(),
    ]);

    run_ok(&[
        "pinvou",
        "memory",
        "add",
        "work-context",
        "Reviewing",
        "the",
        "cli",
        "contract",
    ]);

    let json = run_ok(&[
        "pinvou",
        "memory",
        "list",
        "--store",
        "work-context",
        "--output",
        "json",
    ]);
    let items: serde_json::Value = serde_json::from_str(&json).unwrap();
    let items = items["items"].as_array().expect("envelope items array");
    // The feature upserts work context by topic; the CLI adds without a topic
    // share the default topic, so the second add rewrites the first entry.
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["text"], "Reviewing the cli contract");
}

#[test]
fn memory_pending_confirm_ignore_and_never_resolve_fixture_entries() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("pending");

    // Fixture entries in _pending.jsonl, written by the feature writer itself.
    let confirmed = enqueue_fixture("preference", "Prefer concise answers");
    let ignored = enqueue_fixture("preference", "Prefer short summaries");
    let nevered = enqueue_fixture("preference", "Avoid storing billing notes");

    run_ok(&["pinvou", "memory", "pending", "confirm", &confirmed.id]);
    run_ok(&["pinvou", "memory", "pending", "ignore", &ignored.id]);
    run_ok(&[
        "pinvou",
        "memory",
        "pending",
        "never",
        &nevered.id,
        "--reason",
        "billing details",
    ]);

    let pending = pinvou3_lib::features::memory::load_pending_memory().unwrap();
    let status_of = |id: &str| {
        pending
            .iter()
            .find(|item| item.id == id)
            .unwrap_or_else(|| panic!("pending fixture {id} missing"))
            .status
            .clone()
    };
    assert_eq!(status_of(&confirmed.id), "confirmed");
    assert_eq!(status_of(&ignored.id), "ignored");

    // confirm materializes the preference, matching the GUI pipeline
    let stored = pinvou3_lib::features::memory::list_preferences().unwrap();
    assert!(
        stored
            .iter()
            .any(|item| item.text == "Prefer concise answers")
    );

    let never = pinvou3_lib::features::memory::load_never_memory().unwrap();
    assert_eq!(never.len(), 1);
    assert_eq!(never[0].pattern, "Avoid storing billing notes");
    assert_eq!(never[0].reason, "billing details");

    // unknown ids surface as host failures
    let error = expect_usage_error(&["pinvou", "memory", "pending", "confirm", "missing-id"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("pending_not_found"), "{error}");
}

/// Writes one active recent-work line straight into the store file: the app
/// has no public writer for this store (recent work is produced by the
/// engine's turn capture), and the tests only need a fixture to read back.
fn seed_recent_work(title: &str, summary: &str) -> pinvou3_lib::features::memory::RecentWorkItem {
    let now = chrono::Utc::now();
    let item = pinvou3_lib::features::memory::RecentWorkItem {
        id: "recent-contract-fixture".to_owned(),
        title: title.to_owned(),
        summary: summary.to_owned(),
        status: "active".to_owned(),
        source: "contract-test".to_owned(),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        last_hit: now.to_rfc3339(),
        expires_at: (now + chrono::Duration::days(30)).to_rfc3339(),
    };
    let path = pinvou3_lib::features::memory::recent_work_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let line = serde_json::to_string(&item).unwrap();
    std::fs::write(&path, format!("{line}\n")).unwrap();
    item
}

#[test]
fn memory_archive_marks_recent_work_fixture_archived() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("archive");

    let item = seed_recent_work("Shipped CLI memory parity", "memory family contract");

    run_ok(&["pinvou", "memory", "archive", &item.id]);

    let stored = pinvou3_lib::features::memory::load_recent_work().unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].status, "archived");

    // archiving an unknown id fails instead of silently succeeding
    let error = expect_usage_error(&["pinvou", "memory", "archive", "missing-id"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("recent_work_not_found"),
        "{error}"
    );
}

#[test]
fn memory_overview_counts_match_fixtures_and_write_snapshot() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("overview");

    // one materialized preference
    let preference = enqueue_fixture("preference", "Prefer concise answers");
    pinvou3_lib::features::memory::confirm_pending_memory(&preference.id)
        .unwrap()
        .unwrap();
    // one pending entry left unresolved
    enqueue_fixture("preference", "Prefer short summaries");
    // one recent work entry
    seed_recent_work("Shipped CLI memory parity", "");

    let json = run_ok(&["pinvou", "memory", "overview", "--output", "json"]);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        value["preferences"].as_array().unwrap().len(),
        1,
        "overview preference count mismatch"
    );
    assert_eq!(
        value["pending"].as_array().unwrap().len(),
        2,
        "overview pending count mismatch"
    );
    assert_eq!(
        value["recent_work"].as_array().unwrap().len(),
        1,
        "overview recent-work count mismatch"
    );
    assert_eq!(value["current_focus"].as_array().unwrap().len(), 0);
    assert_eq!(value["recent_activity"].as_array().unwrap().len(), 0);
    assert_eq!(value["never"].as_array().unwrap().len(), 0);
    // all authoritative sources available: the snapshot document was refreshed
    assert!(
        !value["snapshot_path"].as_str().unwrap().is_empty(),
        "overview should refresh the snapshot document when every source is available"
    );
    assert_eq!(value["sources"]["preferences"]["available"], true);
    assert_eq!(value["sources"]["runtime"]["available"], true);
    assert_eq!(value["warnings"].as_array().unwrap().len(), 0);

    let human = run_ok(&["pinvou", "memory", "overview"]);
    assert!(
        human.contains("Preferences: 1"),
        "overview should count preferences sources"
    );
    assert!(
        human.contains("Pending: 2"),
        "overview should count pending sources"
    );
    assert!(
        human.contains("Recent work: 1"),
        "overview should count recent work sources"
    );
}

#[test]
fn memory_organize_history_is_empty_on_fresh_state() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("organize-history");

    let json = run_ok(&["pinvou", "memory", "organize-history", "--output", "json"]);
    assert_eq!(json, "[]");

    let human = run_ok(&["pinvou", "memory", "organize-history"]);
    assert_eq!(human, "No organize history.");
}

/// Opt-in check for `pinvou memory organize`: it boots the windowless product
/// host (`pinvou3_lib::headless_bridge::run_windowless_host`, the bootstrap
/// `run_with_product_backend` wraps) and calls
/// `pinvou3_lib::features::memory::organize_memory_with_llm` exactly like the
/// scheduled memory-organize executor, so it needs a display (xvfb on headless
/// Linux) and a configured, active model. Default tests never call the host or
/// a model (AGENTS.md rule); the wiring itself is intentionally not invoked
/// here — run the real command manually to exercise it.
#[test]
fn memory_organize_refuses_when_memory_is_disabled() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("organize-disabled");
    // Memory is disabled in a fresh home, so the refusal fires before any
    // host boot (display and model remain the opt-in part exercised
    // manually); this pins the honest error instead of a vacuous pass.
    // --yes clears the destructive-action gate so the disabled refusal is
    // what is under test.
    let error = expect_usage_error(&["pinvou", "memory", "organize", "--yes"]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("memory_organize_disabled"),
        "{error}"
    );
}

/// `memory organize` takes a cross-process single-flight lock BEFORE the
/// windowless host boots: the feature layer's `ORGANIZE_IN_FLIGHT` guard is
/// process-local (`features/memory/organize.rs` — "the two passes would
/// interleave destructive actions based on their own up-to-75-second-old
/// snapshots"), so a second CLI process would interleave exactly those
/// actions. This test holds the lock in the test process — the exact state
/// "another pinvou process is organizing" produces — and requires the
/// command to refuse with `memory_organize_busy` rather than start a second
/// pass. Reachable without a display or a model because the lock precedes
/// the host boot, so the default no-host/no-model test policy holds.
#[test]
fn memory_organize_refuses_when_another_process_holds_the_lock() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("organize-busy");

    // Enable memory so the command gets past the disabled check and reaches
    // the lock (the refusal under test is the busy one, not the disabled
    // one). The language must be zh-Hans: the memory locale policy forces
    // `memory_enabled` back to false for any other language
    // (`platform::prefs` `enforce_memory_locale_policy`).
    let settings = home.path().join("settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(
        &settings,
        serde_json::json!({ "language": "zh-Hans", "memory_enabled": true }).to_string(),
    )
    .unwrap();

    // Hold the lock the way a concurrent organize would.
    let dir = home.path().join("locks");
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("memory-organize.lock"))
        .unwrap();
    let mut lock = fd_lock::RwLock::new(file);
    let _held = lock.write().unwrap();

    let error = expect_usage_error(&["pinvou", "memory", "organize", "--yes"]);
    assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
    assert!(
        error.to_string().contains("memory_organize_busy"),
        "a second concurrent organize must be refused, not interleaved: {error}"
    );
}

#[test]
fn memory_add_accepts_ordinary_punctuated_work_context() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let home = TempHome::new("work-context-punctuated");

    // The confirm path stores the punctuation-stripped normalization; the
    // verification must compare against the same form or this exact input
    // false-fails with memory_add_not_materialized after storing fine.
    run_ok(&[
        "pinvou",
        "memory",
        "add",
        "work-context",
        "We deploy on Fridays.",
    ]);

    let json: serde_json::Value = serde_json::from_str(&run_ok(&[
        "pinvou",
        "memory",
        "list",
        "--store",
        "work-context",
        "--output",
        "json",
    ]))
    .expect("single-line JSON output");
    let items = json["items"].as_array().expect("work-context items array");
    assert!(
        items
            .iter()
            .any(|item| item["text"] == serde_json::json!("We deploy on Fridays")),
        "the punctuated work-context item must be materialized with the \
         punctuation-stripped normalization"
    );
    let _ = home;
}

#[test]
fn memory_add_preference_reports_the_replaced_item() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("preference-replacement");

    let first = run_ok(&[
        "pinvou",
        "memory",
        "add",
        "preference",
        "--content",
        "Prefer concise answers",
        "--output",
        "json",
    ]);
    let first: serde_json::Value = serde_json::from_str(&first).unwrap();
    let first_id = first["id"].as_str().unwrap().to_owned();
    assert!(
        first.get("replaced").is_none(),
        "a first add must replace nothing"
    );

    // The preference store is replace-per-topic: the CLI adds without a
    // topic, so every add targets the same bucket and the write deletes the
    // previous item. The second add must say so instead of presenting the
    // store as append-only.
    let second = run_ok(&[
        "pinvou",
        "memory",
        "add",
        "preference",
        "--content",
        "Prefer bullet answers",
        "--output",
        "json",
    ]);
    let second: serde_json::Value = serde_json::from_str(&second).unwrap();
    assert_eq!(second["replaced"], serde_json::json!([first_id]));
    let stored = pinvou3_lib::features::memory::list_preferences().unwrap();
    assert_eq!(stored.len(), 1, "the replaced item is gone");

    let human = run_ok(&[
        "pinvou",
        "memory",
        "add",
        "preference",
        "--content",
        "Prefer tables in reports",
    ]);
    assert!(
        human.contains("replaced 1 earlier item"),
        "human output must surface the replacement"
    );
}

#[test]
fn memory_add_profile_shaped_preference_text_fails_before_the_pending_store() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("preference-profile-shaped");

    // Chinese profile-preference phrasing (the feature heuristic
    // `looks_like_profile_preference_text`, features/memory/types.rs) is
    // routed to the profile, not the preference store: the confirm path
    // silently skips the write while still marking the candidate confirmed.
    // The add must fail up front (exit 1) WITHOUT enqueueing the candidate.
    let error = expect_usage_error(&[
        "pinvou",
        "memory",
        "add",
        "preference",
        "--content",
        "请以后称呼用户为老板",
    ]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("memory_add_not_materialized"),
        "{error}"
    );
    // The pending store is untouched: no candidate was enqueued, so nothing
    // was marked confirmed behind the failure.
    assert!(
        pinvou3_lib::features::memory::load_pending_memory()
            .unwrap()
            .is_empty()
    );
    // And nothing was materialized into the preference store either.
    assert!(
        pinvou3_lib::features::memory::list_preferences()
            .unwrap()
            .is_empty()
    );
}

/// `memory pending confirm` must verify the write landed: the confirm path
/// marks profile-shaped preference text confirmed while the feature
/// deliberately skips materializing it, so a success report would strand
/// the item confirmed-but-never-written. The command reports the no-op
/// honestly (exit 1). The add-path analogue is refused up front, so the
/// fixture enqueues the candidate through the feature API directly — the
/// only way this state is reachable.
#[test]
fn memory_pending_confirm_reports_a_profile_shaped_no_op_honestly() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("confirm-profile-noop");

    let item = pinvou3_lib::features::memory::enqueue_memory_candidate(
        pinvou3_lib::features::memory::MemorySuggestion {
            kind: "preference".to_owned(),
            topic: String::new(),
            content: "请以后称呼用户为老板".to_owned(),
            source: "test".to_owned(),
        },
    )
    .expect("enqueue the profile-shaped candidate");

    let error = expect_usage_error(&["pinvou", "memory", "pending", "confirm", &item.id]);
    assert_eq!(error.exit_code(), ExitCode::Failed);
    let message = error.to_string();
    // The message states the observable fact first — the confirm produced no
    // visible store row — and then lists the causes, because
    // `confirmed_pending_memory_is_materialized` returns false for several
    // (TTL archival, a re-confirm that rewrites nothing, an unverifiable
    // profile topic) and cannot say which applies. The profile-shaped skip
    // exercised here must still be named among them.
    assert!(
        message.contains("no matching item is visible in its target store"),
        "the no-op must be reported, not success: {message}"
    );
    assert!(
        message.contains("profile-shaped preference text"),
        "the applicable cause must still be named: {message}"
    );
}

/// `memory pending confirm` must not report an unverified write as materialized
/// when the id it was handed does not round-trip.
///
/// `confirm_pending_memory` resolves `clean_id(id)` while the CLI's read-back
/// matched the RAW argv string: an id with characters `clean_id` rewrites
/// confirms a real row, then finds nothing to check, and the old
/// `unwrap_or(true)` fallback declared the write materialized — skipping the
/// honesty check exactly when the input was off. The safe direction is to fail.
#[test]
fn memory_pending_confirm_fails_when_the_id_does_not_round_trip() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("confirm-id-roundtrip");

    let item = enqueue_fixture("preference", "Prefer concise answers");
    // `clean_id` maps every character outside [A-Za-z0-9-_] to '_' and trims
    // the result, so this spelling confirms the same row but never equals the
    // stored id on read-back.
    let raw_id = format!("{}.", item.id);

    let error = expect_usage_error(&["pinvou", "memory", "pending", "confirm", &raw_id]);
    assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
    assert!(
        error.to_string().contains("cannot be verified"),
        "an unverifiable confirm must not be reported as materialized: {error}"
    );
}

/// `memory update` rejects an empty/whitespace body as a usage error at
/// parse time — the same gate `add` applies — instead of an exit 1 from a
/// store rejection for a knowable-at-parse-time invalid argument.
///
/// Both spellings stay exit 2; only which layer refuses them differs. A
/// literally empty `--content ""` is now stopped by the option parser, which
/// treats an empty value as a MISSING value in every family (the shell writes
/// it that way when an unset variable expands), so it never reaches the
/// command. `"   "` carries a value and is refused by `update`'s own
/// non-empty-content gate. The exit code is the contract; the message names
/// the layer that caught it.
#[test]
fn memory_update_rejects_empty_content_as_a_usage_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("update-empty-content");
    for (content, needle) in [("", "requires a value"), ("   ", "non-empty content")] {
        let error = expect_usage_error(&[
            "pinvou",
            "memory",
            "update",
            "preferences",
            "some-id",
            "--content",
            content,
        ]);
        assert_eq!(error.exit_code(), ExitCode::Usage, "content={content:?}");
        assert!(
            error.to_string().contains(needle),
            "content={content:?}: {error}"
        );
    }
}

/// `memory update` classifies content that normalizes away exactly like
/// `memory add` does, up front, instead of letting the store reject it.
///
/// Every editable store's writer runs the patch text through
/// `clean_candidate_sentence` and refuses an empty result, so punctuation-only
/// text came back as a store-flavoured `memory_update_failed: ...` io error
/// while `add` — which pre-checks the same predicate — refused the identical
/// input with a message naming the real reason. Both spellings stay exit 1;
/// what this pins is that the two commands answer the same input the same way.
#[test]
fn memory_update_refuses_content_that_normalizes_away_like_add_does() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("update-normalizes-away");

    // Punctuation-only: `clean_candidate_sentence` strips it to nothing.
    let update = expect_usage_error(&[
        "pinvou",
        "memory",
        "update",
        "preferences",
        "some-id",
        "--content",
        "...",
    ]);
    assert_eq!(update.exit_code(), ExitCode::Failed, "{update}");
    assert!(
        update.to_string().contains("memory_update_not_applied")
            && update.to_string().contains("empty after normalization"),
        "update must name the normalization, not the store: {update}"
    );

    // The add-side classification of the very same input, for comparison.
    let add = expect_usage_error(&[
        "pinvou",
        "memory",
        "add",
        "work-context",
        "--content",
        "...",
    ]);
    assert_eq!(add.exit_code(), ExitCode::Failed, "{add}");
    assert!(
        add.to_string().contains("empty after normalization"),
        "{add}"
    );
}

// ---------------------------------------------------------------------------
// parser contract parity with support::parse_family_flags
// ---------------------------------------------------------------------------

/// A repeated boolean flag is a usage error, exactly as in
/// `support::parse_family_flags` and the `models` family.
///
/// This mattered most for the destructive subcommand: `memory delete` took
/// `--yes --yes` and DELETED, while the equivalent `models remove X --yes
/// --yes` exited 2. A duplicated confirmation flag is the signature of a
/// command line assembled by a script that appended `--yes` to an argv that
/// already had one — precisely the situation where a destructive family must
/// stop rather than guess. Without the `has_flag` guard in
/// `memory::parse_options` every case below parses and exits 0.
#[test]
fn memory_rejects_duplicate_boolean_flags() {
    for arguments in [
        &[
            "pinvou",
            "memory",
            "delete",
            "preferences",
            "id",
            "--yes",
            "--yes",
        ][..],
        &["pinvou", "memory", "organize", "--yes", "--yes"][..],
    ] {
        let error = expect_usage_error(arguments);
        assert_eq!(error.exit_code(), ExitCode::Usage, "{arguments:?}: {error}");
        assert!(
            error.to_string().contains("duplicate option --yes"),
            "{arguments:?}: {error}"
        );
    }
}

/// An empty value for a valued option is a missing value, not a request to
/// address the empty-named item.
///
/// `memory` used to accept every one of these and hand the blank string to
/// the store, where `--store ""` became an unknown-store error phrased in
/// store vocabulary and `--reason ""`/`--call-name ""` were written as real
/// blank fields. `support::parse_family_flags` has always refused them; this
/// pins the `memory` family onto the same contract. Without the
/// `value.is_empty()` guard every case below reaches `execute`.
#[test]
fn memory_rejects_empty_option_values() {
    for arguments in [
        &["pinvou", "memory", "list", "--store", ""][..],
        &["pinvou", "memory", "add", "preference", "--content", ""][..],
        &["pinvou", "memory", "add", "preference", "--file", ""][..],
        &["pinvou", "memory", "profile", "set", "--call-name", ""][..],
        &[
            "pinvou", "memory", "pending", "confirm", "id", "--reason", "",
        ][..],
    ] {
        let error = expect_usage_error(arguments);
        assert_eq!(error.exit_code(), ExitCode::Usage, "{arguments:?}: {error}");
        assert!(
            error.to_string().contains("requires a value"),
            "{arguments:?}: {error}"
        );
    }
}

// ---------------------------------------------------------------------------
// add: truncation honesty
// ---------------------------------------------------------------------------

/// A work-context add of 130 characters must never silently lose its tail.
///
/// The `add` path is enqueue-then-confirm, and the FIRST normalization on it
/// is `pending_item_from_suggestion`'s `clean_text(content, 120)` — a hard
/// `chars().take(120)`. The work-context store's own
/// `WORK_CONTEXT_TEXT_MAX_CHARS` (160) therefore NEVER binds here, so an
/// input of 121..=160 characters was truncated by a cap the CLI was not
/// warning about. Worse, the post-write verification compared the store
/// against `pending.content` — the already-truncated value the pipeline
/// echoed back — so it confirmed the truncation instead of catching it and
/// the command exited 0 with no indication anything was dropped.
///
/// This pins all three halves of the fix: the truncation really happens at
/// 120 (asserted against the feature store, so the CLI's constant cannot
/// drift from the feature layer unnoticed), the add still succeeds, and the
/// loss is disclosed on the command's own output rather than only on stderr.
/// Before the fix the assertions on `truncated` fail: no such field existed
/// and the 160-char warning never fired for a 130-char input.
#[test]
fn memory_add_work_context_over_the_cap_reports_the_truncation() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("work-context-over-cap");

    // 130 single-byte characters, no whitespace runs and no leading or
    // trailing punctuation, so the only normalization that can change the
    // text is the cap itself. Deliberately in the 121..=160 window that the
    // 160-char warning could never catch.
    let content: String = std::iter::repeat_n("abcdefghij", 13).collect();
    assert_eq!(content.chars().count(), 130);

    let json: serde_json::Value = serde_json::from_str(&run_ok(&[
        "pinvou",
        "memory",
        "add",
        "work-context",
        "--content",
        &content,
        "--output",
        "json",
    ]))
    .expect("single-line JSON output");

    assert_eq!(
        json["truncated"],
        serde_json::json!(true),
        "an over-cap add must disclose the truncation: {json}"
    );
    assert_eq!(
        json["submitted_characters"],
        serde_json::json!(130),
        "{json}"
    );
    assert_eq!(json["stored_characters"], serde_json::json!(120), "{json}");

    // The store itself is the authority on where the cut fell: this is what
    // makes the 120 in memory.rs a checked fact rather than a comment.
    let stored = pinvou3_lib::features::memory::load_work_context().unwrap();
    let item = stored
        .iter()
        .find(|item| content.starts_with(&item.text))
        .unwrap_or_else(|| panic!("the truncated item must be stored: {stored:?}"));
    assert_eq!(
        item.text.chars().count(),
        120,
        "the add pipeline caps at 120, not at WORK_CONTEXT_TEXT_MAX_CHARS (160)"
    );
    assert_eq!(item.text, content.chars().take(120).collect::<String>());

    // The human rendering carries the same disclosure, for the interactive
    // caller who never looks at JSON.
    let human = run_ok(&[
        "pinvou",
        "memory",
        "add",
        "work-context",
        "--content",
        &content,
    ]);
    assert!(
        human.contains("exceed the 120-character cap") && human.contains("only the first 120"),
        "the human output must disclose the truncation: {human}"
    );
}

/// An add whose text is NOT what the pipeline stored must fail, not succeed.
///
/// `enqueue_memory_candidate`'s second dedupe branch matches an existing
/// *pending* row on a LOWERCASED content key and returns that row unchanged,
/// so the confirm writes the earlier row's wording and the caller's own text
/// never lands. The old verification compared the store against
/// `pending.content` — that same earlier row — which made the check
/// self-satisfying: it passed while the submitted text was silently dropped.
/// Anchoring the comparison on the ORIGINAL user input is what turns this
/// into a visible, actionable failure.
///
/// Before the fix this add exits 0 and reports "Remembered work context".
#[test]
fn memory_add_fails_when_the_pipeline_stores_different_text() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("work-context-case-dedupe");

    // A pending candidate the user never resolved, in a different casing.
    enqueue_fixture("work_context", "We Deploy On Fridays");

    let error = expect_usage_error(&[
        "pinvou",
        "memory",
        "add",
        "work-context",
        "--content",
        "WE DEPLOY ON FRIDAYS",
    ]);
    assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
    let message = error.to_string();
    assert!(
        message.contains("memory_add_not_materialized"),
        "the divergence must be reported: {message}"
    );
    assert!(
        message.contains("We Deploy On Fridays") && message.contains("WE DEPLOY ON FRIDAYS"),
        "the message must name both the stored and the submitted text: {message}"
    );
    // The divergence is detected BEFORE the confirm, so the reused candidate
    // is still awaiting review and nothing reached the work-context store.
    let pending = pinvou3_lib::features::memory::load_pending_memory().unwrap();
    assert_eq!(
        pending.len(),
        1,
        "no second candidate was queued: {pending:?}"
    );
    // `PENDING_STATUS_PENDING` (features/memory/types.rs) — the awaiting-review
    // status is spelled "pending_confirm"; it is not re-exported, so the
    // literal is pinned here rather than widening the app surface for a test.
    assert_eq!(pending[0].status, "pending_confirm", "{pending:?}");
    assert!(
        pinvou3_lib::features::memory::load_work_context()
            .unwrap()
            .is_empty()
    );
}

/// A failed `memory add` must not confirm or materialize an UNRELATED pending
/// candidate.
///
/// `enqueue_memory_candidate`'s second dedupe branch matches an existing
/// *pending* row on a lowercased content key and returns that row with its own
/// `content` intact. The CLI then confirmed the id it was handed, which really
/// approved a candidate the user had not reviewed and wrote the earlier row's
/// text into the topic bucket; only afterwards did the verification lookup
/// fail and the command exit 1. So the failure path had two silent side
/// effects and its remediation ("resolve it with `pinvou memory pending`")
/// pointed at a row that was already resolved and already written.
///
/// This pins the state the bug corrupts: exit 1, the seeded candidate still
/// `pending`, and the authoritative store still empty. Before the fix the
/// candidate reads `confirmed` and the preference store holds its text.
#[test]
fn memory_add_case_dedupe_failure_leaves_the_pending_queue_and_store_untouched() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("preference-case-dedupe-no-side-effects");

    // A candidate the user queued but never reviewed.
    let seeded = enqueue_fixture("preference", "Prefer Concise Answers");

    // Same text, different casing: the dedupe key is lowercased, so the
    // enqueue hands back the seeded row instead of queueing this one.
    let error = expect_usage_error(&[
        "pinvou",
        "memory",
        "add",
        "preference",
        "--content",
        "PREFER CONCISE ANSWERS",
    ]);
    assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
    assert!(
        error.to_string().contains("memory_add_not_materialized"),
        "{error}"
    );
    // The remediation must describe what actually happened: nothing written.
    assert!(
        error.to_string().contains("nothing was confirmed"),
        "the remediation must not claim a write happened: {error}"
    );

    // The seeded candidate is untouched — still awaiting the user's review.
    let pending = pinvou3_lib::features::memory::load_pending_memory().unwrap();
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].id, seeded.id, "{pending:?}");
    // `PENDING_STATUS_PENDING` (features/memory/types.rs) == "pending_confirm";
    // "confirmed" here would mean the add wrote the reused row before failing.
    assert_eq!(
        pending[0].status, "pending_confirm",
        "a failed add must not confirm a candidate the user has not reviewed: {pending:?}"
    );
    assert_eq!(pending[0].content, "Prefer Concise Answers", "{pending:?}");

    // And the authoritative store never received the other row's text.
    assert!(
        pinvou3_lib::features::memory::list_preferences()
            .unwrap()
            .is_empty(),
        "a failed add must not materialize the reused candidate"
    );
}

// ---------------------------------------------------------------------------
// add: the enqueue->confirm divergence check compares every identifying field
// ---------------------------------------------------------------------------

/// `enqueue_fixture` with an explicit topic: pending candidates the GUI
/// pipeline queues carry a real topic bucket (llm_review normalizes them
/// before enqueueing), so cross-bucket divergence fixtures need one.
fn enqueue_fixture_with_topic(
    kind: &str,
    topic: &str,
    content: &str,
) -> pinvou3_lib::features::memory::PendingMemoryItem {
    pinvou3_lib::features::memory::enqueue_memory_candidate(
        pinvou3_lib::features::memory::MemorySuggestion {
            kind: kind.to_owned(),
            topic: topic.to_owned(),
            content: content.to_owned(),
            source: "contract-test".to_owned(),
        },
    )
    .expect("fixture pending entry")
}

/// `memory add` must not confirm a pending candidate that is not this add's
/// own, even when the text body matches exactly.
///
/// Round-18 finding: the enqueue->confirm bridging compared ONLY the text
/// body. `enqueue_memory_candidate`'s content-key dedupe branch hands back an
/// existing pending row that matches this add on the kind plus a
/// case-insensitive content key while IGNORING the topic, so a GUI candidate
/// sitting in a different topic bucket with this add's exact text was adopted
/// and confirmed: the add approved a candidate the user had not reviewed, and
/// its confirm wrote through the GUI's entry — deleting the item already in
/// that bucket (replace-per-topic) and putting this add's text there instead.
/// Exit 0, presented as an ordinary remember.
///
/// Before the fix this test is red at its first assertion: the add succeeds.
#[test]
fn memory_add_refuses_a_same_text_candidate_in_a_foreign_topic_bucket() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("preference-foreign-bucket");

    // Someone else's data in the workflow_preference bucket, materialized
    // through the GUI's own pipeline (enqueue + confirm).
    let bucket_owner = enqueue_fixture_with_topic(
        "preference",
        "workflow_preference",
        "Use git rebase for edits",
    );
    pinvou3_lib::features::memory::confirm_pending_memory(&bucket_owner.id)
        .unwrap()
        .unwrap();
    // A GUI candidate awaiting review in that same bucket, carrying the exact
    // text this add will submit: identical on the text body, foreign on the
    // topic, so only the identifying fields can tell the two apart.
    let gui = enqueue_fixture_with_topic(
        "preference",
        "workflow_preference",
        "Prefer concise answers",
    );

    let error = expect_usage_error(&[
        "pinvou",
        "memory",
        "add",
        "preference",
        "--content",
        "Prefer concise answers",
    ]);
    assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
    let message = error.to_string();
    assert!(
        message.contains("memory_add_not_materialized"),
        "the divergence must be reported: {message}"
    );
    assert!(
        message.contains("nothing was confirmed"),
        "the remediation must not claim a write happened: {message}"
    );
    assert!(
        message.contains("workflow_preference") && message.contains("answer_style"),
        "the message must name both the reused entry's topic and this add's own topic: {message}"
    );

    // The GUI candidate is untouched — still awaiting its owner's review,
    // still in its own bucket.
    let pending = pinvou3_lib::features::memory::load_pending_memory().unwrap();
    let gui_row = pending
        .iter()
        .find(|item| item.id == gui.id)
        .unwrap_or_else(|| panic!("pending fixture {} missing: {pending:?}", gui.id));
    assert_eq!(gui_row.status, "pending_confirm", "{pending:?}");
    assert_eq!(gui_row.topic, "workflow_preference", "{pending:?}");
    assert_eq!(gui_row.content, "Prefer concise answers", "{pending:?}");

    // And someone else's data survived: the foreign bucket still holds its
    // own item, and this add's text reached no bucket through the GUI's row.
    let stored = pinvou3_lib::features::memory::list_preferences().unwrap();
    assert_eq!(stored.len(), 1, "nothing was written: {stored:?}");
    assert_eq!(stored[0].topic, "workflow_preference", "{stored:?}");
    assert_eq!(stored[0].text, "Use git rebase for edits", "{stored:?}");
}

/// The work-context half of the same finding, pinned separately because the
/// CLI's own work-context candidate carries an EMPTY pending-row topic (the
/// pending stage normalizes only preference topics), so the foreign bucket is
/// any non-empty one — here `role_domain`, a bucket the GUI pipeline really
/// queues candidates into.
///
/// Before the fix the add confirms the GUI's row: the `role_domain` bucket's
/// previous item is deleted and this add's text written in its place.
#[test]
fn memory_add_refuses_a_same_text_work_context_candidate_in_a_foreign_bucket() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("work-context-foreign-bucket");

    let bucket_owner =
        enqueue_fixture_with_topic("work_context", "role_domain", "Use git rebase for edits");
    pinvou3_lib::features::memory::confirm_pending_memory(&bucket_owner.id)
        .unwrap()
        .unwrap();
    let gui = enqueue_fixture_with_topic("work_context", "role_domain", "We deploy on Fridays");

    let error = expect_usage_error(&[
        "pinvou",
        "memory",
        "add",
        "work-context",
        "--content",
        "We deploy on Fridays",
    ]);
    assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
    let message = error.to_string();
    assert!(
        message.contains("memory_add_not_materialized"),
        "the divergence must be reported: {message}"
    );
    assert!(
        message.contains("nothing was confirmed"),
        "the remediation must not claim a write happened: {message}"
    );
    assert!(
        message.contains("role_domain"),
        "the message must name the reused entry's topic: {message}"
    );

    let pending = pinvou3_lib::features::memory::load_pending_memory().unwrap();
    let gui_row = pending
        .iter()
        .find(|item| item.id == gui.id)
        .unwrap_or_else(|| panic!("pending fixture {} missing: {pending:?}", gui.id));
    assert_eq!(gui_row.status, "pending_confirm", "{pending:?}");
    assert_eq!(gui_row.topic, "role_domain", "{pending:?}");
    assert_eq!(gui_row.content, "We deploy on Fridays", "{pending:?}");

    let stored = pinvou3_lib::features::memory::load_work_context().unwrap();
    assert_eq!(stored.len(), 1, "nothing was written: {stored:?}");
    assert!(
        stored
            .iter()
            .any(|item| item.text == "Use git rebase for edits"),
        "the foreign bucket must keep its own item: {stored:?}"
    );
}

/// Guard against over-tightening: the check now compares kind, topic AND
/// text, so it must still confirm whenever every field is this add's own —
/// both the fresh row the enqueue creates and the equivalent row its dedupe
/// legitimately folds onto (the id branch matches kind+topic+content, i.e. it
/// really is this add's candidate, queued by an earlier identical add).
///
/// The two arms also pin the CLI's own-field mirrors against the feature
/// layer: `answer_style` (the pending-stage normalization of the empty
/// preference topic) and the empty work-context pending topic. If either
/// drifts, every add refuses in the check and THIS test is the one that goes
/// red — the same pinning shape `memory_add_work_context_over_the_cap_...`
/// uses for the 120-character cap.
#[test]
fn memory_add_still_confirms_when_every_field_matches_this_adds_own_candidate() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("own-candidate-roundtrip");

    // Preference arm: a pending row with the CLI's own fields — the empty
    // suggestion topic normalizes to answer_style at the pending stage, the
    // same value the CLI's own add produces — and the same text.
    let seeded = enqueue_fixture("preference", "Prefer concise answers");
    run_ok(&[
        "pinvou",
        "memory",
        "add",
        "preference",
        "--content",
        "Prefer concise answers",
    ]);
    let pending = pinvou3_lib::features::memory::load_pending_memory().unwrap();
    let seeded_row = pending
        .iter()
        .find(|item| item.id == seeded.id)
        .unwrap_or_else(|| panic!("pending fixture {} missing: {pending:?}", seeded.id));
    assert_eq!(seeded_row.status, "confirmed", "{pending:?}");
    let stored = pinvou3_lib::features::memory::list_preferences().unwrap();
    assert_eq!(stored.len(), 1, "{stored:?}");
    assert_eq!(stored[0].text, "Prefer concise answers", "{stored:?}");
    // The answer_style literal in memory.rs mirrors the feature default
    // bucket; this assertion is what makes that mirror a checked fact.
    assert_eq!(stored[0].topic, "answer_style", "{stored:?}");

    // Work-context arm: the CLI's own add leaves the pending row's topic
    // empty (topics are not normalized for work context at the pending
    // stage), so an equivalent seeded row matches on every field too.
    let seeded_ctx = enqueue_fixture("work_context", "We deploy on Fridays");
    run_ok(&[
        "pinvou",
        "memory",
        "add",
        "work-context",
        "--content",
        "We deploy on Fridays",
    ]);
    let pending = pinvou3_lib::features::memory::load_pending_memory().unwrap();
    let ctx_row = pending
        .iter()
        .find(|item| item.id == seeded_ctx.id)
        .unwrap_or_else(|| panic!("pending fixture {} missing: {pending:?}", seeded_ctx.id));
    assert_eq!(ctx_row.status, "confirmed", "{pending:?}");
    let stored_ctx = pinvou3_lib::features::memory::load_work_context().unwrap();
    assert_eq!(stored_ctx.len(), 1, "{stored_ctx:?}");
    assert_eq!(stored_ctx[0].text, "We deploy on Fridays", "{stored_ctx:?}");
}

/// The cross-kind half of the round-18 finding, pinned at its real boundary.
///
/// A pending candidate of a DIFFERENT kind with the same text can never be
/// adopted by this add: `enqueue_memory_candidate`'s dedupe key includes the
/// kind, so the CLI's add confirms its OWN row and leaves the foreign
/// candidate exactly as it was. This test keeps that boundary observable: if
/// the feature dedupe ever drops the kind from its key, the enqueue would
/// hand back a foreign row, the field-by-field check would refuse the add —
/// and this test's `run_ok` would go red, surfacing the change here instead
/// of in a user's bucket. (The refusal side itself needs no separate
/// reachable case: the check compares the kind alongside topic and text, so
/// the topic tests below the hood exercise the same refusal lane.)
#[test]
fn memory_add_leaves_a_same_text_candidate_of_a_different_kind_alone() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("cross-kind-no-adoption");

    // A GUI candidate awaiting review in a different store, carrying the
    // exact text this add will submit. `recent_work` folds into
    // `current_focus` at the pending stage, so the row is genuinely of
    // another kind than this preference add.
    let gui = enqueue_fixture_with_topic("recent_work", "", "Prefer concise answers");
    assert_eq!(gui.kind, "current_focus", "{gui:?}");

    // The add confirms its OWN candidate only: it succeeds (the current-focus
    // row is a different memory, not this add's), the foreign candidate stays
    // unreviewed, and nothing is written through it.
    run_ok(&[
        "pinvou",
        "memory",
        "add",
        "preference",
        "--content",
        "Prefer concise answers",
    ]);

    let pending = pinvou3_lib::features::memory::load_pending_memory().unwrap();
    let gui_row = pending
        .iter()
        .find(|item| item.id == gui.id)
        .unwrap_or_else(|| panic!("pending fixture {} missing: {pending:?}", gui.id));
    assert_eq!(gui_row.status, "pending_confirm", "{pending:?}");
    assert_eq!(gui_row.kind, "current_focus", "{pending:?}");
    assert_eq!(gui_row.topic, "", "{pending:?}");

    // Nothing was written through the foreign row: the timed stores stay
    // empty and the preference bucket holds only this add's own text.
    assert!(
        pinvou3_lib::features::memory::load_current_focus()
            .unwrap()
            .is_empty()
    );
    let stored = pinvou3_lib::features::memory::list_preferences().unwrap();
    assert_eq!(stored.len(), 1, "{stored:?}");
    assert_eq!(stored[0].text, "Prefer concise answers", "{stored:?}");
    assert_eq!(stored[0].topic, "answer_style", "{stored:?}");
}
