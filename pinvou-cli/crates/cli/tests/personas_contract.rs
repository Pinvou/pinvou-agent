//! Contract tests for the `personas` family (GUI-parity project).
//!
//! Parse-level coverage runs against the typed command tree. Execute-level
//! coverage is strictly hermetic: every test runs against a throwaway
//! `PINVOU3_HOME` (ENV_LOCK serialization, same pattern as
//! `sessions_contract.rs`) and touches no network, model, engine, or display.
//! Equip fixtures build a session through `SessionStore::boot()` in the test
//! process — the same standalone constructor the CLI uses.
//!
//! The user persona pool is cached process-globally by
//! `pinvou3_lib::features::personas` (reloaded on every create/update/
//! delete), so assertions never assume exact user-card counts or exclusive
//! pools: cards are always located (or absence-checked) by the exact id a
//! test created.

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
            "pinvou-cli-personas-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let previous = std::env::var_os("PINVOU3_HOME");
        // SAFETY: the caller holds ENV_LOCK for the whole test, so env writes
        // are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
        Self { previous, root }
    }

    /// `~/.pinvou3/user/personas` — where user cards persist.
    fn user_personas_dir(&self) -> PathBuf {
        self.root.join("user").join("personas")
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

/// Writes a persona body file and returns its path.
fn write_body_file(label: &str, body: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "pinvou-cli-personas-body-{label}-{}-{nonce}.md",
        std::process::id()
    ));
    std::fs::write(&path, body).unwrap();
    path
}

// ── parse-level coverage ────────────────────────────────────────────────────

#[test]
fn every_personas_subcommand_parses_and_invalid_usage_exits_two() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let valid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "personas", "list"],
        vec!["pinvou", "personas", "list", "--source", "user"],
        vec!["pinvou", "personas", "list", "--source", "builtin"],
        vec!["pinvou", "personas", "list", "--source", "all"],
        vec!["pinvou", "personas", "show", "p-1"],
        vec![
            "pinvou", "personas", "create", "--name", "N", "--file", "body.md",
        ],
        vec![
            "pinvou",
            "personas",
            "create",
            "--name",
            "N",
            "--description",
            "D",
            "--file",
            "body.md",
        ],
        vec!["pinvou", "personas", "create", "--name", "N", "--stdin"],
        vec!["pinvou", "personas", "update", "p-1", "--name", "N2"],
        vec!["pinvou", "personas", "update", "p-1", "--description", "D"],
        vec!["pinvou", "personas", "update", "p-1", "--file", "body.md"],
        vec!["pinvou", "personas", "update", "p-1", "--stdin"],
        vec!["pinvou", "personas", "delete", "p-1"],
        vec!["pinvou", "personas", "delete", "p-1", "--yes"],
        vec!["pinvou", "personas", "equip", "s-1", "p-1"],
        vec!["pinvou", "personas", "unequip", "s-1"],
        vec!["pinvou", "personas", "active", "s-1"],
    ];
    for arguments in &valid {
        let parsed = parse_args(arguments.clone())
            .unwrap_or_else(|error| panic!("{arguments:?} must parse: {error}"));
        // The Personas command tree is dispatched exclusively through the
        // Personas variant; assert family + subcommand through the derived
        // Debug form (the types are not nameable from integration tests).
        let debug = format!("{:?}", parsed.command());
        let variant = match arguments[2] {
            "list" => "List",
            "show" => "Show",
            "create" => "Create",
            "update" => "Update",
            "delete" => "Delete",
            "equip" => "Equip",
            "unequip" => "Unequip",
            "active" => "Active",
            other => panic!("unmapped subcommand {other}"),
        };
        assert!(debug.starts_with("Personas("), "{arguments:?} -> {debug}");
        assert!(debug.contains(variant), "{arguments:?} -> {debug}");
    }

    let invalid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "personas"],
        vec!["pinvou", "personas", "bogus"],
        // list: --source only accepts builtin | user | all.
        vec!["pinvou", "personas", "list", "--source", "venv"],
        vec!["pinvou", "personas", "list", "--source"],
        vec!["pinvou", "personas", "list", "--nope"],
        vec!["pinvou", "personas", "list", "--source", "user", "--extra"],
        // show/delete require an id.
        vec!["pinvou", "personas", "show"],
        vec!["pinvou", "personas", "delete"],
        // create requires --name plus a body via --file or --stdin.
        vec!["pinvou", "personas", "create"],
        vec!["pinvou", "personas", "create", "--name", "N"],
        vec!["pinvou", "personas", "create", "--file", "body.md"],
        vec!["pinvou", "personas", "create", "--stdin"],
        vec!["pinvou", "personas", "create", "--name", ""],
        vec!["pinvou", "personas", "create", "--name", "   "],
        vec![
            "pinvou", "personas", "create", "--name", "N", "--file", "a.md", "--stdin",
        ],
        vec!["pinvou", "personas", "create", "--name"],
        vec!["pinvou", "personas", "create", "--name", "N", "--bogus"],
        // update requires an id and only knows the three field flags.
        vec!["pinvou", "personas", "update"],
        vec!["pinvou", "personas", "update", "p-1", "--bogus"],
        vec!["pinvou", "personas", "update", "p-1", "--name"],
        vec!["pinvou", "personas", "update", "p-1", "--file"],
        vec![
            "pinvou", "personas", "update", "p-1", "--file", "a.md", "--stdin",
        ],
        // equip needs both ids and accepts no options.
        vec!["pinvou", "personas", "equip"],
        vec!["pinvou", "personas", "equip", "s-1"],
        vec!["pinvou", "personas", "equip", "s-1", "--yes"],
        vec!["pinvou", "personas", "equip", "s-1", "p-1", "extra"],
        // unequip/active need a session id and accept no options.
        vec!["pinvou", "personas", "unequip"],
        vec!["pinvou", "personas", "unequip", "s-1", "--extra"],
        vec!["pinvou", "personas", "active"],
        vec!["pinvou", "personas", "active", "s-1", "--extra"],
    ];
    for arguments in &invalid {
        let error = parse_args(arguments.clone()).expect_err(&arguments.join(" "));
        assert_eq!(error.exit_code(), ExitCode::Usage, "{arguments:?}");
    }
}

#[test]
fn json_output_mode_flows_through_every_personas_subcommand() {
    // Parse-level assertion only: no PINVOU3_HOME mutation needed here.
    for arguments in [
        vec!["pinvou", "personas", "list"],
        vec!["pinvou", "personas", "show", "p-1"],
        vec!["pinvou", "personas", "create", "--name", "N", "--stdin"],
        vec!["pinvou", "personas", "update", "p-1", "--name", "N"],
        vec!["pinvou", "personas", "delete", "p-1"],
        vec!["pinvou", "personas", "equip", "s-1", "p-1"],
        vec!["pinvou", "personas", "unequip", "s-1"],
        vec!["pinvou", "personas", "active", "s-1"],
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
fn personas_delete_without_yes_is_refused_before_any_state_change() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let error = run(&["pinvou", "personas", "delete", "user-some-card"])
        .expect_err("delete without --yes must refuse");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(error.to_string().contains("--yes"), "{error}");
}

#[test]
fn personas_list_shows_builtin_catalog_and_source_filters() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("list-catalog");

    let outcome = run(&["pinvou", "personas", "list"]).expect("list must succeed");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    // Human rows are id/source/name/dept/description, tab separated.
    let first = outcome.stdout.lines().next().unwrap_or_default();
    let columns: Vec<&str> = first.split('\t').collect();
    assert!(columns.len() >= 4, "row layout changed: {first:?}");
    assert!(columns[0].starts_with("pinvou-") || columns[1] == "builtin");

    // The whole catalog: builtin cards only for a fresh home, and every
    // summary carries the stable fields without the heavy body.
    let value = run_json(&["pinvou", "personas", "list"]);
    assert_eq!(value["source"], "all");
    let entries = value["personas"].as_array().unwrap();
    assert!(
        !entries.is_empty(),
        "the builtin catalog must never be empty"
    );
    for entry in entries {
        assert!(!entry["id"].as_str().unwrap().is_empty());
        assert!(!entry["name"].as_str().unwrap().is_empty());
        assert!(!entry["dept"].as_str().unwrap().is_empty());
        assert!(
            entry["source"] == "builtin" || entry["source"] == "user",
            "{entry}"
        );
        assert!(
            entry.get("body").is_none(),
            "summaries must not carry bodies"
        );
    }
    let creator = entries
        .iter()
        .find(|entry| entry["id"] == "pinvou-card-creator")
        .expect("the builtin card-creator persona must be in the catalog");
    assert_eq!(creator["source"], "builtin");
    assert_eq!(creator["dept"], "tool");

    // --source builtin keeps only builtin cards (always non-empty).
    let value = run_json(&["pinvou", "personas", "list", "--source", "builtin"]);
    assert_eq!(value["source"], "builtin");
    let entries = value["personas"].as_array().unwrap();
    assert!(!entries.is_empty());
    assert!(entries.iter().all(|entry| entry["source"] == "builtin"));

    // --source user keeps only user cards (a fresh home starts empty; other
    // tests in this binary may have left cached user cards behind, so only
    // the source field is asserted).
    let value = run_json(&["pinvou", "personas", "list", "--source", "user"]);
    assert_eq!(value["source"], "user");
    let entries = value["personas"].as_array().unwrap();
    assert!(entries.iter().all(|entry| entry["source"] == "user"));
}

#[test]
fn personas_create_show_update_delete_round_trip_persists_user_card() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("crud-round-trip");
    let body_path = write_body_file("crud", "# Expert\n\nmethodology body v1\n");
    let body = std::fs::read_to_string(&body_path).unwrap();

    // create (file body) — generated user- prefixed id, GUI default fields.
    let value = run_json(&[
        "pinvou",
        "personas",
        "create",
        "--name",
        "Contract Tester",
        "--description",
        "first",
        "--file",
        body_path.to_str().unwrap(),
    ]);
    assert_eq!(value["name"], "Contract Tester");
    assert_eq!(value["source"], "user");
    assert_eq!(value["dept"], "specialized");
    assert_eq!(value["description"], "first");
    let id = value["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with("user-contract-tester-"), "{id}");
    assert!(
        home.user_personas_dir()
            .join(format!("{id}.json"))
            .is_file(),
        "the card must persist under user/personas"
    );

    // The card shows up under --source user with stable fields.
    let value = run_json(&["pinvou", "personas", "list", "--source", "user"]);
    let entry = value["personas"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == id)
        .expect("created card must be listed")
        .clone();
    assert_eq!(entry["name"], "Contract Tester");
    assert_eq!(entry["description"], "first");
    assert!(entry.get("body").is_none());

    // show returns the full card, body included; human output is the body.
    let value = run_json(&["pinvou", "personas", "show", &id]);
    assert_eq!(value["id"], id);
    assert_eq!(value["body"], body);
    assert_eq!(value["source"], "user");
    let outcome = run(&["pinvou", "personas", "show", &id]).expect("human show");
    assert_eq!(outcome.stdout, body);

    // show on a builtin card works (read-only detail view).
    let value = run_json(&["pinvou", "personas", "show", "pinvou-card-creator"]);
    assert_eq!(value["source"], "builtin");
    assert_eq!(value["name"], "卡牌制造专家");
    assert!(!value["body"].as_str().unwrap().is_empty());
    // Unknown ids are clean host failures.
    let error = run(&["pinvou", "personas", "show", "user-missing"])
        .expect_err("unknown persona must fail");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("unknown persona"), "{error}");

    // update --description overlays only that field.
    let value = run_json(&[
        "pinvou",
        "personas",
        "update",
        &id,
        "--description",
        "second",
    ]);
    assert_eq!(value["description"], "second");
    assert_eq!(value["id"], id);
    let value = run_json(&["pinvou", "personas", "show", &id]);
    assert_eq!(value["description"], "second");
    assert_eq!(value["body"], body, "body must be untouched");
    assert_eq!(value["name"], "Contract Tester", "name must be untouched");

    // Builtin cards can never be updated or deleted ("只能编辑/删除自制卡").
    let error = run(&[
        "pinvou",
        "personas",
        "update",
        "pinvou-card-creator",
        "--name",
        "X",
    ])
    .expect_err("builtin card must not be editable");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("personas update"), "{error}");
    let error = run(&[
        "pinvou",
        "personas",
        "delete",
        "pinvou-card-creator",
        "--yes",
    ])
    .expect_err("builtin card must not be deletable");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("personas delete"), "{error}");
    // ...and the builtin catalog is untouched.
    assert!(
        run_json(&["pinvou", "personas", "show", "pinvou-card-creator"])
            .get("id")
            .is_some()
    );

    // delete without --yes is refused and keeps the card.
    let error =
        run(&["pinvou", "personas", "delete", &id]).expect_err("delete without --yes must refuse");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(
        home.user_personas_dir()
            .join(format!("{id}.json"))
            .is_file()
    );

    // delete --yes removes the persisted card everywhere.
    let value = run_json(&["pinvou", "personas", "delete", &id, "--yes"]);
    assert_eq!(value["id"], id);
    assert_eq!(value["action"], "deleted");
    assert!(!home.user_personas_dir().join(format!("{id}.json")).exists());
    let error = run(&["pinvou", "personas", "show", &id]).expect_err("deleted card must be gone");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    let value = run_json(&["pinvou", "personas", "list", "--source", "user"]);
    assert!(
        !value["personas"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == id)
    );

    std::fs::remove_file(&body_path).unwrap();
}

#[test]
fn personas_equip_unequip_active_round_trip_with_fixture_session() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("equip-round-trip");
    let session_id = create_session_fixture();
    let body_path = write_body_file("equip", "# Equip Expert\n\nbody\n");

    // Fresh session: nothing equipped.
    let outcome = run(&["pinvou", "personas", "active", &session_id]).expect("human active");
    assert_eq!(outcome.stdout, "none");
    let value = run_json(&["pinvou", "personas", "active", &session_id]);
    assert!(value.is_null(), "no active persona must serialize as null");

    // Create a user card to equip.
    let value = run_json(&[
        "pinvou",
        "personas",
        "create",
        "--name",
        "Equip Expert",
        "--file",
        body_path.to_str().unwrap(),
    ]);
    let persona_id = value["id"].as_str().unwrap().to_owned();

    // equip returns the card summary and persists the state for the next
    // CLI invocation (the GUI's in-memory sidecar would die with the
    // one-shot process).
    let value = run_json(&["pinvou", "personas", "equip", &session_id, &persona_id]);
    assert_eq!(value["id"], persona_id);
    assert_eq!(value["name"], "Equip Expert");
    assert_eq!(value["source"], "user");
    let outcome =
        run(&["pinvou", "personas", "equip", &session_id, &persona_id]).expect("human equip");
    assert!(
        outcome
            .stdout
            .contains(&format!("equipped {persona_id} on {session_id}"))
    );
    let sidecar = home
        .sessions_root()
        .join(&session_id)
        .join("persona_equipped.json");
    assert!(sidecar.is_file(), "equip state must persist to the sidecar");
    let sidecar_value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
    assert_eq!(sidecar_value["persona_id"], persona_id);
    let body = std::fs::read_to_string(&body_path).unwrap();
    let injection = sidecar_value["pending_body"].as_str().unwrap();
    assert!(injection.contains(&body), "the full body must be staged");
    assert!(injection.contains("专家面具"), "injection wraps the body");

    // active reports the equipped card across invocations.
    let value = run_json(&["pinvou", "personas", "active", &session_id]);
    assert_eq!(value["id"], persona_id);
    assert_eq!(value["name"], "Equip Expert");
    let outcome = run(&["pinvou", "personas", "active", &session_id]).expect("human active");
    let columns: Vec<&str> = outcome.stdout.split('\t').collect();
    assert_eq!(columns[0], persona_id);
    assert_eq!(columns[1], "Equip Expert");
    assert_eq!(columns[2], "user");

    // Equip with an unknown persona id fails cleanly.
    let error = run(&["pinvou", "personas", "equip", &session_id, "user-missing"])
        .expect_err("unknown persona must fail");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("unknown persona"), "{error}");

    // Session ids join onto the sidecar path, so traversal attempts are
    // usage errors, never writes outside the sessions root.
    let error = run(&["pinvou", "personas", "equip", "../escape", &persona_id])
        .expect_err("path traversal session id must be rejected");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    let error = run(&["pinvou", "personas", "active", "../escape"])
        .expect_err("path traversal session id must be rejected");
    assert_eq!(error.exit_code(), ExitCode::Usage);

    // unequip clears the persisted state; active falls back to none.
    let value = run_json(&["pinvou", "personas", "unequip", &session_id]);
    assert_eq!(value["session_id"], session_id);
    assert_eq!(value["action"], "unequipped");
    assert!(!sidecar.exists());
    let value = run_json(&["pinvou", "personas", "active", &session_id]);
    assert!(value.is_null());

    // unequip is idempotent.
    let value = run_json(&["pinvou", "personas", "unequip", &session_id]);
    assert_eq!(value["action"], "unequipped");

    // Equipping a builtin card works too (read-only pool, equip allowed).
    let value = run_json(&[
        "pinvou",
        "personas",
        "equip",
        &session_id,
        "pinvou-card-creator",
    ]);
    assert_eq!(value["id"], "pinvou-card-creator");
    let value = run_json(&["pinvou", "personas", "active", &session_id]);
    assert_eq!(value["id"], "pinvou-card-creator");

    std::fs::remove_file(&body_path).unwrap();
}
