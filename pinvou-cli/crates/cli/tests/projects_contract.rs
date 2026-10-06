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
        vec!["pinvou", "projects", "update", "prj-1", "--clear-roots"],
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
        // --clear-roots and --root values are exclusive by construction:
        // bare --root absence means "keep the current roots".
        vec![
            "pinvou",
            "projects",
            "update",
            "prj-1",
            "--root",
            "/tmp/a",
            "--clear-roots",
        ],
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
    // Seed the ACP state the GUI's spawn path writes (boot recovery reads
    // `workspace.path` BEFORE the workspace baseline, so an untranslated
    // state file resurrects the vanished root if the agent index is later
    // lost). A regression deleting the rebind's storage-half translate call
    // strands this at the old path.
    std::fs::create_dir_all(home.sessions_root().join(&code_id)).unwrap();
    std::fs::write(
        home.sessions_root().join(&code_id).join("acp-state.json"),
        serde_json::json!({ "workspace": { "path": from.to_string_lossy() } }).to_string(),
    )
    .unwrap();
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
    // ACP state translated (GUI parity, `translate_acp_state_workspace`).
    let acp_state =
        std::fs::read_to_string(home.sessions_root().join(&code_id).join("acp-state.json"))
            .expect("acp-state file still on disk");
    assert!(
        acp_state.contains(to.to_str().unwrap()),
        "the acp-state workspace.path must move: {acp_state}"
    );
    // Round-39 review: arm 1 (a from-lane moved code session) must recapture
    // the workspace baseline exactly like the round-38 arm-3 pin below does —
    // a stale baseline pointing into the vanished root is the boot-recovery
    // fallback. Deleting the `code_rebound_ids.contains` arm from the
    // recapture gate fails this assertion while the rest of the suite stays
    // green.
    let baseline = std::fs::read_to_string(
        home.sessions_root()
            .join(&code_id)
            .join("codex-workspace-baseline.json"),
    )
    .expect("a from-lane moved code session must recapture its workspace baseline");
    assert!(
        baseline.contains(to.to_str().unwrap()),
        "the arm-1 recaptured baseline must name the new root: {baseline}"
    );
    assert!(
        !baseline.contains(from.to_str().unwrap()),
        "the arm-1 recaptured baseline must not name the vanished source: {baseline}"
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
fn projects_rebind_rerun_converges_a_half_migrated_session() {
    // Round-22 review MAJOR: the report copy promises "a rerun converges
    // them" for failed sessions, but the metadata replay was driven only by
    // this run's lane outcomes. A session whose binding lanes moved in run 1
    // while its `set_workspace` failed (the corrupt-JSON / transient-EACCES
    // class the loop itself classifies as retryable) left the rerun with
    // empty lane outcomes: `rebound 0 (0 failed)`, exit 0, stale metadata
    // pointing at the vanished `from` directory. The to-lane retry pass —
    // the GUI's `admit_rebind_retry_candidate` scan — is what converges it.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("rebind-retry");
    let from_dir = make_root_dir("rebind-retry-from");
    let to_dir = make_root_dir("rebind-retry-to");
    let from = std::fs::canonicalize(&from_dir).unwrap();
    let to = std::fs::canonicalize(&to_dir).unwrap();

    // Seed the exact state a failed run-1 leaves behind: the SavedSession
    // metadata still names `from` (the write that failed), while both
    // binding lanes already sit under `to` (the writes that succeeded).
    // Nothing is left under `from`, so the lane batches of the rerun return
    // empty and only the retry pass can see the session.
    let sessions = SessionStore::boot().expect("boot session store");
    let plain_session = sessions
        .create_new("test-model".to_owned(), None, from.clone())
        .expect("create plain session");
    let plain_id = plain_session.metadata.id;
    sessions
        .bind_session_workspace(&plain_id, to.clone())
        .expect("move the plain binding to to");
    let code_session = sessions
        .create_new("test-model".to_owned(), None, from.clone())
        .expect("create code session");
    let code_id = code_session.metadata.id;
    drop(sessions);
    let agents = SessionAgentStore::load_or_empty();
    agents
        .bind_code_native_session(&code_id, CodexWorkspaceKind::Project, Some(to.clone()))
        .expect("move the code binding to to");
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
        "a converged rerun must not report failures: {failed:?}"
    );
    let rebound: Vec<&str> = value["rebound_session_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry.as_str())
        .collect();
    assert!(
        rebound.contains(&plain_id.as_str()) && rebound.contains(&code_id.as_str()),
        "both half-migrated sessions must be reported as rebound: {rebound:?}"
    );

    // The stale metadata actually converged to the destination.
    for session_id in [&plain_id, &code_id] {
        let raw = std::fs::read_to_string(home.sessions_root().join(format!("{session_id}.json")))
            .expect("session record still on disk");
        assert!(
            raw.contains(to.to_str().unwrap()),
            "session {session_id} metadata must be replayed to the new directory"
        );
        assert!(
            !raw.contains(from.to_str().unwrap()),
            "session {session_id} metadata must not name the vanished source"
        );
    }

    // Round-38 review MAJOR: the code session is a TO-LANE admittee (its
    // pre-sync metadata names exactly the rerun's `from`), so the reverse-
    // mapped metadata-behind-binding arm reads false and only the GUI's
    // round-19 SF-2 arm 3 can order the baseline recapture. Without it the
    // stale `codex-workspace-baseline.json` keeps pointing into the vanished
    // root as the boot-recovery fallback.
    let baseline = std::fs::read_to_string(
        home.sessions_root()
            .join(&code_id)
            .join("codex-workspace-baseline.json"),
    )
    .expect("the code admittee's workspace baseline must be recaptured");
    assert!(
        baseline.contains(to.to_str().unwrap()),
        "the recaptured baseline must name the new root: {baseline}"
    );
    assert!(
        !baseline.contains(from.to_str().unwrap()),
        "the recaptured baseline must not name the vanished source: {baseline}"
    );
    // Round-21 SF-4's stray-file rule: a plain-chat admittee in neither
    // plain set must not gain a codex baseline it never reads.
    assert!(
        !home
            .sessions_root()
            .join(&plain_id)
            .join("codex-workspace-baseline.json")
            .exists(),
        "a plain-chat admittee must not gain a codex baseline"
    );

    std::fs::remove_dir_all(&from_dir).ok();
    std::fs::remove_dir_all(&to_dir).ok();
}

#[test]
fn projects_rebind_rerun_translates_storage_behind_the_binding() {
    // Round-36 review MAJOR (the GUI's round-26 MAJOR-1): run 1 can move the
    // binding lanes and still fail its storage/metadata passes, leaving the
    // session's artifacts and acp-state on run 1's target while the binding
    // already sits on run 1's destination. A run 2 whose `from` is THAT
    // destination must translate the two storage halves with the session's
    // PRE-sync metadata workspace — the run-global pair maps neither half,
    // both persist nothing, and the run would claim Rebound over dead paths.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("rebind-behind-binding");
    let origin_dir = make_root_dir("rebind-behind-origin");
    let mid_dir = make_root_dir("rebind-behind-mid");
    let to_dir = make_root_dir("rebind-behind-to");
    let origin = std::fs::canonicalize(&origin_dir).unwrap();
    let mid = std::fs::canonicalize(&mid_dir).unwrap();
    let to = std::fs::canonicalize(&to_dir).unwrap();

    // Seed the exact state a failed run 1 (origin→mid) leaves behind: the
    // binding lane moved to `mid`, but the metadata, the artifact storage
    // path and the acp-state workspace all stayed on `origin`.
    let sessions = SessionStore::boot().expect("boot session store");
    let stranded = sessions
        .create_new("test-model".to_owned(), None, origin.clone())
        .expect("create stranded session");
    let stranded_id = stranded.metadata.id;
    sessions
        .update_artifacts(
            &stranded_id,
            vec![origin.join("report.md").to_string_lossy().into_owned()],
        )
        .expect("seed the stranded artifact storage path");
    sessions
        .bind_session_workspace(&stranded_id, mid.clone())
        .expect("move the binding lane to mid");
    drop(sessions);
    std::fs::create_dir_all(home.sessions_root().join(&stranded_id)).unwrap();
    std::fs::write(
        home.sessions_root()
            .join(&stranded_id)
            .join("acp-state.json"),
        serde_json::json!({ "workspace": { "path": origin.to_string_lossy() } }).to_string(),
    )
    .unwrap();

    // Run 2 (mid→to): nothing is left under `mid` except this binding, so
    // only the per-session pre-sync geometry can reach the `origin` paths.
    let value = run_json(&[
        "pinvou",
        "projects",
        "rebind",
        mid.to_str().unwrap(),
        to.to_str().unwrap(),
        "--yes",
    ]);
    let failed = value["failed_session_ids"].as_array().unwrap();
    assert!(
        failed.is_empty(),
        "the stranded session must converge: {failed:?}"
    );
    let rebound: Vec<&str> = value["rebound_session_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry.as_str())
        .collect();
    assert!(
        rebound.contains(&stranded_id.as_str()),
        "the stranded session must be reported as rebound: {rebound:?}"
    );

    // Every stored path must now name the new destination and none may name
    // the vanished origin: with the run-global pair only, both storage
    // halves persist nothing and the record keeps its `origin` paths (red).
    let raw = std::fs::read_to_string(home.sessions_root().join(format!("{stranded_id}.json")))
        .expect("session record still on disk");
    assert!(
        !raw.contains(origin.to_str().unwrap()),
        "artifact storage paths must be rebased off the vanished origin: {raw}"
    );
    assert!(
        raw.contains(to.to_str().unwrap()),
        "the record must carry the new destination: {raw}"
    );
    let acp_state = std::fs::read_to_string(
        home.sessions_root()
            .join(&stranded_id)
            .join("acp-state.json"),
    )
    .expect("acp-state file still on disk");
    assert!(
        acp_state.contains(to.to_str().unwrap()) && !acp_state.contains(origin.to_str().unwrap()),
        "the acp-state workspace.path must translate off the origin: {acp_state}"
    );
    let sidecar = std::fs::read_to_string(
        home.sessions_root()
            .join(&stranded_id)
            .join("workspace-binding.json"),
    )
    .expect("plain binding sidecar still on disk");
    assert!(
        sidecar.contains(to.to_str().unwrap()),
        "the binding sidecar must move: {sidecar}"
    );

    std::fs::remove_dir_all(&origin_dir).ok();
    std::fs::remove_dir_all(&mid_dir).ok();
    std::fs::remove_dir_all(&to_dir).ok();
}

#[test]
fn projects_rebind_translates_storage_for_a_session_bound_under_a_subpath() {
    // Round-37 review BLOCKER: a session whose binding is a PROPER SUBPATH
    // of the moved root (`from/sub`, not `from` itself) must get its
    // artifact and acp-state storage halves translated onto its own
    // translated binding (`to/sub`), the way the GUI's per-session closure
    // does — not onto the destination root (`to`). A healthy session (its
    // metadata already matches its pre-move binding) must also NOT be
    // classified metadata-behind-binding: the lane outcomes carry the
    // already-translated binding, so the GUI's stale-metadata detection
    // only holds when it runs against the PRE-translation binding.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("rebind-subpath");
    let from_dir = make_root_dir("rebind-subpath-from");
    let to_dir = make_root_dir("rebind-subpath-to");
    let from = std::fs::canonicalize(&from_dir).unwrap();
    let to = std::fs::canonicalize(&to_dir).unwrap();
    let from_sub = from.join("sub");
    let to_sub = to.join("sub");
    std::fs::create_dir_all(&from_sub).unwrap();
    let artifact_on_from = from_sub.join("artifacts").join("report.md");
    let artifact_on_to_sub = to_sub.join("artifacts").join("report.md");
    let artifact_on_to_root = to.join("artifacts").join("report.md");

    // A healthy session bound at the subpath: metadata, artifact storage
    // and acp-state all name `from/sub` (nothing is stranded — this is the
    // plain moved-project shape, the most common rebind input).
    let sessions = SessionStore::boot().expect("boot session store");
    let session = sessions
        .create_new("test-model".to_owned(), None, from_sub.clone())
        .expect("create subpath session");
    let session_id = session.metadata.id;
    sessions
        .update_artifacts(
            &session_id,
            vec![artifact_on_from.to_string_lossy().into_owned()],
        )
        .expect("seed the artifact storage path");
    sessions
        .bind_session_workspace(&session_id, from_sub.clone())
        .expect("bind the session at the subpath");
    drop(sessions);
    std::fs::create_dir_all(home.sessions_root().join(&session_id)).unwrap();
    std::fs::write(
        home.sessions_root()
            .join(&session_id)
            .join("acp-state.json"),
        serde_json::json!({ "workspace": { "path": from_sub.to_string_lossy() } }).to_string(),
    )
    .unwrap();

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
        "the subpath session must rebind cleanly: {failed:?}"
    );
    let rebound: Vec<&str> = value["rebound_session_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry.as_str())
        .collect();
    assert!(
        rebound.contains(&session_id.as_str()),
        "the subpath session must be reported as rebound: {rebound:?}"
    );

    // Every stored path must name the session's own translated subpath
    // destination and none may survive on the vanished origin. Translating
    // onto the destination ROOT (the round-37 bug) loses the `sub`
    // component: the record carries `to/artifacts/report.md` instead.
    let raw = std::fs::read_to_string(home.sessions_root().join(format!("{session_id}.json")))
        .expect("session record still on disk");
    assert!(
        raw.contains(artifact_on_to_sub.to_str().unwrap()),
        "artifact storage paths must rebase onto the translated subpath: {raw}"
    );
    assert!(
        !raw.contains(artifact_on_to_root.to_str().unwrap()),
        "artifact storage paths must not be flattened onto the destination root: {raw}"
    );
    assert!(
        !raw.contains(from_sub.to_str().unwrap()),
        "no record path may name the vanished subpath origin: {raw}"
    );
    let acp_state = std::fs::read_to_string(
        home.sessions_root()
            .join(&session_id)
            .join("acp-state.json"),
    )
    .expect("acp-state file still on disk");
    assert!(
        acp_state.contains(to_sub.to_str().unwrap())
            && !acp_state.contains(from_sub.to_str().unwrap()),
        "the acp-state workspace.path must translate onto the translated subpath: {acp_state}"
    );

    std::fs::remove_dir_all(&from_dir).ok();
    std::fs::remove_dir_all(&to_dir).ok();
    let _ = home;
}

#[test]
fn projects_rebind_to_lane_healthy_sessions_are_not_reported() {
    // The retry pass must admit only metadata that LAGS its binding: a
    // session created directly under `to` whose metadata already matches is
    // healthy, and reporting it would be a fabricated "rebound" entry.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("rebind-healthy");
    let from_dir = make_root_dir("rebind-healthy-from");
    let to_dir = make_root_dir("rebind-healthy-to");
    let from = std::fs::canonicalize(&from_dir).unwrap();
    let to = std::fs::canonicalize(&to_dir).unwrap();

    let sessions = SessionStore::boot().expect("boot session store");
    let healthy = sessions
        .create_new("test-model".to_owned(), None, to.clone())
        .expect("create a healthy to-lane session");
    let healthy_id = healthy.metadata.id;
    sessions
        .bind_session_workspace(&healthy_id, to.clone())
        .expect("bind the healthy session under to");
    drop(sessions);

    let value = run_json(&[
        "pinvou",
        "projects",
        "rebind",
        from.to_str().unwrap(),
        to.to_str().unwrap(),
        "--yes",
    ]);
    let rebound: Vec<&str> = value["rebound_session_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry.as_str())
        .collect();
    let failed: Vec<&str> = value["failed_session_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry.as_str())
        .collect();
    assert!(
        !rebound.contains(&healthy_id.as_str()) && !failed.contains(&healthy_id.as_str()),
        "a healthy to-lane session must not be re-reported: rebound {rebound:?}, failed {failed:?}"
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

/// The GUI's mirror geometry (#463 round-17 MAJOR-2): `from` strictly inside
/// `to` deepens a `from/sub` binding one level per rerun and is false-failed
/// by run 1's fence rescan, so the CLI refuses it with the same typed marker
/// instead of performing a rebind the desktop app would have rejected. The
/// refusal must leave the project root and the session sidecar untouched.
#[test]
fn projects_rebind_refuses_from_strictly_inside_to() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("rebind-mirror-nested");
    let dir = make_root_dir("rebind-mirror-nested");
    let outer = std::fs::canonicalize(&dir).unwrap();
    let inner = outer.join("B");
    std::fs::create_dir_all(&inner).unwrap();

    let value = run_json(&[
        "pinvou",
        "projects",
        "create",
        "--name",
        "Mirror",
        "--root",
        outer.to_str().unwrap(),
    ]);
    let project_id = value["id"].as_str().unwrap().to_owned();
    let sessions = SessionStore::boot().expect("boot session store");
    let session = sessions
        .create_new("test-model".to_owned(), None, inner.clone())
        .expect("create session");
    let session_id = session.metadata.id;
    sessions
        .bind_session_workspace(&session_id, inner.clone())
        .expect("bind workspace");
    drop(sessions);

    let error = run(&[
        "pinvou",
        "projects",
        "rebind",
        inner.to_str().unwrap(),
        outer.to_str().unwrap(),
        "--yes",
    ])
    .expect_err("a rebind with the original inside the destination must refuse");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("REBIND_TO_NESTED"),
        "the refusal must carry the typed marker: {error}"
    );

    // Nothing moved: the project keeps its root and the sidecar binding is
    // intact.
    let list = run_json(&["pinvou", "projects", "list"]);
    assert_eq!(list["projects"][0]["id"], project_id);
    assert_eq!(
        list["projects"][0]["roots"][0]["path"].as_str().unwrap(),
        outer.to_str().unwrap()
    );
    let sidecar = std::fs::read_to_string(
        home.sessions_root()
            .join(&session_id)
            .join("workspace-binding.json"),
    )
    .expect("the sidecar must still exist");
    assert!(
        sidecar.contains(inner.to_str().unwrap()),
        "the refusal must leave the binding as-is: {sidecar}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The other mirror geometry — the DESTINATION strictly inside the source —
/// is the arm the head commit restored: `/a/x -> /a/x/new` deepens the
/// project root one level per rerun and breaks idempotency, so the CLI
/// refuses it with the same typed `REBIND_TO_NESTED` marker. Without this
/// pin, a regression dropping the `to.starts_with(from)` arm passes the
/// whole suite (the sibling test covers only from-inside-to).
#[test]
fn projects_rebind_refuses_to_strictly_inside_from() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("rebind-deepen-nested");
    let dir = make_root_dir("rebind-deepen-nested");
    let from = std::fs::canonicalize(&dir).unwrap();
    let to = from.join("new");
    std::fs::create_dir_all(&to).unwrap();

    let value = run_json(&[
        "pinvou",
        "projects",
        "create",
        "--name",
        "Deepen",
        "--root",
        from.to_str().unwrap(),
    ]);
    let project_id = value["id"].as_str().unwrap().to_owned();
    let sessions = SessionStore::boot().expect("boot session store");
    let session = sessions
        .create_new("test-model".to_owned(), None, from.clone())
        .expect("create session");
    let session_id = session.metadata.id;
    sessions
        .bind_session_workspace(&session_id, from.clone())
        .expect("bind workspace");
    drop(sessions);

    let error = run(&[
        "pinvou",
        "projects",
        "rebind",
        from.to_str().unwrap(),
        to.to_str().unwrap(),
        "--yes",
    ])
    .expect_err("a rebind with the destination inside the source must refuse");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(
        error.to_string().contains("REBIND_TO_NESTED"),
        "the refusal must carry the typed marker: {error}"
    );

    // Nothing moved: the project keeps its root and the sidecar binding is
    // intact.
    let list = run_json(&["pinvou", "projects", "list"]);
    assert_eq!(list["projects"][0]["id"], project_id);
    assert_eq!(
        list["projects"][0]["roots"][0]["path"].as_str().unwrap(),
        from.to_str().unwrap()
    );
    let sidecar = std::fs::read_to_string(
        home.sessions_root()
            .join(&session_id)
            .join("workspace-binding.json"),
    )
    .expect("the sidecar must still exist");
    assert!(
        sidecar.contains(from.to_str().unwrap()),
        "the refusal must leave the binding as-is: {sidecar}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A legacy table that exists but never parses (corrupt or hand-truncated)
/// must fail the whole rebind with the typed `REBIND_LEGACY_TABLE_CORRUPT`
/// marker BEFORE anything moves (the #463 plan/apply split made the legacy
/// sync a fail-closed plan-phase gate): no sidecar is translated, so no
/// report can claim a partial success while the stale table survives on disk
/// (a later repair would let the boot migration move bindings back). The
/// corrupt table itself must not be normalized away.
#[test]
fn projects_rebind_surfaces_an_unparseable_legacy_table() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("rebind-legacy-broken");
    let from_dir = make_root_dir("rebind-legacy-broken-from");
    let to_dir = make_root_dir("rebind-legacy-broken-to");
    let from = std::fs::canonicalize(&from_dir).unwrap();
    let to = std::fs::canonicalize(&to_dir).unwrap();

    let sessions = SessionStore::boot().expect("boot session store");
    let session = sessions
        .create_new("test-model".to_owned(), None, from.clone())
        .expect("create the session");
    let session_id = session.metadata.id;
    // A real binding sidecar under `from`: the gate must refuse BEFORE this
    // sidecar moves — that is the fail-closed contract under test.
    sessions
        .bind_session_workspace(&session_id, from.clone())
        .expect("seed the binding sidecar");
    // A codex-lane session too: the plan-phase gate runs BEFORE the codex
    // lane (the store's "nothing was moved" contract is lane-wide), so the
    // agent index must still name the source when the run aborts. A command
    // that let the codex lane rewrite first and only then hit the gate would
    // fail this assertion.
    let code_session = sessions
        .create_new("test-model".to_owned(), None, from.clone())
        .expect("create the code session");
    let code_id = code_session.metadata.id;
    drop(sessions);
    let agents = SessionAgentStore::load_or_empty();
    agents
        .bind_code_native_session(&code_id, CodexWorkspaceKind::Project, Some(from.clone()))
        .expect("bind the code session");
    drop(agents);

    // A legacy table that exists but can never parse: the plan-phase sync
    // reads it as corrupt and must abort the run.
    let legacy_table = home.sessions_root().join("_session_workspaces.json");
    std::fs::write(&legacy_table, b"{ truncated").expect("seed the corrupt legacy table");

    let error = run(&[
        "pinvou",
        "projects",
        "rebind",
        from.to_str().unwrap(),
        to.to_str().unwrap(),
        "--yes",
        "--output",
        "json",
    ])
    .expect_err("the corrupt legacy table must fail the rebind");
    let message = error.to_string();
    assert!(
        message.contains("REBIND_LEGACY_TABLE_CORRUPT"),
        "the typed marker must reach the message: {message}"
    );
    // Fail-closed: nothing moved — the sidecar still names the source, so a
    // rerun after repairing the table converges from a coherent state.
    let sidecar = std::fs::read_to_string(
        home.sessions_root()
            .join(&session_id)
            .join("workspace-binding.json"),
    )
    .expect("the binding sidecar must exist");
    assert!(
        sidecar.contains(from.to_str().unwrap()),
        "the binding must still name the source (nothing may move): {sidecar}"
    );
    // Lane-wide: the codex lane had not started either when the gate fired.
    let agents_after = SessionAgentStore::load_or_empty();
    assert!(
        agents_after
            .sessions_under_workspace(&from)
            .iter()
            .any(|(id, _)| id == &code_id),
        "the codex lane must not move before the legacy gate passes"
    );
    assert!(
        agents_after.sessions_under_workspace(&to).is_empty(),
        "no codex sidecar may name the destination after the abort"
    );
    // The corrupt table itself is preserved untouched (the gate refuses
    // instead of renaming or normalizing it).
    assert_eq!(
        std::fs::read(&legacy_table).unwrap(),
        b"{ truncated",
        "the corrupt table must not be normalized away by the rebind"
    );
}

#[test]
fn projects_rebind_preserves_legacy_table_only_bindings() {
    // Round-23 review MAJOR: the rebind path opened the session store with
    // plain `SessionStore::boot()`, which deliberately skips the legacy
    // binding-table migration, and then rewrote `_session_workspaces.json`
    // wholesale as `cache ∪ plan` — from a cache that never saw the legacy
    // entries. On a home still carrying the intermediate-format table, a
    // rebind silently destroyed every legacy-format-only binding (including
    // ones unrelated to the rebind) and reported success. The fix runs the
    // GUI's boot-time migration before the rewrite; this test pins the
    // survival of both the rebound entry and an unrelated one.
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("rebind-legacy");
    let from_dir = make_root_dir("rebind-legacy-from");
    let to_dir = make_root_dir("rebind-legacy-to");
    let from = std::fs::canonicalize(&from_dir).unwrap();
    let to = std::fs::canonicalize(&to_dir).unwrap();
    let elsewhere_dir = make_root_dir("rebind-legacy-elsewhere");
    let elsewhere = std::fs::canonicalize(&elsewhere_dir).unwrap();

    // A live session whose ONLY binding is a legacy-table entry: any
    // sidecar the creation seeded is stripped, so nothing but the
    // migration can put it in front of the rebind.
    let sessions = SessionStore::boot().expect("boot session store");
    let legacy_session = sessions
        .create_new("test-model".to_owned(), None, from.clone())
        .expect("create the legacy-bound session");
    let legacy_id = legacy_session.metadata.id;
    let other_session = sessions
        .create_new("test-model".to_owned(), None, elsewhere.clone())
        .expect("create the unrelated legacy-bound session");
    let other_id = other_session.metadata.id;
    drop(sessions);
    for session_id in [&legacy_id, &other_id] {
        std::fs::remove_file(
            home.sessions_root()
                .join(session_id)
                .join("workspace-binding.json"),
        )
        .ok();
    }
    assert!(
        !home
            .sessions_root()
            .join(&legacy_id)
            .join("workspace-binding.json")
            .exists(),
        "fixture requires a session without a binding sidecar"
    );
    // The intermediate-format table: the exact shape a dev build of the
    // legacy era left behind (round-8 review B1's population).
    let legacy_table = home.sessions_root().join("_session_workspaces.json");
    std::fs::write(
        &legacy_table,
        serde_json::json!({
            legacy_id.clone(): from.clone(),
            other_id.clone(): elsewhere.clone(),
        })
        .to_string(),
    )
    .expect("seed the legacy table");

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
        "the legacy-bound session must rebind cleanly: {failed:?}"
    );
    let rebound: Vec<&str> = value["rebound_session_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry.as_str())
        .collect();
    assert!(
        rebound.contains(&legacy_id.as_str()),
        "the legacy-table-only binding must be seen and rebound: {rebound:?}"
    );

    // The rebound entry landed on the destination…
    let sidecar = std::fs::read_to_string(
        home.sessions_root()
            .join(&legacy_id)
            .join("workspace-binding.json"),
    )
    .expect("the migrated sidecar must exist after the rebind");
    assert!(
        sidecar.contains(to.to_str().unwrap()),
        "the legacy binding must be translated to the destination: {sidecar}"
    );
    // …and the unrelated legacy entry must survive untouched (this is the
    // assertion the pre-fix wholesale rewrite failed: its cache ∪ plan
    // view dropped the entry, and with no other legacy entries left the
    // whole file was deleted).
    let other_sidecar = std::fs::read_to_string(
        home.sessions_root()
            .join(&other_id)
            .join("workspace-binding.json"),
    )
    .expect("the unrelated entry must be migrated, not dropped");
    assert!(
        other_sidecar.contains(elsewhere.to_str().unwrap()),
        "the unrelated legacy binding must survive the rebind: {other_sidecar}"
    );

    std::fs::remove_dir_all(&from_dir).ok();
    std::fs::remove_dir_all(&to_dir).ok();
    std::fs::remove_dir_all(&elsewhere_dir).ok();
}

/// The GUI's `Some([])` (drop every root) has a CLI spelling: `--clear-roots`.
/// Bare `--root` absence keeps the current roots, and the two forms are
/// exclusive.
#[test]
fn update_clear_roots_expresses_the_gui_empty_roots_form() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("clear-roots");
    let root_dir = make_root_dir("clear-roots");
    let created = run_json(&[
        "pinvou",
        "projects",
        "create",
        "--name",
        "Doc",
        "--root",
        root_dir.to_str().unwrap(),
    ]);
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(
        created["roots"].as_array().map(Vec::len),
        Some(1),
        "precondition: the project carries one root: {created}"
    );

    let updated = run_json(&["pinvou", "projects", "update", &id, "--clear-roots"]);
    assert_eq!(
        updated["roots"],
        serde_json::json!([]),
        "--clear-roots must produce the GUI's empty roots list: {updated}"
    );
    let listed = run_json(&["pinvou", "projects", "list"]);
    assert_eq!(listed["projects"][0]["roots"], serde_json::json!([]));

    // The exclusive form is a usage error (raised at parse time), not a
    // silent overwrite.
    let error = parse_args(vec![
        "pinvou",
        "projects",
        "update",
        &id,
        "--root",
        root_dir.to_str().unwrap(),
        "--clear-roots",
    ])
    .unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Usage);
}

/// Round-36 review minor (GUI `move_session_to_project` parity): aux ids
/// are chat-kind, so the chat-only admission would pass them — but an
/// assignment row for an aux id is a ghost entry no list can clear.
#[test]
fn projects_move_refuses_aux_session_ids_like_the_gui() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("move-aux-refusal");
    let store = SessionStore::boot().expect("boot session store");
    let aux = store
        .create_new("test-model".to_owned(), None, std::env::temp_dir())
        .expect("create session");
    let aux_id = format!("aux-{}", aux.metadata.id);
    drop(store);

    let parsed =
        parse_args(["pinvou", "projects", "move", &aux_id, "--yes"]).expect("the move line parses");
    let error = execute(parsed).expect_err("an aux id must be refused by move");
    assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
    assert!(
        error.to_string().contains("auxiliary conversations"),
        "{error}"
    );
}

/// Round-40 review: `rebind` to a destination that does not exist must wear
/// the typed `REBIND_TO_UNUSABLE` marker (the validator failure mapped at
/// projects.rs's to-lane gate), and nothing may move. No contract test
/// exercised this arm — a regression that dropped the mapping for a raw
/// `project_error` kept the suite green while scripts matching the typed
/// markers broke.
#[test]
fn projects_rebind_to_a_missing_destination_is_a_typed_refusal() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("rebind-to-unusable");
    let from_dir = make_root_dir("rebind-unusable-from");
    let from = std::fs::canonicalize(&from_dir).unwrap();
    let to = from
        .parent()
        .unwrap()
        .join("rebind-never-created-destination");

    run_json(&[
        "pinvou",
        "projects",
        "create",
        "--name",
        "UnusableTarget",
        "--root",
        from.to_str().unwrap(),
    ]);
    let sessions = SessionStore::boot().expect("boot session store");
    let session = sessions
        .create_new("test-model".to_owned(), None, from.clone())
        .expect("create session");
    let id = session.metadata.id;
    sessions
        .bind_session_workspace(&id, from.clone())
        .expect("bind workspace");
    drop(sessions);

    let outcome = run(&[
        "pinvou",
        "projects",
        "rebind",
        from.to_str().unwrap(),
        to.to_str().unwrap(),
        "--yes",
    ]);
    let error = outcome.expect_err("rebind to a missing destination must refuse");
    assert!(
        error.to_string().contains("REBIND_TO_UNUSABLE"),
        "the refusal must carry the typed marker: {error}"
    );

    // Nothing moved: the binding sidecar still names the old root.
    let sidecar = std::fs::read_to_string(
        home.sessions_root()
            .join(&id)
            .join("workspace-binding.json"),
    )
    .expect("binding sidecar still on disk");
    assert!(
        sidecar.contains(from.to_str().unwrap()),
        "nothing may move on a refused rebind: {sidecar}"
    );
}

/// Round-40 review: rebinding onto another project's existing root would
/// produce overlapping project roots — the preflight partition refuses with
/// the typed `REBIND_ROOTS_CONFLICT` copy before any session binding moves.
/// Like the missing-destination arm above, this arm had no contract test.
#[test]
fn projects_rebind_to_an_overlapping_existing_root_is_a_typed_conflict() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("rebind-roots-conflict");
    let from_dir = make_root_dir("rebind-conflict-from");
    let to_dir = make_root_dir("rebind-conflict-to");
    let from = std::fs::canonicalize(&from_dir).unwrap();
    let to = std::fs::canonicalize(&to_dir).unwrap();

    run_json(&[
        "pinvou",
        "projects",
        "create",
        "--name",
        "Moving",
        "--root",
        from.to_str().unwrap(),
    ]);
    run_json(&[
        "pinvou",
        "projects",
        "create",
        "--name",
        "Occupied",
        "--root",
        to.to_str().unwrap(),
    ]);

    let outcome = run(&[
        "pinvou",
        "projects",
        "rebind",
        from.to_str().unwrap(),
        to.to_str().unwrap(),
        "--yes",
    ]);
    let error = outcome.expect_err("rebinding onto an occupied root must refuse");
    assert!(
        error.to_string().contains("REBIND_ROOTS_CONFLICT"),
        "the refusal must carry the typed conflict marker: {error}"
    );

    // Both projects keep their own roots.
    let list = run_json(&["pinvou", "projects", "list"]);
    let roots: Vec<String> = list["projects"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|project| {
            project["roots"]
                .as_array()
                .unwrap()
                .iter()
                .map(|root| root["path"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        roots.contains(&from.to_str().unwrap().to_owned())
            && roots.contains(&to.to_str().unwrap().to_owned()),
        "both roots must survive the refused rebind: {roots:?}"
    );
}

/// Round-40 review: tier-2 auto-group resolution (workspace bound under a
/// project root, no explicit assignment, longest root wins) had no positive
/// contract test — only the negative ungroup refusals exercised the wiring.
/// A regression there (passing the session-record path instead of the
/// session's workspace, or the `Some(None)` short-circuit leaking into tier
/// 2) would refuse ungroups the GUI's dialog allows — the family's most
/// irreversible write silently diverging from the GUI.
#[test]
fn projects_move_ungroups_a_tier2_auto_grouped_session() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("rebind-tier2-ungroup");
    let root_dir = make_root_dir("rebind-tier2-root");
    let root = std::fs::canonicalize(&root_dir).unwrap();

    let value = run_json(&[
        "pinvou",
        "projects",
        "create",
        "--name",
        "AutoGroup",
        "--root",
        root.to_str().unwrap(),
    ]);
    let _project_id = value["id"].as_str().unwrap().to_owned();

    // The session's workspace sits UNDER the project root, and no explicit
    // assignment is created — resolution must come from tier 2.
    let sessions = SessionStore::boot().expect("boot session store");
    let session = sessions
        .create_new("test-model".to_owned(), None, root.join("sub"))
        .expect("create session");
    let id = session.metadata.id;
    sessions
        .bind_session_workspace(&id, root.join("sub"))
        .expect("bind workspace");
    drop(sessions);

    // The ungroup gate resolves the session through tier 2 and lets the
    // explicit-opt-out write through.
    let value = run_json(&["pinvou", "projects", "move", &id, "--yes"]);
    assert!(
        value.get("project_id").is_none() || value["project_id"].is_null(),
        "an ungroup resolves to no project: {value}"
    );

    // The explicit ungroup entry now exists: a repeated ungroup is refused
    // (the gate's "already ungrouped" arm), proving the entry was written.
    let outcome = run(&["pinvou", "projects", "move", &id, "--yes"]);
    let error = outcome.expect_err("the ungroup entry must pin the explicit opt-out");
    assert!(error.to_string().contains("not in a project"), "{error}");
    let _ = home;
}
