//! Contract tests for the `projects` family (GUI-parity project).
//!
//! Parse-level coverage runs against the typed command tree. Execute-level
//! coverage is strictly hermetic: every test runs against a throwaway
//! `PINVOU3_HOME` (ENV_LOCK serialization, same pattern as
//! `personas_contract.rs`) and touches no network, model, engine, or display.
//! Move fixtures build a session through `SessionStore::boot()` in the test
//! process — the same standalone constructor the CLI uses.
//!
//! The projects store is a single JSON file under `~/.pinvou3/projects/`
//! shared with the GUI app, so assertions locate projects by the exact id a
//! test created and never assume anything about other stores.

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
            "pinvou-cli-projects-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let previous = std::env::var_os("PINVOU3_HOME");
        // SAFETY: the caller holds ENV_LOCK for the whole test, so env writes
        // are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
        Self { previous, root }
    }

    /// `~/.pinvou3/projects/projects.json` — the single projects store file.
    fn store_file(&self) -> PathBuf {
        self.root.join("projects").join("projects.json")
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

/// Creates an absolute, existing directory to use as a project root.
fn make_root_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "pinvou-cli-projects-root-{label}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ── parse-level coverage ────────────────────────────────────────────────────

#[test]
fn every_projects_subcommand_parses_and_invalid_usage_exits_two() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let valid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "projects", "list"],
        vec!["pinvou", "projects", "create", "--name", "N"],
        vec![
            "pinvou", "projects", "create", "--name", "N", "--root", "/tmp/a",
        ],
        vec![
            "pinvou", "projects", "create", "--name", "N", "--root", "/tmp/a", "--root", "/tmp/b",
        ],
        vec!["pinvou", "projects", "update", "prj-1"],
        vec!["pinvou", "projects", "update", "prj-1", "--name", "N2"],
        vec!["pinvou", "projects", "update", "prj-1", "--root", "/tmp/a"],
        vec!["pinvou", "projects", "delete", "prj-1"],
        vec!["pinvou", "projects", "delete", "prj-1", "--yes"],
        vec!["pinvou", "projects", "move", "s-1", "prj-1"],
    ];
    for arguments in &valid {
        let parsed = parse_args(arguments.clone())
            .unwrap_or_else(|error| panic!("{arguments:?} must parse: {error}"));
        // The Projects command tree is dispatched exclusively through the
        // Projects variant; assert family + subcommand through the derived
        // Debug form (the types are not nameable from integration tests).
        let debug = format!("{:?}", parsed.command());
        let variant = match arguments[2] {
            "list" => "List",
            "create" => "Create",
            "update" => "Update",
            "delete" => "Delete",
            "move" => "Move",
            other => panic!("unmapped subcommand {other}"),
        };
        assert!(
            debug.starts_with("Projects("),
            "{arguments:?} did not render a Projects debug"
        );
        assert!(
            debug.contains(variant),
            "{arguments:?} did not mention the {variant} subcommand"
        );
    }

    let invalid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "projects"],
        vec!["pinvou", "projects", "bogus"],
        // list accepts no options at all.
        vec!["pinvou", "projects", "list", "--extra"],
        // create requires --name; --root is repeatable but each needs a value.
        vec!["pinvou", "projects", "create"],
        vec!["pinvou", "projects", "create", "--root", "/tmp/a"],
        vec!["pinvou", "projects", "create", "--name", ""],
        vec!["pinvou", "projects", "create", "--name", "   "],
        vec!["pinvou", "projects", "create", "--name", "N", "--root"],
        vec![
            "pinvou", "projects", "create", "--name", "N", "--root", "--name",
        ],
        vec!["pinvou", "projects", "create", "--name", "N", "--name", "M"],
        vec!["pinvou", "projects", "create", "--name", "N", "--bogus"],
        // update requires an id and only knows the two field flags.
        vec!["pinvou", "projects", "update"],
        vec!["pinvou", "projects", "update", "prj-1", "--bogus"],
        vec!["pinvou", "projects", "update", "prj-1", "--name"],
        // delete requires an id; --yes is a boolean flag.
        vec!["pinvou", "projects", "delete"],
        vec!["pinvou", "projects", "delete", "prj-1", "--yes", "--yes"],
        vec!["pinvou", "projects", "delete", "prj-1", "--yes", "1"],
        // move needs both ids and accepts no options.
        vec!["pinvou", "projects", "move"],
        // an omitted project id is the ungroup form; an empty or
        // flag-shaped project token is still malformed
        vec!["pinvou", "projects", "move", "s-1", ""],
        vec!["pinvou", "projects", "move", "s-1", "--json"],
        vec!["pinvou", "projects", "move", "s-1", "prj-1", "extra"],
        vec!["pinvou", "projects", "move", "s-1", "prj-1", "--yes"],
    ];
    for arguments in &invalid {
        let error = parse_args(arguments.clone()).expect_err(&arguments.join(" "));
        assert_eq!(error.exit_code(), ExitCode::Usage, "{arguments:?}");
    }
}

#[test]
fn json_output_mode_flows_through_every_projects_subcommand() {
    // Parse-level assertion only: no PINVOU3_HOME mutation needed here.
    for arguments in [
        vec!["pinvou", "projects", "list"],
        vec!["pinvou", "projects", "create", "--name", "N"],
        vec!["pinvou", "projects", "update", "prj-1", "--name", "N"],
        vec!["pinvou", "projects", "delete", "prj-1"],
        vec!["pinvou", "projects", "move", "s-1", "prj-1"],
    ] {
        let mut owned = arguments.clone();
        owned.push("--output");
        owned.push("json");
        let parsed = parse_args(owned).unwrap();
        assert_eq!(parsed.output(), OutputMode::Json, "{arguments:?}");
    }
}

// ── execute-level coverage (pure storage, temp PINVOU3_HOME) ───────────────

#[test]
fn projects_list_zero_state_is_empty_and_leaves_no_store_file() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("zero-state");

    let outcome = run(&["pinvou", "projects", "list"]).expect("list must execute");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(
        outcome.stdout.is_empty(),
        "the zero-state human list prints no rows"
    );

    let value = run_json(&["pinvou", "projects", "list"]);
    assert!(
        value["projects"].as_array().unwrap().is_empty(),
        "the zero-state project list must be empty"
    );
    assert!(
        value["assignments"].as_object().unwrap().is_empty(),
        "the zero-state assignment map must be empty"
    );
    assert!(
        !home.store_file().exists(),
        "the zero state must not create the store file"
    );
}

#[test]
fn projects_create_list_update_delete_round_trip_persists_the_store() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("crud-round-trip");

    // create — generated prj- id, GUI list-DTO fields, no roots.
    let value = run_json(&["pinvou", "projects", "create", "--name", "Alpha"]);
    assert_eq!(value["name"], "Alpha");
    assert_eq!(value["position"], 0);
    assert_eq!(value["assigned_session_count"], 0);
    assert!(
        value["roots"].as_array().unwrap().is_empty(),
        "a project without --root is a pure label"
    );
    assert!(
        value["created_at"].as_str().unwrap().len() >= 20,
        "created_at must serialize as a timestamp string"
    );
    assert_eq!(value["created_at"], value["updated_at"]);
    let id = value["id"].as_str().unwrap().to_owned();
    assert!(
        id.starts_with("prj-"),
        "project ids must use the GUI prj- scheme"
    );
    assert!(
        home.store_file().is_file(),
        "create must persist the store file"
    );

    // list shows the created project with stable field names.
    let value = run_json(&["pinvou", "projects", "list"]);
    let entries = value["projects"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "exactly one project must be listed");
    assert_eq!(entries[0]["id"], id);
    assert_eq!(entries[0]["name"], "Alpha");
    assert!(
        value["assignments"].as_object().is_some(),
        "assignments must stay an object"
    );
    let outcome = run(&["pinvou", "projects", "list"]).expect("human list");
    let first = outcome.stdout.lines().next().unwrap_or_default();
    let columns: Vec<&str> = first.split('\t').collect();
    assert_eq!(columns.len(), 4, "projects list row layout changed");
    assert_eq!(columns[0], id);
    assert_eq!(columns[1], "Alpha");

    // update --name overlays only the name.
    let value = run_json(&["pinvou", "projects", "update", &id, "--name", "Beta"]);
    assert_eq!(value["id"], id);
    assert_eq!(value["name"], "Beta");
    assert_eq!(value["position"], 0, "position must be untouched");
    let value = run_json(&["pinvou", "projects", "list"]);
    assert_eq!(value["projects"][0]["name"], "Beta");

    // update --root replaces the root list; availability reflects the disk.
    let root_dir = make_root_dir("crud");
    let value = run_json(&[
        "pinvou",
        "projects",
        "update",
        &id,
        "--root",
        root_dir.to_str().unwrap(),
    ]);
    let roots = value["roots"].as_array().unwrap();
    assert_eq!(roots.len(), 1, "one root must be stored");
    assert_eq!(roots[0]["available"], true);
    let root_name = root_dir.file_name().unwrap().to_str().unwrap().to_owned();
    let stored = roots[0]["path"].as_str().unwrap().to_owned();
    assert!(
        stored.ends_with(&root_name),
        "the stored root must keep the fixture directory name"
    );
    assert_eq!(value["name"], "Beta", "name must be untouched by --root");

    // Unknown ids are clean host failures.
    let error = run(&["pinvou", "projects", "update", "prj-missing", "--name", "X"])
        .expect_err("updating an unknown project must fail");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("project not found"),
        "an unknown project must be named in the failure"
    );

    // delete without --yes is refused before any store access.
    let error =
        run(&["pinvou", "projects", "delete", &id]).expect_err("delete without --yes must refuse");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(
        error.to_string().contains("--yes"),
        "the refusal must point at the --yes gate"
    );
    let value = run_json(&["pinvou", "projects", "list"]);
    assert_eq!(value["projects"][0]["id"], id, "the project must survive");

    // delete --yes removes the project everywhere; the empty state deletes
    // the store file again.
    let value = run_json(&["pinvou", "projects", "delete", &id, "--yes"]);
    assert_eq!(value["id"], id);
    assert_eq!(value["action"], "deleted");
    assert!(
        value["affected_session_ids"].as_array().unwrap().is_empty(),
        "no session was assigned"
    );
    assert!(
        !home.store_file().exists(),
        "the empty store must not keep a file"
    );
    let value = run_json(&["pinvou", "projects", "list"]);
    assert!(
        value["projects"].as_array().unwrap().is_empty(),
        "the deleted project must be gone"
    );
    let error = run(&["pinvou", "projects", "delete", &id, "--yes"])
        .expect_err("deleting an unknown project must fail");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("project not found"),
        "an unknown project must be named in the failure"
    );

    std::fs::remove_dir_all(&root_dir).ok();
}

#[test]
fn projects_root_validation_errors_are_clean_failures() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("root-validation");

    // Relative roots are rejected by the feature's own validation.
    let error = run(&[
        "pinvou",
        "projects",
        "create",
        "--name",
        "Bad",
        "--root",
        "relative/path",
    ])
    .expect_err("a relative root must be rejected");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("must be absolute"),
        "the relative root must be named in the failure"
    );

    // Roots that nest inside each other are rejected (auto-grouping would
    // otherwise be ambiguous).
    let parent = make_root_dir("nest-parent");
    let child = parent.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let error = run(&[
        "pinvou",
        "projects",
        "create",
        "--name",
        "Nested",
        "--root",
        parent.to_str().unwrap(),
        "--root",
        child.to_str().unwrap(),
    ])
    .expect_err("nested roots must be rejected");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("must not nest"),
        "the nested roots must be named in the failure"
    );

    // Failed validation never mutates the store.
    let value = run_json(&["pinvou", "projects", "list"]);
    assert!(
        value["projects"].as_array().unwrap().is_empty(),
        "rejected creates must not leave a project behind"
    );

    std::fs::remove_dir_all(&parent).ok();
}

#[test]
fn projects_move_assigns_sessions_and_reports_unknowns() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("move");
    let session_id = create_session_fixture();

    let value = run_json(&["pinvou", "projects", "create", "--name", "Inbox"]);
    let project_id = value["id"].as_str().unwrap().to_owned();

    // move returns the outcome fields and persists the assignment.
    let value = run_json(&["pinvou", "projects", "move", &session_id, &project_id]);
    assert_eq!(value["session_id"], session_id);
    assert_eq!(value["project_id"], project_id);
    assert!(
        value.get("added_root").is_some_and(|root| root.is_null()),
        "a headless move never adds roots"
    );
    let outcome =
        run(&["pinvou", "projects", "move", &session_id, &project_id]).expect("human move");
    assert!(
        outcome.stdout.contains("moved"),
        "human move should confirm the move"
    );

    // list reflects the assignment map and the member count.
    let value = run_json(&["pinvou", "projects", "list"]);
    assert_eq!(value["assignments"][session_id.as_str()], project_id);
    assert_eq!(value["projects"][0]["assigned_session_count"], 1);

    // An unknown session is a clean failure that writes nothing.
    let error = run(&["pinvou", "projects", "move", "no-such-session", &project_id])
        .expect_err("an unknown session must fail");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("does not exist"),
        "the unknown session must be named in the failure"
    );
    let value = run_json(&["pinvou", "projects", "list"]);
    assert!(
        value["assignments"].get("no-such-session").is_none(),
        "a failed move must not write an assignment"
    );

    // An unknown target project is a clean failure too.
    let error = run(&["pinvou", "projects", "move", &session_id, "prj-missing"])
        .expect_err("an unknown project must fail");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("project not found"),
        "an unknown project must be named in the failure"
    );

    // Session ids join onto store paths, so traversal attempts are usage
    // errors, never writes outside the sessions root.
    let error = run(&["pinvou", "projects", "move", "../escape", &project_id])
        .expect_err("a path-traversal session id must be rejected");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(
        error.to_string().contains("valid session id"),
        "the traversal id must be rejected as a usage error"
    );

    // delete reports the affected assignment and never deletes sessions.
    let value = run_json(&["pinvou", "projects", "delete", &project_id, "--yes"]);
    let affected = value["affected_session_ids"].as_array().unwrap();
    assert!(
        affected
            .iter()
            .any(|entry| entry.as_str() == Some(session_id.as_str())),
        "delete must report the unassigned session"
    );
    let value = run_json(&["pinvou", "projects", "list"]);
    assert!(
        value["assignments"].get(session_id.as_str()).is_none(),
        "the assignment entry must be gone"
    );
    assert!(
        home.sessions_root()
            .join(format!("{session_id}.json"))
            .is_file(),
        "project delete must never delete the session"
    );

    // Omitting the project id moves a session out of its project — the
    // store's ungroup arm, matching the GUI picker's ungrouped entry.
    let value = run_json(&["pinvou", "projects", "create", "--name", "Temp"]);
    let project_id = value["id"].as_str().unwrap().to_owned();
    let value = run_json(&["pinvou", "projects", "move", &session_id, &project_id]);
    assert_eq!(value["project_id"], project_id);
    let outcome = run(&["pinvou", "projects", "move", &session_id]).expect("human ungroup");
    assert!(
        outcome.stdout.contains("out of its project"),
        "{}",
        outcome.stdout
    );
    let value = run_json(&[
        "pinvou",
        "projects",
        "move",
        &session_id,
        "--output",
        "json",
    ]);
    assert_eq!(value["session_id"], session_id);
    assert!(
        value["project_id"].is_null(),
        "a repeat ungroup stays idempotent: {value}"
    );
    let value = run_json(&["pinvou", "projects", "list"]);
    let assignment = value["assignments"].get(session_id.as_str());
    assert!(
        assignment.is_none() || assignment == Some(&serde_json::Value::Null),
        "the ungroup must clear the assignment: {value}"
    );
    let error = run(&["pinvou", "projects", "move", "no-such-session"])
        .expect_err("an unknown session is still a failure without a project id");
    assert!(error.to_string().contains("does not exist"), "{}", error);
}
