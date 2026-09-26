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
    assert!(columns.len() >= 4, "personas list row layout changed");
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
            "persona entries must carry a known source"
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
    assert!(
        id.starts_with("user-contract-tester-"),
        "the imported persona id must derive from the raw name"
    );
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
    // Round-18 contract: the staged body is delivered by the CLI's own
    // `agent run --session <id>` lane (same injection point the GUI chat send
    // uses). The old field said `false` with a note claiming injection
    // happens "in the desktop app's turns" — false twice over: the desktop
    // app never reads this sidecar, and before the wiring nothing consumed it.
    assert_eq!(value["applies_to_next_turn"], serde_json::json!(true));
    assert!(
        value["note"].as_str().unwrap().contains("agent run"),
        "the note must name the lane that actually delivers the body: {value}"
    );
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
    // usage errors, never writes outside the sessions root. Usage wins over
    // Failed: even with an unknown persona id, the malformed session id
    // reports the usage error.
    let error = run(&["pinvou", "personas", "equip", "../escape", &persona_id])
        .expect_err("path traversal session id must be rejected");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    let error = run(&["pinvou", "personas", "equip", "../escape", "user-missing"])
        .expect_err("path traversal session id must be rejected");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    let error = run(&["pinvou", "personas", "active", "../escape"])
        .expect_err("path traversal session id must be rejected");
    assert_eq!(error.exit_code(), ExitCode::Usage);

    // A well-formed but nonexistent session id is a failed lookup (exit 1),
    // and must not materialize a stray sessions/<id>/ directory.
    let error = run(&[
        "pinvou",
        "personas",
        "equip",
        "no-such-session",
        &persona_id,
    ])
    .expect_err("unknown session must fail");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("does not exist"), "{error}");
    assert!(!home.sessions_root().join("no-such-session").exists());

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

/// `personas delete` must sweep the CLI's own equip persistence: every
/// `persona_equipped.json` sidecar that still references the deleted card
/// (which carries its full pending-body injection text) is removed and
/// reported as `cleared_sessions`, while sidecars for other personas stay
/// untouched.
#[test]
fn personas_delete_sweeps_equipped_sidecars_for_the_deleted_card_only() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("delete-sidecar-sweep");
    let session_a = create_session_fixture();
    let session_b = create_session_fixture();
    let body_path = write_body_file("sweep", "# Sweep Expert\n\nbody\n");

    // Two user cards: one to delete, one that must stay equipped.
    let value = run_json(&[
        "pinvou",
        "personas",
        "create",
        "--name",
        "Sweep Expert",
        "--file",
        body_path.to_str().unwrap(),
    ]);
    let deleted_id = value["id"].as_str().unwrap().to_owned();
    let value = run_json(&[
        "pinvou",
        "personas",
        "create",
        "--name",
        "Keeper Expert",
        "--file",
        body_path.to_str().unwrap(),
    ]);
    let keeper_id = value["id"].as_str().unwrap().to_owned();

    // Equip the doomed card on session A and the keeper on session B.
    run(&["pinvou", "personas", "equip", &session_a, &deleted_id]).unwrap();
    run(&["pinvou", "personas", "equip", &session_b, &keeper_id]).unwrap();
    let sidecar_a = home
        .sessions_root()
        .join(&session_a)
        .join("persona_equipped.json");
    let sidecar_b = home
        .sessions_root()
        .join(&session_b)
        .join("persona_equipped.json");
    assert!(sidecar_a.is_file() && sidecar_b.is_file());

    let value = run_json(&["pinvou", "personas", "delete", &deleted_id, "--yes"]);
    assert_eq!(value["action"], "deleted");
    let cleared = value["cleared_sessions"]
        .as_array()
        .expect("cleared_sessions list");
    assert_eq!(cleared.len(), 1);
    assert_eq!(
        cleared[0], session_a,
        "the delete must name every session whose sidecar referenced the card"
    );
    // The deleted persona's sidecar is gone and `active` reports nothing;
    // the keeper's sidecar (a different persona id) is untouched.
    assert!(
        !sidecar_a.exists(),
        "the deleted persona's sidecar must be removed"
    );
    let value = run_json(&["pinvou", "personas", "active", &session_a]);
    assert!(value.is_null(), "the deleted persona must not stay active");
    assert!(
        sidecar_b.is_file(),
        "a sidecar for a different persona must be untouched"
    );
    let value = run_json(&["pinvou", "personas", "active", &session_b]);
    assert_eq!(value["id"], keeper_id);

    // The human line reports the cleared-session count; the ids live in the
    // JSON `cleared_sessions` field (delete the keeper to exercise the line).
    let outcome = run(&["pinvou", "personas", "delete", &keeper_id, "--yes"])
        .expect("human delete must succeed");
    assert!(
        outcome
            .stdout
            .contains("cleared the equipped-persona sidecar on 1 session"),
        "the human output must report the cleared-session count"
    );

    std::fs::remove_file(&body_path).unwrap();
}

/// Regression: the delete-time sweep reads each sidecar through a bounded
/// reader whose cap must cover the largest legal sidecar (injection-wrapped
/// 4 MiB body, JSON-escaped). A sidecar staged from a persona whose body is
/// well over the old 64 KiB read cap must still be matched and removed —
/// otherwise delete reports success while a stale `persona_equipped.json`
/// keeps the deleted persona "active" on the session.
#[test]
fn personas_delete_sweep_clears_sidecars_with_large_bodies() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("delete-sidecar-large-body");
    let session_id = create_session_fixture();
    // 100 KiB: over the old 64 KiB sweep cap, far under the 4 MiB body cap.
    let big_body = format!("# Big Expert\n\n{}\n", "x".repeat(100 * 1024));
    let body_path = write_body_file("sweep-big", &big_body);

    let value = run_json(&[
        "pinvou",
        "personas",
        "create",
        "--name",
        "Big Expert",
        "--file",
        body_path.to_str().unwrap(),
    ]);
    let persona_id = value["id"].as_str().unwrap().to_owned();
    run(&["pinvou", "personas", "equip", &session_id, &persona_id]).unwrap();
    let sidecar = home
        .sessions_root()
        .join(&session_id)
        .join("persona_equipped.json");
    assert!(
        sidecar.is_file(),
        "equip must persist the sidecar for the large body"
    );

    let value = run_json(&["pinvou", "personas", "delete", &persona_id, "--yes"]);
    assert_eq!(value["action"], "deleted");
    let cleared = value["cleared_sessions"]
        .as_array()
        .expect("cleared_sessions list");
    assert_eq!(
        cleared.len(),
        1,
        "the large-body sidecar must be swept, not skipped by the read cap"
    );
    assert_eq!(cleared[0], session_id);
    assert!(
        !sidecar.exists(),
        "the large-body sidecar must be removed by the delete sweep"
    );
    let value = run_json(&["pinvou", "personas", "active", &session_id]);
    assert!(value.is_null(), "the deleted persona must not stay active");

    std::fs::remove_file(&body_path).unwrap();
}

/// The human `personas list` and `personas active` rows are tab-separated
/// records (five and three columns) and the card text in them is untrusted:
/// the feature layer only rejects an empty trimmed name, the CLI takes
/// `--name`/`--description` verbatim from argv, and the user pool is parsed
/// from arbitrary `~/.pinvou3/user/personas/*.json`. A tab would invent a
/// column, a newline would split one card across two rows, and an ESC would
/// reach the terminal. JSON keeps the verbatim strings.
#[test]
fn personas_list_and_active_rows_survive_control_characters_in_card_text() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("row-control-chars");
    let session_id = create_session_fixture();
    let body_path = write_body_file("rows", "# Rows Expert\n\nbody\n");
    let hostile_name = "Alpha\tBeta\nGamma\x1b[31m";
    let hostile_description = "does\tthings\nand more\x07";

    let value = run_json(&[
        "pinvou",
        "personas",
        "create",
        "--name",
        hostile_name,
        "--description",
        hostile_description,
        "--file",
        body_path.to_str().unwrap(),
    ]);
    let persona_id = value["id"].as_str().unwrap().to_owned();
    assert_eq!(
        value["name"],
        serde_json::json!(hostile_name),
        "the collapse is a rendering choice; JSON keeps the stored name"
    );

    let outcome = run(&["pinvou", "personas", "list"]).expect("list must succeed");
    assert!(
        !outcome.stdout.contains('\x1b'),
        "ESC must not reach the terminal: {:?}",
        outcome.stdout
    );
    let row = outcome
        .stdout
        .lines()
        .find(|line| line.starts_with(&persona_id))
        .unwrap_or_else(|| panic!("the created card must be listed: {:?}", outcome.stdout));
    assert_eq!(
        row,
        format!("{persona_id}\tuser\tAlpha Beta Gamma [31m\tspecialized\tdoes things and more "),
        "the list row must stay one line of five collapsed columns"
    );

    // `active` renders the same untrusted cells one command away.
    run(&["pinvou", "personas", "equip", &session_id, &persona_id]).expect("equip must succeed");
    let outcome = run(&["pinvou", "personas", "active", &session_id]).expect("active must succeed");
    assert_eq!(
        outcome.stdout,
        format!("{persona_id}\tAlpha Beta Gamma [31m\tuser"),
        "the active row must stay one line of three collapsed columns"
    );

    std::fs::remove_file(&body_path).unwrap();
}

/// The desktop app's persona delete never touches `persona_equipped.json`
/// (the filename appears nowhere in `pinvou3-app`), and the CLI's own sweep
/// cannot run afterwards because `personas delete` gates on the card
/// existing. So a sidecar can outlive its card while still holding that
/// card's full injection body. Reporting it as "no persona equipped" hid the
/// only remaining evidence; `active` names it and `unequip` clears it without
/// consulting the card pool, so the state is visible AND clearable from the
/// CLI alone.
#[test]
fn personas_active_reports_an_orphaned_sidecar_and_unequip_clears_it() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("orphan-sidecar");
    let session_id = create_session_fixture();
    let body_path = write_body_file("orphan", "# Orphan Expert\n\nbody\n");

    let value = run_json(&[
        "pinvou",
        "personas",
        "create",
        "--name",
        "Orphan Expert",
        "--file",
        body_path.to_str().unwrap(),
    ]);
    let persona_id = value["id"].as_str().unwrap().to_owned();
    run(&["pinvou", "personas", "equip", &session_id, &persona_id]).expect("equip must succeed");
    let sidecar = home
        .sessions_root()
        .join(&session_id)
        .join("persona_equipped.json");
    assert!(sidecar.is_file());

    // Exactly what the desktop app's delete does: remove the card file and
    // nothing else. The user pool is cached process-globally and only
    // reloaded on a create/update/delete, so a second create is what makes
    // this process observe the removal — the same reload the app performs.
    std::fs::remove_file(home.user_personas_dir().join(format!("{persona_id}.json")))
        .expect("the card file must exist");
    run_json(&[
        "pinvou",
        "personas",
        "create",
        "--name",
        "Reload Trigger",
        "--file",
        body_path.to_str().unwrap(),
    ]);

    let error = run(&["pinvou", "personas", "active", &session_id])
        .expect_err("an orphaned sidecar must not report as 'none'");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains(&persona_id), "{error}");
    assert!(
        error
            .to_string()
            .contains(&format!("personas unequip {session_id}")),
        "the error must name the command that clears it: {error}"
    );
    assert!(
        sidecar.is_file(),
        "reporting must not delete anything by itself"
    );

    // The remedy works without the card: `unequip` never consults the pool.
    run(&["pinvou", "personas", "unequip", &session_id]).expect("unequip must clear the orphan");
    assert!(!sidecar.exists());
    let value = run_json(&["pinvou", "personas", "active", &session_id]);
    assert!(value.is_null(), "cleared state is the honest 'none'");

    std::fs::remove_file(&body_path).unwrap();
}

/// `personas delete` deliberately reports sweep failures on a `sidecar_errors`
/// channel while still exiting 0: the card itself is gone by then, and a
/// failure verdict would claim nothing was deleted. That channel had no test
/// with a NON-EMPTY list, so nothing pinned either half of the contract.
#[test]
fn personas_delete_reports_sidecar_errors_and_still_exits_zero() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("delete-sidecar-errors");
    let session_id = create_session_fixture();
    let body_path = write_body_file("sweep-errors", "# Stuck Expert\n\nbody\n");

    let value = run_json(&[
        "pinvou",
        "personas",
        "create",
        "--name",
        "Stuck Expert",
        "--file",
        body_path.to_str().unwrap(),
    ]);
    let persona_id = value["id"].as_str().unwrap().to_owned();
    run(&["pinvou", "personas", "equip", &session_id, &persona_id]).expect("equip must succeed");

    // A session directory whose sidecar cannot be inspected at all: a
    // DIRECTORY where the sweep expects a file. The sweep cannot tell whether
    // it references the deleted card, so it must be named rather than skipped.
    let blocked = home
        .sessions_root()
        .join("ghost-session")
        .join("persona_equipped.json");
    std::fs::create_dir_all(&blocked).unwrap();

    let outcome = run(&[
        "pinvou",
        "personas",
        "delete",
        &persona_id,
        "--yes",
        "--output",
        "json",
    ])
    .expect("a stuck sidecar must not fail the delete");
    assert_eq!(
        outcome.exit_code,
        ExitCode::Success,
        "the card is already deleted; a failure verdict would claim otherwise"
    );
    let value: serde_json::Value = serde_json::from_str(&outcome.stdout).unwrap();
    assert_eq!(value["action"], "deleted");
    let errors = value["sidecar_errors"]
        .as_array()
        .expect("sidecar_errors list");
    assert_eq!(errors.len(), 1, "the stuck sidecar must be named: {value}");
    assert!(
        errors[0]
            .as_str()
            .unwrap()
            .starts_with("ghost-session: personas delete:"),
        "the entry must name the session it is stuck on: {}",
        errors[0]
    );
    // The reachable sidecar was still swept, and the stuck one is untouched.
    let cleared = value["cleared_sessions"]
        .as_array()
        .expect("cleared_sessions list");
    assert_eq!(cleared.len(), 1);
    assert_eq!(cleared[0], session_id);
    assert!(
        blocked.is_dir(),
        "the sweep must not delete what it cannot read"
    );

    std::fs::remove_file(&body_path).unwrap();
}

/// The 4 MiB stdin body cap (`read_body`'s `--stdin` lane) is a content
/// error (exit 1), not a usage error, and must refuse before anything is
/// persisted: an oversized body leaves no card behind. Driven through the
/// real binary because the body arrives on stdin, which the in-process
/// helpers cannot pipe.
#[test]
fn personas_create_refuses_an_oversized_stdin_body_and_persists_nothing() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("stdin-over-cap");

    // One byte past the cap, so the refusal is exactly the over-cap case and
    // an off-by-one in the cap cannot pass.
    let body = "a".repeat(4 * 1024 * 1024 + 1);
    let body_path = std::env::temp_dir().join(format!(
        "pinvou-cli-personas-stdin-over-cap-{}.txt",
        std::process::id()
    ));
    std::fs::write(&body_path, &body).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pinvou"))
        .args(["personas", "create", "--name", "Too Big", "--stdin"])
        .env("PINVOU3_HOME", &home.root)
        .stdin(std::fs::File::open(&body_path).unwrap())
        .output()
        .expect("the pinvou binary must run");
    assert_eq!(
        output.status.code(),
        Some(1),
        "an oversized stdin body is a content error, not a usage error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("personas create") && stderr.contains("4 MiB stdin limit"),
        "the refusal must name the command and the cap: {stderr}"
    );

    // Nothing persisted: no user card file anywhere in the pool.
    let persisted = std::fs::read_dir(home.user_personas_dir())
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(persisted, 0, "a refused body must not persist a card");

    std::fs::remove_file(&body_path).unwrap();
}
