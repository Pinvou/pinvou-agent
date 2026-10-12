//! Contract tests for the `personas` family (GUI-parity project).
//!
//! Parse-level coverage runs against the typed command tree. Execute-level
//! coverage is strictly hermetic: every test runs against a throwaway
//! `PINVOU3_HOME` (the shared `common` harness's `env_lock`) and touches no
//! network, model, engine, or display.
//! Equip fixtures build a session through `SessionStore::boot()` in the test
//! process — the same standalone constructor the CLI uses.
//!
//! The user persona pool is cached process-globally by
//! `pinvou3_lib::features::personas` (reloaded on every create/update/
//! delete), so assertions never assume exact user-card counts or exclusive
//! pools: cards are always located (or absence-checked) by the exact id a
//! test created.

mod common;

use std::path::{Path, PathBuf};

use pinvou_cli::{ExitCode, OutputMode, parse_args};

use common::{
    HomeGuard, Isolation, assert_usage, create_session_fixture, env_lock, expect_failed,
    parsed_debug, run, run_json, unique_root, usage_error,
};

/// A throwaway `PINVOU3_HOME` for one test.
fn temp_home(label: &str) -> HomeGuard {
    HomeGuard::new("personas", label, Isolation::ProductHome)
}

/// `~/.pinvou3/user/personas` — where user cards persist. An inherent method
/// on the shared harness's `HomeGuard` (the `common` module is compiled into
/// this same test binary), so call sites keep plain method syntax.
impl HomeGuard {
    fn user_personas_dir(&self) -> PathBuf {
        self.path().join("user").join("personas")
    }
}

/// `sessions_root/<session>/persona_equipped.json` — the equip-state sidecar
/// path the round-trip and delete-sweep tests assert against.
fn equipped_sidecar(home: &HomeGuard, session: &str) -> PathBuf {
    home.sessions_root()
        .join(session)
        .join("persona_equipped.json")
}

/// Writes a persona body file and returns its path.
/// Round-37 review: the orphan verdicts are only trustworthy against a pool
/// the process actually enumerated. Both shapes need a FRESH process (the
/// pool stamp is a process global, so in-process tests cannot start from
/// "never enumerated"): a faulted first load — the personas directory
/// missing entirely, the GUI's own not-yet-mounted-volume shape — must
/// refuse to prescribe the destructive `unequip` remedy, while a confirmed
/// enumeration still reports the deletion with it.
#[test]
fn active_distinguishes_an_unreadable_pool_from_a_deleted_card() {
    let _env_guard = env_lock();
    let home = temp_home("active-pool-fault");
    let session_id = create_session_fixture();
    // A sidecar whose card does not resolve (never existed).
    let sidecar_dir = home.sessions_root().join(&session_id);
    std::fs::create_dir_all(&sidecar_dir).unwrap();
    std::fs::write(
        sidecar_dir.join("persona_equipped.json"),
        serde_json::json!({ "persona_id": "user-ghost" }).to_string(),
    )
    .unwrap();

    let run_active = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_pinvou"))
            .args(["personas", "active", &session_id])
            .env("PINVOU3_HOME", home.path())
            .env("PINVOU_NO_COLOR", "1")
            .output()
            .expect("active child runs")
    };

    // The personas directory is absent entirely: the first enumeration
    // faults, the empty pool proves nothing, and the remedy must NOT be
    // the equip-destroying `unequip`.
    let outcome = run_active();
    assert!(!outcome.status.success(), "the fault shape must fail");
    let stderr = String::from_utf8_lossy(&outcome.stderr);
    assert!(
        stderr.contains("pool could not be read") && stderr.contains("nothing was changed"),
        "a faulted pool must not prescribe unequip: {stderr}"
    );

    // With a readable (empty) pool the miss is a confirmed deletion and the
    // sidecar cleanup remedy applies.
    std::fs::create_dir_all(home.path().join("user").join("personas")).unwrap();
    let outcome = run_active();
    assert!(!outcome.status.success(), "the deletion shape must fail");
    let stderr = String::from_utf8_lossy(&outcome.stderr);
    assert!(
        stderr.contains("no longer exists") && stderr.contains("unequip"),
        "a confirmed deletion must name the unequip remedy: {stderr}"
    );
}

fn write_body_file(label: &str, body: &str) -> PathBuf {
    let path = unique_root(&format!("pinvou-cli-personas-body-{label}")).with_extension("md");
    std::fs::write(&path, body).unwrap();
    path
}

/// Creates a user card through the CLI's JSON lane and returns its generated
/// id. Cards are always located (or absence-checked) by this exact id — the
/// user pool is cached process-globally, so counts are never assumed (module
/// docs above).
fn create_user_persona(name: &str, body: &Path) -> String {
    let value = run_json(&[
        "pinvou",
        "personas",
        "create",
        "--name",
        name,
        "--file",
        body.to_str().unwrap(),
    ]);
    value["id"]
        .as_str()
        .expect("create must return the generated id")
        .to_owned()
}

/// Round-44 review: the body file is a model-context lane (the card body is
/// injected verbatim into the next turn's prompt on equip), so it joins the
/// gated-read rule — a credential-path file must be refused before any store
/// write.
#[test]
fn personas_create_refuses_a_credential_location_body_file() {
    let _env_guard = env_lock();
    let home = temp_home("create-credential-body");
    let file = home.path().join(".aws").join("credentials");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        "[default]\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\n",
    )
    .unwrap();

    let error = expect_failed(&[
        "pinvou",
        "personas",
        "create",
        "--name",
        "Leaky",
        "--file",
        file.to_str().unwrap(),
    ]);
    assert!(error.to_string().contains("refusing body file"), "{error}");

    // And nothing was stored: the refused body must not become a card. The
    // refusal fires before any store open, so the personas dir may not even
    // exist yet — a missing directory is zero cards, not a failure.
    let cards = std::fs::read_dir(home.user_personas_dir())
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(cards, 0, "the refused body must not persist as a card");
}

// ── parse-level coverage ────────────────────────────────────────────────────

#[test]
fn every_personas_subcommand_parses_and_invalid_usage_exits_two() {
    let _env_guard = env_lock();
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
        // The Personas command tree is dispatched exclusively through the
        // Personas variant; assert family + subcommand through the derived
        // Debug form (the types are not nameable from integration tests).
        let debug = parsed_debug(arguments);
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
        // Round-52 review: a fieldless update used to re-persist the card
        // unchanged and report success — a silent no-op — so it refuses as
        // usage, mirroring memory profile set's empty refusal.
        vec!["pinvou", "personas", "update", "p-1"],
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
        usage_error(arguments);
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
    let _env_guard = env_lock();
    let _home = temp_home("delete-yes-gate");

    // The refusal runs against an ISOLATED home (never the developer's real
    // one) and is pinned behaviorally: the visible catalog is byte-identical
    // across the refused run, which is the "no state change" the test name
    // claims.
    let before = run(&["pinvou", "personas", "list"]).expect("list before the refusal");
    let message = assert_usage(&["pinvou", "personas", "delete", "user-some-card"]);
    assert!(message.contains("--yes"), "{message}");
    let after = run(&["pinvou", "personas", "list"]).expect("list after the refusal");
    assert_eq!(
        before.stdout, after.stdout,
        "a refused delete must not change the visible catalog"
    );
}

/// Round-50 review: a `--source` filter matching nothing (a fresh home's
/// `user` pool) prints an explicit `No personas.` line in human mode
/// instead of silent empty stdout; the JSON deck stays the authoritative
/// empty pool. (The unfiltered pool never empties — builtins ship
/// embedded.)
#[test]
fn personas_list_prints_an_explicit_empty_line_in_human_mode() {
    let _env_guard = env_lock();
    let home = temp_home("list-empty");
    // The user-pool cache reloads on a directory-stamp change; an ABSENT
    // personas dir fails open to whatever the previous test in this process
    // cached (the same fail-open that protects a real user's cards from a
    // transient read failure). Materialize the empty dir so this test's
    // stamp is a real, empty one.
    std::fs::create_dir_all(home.path().join("user/personas"))
        .expect("materialize the empty user persona dir");

    let outcome = run(&["pinvou", "personas", "list", "--source", "user"])
        .expect("filtered list must succeed");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(
        outcome.stdout.contains("No personas."),
        "a filter matching nothing must say so in human mode: {:?}",
        outcome.stdout
    );

    let value = run_json(&["pinvou", "personas", "list", "--source", "user"]);
    assert_eq!(
        value["personas"].as_array().map(Vec::len),
        Some(0),
        "the json deck stays the authoritative empty pool: {value}"
    );
}

#[test]
fn personas_list_shows_builtin_catalog_and_source_filters() {
    let _env_guard = env_lock();
    let _home = temp_home("list-catalog");

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
    let _env_guard = env_lock();
    let home = temp_home("crud-round-trip");
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
    let error = expect_failed(&["pinvou", "personas", "show", "user-missing"]);
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
    let error = expect_failed(&[
        "pinvou",
        "personas",
        "update",
        "pinvou-card-creator",
        "--name",
        "X",
    ]);
    assert!(error.to_string().contains("personas update"), "{error}");
    let error = expect_failed(&[
        "pinvou",
        "personas",
        "delete",
        "pinvou-card-creator",
        "--yes",
    ]);
    assert!(error.to_string().contains("personas delete"), "{error}");
    // ...and the builtin catalog is untouched.
    assert!(
        run_json(&["pinvou", "personas", "show", "pinvou-card-creator"])
            .get("id")
            .is_some()
    );

    // delete without --yes is refused and keeps the card.
    assert_usage(&["pinvou", "personas", "delete", &id]);
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
    expect_failed(&["pinvou", "personas", "show", &id]);
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
    let _env_guard = env_lock();
    let home = temp_home("equip-round-trip");
    let session_id = create_session_fixture();
    let body_path = write_body_file("equip", "# Equip Expert\n\nbody\n");

    // Fresh session: nothing equipped.
    let outcome = run(&["pinvou", "personas", "active", &session_id]).expect("human active");
    assert_eq!(outcome.stdout, "none");
    let value = run_json(&["pinvou", "personas", "active", &session_id]);
    assert!(value.is_null(), "no active persona must serialize as null");

    // Create a user card to equip.
    let persona_id = create_user_persona("Equip Expert", &body_path);

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
    let sidecar = equipped_sidecar(&home, &session_id);
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
    let error = expect_failed(&["pinvou", "personas", "equip", &session_id, "user-missing"]);
    assert!(error.to_string().contains("unknown persona"), "{error}");

    // Session ids join onto the sidecar path, so traversal attempts are
    // usage errors, never writes outside the sessions root. Usage wins over
    // Failed: even with an unknown persona id, the malformed session id
    // reports the usage error.
    assert_usage(&["pinvou", "personas", "equip", "../escape", &persona_id]);
    assert_usage(&["pinvou", "personas", "equip", "../escape", "user-missing"]);
    assert_usage(&["pinvou", "personas", "active", "../escape"]);

    // A well-formed but nonexistent session id is a failed lookup (exit 1),
    // and must not materialize a stray sessions/<id>/ directory.
    let error = expect_failed(&[
        "pinvou",
        "personas",
        "equip",
        "no-such-session",
        &persona_id,
    ]);
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
    let _env_guard = env_lock();
    let home = temp_home("delete-sidecar-sweep");
    let session_a = create_session_fixture();
    let session_b = create_session_fixture();
    let body_path = write_body_file("sweep", "# Sweep Expert\n\nbody\n");

    // Two user cards: one to delete, one that must stay equipped.
    let deleted_id = create_user_persona("Sweep Expert", &body_path);
    let keeper_id = create_user_persona("Keeper Expert", &body_path);

    // Equip the doomed card on session A and the keeper on session B.
    run(&["pinvou", "personas", "equip", &session_a, &deleted_id]).unwrap();
    run(&["pinvou", "personas", "equip", &session_b, &keeper_id]).unwrap();
    let sidecar_a = equipped_sidecar(&home, &session_a);
    let sidecar_b = equipped_sidecar(&home, &session_b);
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
    let _env_guard = env_lock();
    let home = temp_home("delete-sidecar-large-body");
    let session_id = create_session_fixture();
    // 100 KiB: over the old 64 KiB sweep cap, far under the 4 MiB body cap.
    let big_body = format!("# Big Expert\n\n{}\n", "x".repeat(100 * 1024));
    let body_path = write_body_file("sweep-big", &big_body);

    let persona_id = create_user_persona("Big Expert", &body_path);
    run(&["pinvou", "personas", "equip", &session_id, &persona_id]).unwrap();
    let sidecar = equipped_sidecar(&home, &session_id);
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
    let _env_guard = env_lock();
    let _home = temp_home("row-control-chars");
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
    let _env_guard = env_lock();
    let home = temp_home("orphan-sidecar");
    let session_id = create_session_fixture();
    let body_path = write_body_file("orphan", "# Orphan Expert\n\nbody\n");

    let persona_id = create_user_persona("Orphan Expert", &body_path);
    run(&["pinvou", "personas", "equip", &session_id, &persona_id]).expect("equip must succeed");
    let sidecar = equipped_sidecar(&home, &session_id);
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

    let error = expect_failed(&["pinvou", "personas", "active", &session_id]);
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
    let _env_guard = env_lock();
    let home = temp_home("delete-sidecar-errors");
    let session_id = create_session_fixture();
    let body_path = write_body_file("sweep-errors", "# Stuck Expert\n\nbody\n");

    let persona_id = create_user_persona("Stuck Expert", &body_path);
    run(&["pinvou", "personas", "equip", &session_id, &persona_id]).expect("equip must succeed");

    // A session directory whose sidecar cannot be inspected at all: a
    // DIRECTORY where the sweep expects a file. The sweep cannot tell whether
    // it references the deleted card, so it must be named rather than skipped.
    let blocked = equipped_sidecar(&home, "ghost-session");
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
    let _env_guard = env_lock();
    let home = temp_home("stdin-over-cap");

    // One byte past the cap, so the refusal is exactly the over-cap case and
    // an off-by-one in the cap cannot pass.
    let body = "a".repeat(4 * 1024 * 1024 + 1);
    let body_path = write_body_file("stdin-over-cap", &body);

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pinvou"))
        .args(["personas", "create", "--name", "Too Big", "--stdin"])
        .env("PINVOU3_HOME", home.path())
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

/// Round-36 review minor: a flag-shaped token in the persona-id slot (a
/// truncated `personas show --source <id>`) passed the id guard and became
/// an exit-1 store lookup for a persona named after the flag; it must be
/// refused as usage, naming the real mistake. (`equip`'s persona slot
/// already had this guard; show/delete now share it.)
#[test]
fn flag_shaped_token_in_the_id_slot_is_a_usage_error() {
    for arguments in [
        ["pinvou", "personas", "show", "--source"],
        ["pinvou", "personas", "delete", "--yes"],
    ] {
        let message = usage_error(&arguments);
        assert!(
            message.contains("flag-shaped"),
            "the refusal must name the flag-shaped token: {message}"
        );
    }
}
