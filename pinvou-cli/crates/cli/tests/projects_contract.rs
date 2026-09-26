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
use pinvou3_lib::features::codex_acp::{CodexWorkspaceKind, SessionAgentStore};
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
        // the ungroup form parses with and without --yes; the exit-2
        // confirmation gate runs at execute time (support::require_yes),
        // the same unconfirmed-but-parseable contract as sessions delete.
        vec!["pinvou", "projects", "move", "s-1"],
        vec!["pinvou", "projects", "move", "s-1", "--yes"],
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
        // --yes is only a flag on the ungroup form; between the two ids it
        // is positional garbage.
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
    // store's ungroup arm, matching the GUI picker's ungrouped entry. That
    // entry is an EXPLICIT, irreversible opt-out of auto-grouping, not a
    // "clear", so it is only reachable for a session that currently resolves
    // to a project — the same condition the GUI's disabled button applies.
    //
    // The session created by the fixture has no workspace binding and no
    // assignment, so it resolves to nothing: ungrouping it is refused BEFORE
    // it is put into a project. --yes is supplied so this arm asserts the
    // resolution gate itself; the no-confirmation form is covered further
    // down, after the session is put into a project.
    let error = run(&["pinvou", "projects", "move", &session_id, "--yes"])
        .expect_err("an already-ungrouped session must not be pinned as explicitly ungrouped");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("nothing to move it out of"),
        "{error}"
    );
    assert!(
        error.to_string().contains("cannot be reverted"),
        "the refusal must disclose that the write it refused is irreversible: {error}"
    );
    let value = run_json(&["pinvou", "projects", "list"]);
    assert!(
        value["assignments"].get(session_id.as_str()).is_none(),
        "a refused ungroup must not write an assignment entry"
    );

    // Inside a project it resolves, so the ungroup is allowed exactly once.
    let value = run_json(&["pinvou", "projects", "create", "--name", "Temp"]);
    let project_id = value["id"].as_str().unwrap().to_owned();
    let value = run_json(&["pinvou", "projects", "move", &session_id, &project_id]);
    assert_eq!(value["project_id"], project_id);

    // --yes gate, red-first: the ungroup write is irreversible, so without
    // --yes it must be refused before any store access — the same
    // confirmation convention (and refusal copy) as projects delete /
    // sessions delete.
    let error = run(&["pinvou", "projects", "move", &session_id])
        .expect_err("an ungroup without --yes must refuse");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(
        error.to_string().contains("--yes"),
        "the refusal must point at the --yes gate: {error}"
    );
    let value = run_json(&["pinvou", "projects", "list"]);
    assert_eq!(
        value["assignments"][session_id.as_str()],
        project_id,
        "a refused ungroup must not write the explicit entry"
    );

    let outcome =
        run(&["pinvou", "projects", "move", &session_id, "--yes"]).expect("human ungroup");
    assert!(
        outcome.stdout.contains("out of its project"),
        "the human ungroup must report the session leaving its project"
    );
    let value = run_json(&["pinvou", "projects", "list"]);
    let assignment = value["assignments"].get(session_id.as_str());
    assert!(
        assignment == Some(&serde_json::Value::Null),
        "the ungroup writes the explicit entry, not an absence: {assignment:?}"
    );

    // The repeat is now refused rather than silently re-pinning: the session
    // no longer resolves to a project, which is the state the explicit entry
    // itself created. --yes present: this asserts the resolution gate, not
    // the confirmation gate.
    let error = run(&["pinvou", "projects", "move", &session_id, "--yes"])
        .expect_err("a repeat ungroup must be refused, not reported as idempotent");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("nothing to move it out of"),
        "{error}"
    );

    // Without --yes the refusal is the confirmation gate, and it fires
    // before the resolution gate can be reached at all.
    let error = run(&["pinvou", "projects", "move", &session_id])
        .expect_err("a repeat ungroup without --yes must hit the --yes gate");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(error.to_string().contains("--yes"), "{error}");

    // The only documented way back is naming a project again.
    let value = run_json(&["pinvou", "projects", "move", &session_id, &project_id]);
    assert_eq!(value["project_id"], project_id);

    // The session gate still runs before the ungroup resolution gate, so an
    // unknown session is reported as unknown rather than as "not in a
    // project" (--yes supplied: this arm asserts the session gate).
    let error = run(&["pinvou", "projects", "move", "no-such-session", "--yes"])
        .expect_err("an unknown session is still a failure without a project id");
    assert!(error.to_string().contains("does not exist"), "{}", error);

    // The ungroup asymmetry is disclosed on the move usage errors too. This one
    // is rejected at PARSE time, so it never reaches `run`'s execute step.
    let error = parse_args(&["pinvou", "projects", "move", &session_id, &project_id, "x"])
        .expect_err("a trailing token is a usage error");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(error.to_string().contains("cannot be reverted"), "{error}");
}

// ── rebind (the storage half of the GUI's rebind_workspace_root) ────────────

#[test]
fn projects_rebind_parses_two_positionals_and_the_yes_flag() {
    // Parse only: no PINVOU3_HOME mutation, so no ENV_LOCK.
    let valid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "projects", "rebind", "/tmp/a", "/tmp/b"],
        vec!["pinvou", "projects", "rebind", "/tmp/a", "/tmp/b", "--yes"],
    ];
    for arguments in &valid {
        let parsed = parse_args(arguments.clone())
            .unwrap_or_else(|error| panic!("{arguments:?} must parse: {error}"));
        let debug = format!("{:?}", parsed.command());
        assert!(
            debug.starts_with("Projects(") && debug.contains("Rebind"),
            "{arguments:?} did not parse into Projects(Rebind): {debug}"
        );
    }

    let invalid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "projects", "rebind"],
        vec!["pinvou", "projects", "rebind", "/tmp/a"],
        // A flag-shaped token before a positional is malformed, not a flag.
        vec!["pinvou", "projects", "rebind", "--yes", "/tmp/b"],
        vec!["pinvou", "projects", "rebind", "/tmp/a", "--yes"],
        vec![
            "pinvou", "projects", "rebind", "/tmp/a", "/tmp/b", "--bogus",
        ],
        vec![
            "pinvou", "projects", "rebind", "/tmp/a", "/tmp/b", "--yes", "--yes",
        ],
        vec!["pinvou", "projects", "rebind", "/tmp/a", "/tmp/b", "extra"],
    ];
    for arguments in &invalid {
        let error = parse_args(arguments.clone()).expect_err(&arguments.join(" "));
        assert_eq!(error.exit_code(), ExitCode::Usage, "{arguments:?}");
    }
}

#[test]
fn projects_rebind_migrates_roots_both_binding_lanes_and_metadata() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("rebind");
    let from_dir = make_root_dir("rebind-from");
    let to_dir = make_root_dir("rebind-to");
    // Everything the lanes compare runs in the resolved display domain, so
    // the fixtures seed and assert the canonical spelling (on macOS the temp
    // root sits behind /var → /private/var).
    let from = std::fs::canonicalize(&from_dir).unwrap();
    let to = std::fs::canonicalize(&to_dir).unwrap();

    // A project whose root sits under `from`.
    let value = run_json(&[
        "pinvou",
        "projects",
        "create",
        "--name",
        "Moved",
        "--root",
        from.to_str().unwrap(),
    ]);
    let project_id = value["id"].as_str().unwrap().to_owned();

    // Plain lane: session JSON + workspace-binding.json sidecar, both seeded
    // through the same store constructors the CLI command itself uses.
    let sessions = SessionStore::boot().expect("boot session store");
    let plain_session = sessions
        .create_new("test-model".to_owned(), None, from.clone())
        .expect("create plain session");
    let plain_id = plain_session.metadata.id;
    sessions
        .bind_session_workspace(&plain_id, from.clone())
        .expect("bind plain workspace");
    // Codex lane: the session JSON plus the agent-index record and the
    // authoritative code-session sidecar.
    let code_session = sessions
        .create_new("test-model".to_owned(), None, from.clone())
        .expect("create code session");
    let code_id = code_session.metadata.id;
    drop(sessions);
    let agents = SessionAgentStore::load_or_empty();
    agents
        .bind_code_native_session(&code_id, CodexWorkspaceKind::Project, Some(from.clone()))
        .expect("bind code session");
    drop(agents);

    let value = run_json(&[
        "pinvou",
        "projects",
        "rebind",
        from.to_str().unwrap(),
        to.to_str().unwrap(),
        "--yes",
    ]);
    let failed = value["failed_session_ids"].as_array().unwrap();
    assert!(
        failed.is_empty(),
        "a healthy two-lane rebind must not report failures: {failed:?}"
    );
    let rebound: Vec<&str> = value["rebound_session_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry.as_str())
        .collect();
    assert!(
        rebound.contains(&plain_id.as_str()) && rebound.contains(&code_id.as_str()),
        "both lanes' sessions must be reported as rebound: {rebound:?}"
    );
    assert_eq!(
        value["affected_project_ids"],
        serde_json::json!([project_id]),
        "the project whose root moved must be named"
    );

    // Project root rewritten in the store.
    let list = run_json(&["pinvou", "projects", "list"]);
    let roots = list["projects"][0]["roots"].as_array().unwrap();
    assert_eq!(roots.len(), 1, "the moved project keeps exactly one root");
    assert_eq!(roots[0]["path"].as_str().unwrap(), to.to_str().unwrap());

    // SavedSession metadata replayed for both sessions.
    for session_id in [&plain_id, &code_id] {
        let raw = std::fs::read_to_string(home.sessions_root().join(format!("{session_id}.json")))
            .expect("session record still on disk");
        assert!(
            raw.contains(to.to_str().unwrap()),
            "session {session_id} metadata must point at the new directory"
        );
    }
    // Plain sidecar moved.
    let sidecar = std::fs::read_to_string(
        home.sessions_root()
            .join(&plain_id)
            .join("workspace-binding.json"),
    )
    .expect("plain binding sidecar still on disk");
    assert!(
        sidecar.contains(to.to_str().unwrap()),
        "the workspace-binding sidecar must move: {sidecar}"
    );
    // Agent index and code-session sidecar moved.
    let record = SessionAgentStore::load_or_empty().get(&code_id);
    assert_eq!(
        record.workspace_path,
        Some(to.clone()),
        "the agent-index binding must move"
    );
    let code_sidecar = std::fs::read_to_string(
        home.sessions_root()
            .join(&code_id)
            .join("code-session.json"),
    )
    .expect("code-session sidecar still on disk");
    assert!(
        code_sidecar.contains(to.to_str().unwrap()),
        "the authoritative sidecar must move: {code_sidecar}"
    );

    // Idempotent rerun: nothing is left under `from`, so a rerun converges
    // to an honest empty report under exit 0.
    let outcome = run(&[
        "pinvou",
        "projects",
        "rebind",
        from.to_str().unwrap(),
        to.to_str().unwrap(),
        "--yes",
    ])
    .expect("the idempotent rerun must succeed");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(
        outcome
            .stdout
            .contains("rebound 0 session(s) (0 failed), updated roots of 0 project(s)"),
        "the rerun must report an empty convergence: {:?}",
        outcome.stdout
    );

    std::fs::remove_dir_all(&from_dir).ok();
    std::fs::remove_dir_all(&to_dir).ok();
}

#[test]
fn projects_rebind_without_yes_is_refused_before_any_store_access() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("rebind-yes");
    let from_dir = make_root_dir("rebind-yes-from");
    let to_dir = make_root_dir("rebind-yes-to");

    let error = run(&[
        "pinvou",
        "projects",
        "rebind",
        from_dir.to_str().unwrap(),
        to_dir.to_str().unwrap(),
    ])
    .expect_err("rebind without --yes must refuse");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(
        error.to_string().contains("--yes"),
        "the refusal must point at the --yes gate: {error}"
    );
    assert!(
        !home.store_file().exists(),
        "a refused rebind must not touch the projects store"
    );
    assert!(
        !home.root.join("session-agents.json").exists(),
        "a refused rebind must not touch the agent index"
    );
    assert!(
        !home.sessions_root().exists(),
        "a refused rebind must not create the sessions root"
    );

    std::fs::remove_dir_all(&from_dir).ok();
    std::fs::remove_dir_all(&to_dir).ok();
}

#[test]
fn projects_rebind_rejects_relative_empty_and_root_from_as_usage() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("rebind-shape");
    let to_dir = make_root_dir("rebind-shape-to");

    // One mirrored message covers empty, relative and filesystem-root `from`
    // — the same single rule the GUI command applies at its entry.
    for from in ["relative/dir", ""] {
        let error = run(&[
            "pinvou",
            "projects",
            "rebind",
            from,
            to_dir.to_str().unwrap(),
        ])
        .expect_err("a shape-invalid from must be rejected");
        assert_eq!(error.exit_code(), ExitCode::Usage, "from = {from:?}");
        assert!(
            error.to_string().contains("absolute, non-root"),
            "from = {from:?} must be named by the mirrored rule: {error}"
        );
    }
    // Filesystem root: the deepest ancestor of any absolute path (POSIX "/",
    // a Windows drive root) has no parent.
    let root = std::env::temp_dir()
        .canonicalize()
        .ok()
        .and_then(|path| {
            path.ancestors()
                .last()
                .map(|ancestor| ancestor.to_path_buf())
        })
        .expect("temp dir must have a root ancestor");
    let error = run(&[
        "pinvou",
        "projects",
        "rebind",
        root.to_str().unwrap(),
        to_dir.to_str().unwrap(),
    ])
    .expect_err("a filesystem-root from must be rejected");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(error.to_string().contains("absolute, non-root"), "{error}");

    // And `to` as the filesystem root mirrors the GUI's other entry rule.
    let from_dir = make_root_dir("rebind-shape-from");
    let error = run(&[
        "pinvou",
        "projects",
        "rebind",
        from_dir.to_str().unwrap(),
        root.to_str().unwrap(),
    ])
    .expect_err("a filesystem-root to must be rejected");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(error.to_string().contains("filesystem root"), "{error}");

    assert!(
        !home.store_file().exists(),
        "rejected arguments must not touch the projects store"
    );

    std::fs::remove_dir_all(&from_dir).ok();
    std::fs::remove_dir_all(&to_dir).ok();
}

#[test]
fn projects_rebind_from_equal_to_is_a_reported_noop() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("rebind-same");
    let dir = make_root_dir("rebind-same");
    let same = std::fs::canonicalize(&dir).unwrap();

    // Seed state that WOULD match a real rebind: if the short-circuit were
    // missing, the run would rewrite (or at least touch) all of it.
    let value = run_json(&[
        "pinvou",
        "projects",
        "create",
        "--name",
        "Same",
        "--root",
        same.to_str().unwrap(),
    ]);
    let project_id = value["id"].as_str().unwrap().to_owned();
    let sessions = SessionStore::boot().expect("boot session store");
    let session = sessions
        .create_new("test-model".to_owned(), None, same.clone())
        .expect("create session");
    let session_id = session.metadata.id;
    sessions
        .bind_session_workspace(&session_id, same.clone())
        .expect("bind workspace");
    drop(sessions);

    let value = run_json(&[
        "pinvou",
        "projects",
        "rebind",
        same.to_str().unwrap(),
        same.to_str().unwrap(),
        "--yes",
    ]);
    assert_eq!(value["rebound_session_ids"].as_array().unwrap().len(), 0);
    assert_eq!(value["failed_session_ids"].as_array().unwrap().len(), 0);
    assert_eq!(value["affected_project_ids"].as_array().unwrap().len(), 0);

    // Nothing changed anywhere.
    let list = run_json(&["pinvou", "projects", "list"]);
    assert_eq!(list["projects"][0]["id"], project_id);
    assert_eq!(
        list["projects"][0]["roots"][0]["path"].as_str().unwrap(),
        same.to_str().unwrap()
    );
    let sidecar = std::fs::read_to_string(
        home.sessions_root()
            .join(&session_id)
            .join("workspace-binding.json"),
    )
    .expect("the sidecar must be untouched");
    assert!(
        sidecar.contains(same.to_str().unwrap()),
        "the no-op must leave the binding as-is: {sidecar}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
