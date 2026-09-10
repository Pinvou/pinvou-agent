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
        std::fs::remove_dir_all(&self.root).unwrap();
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
        (&["pinvou", "memory", "organize"][..], "Memory(Organize)"),
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
    assert!(human.contains("Alice"), "{human}");
    assert!(human.contains("Pin"), "{human}");

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
    let items: serde_json::Value = serde_json::from_str(&json).unwrap();
    let items = items.as_array().expect("json array of preferences");
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
    let items = items.as_array().unwrap();
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

#[test]
fn memory_archive_marks_recent_work_fixture_archived() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _home = TempHome::new("archive");

    let item = pinvou3_lib::features::memory::upsert_recent_work(
        pinvou3_lib::features::memory::RecentWorkPatch {
            id: None,
            title: "Shipped CLI memory parity".to_owned(),
            summary: Some("memory family contract".to_owned()),
            source: Some("contract-test".to_owned()),
            ttl_days: None,
        },
    )
    .unwrap();

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
    pinvou3_lib::features::memory::upsert_recent_work(
        pinvou3_lib::features::memory::RecentWorkPatch {
            id: None,
            title: "Shipped CLI memory parity".to_owned(),
            summary: None,
            source: Some("contract-test".to_owned()),
            ttl_days: None,
        },
    )
    .unwrap();

    let json = run_ok(&["pinvou", "memory", "overview", "--output", "json"]);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["preferences"].as_array().unwrap().len(), 1, "{value}");
    assert_eq!(value["pending"].as_array().unwrap().len(), 2, "{value}");
    assert_eq!(value["recent_work"].as_array().unwrap().len(), 1, "{value}");
    assert_eq!(value["current_focus"].as_array().unwrap().len(), 0);
    assert_eq!(value["recent_activity"].as_array().unwrap().len(), 0);
    assert_eq!(value["never"].as_array().unwrap().len(), 0);
    // all authoritative sources available: the snapshot document was refreshed
    assert!(
        !value["snapshot_path"].as_str().unwrap().is_empty(),
        "{value}"
    );
    assert_eq!(value["sources"]["preferences"]["available"], true);
    assert_eq!(value["sources"]["runtime"]["available"], true);
    assert_eq!(value["warnings"].as_array().unwrap().len(), 0);

    let human = run_ok(&["pinvou", "memory", "overview"]);
    assert!(human.contains("Preferences: 1"), "{human}");
    assert!(human.contains("Pending: 2"), "{human}");
    assert!(human.contains("Recent work: 1"), "{human}");
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
#[ignore = "requires a display (xvfb) and a configured model; run `pinvou memory organize` instead"]
#[test]
fn memory_organize_is_the_opt_in_host_and_model_path() {
    // No host invocation in tests. This body only documents the contract; the
    // parse-level tests above prove `memory organize` parses and dispatches.
}
