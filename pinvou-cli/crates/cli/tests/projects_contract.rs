//! Contract tests for the `projects` family (GUI-parity project).
//!
//! Parse-level coverage runs against the typed command tree. Execute-level
//! coverage is strictly hermetic: every test runs against a throwaway
//! `PINVOU3_HOME` (the shared `common` harness's `env_lock`) and touches no
//! network, model, engine, or display.
//! Move fixtures build a session through `SessionStore::boot()` in the test
//! process — the same standalone constructor the CLI uses.
//!
//! The projects store is a single JSON file under `~/.pinvou3/projects/`
//! shared with the GUI app, so assertions locate projects by the exact id a
//! test created and never assume anything about other stores.

mod common;

use std::path::{Path, PathBuf};

use pinvou_cli::{CliError, CliOutcome, ExitCode, OutputMode, parse_args};
use pinvou3_lib::features::codex_acp::{CodexWorkspaceKind, SessionAgentStore};
use pinvou3_lib::features::sessions::SessionStore;

use common::{
    HomeGuard, Isolation, assert_usage, create_session_fixture, env_lock, expect_failed,
    parsed_debug, run, run_child, run_json, scratch_dir, usage_error,
};

/// `~/.pinvou3/projects/projects.json` — the single projects store file.
impl HomeGuard {
    fn store_file(&self) -> PathBuf {
        self.path().join("projects").join("projects.json")
    }

    /// A sidecar file under one session's directory in the sandbox sessions
    /// root: the `workspace-binding.json` / `code-session.json` /
    /// `acp-state.json` / `codex-workspace-baseline.json` paths the rebind
    /// lanes assert against (same inherent-method pattern as
    /// `artifacts_contract.rs`'s `workspace_of`).
    fn session_file(&self, id: &str, name: &str) -> PathBuf {
        self.sessions_root().join(id).join(name)
    }
}

/// Seeds a plain-lane session whose SavedSession metadata and
/// workspace-binding sidecar both point at `dir` — the same store
/// constructors the CLI command itself uses — and returns the session id.
fn bind_plain_session(dir: &Path) -> String {
    let sessions = SessionStore::boot().expect("boot session store");
    let session = sessions
        .create_new("test-model".to_owned(), None, dir.to_path_buf())
        .expect("create plain session");
    let id = session.metadata.id;
    sessions
        .bind_session_workspace(&id, dir.to_path_buf())
        .expect("bind plain workspace");
    id
}

/// The codex-lane twin: the session plus the agent-index record binding the
/// authoritative code-session sidecar to `dir`. Returns the session id.
fn bind_code_session(dir: &Path) -> String {
    let sessions = SessionStore::boot().expect("boot session store");
    let session = sessions
        .create_new("test-model".to_owned(), None, dir.to_path_buf())
        .expect("create code session");
    let id = session.metadata.id;
    drop(sessions);
    let agents = SessionAgentStore::load_or_empty();
    agents
        .bind_code_native_session(
            &id,
            CodexWorkspaceKind::Project,
            Some(dir.to_path_buf()),
            Vec::new(),
        )
        .expect("bind code session");
    drop(agents);
    id
}

/// The id list a rebind report carries under `key` (`rebound_session_ids` /
/// `failed_session_ids`).
fn report_ids(value: &serde_json::Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} must be present in the rebind report"))
        .iter()
        .map(|entry| entry.as_str().expect("report ids are strings").to_owned())
        .collect()
}

/// Seeds the `acp-state.json` the GUI's spawn path writes for `session_id`,
/// with its `workspace.path` pointing at `workspace` — the file the rebind's
/// storage-half translate must carry over.
fn seed_acp_state(home: &HomeGuard, session_id: &str, workspace: &Path) {
    std::fs::create_dir_all(home.sessions_root().join(session_id)).unwrap();
    std::fs::write(
        home.sessions_root().join(session_id).join("acp-state.json"),
        serde_json::json!({ "workspace": { "path": workspace.to_string_lossy() } }).to_string(),
    )
    .unwrap();
}

/// The argv every rebind report/lane test shares, built once: the two path
/// positionals plus the `--yes` gate (the sites whose argv or verdict
/// genuinely differs — the no-`--yes` and shape-error refusals, the
/// corrupt-legacy-table error lane, and the real-binary spawns — keep their
/// inline calls).
fn rebind_argv<'a>(from: &'a Path, to: &'a Path) -> Vec<&'a str> {
    vec![
        "pinvou",
        "projects",
        "rebind",
        from.to_str().unwrap(),
        to.to_str().unwrap(),
        "--yes",
    ]
}

/// Runs `projects rebind <from> <to> --yes` through the in-process dispatch.
fn rebind(from: &Path, to: &Path) -> Result<CliOutcome, CliError> {
    run(&rebind_argv(from, to))
}

/// [`rebind`], asserting the success class and parsing the single-line JSON
/// envelope (the same contract as the harness's `run_json`).
fn rebind_json(from: &Path, to: &Path) -> serde_json::Value {
    run_json(&rebind_argv(from, to))
}

/// Asserts the rebind report carried no `failed_session_ids`; `label` keeps
/// the caller's context in the failure message.
fn assert_no_failed_sessions(value: &serde_json::Value, label: &str) {
    let failed = value["failed_session_ids"].as_array().unwrap();
    assert!(failed.is_empty(), "{label}: {failed:?}");
}

/// An owning project-root fixture: `Drop` removes the directory tree, so a
/// failing assertion can no longer leak it (the manual `remove_dir_all`
/// epilogues this replaces were exactly the cleanup a panic skipped).
struct TempRoot(PathBuf);

impl std::ops::Deref for TempRoot {
    type Target = PathBuf;

    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Creates an absolute, existing directory to use as a project root,
/// canonicalized: the rebind lanes compare — and every test here seeds and
/// asserts — the resolved display domain, which on macOS differs from
/// `scratch_dir`'s spelling because the temp root sits behind
/// /var → /private/var.
fn make_root_dir(label: &str) -> TempRoot {
    let path = scratch_dir(&format!("pinvou-cli-projects-root-{label}"));
    TempRoot(std::fs::canonicalize(path).unwrap())
}

/// Creates a project through the JSON lane; `roots` appends one `--root` per entry.
fn create_project(name: &str, roots: &[&Path]) -> serde_json::Value {
    let mut arguments = vec!["pinvou", "projects", "create", "--name", name];
    for root in roots {
        arguments.push("--root");
        arguments.push(root.to_str().unwrap());
    }
    run_json(&arguments)
}

// ── parse-level coverage ────────────────────────────────────────────────────

#[test]
fn every_projects_subcommand_parses_and_invalid_usage_exits_two() {
    let _env_guard = env_lock();
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
        // The Projects command tree is dispatched exclusively through the
        // Projects variant; the shared harness assertion freezes the family
        // prefix and the subcommand token's capitalized variant in the
        // derived Debug form (the types are not nameable from integration
        // tests).
        common::assert_family_parse("Projects(", arguments);
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
        usage_error(arguments);
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
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "zero-state", Isolation::ProductHome);

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
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "crud-round-trip", Isolation::ProductHome);

    // create — generated prj- id, GUI list-DTO fields, no roots.
    let value = create_project("Alpha", &[]);
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
    let error = expect_failed(&["pinvou", "projects", "update", "prj-missing", "--name", "X"]);
    assert!(
        error.to_string().contains("project not found"),
        "an unknown project must be named in the failure"
    );

    // delete without --yes is refused before any store access.
    let message = assert_usage(&["pinvou", "projects", "delete", &id]);
    assert!(
        message.contains("--yes"),
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
    let error = expect_failed(&["pinvou", "projects", "delete", &id, "--yes"]);
    assert!(
        error.to_string().contains("project not found"),
        "an unknown project must be named in the failure"
    );
}

#[test]
fn projects_root_validation_errors_are_clean_failures() {
    let _env_guard = env_lock();
    let _home = HomeGuard::new("projects", "root-validation", Isolation::ProductHome);

    // Relative roots are rejected by the feature's own validation.
    let error = expect_failed(&[
        "pinvou",
        "projects",
        "create",
        "--name",
        "Bad",
        "--root",
        "relative/path",
    ]);
    assert!(
        error.to_string().contains("must be absolute"),
        "the relative root must be named in the failure"
    );

    // Roots that nest inside each other are rejected (auto-grouping would
    // otherwise be ambiguous).
    let parent = make_root_dir("nest-parent");
    let child = parent.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let error = expect_failed(&[
        "pinvou",
        "projects",
        "create",
        "--name",
        "Nested",
        "--root",
        parent.to_str().unwrap(),
        "--root",
        child.to_str().unwrap(),
    ]);
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
}

#[test]
fn projects_move_assigns_sessions_and_reports_unknowns() {
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "move", Isolation::ProductHome);
    let session_id = create_session_fixture();

    let value = create_project("Inbox", &[]);
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
    let error = expect_failed(&["pinvou", "projects", "move", "no-such-session", &project_id]);
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
    let error = expect_failed(&["pinvou", "projects", "move", &session_id, "prj-missing"]);
    assert!(
        error.to_string().contains("project not found"),
        "an unknown project must be named in the failure"
    );

    // Session ids join onto store paths, so traversal attempts are usage
    // errors, never writes outside the sessions root.
    let message = assert_usage(&["pinvou", "projects", "move", "../escape", &project_id]);
    assert!(
        message.contains("valid session id"),
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
    // §9.9 (review #484): a deleted project's members are tombstoned as an
    // explicit None entry instead of being removed — automatic revival into
    // a surviving project sharing the root is forbidden; moving back is an
    // explicit move.
    assert!(
        value["assignments"]
            .get(session_id.as_str())
            .is_some_and(|entry| entry.is_null()),
        "the assignment entry must be tombstoned, not removed"
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
    let error = expect_failed(&["pinvou", "projects", "move", &session_id, "--yes"]);
    assert!(
        error.to_string().contains("nothing to move it out of"),
        "{error}"
    );
    assert!(
        error.to_string().contains("cannot be reverted"),
        "the refusal must disclose that the write it refused is irreversible: {error}"
    );
    let value = run_json(&["pinvou", "projects", "list"]);
    // The explicit None tombstone from the delete above stands; the refusal
    // itself must not flip it into an assignment.
    assert!(
        value["assignments"]
            .get(session_id.as_str())
            .is_some_and(|entry| entry.is_null()),
        "a refused ungroup must not write an assignment entry"
    );

    // Inside a project it resolves, so the ungroup is allowed exactly once.
    let value = create_project("Temp", &[]);
    let project_id = value["id"].as_str().unwrap().to_owned();
    let value = run_json(&["pinvou", "projects", "move", &session_id, &project_id]);
    assert_eq!(value["project_id"], project_id);

    // --yes gate, red-first: the ungroup write is irreversible, so without
    // --yes it must be refused before any store access — the same
    // confirmation convention (and refusal copy) as projects delete /
    // sessions delete.
    let message = assert_usage(&["pinvou", "projects", "move", &session_id]);
    assert!(
        message.contains("--yes"),
        "the refusal must point at the --yes gate: {message}"
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
    let error = expect_failed(&["pinvou", "projects", "move", &session_id, "--yes"]);
    assert!(
        error.to_string().contains("nothing to move it out of"),
        "{error}"
    );

    // Without --yes the refusal is the confirmation gate, and it fires
    // before the resolution gate can be reached at all.
    let message = assert_usage(&["pinvou", "projects", "move", &session_id]);
    assert!(message.contains("--yes"), "{message}");

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
    let message = usage_error(&["pinvou", "projects", "move", &session_id, &project_id, "x"]);
    assert!(message.contains("cannot be reverted"), "{message}");
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
        let debug = parsed_debug(arguments);
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
        usage_error(arguments);
    }
}

#[test]
fn projects_rebind_migrates_roots_both_binding_lanes_and_metadata() {
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "rebind", Isolation::ProductHome);
    let from = make_root_dir("rebind-from");
    let to = make_root_dir("rebind-to");

    // A project whose root sits under `from`.
    let value = create_project("Moved", &[&from]);
    let project_id = value["id"].as_str().unwrap().to_owned();

    // Plain lane: session JSON + workspace-binding.json sidecar, both seeded
    // through the same store constructors the CLI command itself uses.
    let plain_id = bind_plain_session(&from);
    // Codex lane: the session JSON plus the agent-index record and the
    // authoritative code-session sidecar.
    let code_id = bind_code_session(&from);
    // Seed the ACP state the GUI's spawn path writes (boot recovery reads
    // `workspace.path` BEFORE the workspace baseline, so an untranslated
    // state file resurrects the vanished root if the agent index is later
    // lost). A regression deleting the rebind's storage-half translate call
    // strands this at the old path.
    seed_acp_state(&home, &code_id, &from);

    let value = rebind_json(&from, &to);
    assert_no_failed_sessions(&value, "a healthy two-lane rebind must not report failures");
    let rebound = report_ids(&value, "rebound_session_ids");
    assert!(
        rebound.contains(&plain_id) && rebound.contains(&code_id),
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
    let sidecar = std::fs::read_to_string(home.session_file(&plain_id, "workspace-binding.json"))
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
    let code_sidecar = std::fs::read_to_string(home.session_file(&code_id, "code-session.json"))
        .expect("code-session sidecar still on disk");
    assert!(
        code_sidecar.contains(to.to_str().unwrap()),
        "the authoritative sidecar must move: {code_sidecar}"
    );
    // ACP state translated (GUI parity, `translate_acp_state_workspace`).
    let acp_state = std::fs::read_to_string(home.session_file(&code_id, "acp-state.json"))
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
    let baseline =
        std::fs::read_to_string(home.session_file(&code_id, "codex-workspace-baseline.json"))
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
    let outcome = rebind(&from, &to).expect("the idempotent rerun must succeed");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(
        outcome
            .stdout
            .contains("rebound 0 session(s) (0 failed), updated roots of 0 project(s)"),
        "the rerun must report an empty convergence: {:?}",
        outcome.stdout
    );
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
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "rebind-retry", Isolation::ProductHome);
    let from = make_root_dir("rebind-retry-from");
    let to = make_root_dir("rebind-retry-to");

    // Seed the exact state a failed run-1 leaves behind: the SavedSession
    // metadata still names `from` (the write that failed), while both
    // binding lanes already sit under `to` (the writes that succeeded).
    // Nothing is left under `from`, so the lane batches of the rerun return
    // empty and only the retry pass can see the session. The split geometry
    // IS the fixture: created under `from`, bound to `to` — the shared
    // same-dir bind helpers cannot express it.
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
        .bind_code_native_session(
            &code_id,
            CodexWorkspaceKind::Project,
            Some(to.clone()),
            Vec::new(),
        )
        .expect("move the code binding to to");
    drop(agents);

    let value = rebind_json(&from, &to);
    assert_no_failed_sessions(&value, "a converged rerun must not report failures");
    let rebound = report_ids(&value, "rebound_session_ids");
    assert!(
        rebound.contains(&plain_id) && rebound.contains(&code_id),
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
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "rebind-behind-binding", Isolation::ProductHome);
    let origin = make_root_dir("rebind-behind-origin");
    let mid = make_root_dir("rebind-behind-mid");
    let to = make_root_dir("rebind-behind-to");

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
    seed_acp_state(&home, &stranded_id, &origin);

    // Run 2 (mid→to): nothing is left under `mid` except this binding, so
    // only the per-session pre-sync geometry can reach the `origin` paths.
    let value = rebind_json(&mid, &to);
    assert_no_failed_sessions(&value, "the stranded session must converge");
    let rebound = report_ids(&value, "rebound_session_ids");
    assert!(
        rebound.contains(&stranded_id),
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
    let acp_state = std::fs::read_to_string(home.session_file(&stranded_id, "acp-state.json"))
        .expect("acp-state file still on disk");
    assert!(
        acp_state.contains(to.to_str().unwrap()) && !acp_state.contains(origin.to_str().unwrap()),
        "the acp-state workspace.path must translate off the origin: {acp_state}"
    );
    let sidecar =
        std::fs::read_to_string(home.session_file(&stranded_id, "workspace-binding.json"))
            .expect("plain binding sidecar still on disk");
    assert!(
        sidecar.contains(to.to_str().unwrap()),
        "the binding sidecar must move: {sidecar}"
    );
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
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "rebind-subpath", Isolation::ProductHome);
    let from = make_root_dir("rebind-subpath-from");
    let to = make_root_dir("rebind-subpath-to");
    let from_sub = from.join("sub");
    let to_sub = to.join("sub");
    std::fs::create_dir_all(&from_sub).unwrap();
    let artifact_on_from = from_sub.join("artifacts").join("report.md");
    let artifact_on_to_sub = to_sub.join("artifacts").join("report.md");
    let artifact_on_to_root = to.join("artifacts").join("report.md");

    // A healthy session bound at the subpath: metadata, artifact storage
    // and acp-state all name `from/sub` (nothing is stranded — this is the
    // plain moved-project shape, the most common rebind input).
    let session_id = bind_plain_session(&from_sub);
    // The bind and the artifact list are independent record fields, so
    // seeding the storage path after the bind leaves the same fixture state.
    let sessions = SessionStore::boot().expect("boot session store");
    sessions
        .update_artifacts(
            &session_id,
            vec![artifact_on_from.to_string_lossy().into_owned()],
        )
        .expect("seed the artifact storage path");
    drop(sessions);
    seed_acp_state(&home, &session_id, &from_sub);

    let value = rebind_json(&from, &to);
    assert_no_failed_sessions(&value, "the subpath session must rebind cleanly");
    let rebound = report_ids(&value, "rebound_session_ids");
    assert!(
        rebound.contains(&session_id),
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
    let acp_state = std::fs::read_to_string(home.session_file(&session_id, "acp-state.json"))
        .expect("acp-state file still on disk");
    assert!(
        acp_state.contains(to_sub.to_str().unwrap())
            && !acp_state.contains(from_sub.to_str().unwrap()),
        "the acp-state workspace.path must translate onto the translated subpath: {acp_state}"
    );
}

#[test]
fn projects_rebind_to_lane_healthy_sessions_are_not_reported() {
    // The retry pass must admit only metadata that LAGS its binding: a
    // session created directly under `to` whose metadata already matches is
    // healthy, and reporting it would be a fabricated "rebound" entry.
    let _env_guard = env_lock();
    let _home = HomeGuard::new("projects", "rebind-healthy", Isolation::ProductHome);
    let from = make_root_dir("rebind-healthy-from");
    let to = make_root_dir("rebind-healthy-to");

    let healthy_id = bind_plain_session(&to);

    let value = rebind_json(&from, &to);
    let rebound = report_ids(&value, "rebound_session_ids");
    let failed = report_ids(&value, "failed_session_ids");
    assert!(
        !rebound.contains(&healthy_id) && !failed.contains(&healthy_id),
        "a healthy to-lane session must not be re-reported: rebound {rebound:?}, failed {failed:?}"
    );
}

#[test]
fn projects_rebind_without_yes_is_refused_before_any_store_access() {
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "rebind-yes", Isolation::ProductHome);
    let from_dir = make_root_dir("rebind-yes-from");
    let to_dir = make_root_dir("rebind-yes-to");

    let message = assert_usage(&[
        "pinvou",
        "projects",
        "rebind",
        from_dir.to_str().unwrap(),
        to_dir.to_str().unwrap(),
    ]);
    assert!(
        message.contains("--yes"),
        "the refusal must point at the --yes gate: {message}"
    );
    assert!(
        !home.store_file().exists(),
        "a refused rebind must not touch the projects store"
    );
    assert!(
        !home.path().join("session-agents.json").exists(),
        "a refused rebind must not touch the agent index"
    );
    assert!(
        !home.sessions_root().exists(),
        "a refused rebind must not create the sessions root"
    );
}

#[test]
fn projects_rebind_rejects_relative_empty_and_root_from_as_usage() {
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "rebind-shape", Isolation::ProductHome);
    let to_dir = make_root_dir("rebind-shape-to");

    // One mirrored message covers empty, relative and filesystem-root `from`
    // — the same single rule the GUI command applies at its entry.
    for from in ["relative/dir", ""] {
        let message = assert_usage(&[
            "pinvou",
            "projects",
            "rebind",
            from,
            to_dir.to_str().unwrap(),
        ]);
        assert!(
            message.contains("absolute, non-root"),
            "from = {from:?} must be named by the mirrored rule: {message}"
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
    let message = assert_usage(&[
        "pinvou",
        "projects",
        "rebind",
        root.to_str().unwrap(),
        to_dir.to_str().unwrap(),
    ]);
    assert!(message.contains("absolute, non-root"), "{message}");

    // And `to` as the filesystem root mirrors the GUI's other entry rule.
    let from_dir = make_root_dir("rebind-shape-from");
    let message = assert_usage(&[
        "pinvou",
        "projects",
        "rebind",
        from_dir.to_str().unwrap(),
        root.to_str().unwrap(),
    ]);
    assert!(message.contains("filesystem root"), "{message}");

    assert!(
        !home.store_file().exists(),
        "rejected arguments must not touch the projects store"
    );
}

#[test]
fn projects_rebind_from_equal_to_is_a_reported_noop() {
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "rebind-same", Isolation::ProductHome);
    let same = make_root_dir("rebind-same");

    // Seed state that WOULD match a real rebind: if the short-circuit were
    // missing, the run would rewrite (or at least touch) all of it.
    let value = create_project("Same", &[&same]);
    let project_id = value["id"].as_str().unwrap().to_owned();
    let session_id = bind_plain_session(&same);

    let value = rebind_json(&same, &same);
    assert_eq!(
        value,
        serde_json::json!({
            "rebound_session_ids": [],
            "failed_session_ids": [],
            "affected_project_ids": [],
        })
    );

    // Nothing changed anywhere.
    let list = run_json(&["pinvou", "projects", "list"]);
    assert_eq!(list["projects"][0]["id"], project_id);
    assert_eq!(
        list["projects"][0]["roots"][0]["path"].as_str().unwrap(),
        same.to_str().unwrap()
    );
    let sidecar = std::fs::read_to_string(home.session_file(&session_id, "workspace-binding.json"))
        .expect("the sidecar must be untouched");
    assert!(
        sidecar.contains(same.to_str().unwrap()),
        "the no-op must leave the binding as-is: {sidecar}"
    );
}

/// The GUI's mirror geometry (#463 round-17 MAJOR-2): a rebind whose `from`
/// and `to` nest inside each other deepens a binding or root one level per
/// rerun (and is false-failed by run 1's fence rescan), so the CLI refuses
/// BOTH mirror geometries with the same typed `REBIND_TO_NESTED` marker
/// instead of performing a rebind the desktop app would have rejected. The
/// refusal must leave the project root and the session sidecar untouched.
/// The two geometries are one assertion matrix over a two-case table; the
/// to-inside-from case is the arm the head commit restored — dropping the
/// `to.starts_with(from)` arm fails only that row.
#[test]
fn projects_rebind_refuses_nested_roots_in_both_mirror_directions() {
    let _env_guard = env_lock();
    let outer = make_root_dir("rebind-mirror-nested");
    let inner = outer.join("B");
    std::fs::create_dir_all(&inner).unwrap();
    let to_inner = outer.join("new");
    std::fs::create_dir_all(&to_inner).unwrap();

    // (label, from, to, project_root): the session is bound at the rebind's
    // `from` in both geometries, and the project root is the OUTER directory
    // (case 1: it IS `to`; case 2: it is the source `from`).
    for (label, from, to, project_root) in [
        (
            "from strictly inside to",
            inner.clone(),
            outer.clone(),
            outer.clone(),
        ),
        (
            "to strictly inside from",
            outer.clone(),
            to_inner.clone(),
            outer.clone(),
        ),
    ] {
        let home = HomeGuard::new("projects", "rebind-mirror-nested", Isolation::ProductHome);
        let value = create_project(label, &[&project_root]);
        let project_id = value["id"].as_str().unwrap().to_owned();
        let session_id = bind_plain_session(&from);

        let error = rebind(&from, &to).expect_err("a rebind with nested roots must refuse");
        assert_eq!(error.exit_code(), ExitCode::Failed, "{label}");
        assert!(
            error.to_string().contains("REBIND_TO_NESTED"),
            "{label}: the refusal must carry the typed marker: {error}"
        );

        // Nothing moved: the project keeps its root and the sidecar binding
        // is intact.
        let list = run_json(&["pinvou", "projects", "list"]);
        assert_eq!(list["projects"][0]["id"], project_id, "{label}");
        assert_eq!(
            list["projects"][0]["roots"][0]["path"].as_str().unwrap(),
            project_root.to_str().unwrap(),
            "{label}"
        );
        let sidecar =
            std::fs::read_to_string(home.session_file(&session_id, "workspace-binding.json"))
                .expect("the sidecar must still exist");
        assert!(
            sidecar.contains(from.to_str().unwrap()),
            "{label}: the refusal must leave the binding as-is: {sidecar}"
        );
    }
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
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "rebind-legacy-broken", Isolation::ProductHome);
    let from = make_root_dir("rebind-legacy-broken-from");
    let to = make_root_dir("rebind-legacy-broken-to");

    // A real binding sidecar under `from`: the gate must refuse BEFORE this
    // sidecar moves — that is the fail-closed contract under test.
    let session_id = bind_plain_session(&from);
    // A codex-lane session too: the plan-phase gate runs BEFORE the codex
    // lane (the store's "nothing was moved" contract is lane-wide), so the
    // agent index must still name the source when the run aborts. A command
    // that let the codex lane rewrite first and only then hit the gate would
    // fail this assertion.
    let code_id = bind_code_session(&from);

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
    let sidecar = std::fs::read_to_string(home.session_file(&session_id, "workspace-binding.json"))
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
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "rebind-legacy", Isolation::ProductHome);
    let from = make_root_dir("rebind-legacy-from");
    let to = make_root_dir("rebind-legacy-to");
    let elsewhere = make_root_dir("rebind-legacy-elsewhere");

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
        std::fs::remove_file(home.session_file(session_id, "workspace-binding.json")).ok();
    }
    assert!(
        !home
            .session_file(&legacy_id, "workspace-binding.json")
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

    let value = rebind_json(&from, &to);
    assert_no_failed_sessions(&value, "the legacy-bound session must rebind cleanly");
    let rebound = report_ids(&value, "rebound_session_ids");
    assert!(
        rebound.contains(&legacy_id),
        "the legacy-table-only binding must be seen and rebound: {rebound:?}"
    );

    // The rebound entry landed on the destination…
    let sidecar = std::fs::read_to_string(home.session_file(&legacy_id, "workspace-binding.json"))
        .expect("the migrated sidecar must exist after the rebind");
    assert!(
        sidecar.contains(to.to_str().unwrap()),
        "the legacy binding must be translated to the destination: {sidecar}"
    );
    // …and the unrelated legacy entry must survive untouched (this is the
    // assertion the pre-fix wholesale rewrite failed: its cache ∪ plan
    // view dropped the entry, and with no other legacy entries left the
    // whole file was deleted).
    let other_sidecar =
        std::fs::read_to_string(home.session_file(&other_id, "workspace-binding.json"))
            .expect("the unrelated entry must be migrated, not dropped");
    assert!(
        other_sidecar.contains(elsewhere.to_str().unwrap()),
        "the unrelated legacy binding must survive the rebind: {other_sidecar}"
    );
}

/// The GUI's `Some([])` (drop every root) has a CLI spelling: `--clear-roots`.
/// Bare `--root` absence keeps the current roots, and the two forms are
/// exclusive.
#[test]
fn update_clear_roots_expresses_the_gui_empty_roots_form() {
    let _env_guard = env_lock();
    let _home = HomeGuard::new("projects", "clear-roots", Isolation::ProductHome);
    let root_dir = make_root_dir("clear-roots");
    let created = create_project("Doc", &[&root_dir]);
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
    usage_error(&[
        "pinvou",
        "projects",
        "update",
        &id,
        "--root",
        root_dir.to_str().unwrap(),
        "--clear-roots",
    ]);
}

/// Round-36 review minor (GUI `move_session_to_project` parity): aux ids
/// are chat-kind, so the chat-only admission would pass them — but an
/// assignment row for an aux id is a ghost entry no list can clear.
#[test]
fn projects_move_refuses_aux_session_ids_like_the_gui() {
    let _env_guard = env_lock();
    let _home = HomeGuard::new("projects", "move-aux-refusal", Isolation::ProductHome);
    let aux_id = format!("aux-{}", create_session_fixture());

    let error = expect_failed(&["pinvou", "projects", "move", &aux_id, "--yes"]);
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
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "rebind-to-unusable", Isolation::ProductHome);
    let from = make_root_dir("rebind-unusable-from");
    let to = from
        .parent()
        .unwrap()
        .join("rebind-never-created-destination");

    create_project("UnusableTarget", &[&from]);
    let id = bind_plain_session(&from);

    let error = rebind(&from, &to).expect_err("rebind to a missing destination must refuse");
    assert!(
        error.to_string().contains("REBIND_TO_UNUSABLE"),
        "the refusal must carry the typed marker: {error}"
    );

    // Nothing moved: the binding sidecar still names the old root.
    let sidecar = std::fs::read_to_string(home.session_file(&id, "workspace-binding.json"))
        .expect("binding sidecar still on disk");
    assert!(
        sidecar.contains(from.to_str().unwrap()),
        "nothing may move on a refused rebind: {sidecar}"
    );
}

/// §9.9 (review #484) legalized CROSS-project root overlap: rebinding onto
/// a root another project already holds is a legal relocation. Round-46 M3
/// renamed this test — its round-40 name/doc promised the typed
/// `REBIND_ROOTS_CONFLICT` refusal while §9.9 flipped the body to assert
/// success; the intra-set nesting arm is pinned separately below.
#[test]
fn projects_rebind_to_another_projects_root_relocates_per_s9_9() {
    let _env_guard = env_lock();
    let _home = HomeGuard::new("projects", "rebind-roots-conflict", Isolation::ProductHome);
    let from = make_root_dir("rebind-conflict-from");
    let to = make_root_dir("rebind-conflict-to");

    create_project("Moving", &[&from]);
    create_project("Occupied", &[&to]);

    // §9.9 (review #484) legalizes cross-project root overlap: rebinding
    // onto a root another project already holds is a legal, successful
    // relocation — only the intra-set duplicate/nesting conflicts refuse.
    let value = rebind_json(&from, &to);
    assert!(
        value["affected_project_ids"].as_array().is_some(),
        "the relocation must report the affected project: {value}"
    );

    // Both projects survive; `Moving` now shares `Occupied`'s territory.
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
        !roots.contains(&from.to_str().unwrap().to_owned())
            && roots.contains(&to.to_str().unwrap().to_owned()),
        "the moved project must now reference the shared root: {roots:?}"
    );
}

/// Round-46 M3: the typed `REBIND_ROOTS_CONFLICT` arm — the only rebind
/// refusal §9.9 keeps — had no CLI exercise after the overlap flip above
/// renamed this arm's former host. Rebinding a root INTO the same project's
/// other root nests the target set and must refuse with the typed marker
/// before any binding moves.
#[test]
fn projects_rebind_into_own_sibling_root_is_a_typed_conflict() {
    let _env_guard = env_lock();
    let _home = HomeGuard::new("projects", "rebind-roots-nested", Isolation::ProductHome);
    let first = make_root_dir("rebind-nested-first");
    let second = make_root_dir("rebind-nested-second");
    let inner_path = second.join("inner");
    std::fs::create_dir_all(&inner_path).unwrap();
    let inner = TempRoot(std::fs::canonicalize(&inner_path).unwrap());

    create_project("Nesting", &[&first, &second]);

    let error = expect_failed(&rebind_argv(&first, &inner));
    assert!(
        error.to_string().contains("REBIND_ROOTS_CONFLICT"),
        "the intra-set nesting refusal must stay typed: {error}"
    );

    // Refused before any movement: the project still holds its original roots.
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
        roots.contains(&first.to_str().unwrap().to_owned())
            && roots.contains(&second.to_str().unwrap().to_owned())
            && !roots.contains(&inner.to_str().unwrap().to_owned()),
        "a refused rebind must leave every original root in place: {roots:?}"
    );
}

/// Round-40 review: tier-2 auto-group resolution (workspace bound under a
/// project root, no explicit assignment, §9.9 (position, id) first-match)
/// had no positive
/// contract test — only the negative ungroup refusals exercised the wiring.
/// A regression there (passing the session-record path instead of the
/// session's workspace, or the `Some(None)` short-circuit leaking into tier
/// 2) would refuse ungroups the GUI's dialog allows — the family's most
/// irreversible write silently diverging from the GUI.
#[test]
fn projects_move_ungroups_a_tier2_auto_grouped_session() {
    let _env_guard = env_lock();
    let _home = HomeGuard::new("projects", "rebind-tier2-ungroup", Isolation::ProductHome);
    let root = make_root_dir("rebind-tier2-root");

    let value = create_project("AutoGroup", &[&root]);
    let _project_id = value["id"].as_str().unwrap().to_owned();

    // The session's workspace sits UNDER the project root, and no explicit
    // assignment is created — resolution must come from tier 2.
    let id = bind_plain_session(&root.join("sub"));

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
}

/// Round-49 review: tier 2's agent-index half is read directly (bounded read
/// + parse) instead of through `SessionAgentStore::load_or_empty`, whose
///   corrupt-file path prints the app-side zh `[pinvou3-app] … starting empty`
///   line into the CLI's stderr and answers tier 2 from an empty table. The
///   codex-lane healthy arm also pins the field-for-field serde mirror the
///   direct parse relies on: a `workspace_kind`/`workspace_path`
///   representation change in the store must fail here, not silently drop
///   tier 2.
#[test]
fn projects_move_ungroup_gate_discloses_a_corrupt_agent_index_without_the_app_line() {
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "move-corrupt-index", Isolation::ProductHome);
    let root = make_root_dir("move-corrupt-index-root");

    let value = create_project("CorruptIndex", &[&root]);
    let _project_id = value["id"].as_str().unwrap().to_owned();

    // Healthy codex lane first: the session's project workspace comes from
    // the agent index alone (no plain sidecar, no assignment), so the ungroup
    // resolves through the parsed index and must keep succeeding.
    let healthy_id = bind_code_session(&root.join("sub"));
    let value = run_json(&["pinvou", "projects", "move", &healthy_id, "--yes"]);
    assert!(
        value.get("project_id").is_none() || value["project_id"].is_null(),
        "the healthy index must still resolve tier 2: {value}"
    );

    // Corrupt run: a second codex-workspace-only session, then the index
    // corrupted on disk. The in-process harness cannot capture the note's
    // stderr, so this arm drives the real binary (same pattern as the
    // corrupt-index delete-cascade tests in sessions_contract.rs).
    let corrupt_id = bind_code_session(&root.join("sub"));
    let agents_path = home.path().join("session-agents.json");
    std::fs::write(&agents_path, "{not json").unwrap();

    let spawned = run_child(
        home.path(),
        &["projects", "move", &corrupt_id, "--yes"],
        &[],
    );
    let stderr = String::from_utf8_lossy(&spawned.stderr);
    assert_eq!(
        spawned.status.code(),
        Some(1),
        "the refusal exit class must not change: {stderr}"
    );
    assert!(
        !stderr.contains("[pinvou3-app]"),
        "the app-side boot recovery line must not leak into the CLI: {stderr}"
    );
    assert!(
        stderr.contains("is unreadable") && stderr.contains("session-agents.json"),
        "the CLI-styled note must disclose the failure and name the index: {stderr}"
    );
    assert!(
        stderr.contains("nothing to move it out of"),
        "the ungroup refusal must stand unchanged: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&agents_path).unwrap(),
        "{not json",
        "the corrupt index must be left in place for repair"
    );
}

/// Round-52 review: a corrupt session-agents index must refuse the rebind
/// with the CLI-styled copy (naming the file and the repair) and WITHOUT
/// the store's zh anyhow chain (`解析 … 失败`) or the app-side
/// `[pinvou3-app] … starting empty` recovery line — the empty fallback
/// used to print the latter from the rebind's own boot load, and the codex
/// lane surfaced the former through `project_error`'s `{error:#}`. The
/// in-process harness cannot capture stderr, so this drives the real
/// binary (same pattern as the corrupt-index move/delete tests).
#[test]
fn projects_rebind_refuses_a_corrupt_agent_index_without_the_app_line_or_the_zh_chain() {
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "rebind-corrupt-index", Isolation::ProductHome);
    let from = make_root_dir("rebind-corrupt-from");
    let to = make_root_dir("rebind-corrupt-to");

    // A healthy codex-lane binding first: its authoritative sidecar is what
    // "nothing was moved" is asserted against.
    let code_id = bind_code_session(&from);
    let agents_path = home.path().join("session-agents.json");
    std::fs::write(&agents_path, "{not json").unwrap();

    let spawned = run_child(
        home.path(),
        &[
            "projects",
            "rebind",
            from.to_str().unwrap(),
            to.to_str().unwrap(),
            "--yes",
        ],
        &[],
    );
    let stderr = String::from_utf8_lossy(&spawned.stderr);
    assert_eq!(
        spawned.status.code(),
        Some(1),
        "the corrupt index must refuse with the store-failure exit class: {stderr}"
    );
    assert!(
        stderr.contains("rebind_workspace_root")
            && stderr.contains("the code-session agent index is unreadable")
            && stderr.contains("repair or remove session-agents.json"),
        "the refusal must carry the CLI-styled copy naming the repair: {stderr}"
    );
    assert!(
        !stderr.contains("解析"),
        "the store's zh chain must not leak into the CLI output: {stderr}"
    );
    assert!(
        !stderr.contains("[pinvou3-app]"),
        "the app-side boot recovery line must not leak into the CLI: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&agents_path).unwrap(),
        "{not json",
        "the corrupt index must be left in place for repair"
    );
    // And nothing was moved: the authoritative sidecar still binds `from`.
    let sidecar = std::fs::read_to_string(home.session_file(&code_id, "code-session.json"))
        .expect("the code-session sidecar must be untouched");
    assert!(
        sidecar.contains(from.to_str().unwrap()) && !sidecar.contains(to.to_str().unwrap()),
        "a refused rebind must not move the binding: {sidecar}"
    );
}

/// Round-54 audit A2 pin: the rebind's baseline recapture must be
/// staleness-gated like the GUI's drain — a baseline already current for
/// the new root is the session-start reference and must not be reset
/// (resetting wipes the changes-since-session-start view or flips
/// currently-dirty files to preexisting on a rerun). Comment lines are
/// stripped so prose cannot satisfy the shape; removing the gate, dropping
/// its negation, or reordering it after the capture all fail.
#[test]
fn rebind_baseline_recapture_is_staleness_gated() {
    let code: String = include_str!("../src/projects.rs")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let gate = "&& !pinvou3_lib::features::codex_acp::workspace::baseline_is_current(";
    let capture = "capture_baseline(session_id, &new_path)";
    let gate_at = code
        .find(gate)
        .expect("the rebind drain lost its baseline staleness gate");
    let capture_at = code
        .find(capture)
        .expect("the rebind drain lost its baseline recapture call");
    assert!(
        gate_at < capture_at,
        "the staleness gate must guard (precede) the recapture call"
    );
}

/// Round-1 review M1 (the audit A2's under-coverage clause): the GUI's
/// post-loop `code_lane_retry_ids` pass, behaviorally. A to-lane code
/// session fully synced by an earlier run (bindings AND metadata at the
/// destination) but whose baseline recapture failed or never ran is
/// invisible to every in-loop arm — the lanes found nothing under `from`
/// and the to-lane admission skips an already-synced session. Without the
/// post-loop pass the missing baseline survived every rerun.
#[test]
fn projects_rebind_recaptures_a_missing_to_lane_baseline() {
    let _env_guard = env_lock();
    let home = HomeGuard::new(
        "projects",
        "rebind-to-lane-baseline",
        Isolation::ProductHome,
    );
    let from = make_root_dir("rebind-tolane-from");
    let to = make_root_dir("rebind-tolane-to");

    // Fully converged at the destination: session metadata, agent index,
    // and sidecar all name `to` — no baseline exists (the earlier run's
    // capture failed or never ran).
    let code_id = bind_code_session(&to);

    let value = rebind_json(&from, &to);
    assert_no_failed_sessions(&value, "a converged rerun must not report failures");
    assert!(
        !report_ids(&value, "rebound_session_ids").contains(&code_id),
        "an already-synced session must not be reported rebound: nothing was translated this run"
    );

    // The post-loop pass captured the missing baseline for the new root.
    let baseline =
        std::fs::read_to_string(home.session_file(&code_id, "codex-workspace-baseline.json"))
            .expect("an already-synced to-lane code session must have its baseline recaptured");
    assert!(
        baseline.contains(to.to_str().unwrap()),
        "the recaptured baseline must name the destination: {baseline}"
    );
    assert!(
        !baseline.contains(from.to_str().unwrap()),
        "the recaptured baseline must not name the never-involved source: {baseline}"
    );
}

/// Round-1 review m2: the staleness gate's behavioral pin (the audit A2's
/// over-capture clause, rerun shape). A baseline already current for the
/// new root IS the session-start reference; a rerun must not recapture it
/// — changes made between runs would otherwise be baked into the
/// reference (non-git: the changes-since-session-start view wiped; git:
/// currently-dirty files flipped to preexisting). Seeded through the same
/// lib capture the command itself uses; byte-compared across the rerun so
/// no format detail can mask a recapture.
#[test]
fn projects_rebind_keeps_a_current_to_lane_baseline_across_a_rerun() {
    let _env_guard = env_lock();
    let home = HomeGuard::new("projects", "rebind-baseline-gate", Isolation::ProductHome);
    let from = make_root_dir("rebind-gate-from");
    let to = make_root_dir("rebind-gate-to");

    let code_id = bind_code_session(&to);
    pinvou3_lib::features::codex_acp::workspace::capture_baseline(&code_id, &to)
        .expect("seed the session-start baseline");
    let baseline_path = home.session_file(&code_id, "codex-workspace-baseline.json");
    let seeded = std::fs::read(&baseline_path).expect("the seeded baseline must be on disk");

    // A workspace change lands between the runs …
    std::fs::write(to.join("between-runs.txt"), "change").expect("seed the between-runs change");

    // … and the rerun must NOT bake it into the session-start reference.
    let value = rebind_json(&from, &to);
    assert_no_failed_sessions(&value, "a converged rerun must not report failures");
    let after = std::fs::read(&baseline_path).expect("the current baseline must survive the rerun");
    assert_eq!(
        seeded, after,
        "a baseline already current for the destination must not be recaptured by a rerun"
    );
    let after_text = String::from_utf8_lossy(&after);
    assert!(
        !after_text.contains("between-runs.txt"),
        "the between-runs change must stay OUTSIDE the session-start reference: {after_text}"
    );
}

/// Round-40 M5: the delete's auto-grouped-member expulsion is pinned — a
/// session auto-filed under the project's root (no explicit assignment)
/// must land in `affected_session_ids` and get the tombstone; and a corrupt
/// agent index fails the delete with "nothing was deleted — retry" instead
/// of silently under-enumerating.
#[test]
fn projects_delete_exposes_auto_grouped_members_and_refuses_a_corrupt_index() {
    let _lock = env_lock();
    let home = HomeGuard::new("projects", "delete-pin", Isolation::ProductHome);
    let root = home.path().join("proj-root");
    std::fs::create_dir_all(&root).expect("project root");

    // Create the project through the CLI itself (the lane the pin
    // exercises) and keep the generated id.
    let created = create_project("cli-delete-pin", &[&root]);
    let project_id = created["id"].as_str().unwrap().to_owned();
    // An AUTO-GROUPED member: a code session bound under the root, no
    // explicit assignment (tier-2 only).
    let member = bind_code_session(&root);

    let value = run_json(&["pinvou", "projects", "delete", &project_id, "--yes"]);
    let affected = report_ids(&value, "affected_session_ids");
    assert!(
        affected.contains(&member),
        "the auto-grouped member must land in affected_session_ids: {value}"
    );
    // The project itself is gone from the listing (tombstones may keep
    // the file present by design — retention of explicit move-outs).
    let listing = run_json(&["pinvou", "projects", "list"]);
    let listed: Vec<&str> = listing["projects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert!(
        !listed.contains(&"cli-delete-pin"),
        "the deleted project must not survive in the listing: {listing}"
    );

    // Corrupt-index refusal: a second project + member, then break the
    // agent index before the delete.
    let home2 = HomeGuard::new("projects", "delete-corrupt", Isolation::ProductHome);
    let root2 = home2.path().join("proj-root-2");
    std::fs::create_dir_all(&root2).expect("project root 2");
    let created2 = create_project("cli-delete-corrupt", &[&root2]);
    let project_id2 = created2["id"].as_str().unwrap().to_owned();
    let _ = bind_code_session(&root2);
    std::fs::write(home2.path().join("session-agents.json"), "{not json")
        .expect("corrupt the agent index");
    expect_failed(&["pinvou", "projects", "delete", &project_id2, "--yes"]);
}

// Round-49 M4: `update --roots` expulsion contract. A root replacement must
// route through `cli_replace_roots_and_expel` (the GUI's expel flow, the
// round-40 M4 parity requirement): narrowing the roots tombstones the
// auto-grouped member bound under the dropped root, so the assignment map
// drops it. Reverting the path to a plain `store.update_project` leaves the
// stale assignment in place (count stays 2, and the GUI's align would
// silently re-adopt the member); no other update test reaches the
// replacement lane with auto-grouped members at all.
#[test]
fn projects_update_roots_expels_auto_grouped_members() {
    let _lock = env_lock();
    let home = HomeGuard::new("projects", "update-expel", Isolation::ProductHome);
    let kept_root = home.path().join("kept-root");
    let dropped_root = home.path().join("dropped-root");
    std::fs::create_dir_all(&kept_root).expect("kept project root");
    std::fs::create_dir_all(&dropped_root).expect("dropped project root");

    let created = create_project("cli-update-expel", &[&kept_root, &dropped_root]);
    let project_id = created["id"].as_str().unwrap().to_owned();
    // An AUTO-GROUPED member under each root (tier-2 binding, no explicit
    // assignment): the dropped-root one must be expelled, the kept-root one
    // must survive the replacement.
    let survivor = bind_code_session(&kept_root);
    let expelled = bind_code_session(&dropped_root);

    let value = run_json(&[
        "pinvou",
        "projects",
        "update",
        &project_id,
        "--root",
        kept_root.to_str().unwrap(),
    ]);
    // The replacement is wholesale: only the kept root remains.
    let listed: Vec<&str> = value["roots"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["path"].as_str())
        .collect();
    assert_eq!(
        listed,
        vec![kept_root.to_str().unwrap()],
        "the root list must be replaced wholesale: {value}"
    );

    // The tombstone, asserted where it lives: the expelled member must hold
    // an explicit-None entry in the durable assignment map. This is the
    // exact discriminator against the round-40 M4 divergence class — a
    // plain `store.update_project` would leave NO map entry (the member
    // silently re-adoptable), which a bare count assertion cannot see
    // (tier-2 members never enter the map to begin with).
    let relisting = run_json(&["pinvou", "projects", "list"]);
    let assignments = relisting["assignments"]
        .as_object()
        .expect("projects list must carry the assignment map");
    assert!(
        assignments.contains_key(expelled.as_str()),
        "the expelled member must have an explicit assignment-map entry (the tombstone): {relisting}"
    );
    assert!(
        assignments.get(expelled.as_str()).unwrap().is_null(),
        "the expelled member's entry must be an explicit-None tombstone: {relisting}"
    );
    assert!(
        !assignments.contains_key(survivor.as_str()),
        "the kept-root member must stay tier-2-only (no map entry): {relisting}"
    );
}
