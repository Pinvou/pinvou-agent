//! Contract tests for the `connectors` family (GUI-parity project).
//!
//! Parse-level coverage runs against the typed command tree. Execute-level
//! coverage is strictly hermetic: every test runs against a throwaway
//! `PINVOU3_HOME` (ENV_LOCK serialization, same pattern as
//! `sessions_contract.rs`) and additionally points `PATH` at an empty
//! directory so the vendor connector CLIs (`lark-cli` / `wecom-cli` / `dws`
//! / `tmeet`) are deterministically absent regardless of what the host has
//! installed.
//!
//! Out of hermetic scope (not executed here, behavior mirrors the GUI feature
//! functions): `ensure-cli` (downloads pinned archives / runs npm), `ima
//! connect` against the live ima OpenAPI, and the successful legs of the
//! vendor `connect` login flows, which need the real vendor CLIs.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use pinvou_cli::{CliError, CliOutcome, ExitCode, execute, parse_args};

/// Serialises tests that mutate the process-global `PINVOU3_HOME` / `PATH`
/// environment variables, preventing data races when the parallel test runner
/// executes them concurrently.
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
            "pinvou-cli-connectors-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let previous = std::env::var_os("PINVOU3_HOME");
        // SAFETY: the caller holds ENV_LOCK for the whole test, so env writes
        // are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
        Self { previous, root }
    }

    /// The `<id>_disabled` marker file `platform::connector_state` reads.
    fn disabled_marker(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}_disabled"))
    }

    /// The scope bridge file the enable/disable switch syncs through
    /// `marketplace::sync_disabled_bundles_for_connector_switch`.
    fn disabled_bundles_file(&self) -> PathBuf {
        self.root.join("disabled_bundles.json")
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

/// Points `PATH` at an empty directory so no vendor connector CLI is
/// resolvable, whatever the host has installed. Must be constructed while
/// ENV_LOCK is held; restores the previous value on drop.
struct VendorCliGuard {
    previous: Option<OsString>,
    empty_bin: PathBuf,
}

impl VendorCliGuard {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let empty_bin = std::env::temp_dir().join(format!(
            "pinvou-cli-connectors-empty-path-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&empty_bin).unwrap();
        let previous = std::env::var_os("PATH");
        // SAFETY: the caller holds ENV_LOCK for the whole test.
        unsafe { std::env::set_var("PATH", &empty_bin) };
        Self {
            previous,
            empty_bin,
        }
    }
}

impl Drop for VendorCliGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            // SAFETY: ENV_LOCK is held by the owning test.
            Some(value) => unsafe { std::env::set_var("PATH", value) },
            // SAFETY: ENV_LOCK is held by the owning test.
            None => unsafe { std::env::remove_var("PATH") },
        }
        let _ = std::fs::remove_dir_all(&self.empty_bin);
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

// ── parse-level coverage ────────────────────────────────────────────────────

#[test]
fn every_connectors_subcommand_parses_and_invalid_usage_exits_two() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let valid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "connectors", "status"],
        vec!["pinvou", "connectors", "status", "feishu"],
        vec!["pinvou", "connectors", "status", "wecom"],
        vec!["pinvou", "connectors", "status", "dingtalk"],
        vec!["pinvou", "connectors", "status", "tmeet"],
        vec!["pinvou", "connectors", "ensure-cli", "feishu"],
        vec!["pinvou", "connectors", "enable", "wecom"],
        vec!["pinvou", "connectors", "disable", "dingtalk"],
        vec!["pinvou", "connectors", "logout", "tmeet"],
        vec!["pinvou", "connectors", "apply-skills", "feishu"],
        vec!["pinvou", "connectors", "connect", "feishu"],
        vec![
            "pinvou",
            "connectors",
            "connect",
            "tmeet",
            "--timeout",
            "60",
        ],
        vec!["pinvou", "connectors", "ima", "status"],
        vec!["pinvou", "connectors", "ima", "logout"],
        vec![
            "pinvou",
            "connectors",
            "ima",
            "connect",
            "--client-id-env",
            "IMA_CLIENT_ID",
        ],
        vec![
            "pinvou",
            "connectors",
            "ima",
            "connect",
            "--client-id-env",
            "IMA_CLIENT_ID",
            "--api-key-env",
            "IMA_API_KEY",
        ],
        vec![
            "pinvou",
            "connectors",
            "ima",
            "connect",
            "--client-id-env",
            "IMA_CLIENT_ID",
            "--api-key-stdin",
        ],
    ];
    for arguments in &valid {
        let parsed = parse_args(arguments.clone())
            .unwrap_or_else(|error| panic!("{arguments:?} must parse: {error}"));
        // The Connectors command tree is dispatched exclusively through the
        // Connectors variant; assert family + subcommand through the derived
        // Debug form (the types are not nameable from integration tests).
        let debug = format!("{:?}", parsed.command());
        let variant = match (arguments[2], arguments.get(3).copied().unwrap_or("")) {
            ("status", _) => "Status",
            ("ensure-cli", _) => "EnsureCli",
            ("enable", _) => "Enable",
            ("disable", _) => "Disable",
            ("logout", _) => "Logout",
            ("apply-skills", _) => "ApplySkills",
            ("connect", _) => "Connect",
            ("ima", "status") => "Ima(Status",
            ("ima", "logout") => "Ima(Logout",
            ("ima", "connect") => "Ima(Connect",
            other => panic!("unmapped subcommand {other:?}"),
        };
        assert!(debug.starts_with("Connectors("), "{arguments:?} -> {debug}");
        assert!(debug.contains(variant), "{arguments:?} -> {debug}");
    }

    let invalid: Vec<Vec<&str>> = vec![
        vec!["pinvou", "connectors"],
        vec!["pinvou", "connectors", "bogus"],
        // Unknown connector names must be rejected with the valid set named.
        vec!["pinvou", "connectors", "status", "ima"],
        vec!["pinvou", "connectors", "enable"],
        vec!["pinvou", "connectors", "enable", "bogus"],
        vec!["pinvou", "connectors", "disable", "bogus"],
        vec!["pinvou", "connectors", "logout", "bogus"],
        vec!["pinvou", "connectors", "apply-skills", "bogus"],
        vec!["pinvou", "connectors", "ensure-cli", "bogus"],
        vec!["pinvou", "connectors", "connect", "bogus"],
        // The single-connector subcommands accept no options.
        vec!["pinvou", "connectors", "enable", "feishu", "--json"],
        vec!["pinvou", "connectors", "disable", "feishu", "--extra"],
        vec!["pinvou", "connectors", "logout", "feishu", "--x"],
        vec!["pinvou", "connectors", "apply-skills", "feishu", "--y"],
        // connect --timeout must be a positive integer when present.
        vec!["pinvou", "connectors", "connect", "feishu", "--timeout"],
        vec![
            "pinvou",
            "connectors",
            "connect",
            "feishu",
            "--timeout",
            "abc",
        ],
        vec![
            "pinvou",
            "connectors",
            "connect",
            "feishu",
            "--timeout",
            "0",
        ],
        vec![
            "pinvou",
            "connectors",
            "connect",
            "feishu",
            "--timeout",
            "-5",
        ],
        vec![
            "pinvou",
            "connectors",
            "connect",
            "feishu",
            "--timeout",
            "1.5",
        ],
        vec!["pinvou", "connectors", "connect", "feishu", "--bogus"],
        vec![
            "pinvou",
            "connectors",
            "connect",
            "feishu",
            "--timeout",
            "5",
            "--timeout",
            "6",
        ],
        // ima subfamily.
        vec!["pinvou", "connectors", "ima"],
        vec!["pinvou", "connectors", "ima", "bogus"],
        vec!["pinvou", "connectors", "ima", "status", "--extra"],
        vec!["pinvou", "connectors", "ima", "logout", "--extra"],
        // ima connect: secrets only via env/stdin, and --client-id-env is
        // mandatory at parse time.
        vec!["pinvou", "connectors", "ima", "connect"],
        vec![
            "pinvou",
            "connectors",
            "ima",
            "connect",
            "--api-key-env",
            "K",
        ],
        vec!["pinvou", "connectors", "ima", "connect", "--api-key-stdin"],
        vec!["pinvou", "connectors", "ima", "connect", "--client-id-env"],
        vec![
            "pinvou",
            "connectors",
            "ima",
            "connect",
            "--client-id-env",
            "IMA_CLIENT_ID",
            "--bogus",
        ],
    ];
    for arguments in &invalid {
        let error = parse_args(arguments.clone()).expect_err(&arguments.join(" "));
        assert_eq!(error.exit_code(), ExitCode::Usage, "{arguments:?}");
    }

    // The unknown-connector usage error must name the valid connector set.
    for subcommand in ["status", "enable", "logout", "connect"] {
        let error = parse_args(vec!["pinvou", "connectors", subcommand, "bogus"])
            .expect_err("unknown connector must be rejected");
        assert_eq!(error.exit_code(), ExitCode::Usage, "{subcommand}");
        for id in ["feishu", "wecom", "dingtalk", "tmeet"] {
            assert!(
                error.to_string().contains(id),
                "usage error for {subcommand} must name {id}: {error}"
            );
        }
    }
}

// ── execute-level coverage (hermetic: temp PINVOU3_HOME, empty PATH) ───────

#[test]
fn connectors_status_zero_state_reports_every_connector_uninstalled_and_enabled() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("status-zero");
    let _path = VendorCliGuard::new();

    let outcome = run(&["pinvou", "connectors", "status"]).expect("status must succeed");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    let lines: Vec<&str> = outcome.stdout.lines().collect();
    assert_eq!(lines.len(), 5, "four vendor connectors plus ima");
    for (index, id) in ["feishu", "wecom", "dingtalk", "tmeet"]
        .into_iter()
        .enumerate()
    {
        let columns: Vec<&str> = lines[index].split('\t').collect();
        assert_eq!(columns[0], id);
        assert_eq!(columns[1], "installed=no", "{id}");
        assert_eq!(columns[2], "connected=no", "{id}");
        assert_eq!(columns[3], "enabled=yes", "{id}");
        assert_eq!(columns[4], "skills=no", "{id}");
    }
    assert_eq!(
        lines[4],
        "ima\tconnected=no\tcredentials=no\tskill_installed=no"
    );

    let value = run_json(&["pinvou", "connectors", "status"]);
    let entries = value["connectors"].as_array().unwrap();
    assert_eq!(entries.len(), 5);
    let ids: Vec<&str> = entries
        .iter()
        .map(|entry| entry["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["feishu", "wecom", "dingtalk", "tmeet", "ima"]);
    for entry in &entries[..4] {
        assert_eq!(entry["installed"], false, "{entry}");
        assert_eq!(entry["connected"], false, "{entry}");
        assert_eq!(entry["enabled"], true, "{entry}");
        assert_eq!(entry["skills_applied"], false, "{entry}");
        assert_eq!(entry["ok"], false, "{entry}");
    }
    // wecom/tmeet carry the upgrade_required three-state even uninstalled.
    assert_eq!(entries[1]["upgrade_required"], false);
    assert_eq!(entries[3]["upgrade_required"], false);
    assert_eq!(entries[4]["credentials_present"], false);
    assert_eq!(entries[4]["skill_installed"], false);
    assert_eq!(entries[4]["connected"], false);

    // A single-connector status narrows the report to that connector.
    let value = run_json(&["pinvou", "connectors", "status", "tmeet"]);
    let entries = value["connectors"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], "tmeet");
}

#[test]
fn connectors_enable_disable_round_trip_persists_the_marker_and_scope_bridge() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("enable-disable");
    let _path = VendorCliGuard::new();

    assert!(!home.disabled_marker("feishu").exists());

    // disable writes the marker the GUI gate reads and mirrors the switch
    // into disabled_bundles.json (execpolicy CLI hard-block / materialization).
    let value = run_json(&["pinvou", "connectors", "disable", "feishu"]);
    assert_eq!(value["ok"], true);
    assert_eq!(value["id"], "feishu");
    assert_eq!(value["action"], "disabled");
    assert_eq!(value["enabled"], false);
    assert_eq!(value["connected"], false);
    assert_eq!(value["skills_should_show"], false);
    assert_eq!(value["skills_refresh"], "app-only");
    assert!(home.disabled_marker("feishu").is_file());
    assert!(
        std::fs::read_to_string(home.disabled_bundles_file())
            .unwrap()
            .contains("feishu")
    );

    // status reflects the persisted marker.
    let value = run_json(&["pinvou", "connectors", "status", "feishu"]);
    assert_eq!(value["connectors"][0]["enabled"], false);
    let outcome = run(&["pinvou", "connectors", "status", "feishu"]).expect("human status");
    assert!(outcome.stdout.contains("enabled=no"));

    // enable clears the marker and removes the bridge entry again.
    let value = run_json(&["pinvou", "connectors", "enable", "feishu"]);
    assert_eq!(value["action"], "enabled");
    assert_eq!(value["enabled"], true);
    assert!(!home.disabled_marker("feishu").exists());
    assert!(
        !std::fs::read_to_string(home.disabled_bundles_file())
            .unwrap()
            .contains("feishu")
    );
    let value = run_json(&["pinvou", "connectors", "status", "feishu"]);
    assert_eq!(value["connectors"][0]["enabled"], true);

    // enable is idempotent (the marker removal is a no-op when absent).
    let value = run_json(&["pinvou", "connectors", "enable", "feishu"]);
    assert_eq!(value["enabled"], true);
    assert!(!home.disabled_marker("feishu").exists());

    // disable is idempotent too (the marker write is an overwrite).
    let value = run_json(&["pinvou", "connectors", "disable", "feishu"]);
    assert_eq!(value["enabled"], false);
    let value = run_json(&["pinvou", "connectors", "disable", "feishu"]);
    assert_eq!(value["enabled"], false);
    assert!(home.disabled_marker("feishu").is_file());
}

#[test]
fn connectors_logout_and_apply_skills_on_uninstalled_connector_fail_cleanly() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("uninstalled-errors");
    let _path = VendorCliGuard::new();

    // feishu logout shells out to `lark-cli auth logout`; without the binary
    // that is a clean host failure naming the install hint, never a panic.
    let error = run(&["pinvou", "connectors", "logout", "feishu"])
        .expect_err("logout without lark-cli must fail");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("lark-cli"), "{error}");
    assert!(error.to_string().contains("ensure-cli feishu"), "{error}");

    // apply-skills probes the connected state through the vendor CLI first,
    // so it fails with the same clean error on an uninstalled connector.
    let error = run(&["pinvou", "connectors", "apply-skills", "feishu"])
        .expect_err("apply-skills without lark-cli must fail");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("ensure-cli feishu"), "{error}");

    // dingtalk/tmeet treat "CLI not installed" as already logged out.
    for id in ["dingtalk", "tmeet"] {
        let value = run_json(&["pinvou", "connectors", "logout", id]);
        assert_eq!(value["ok"], true, "{id}");
        assert_eq!(value["id"], id, "{id}");
        assert_eq!(value["installed"], false, "{id}");
    }
}

#[test]
fn connectors_connect_without_vendor_cli_fails_fast_with_install_hint() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("connect-uninstalled");
    let _path = VendorCliGuard::new();

    for id in ["feishu", "wecom", "dingtalk", "tmeet"] {
        let error = run(&["pinvou", "connectors", "connect", id])
            .expect_err("connect without the vendor CLI must fail immediately");
        assert_eq!(error.exit_code(), ExitCode::Failed, "{id}");
        assert!(
            error.to_string().contains("failed to start"),
            "{id}: {error}"
        );
        assert!(
            error.to_string().contains(&format!("ensure-cli {id}")),
            "{id}: {error}"
        );
    }

    // The --timeout flag parses through to the same fast failure.
    let error = run(&[
        "pinvou",
        "connectors",
        "connect",
        "dingtalk",
        "--timeout",
        "1",
    ])
    .expect_err("connect without dws must fail immediately");
    assert_eq!(error.exit_code(), ExitCode::Failed);
}

#[test]
fn connectors_ima_reports_zero_state_and_missing_client_id_env() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("ima-zero");
    let _path = VendorCliGuard::new();

    let value = run_json(&["pinvou", "connectors", "ima", "status"]);
    assert_eq!(value["id"], "ima");
    assert_eq!(value["connected"], false);
    assert_eq!(value["credentials_present"], false);
    assert_eq!(value["skill_installed"], false);
    let outcome = run(&["pinvou", "connectors", "ima", "status"]).expect("human ima status");
    assert_eq!(
        outcome.stdout,
        "ima\tconnected=no\tcredentials=no\tskill_installed=no"
    );

    // ima connect resolves the client id from the environment; a missing
    // variable is a clean host failure before any secret or network access.
    let error = run(&[
        "pinvou",
        "connectors",
        "ima",
        "connect",
        "--client-id-env",
        "PINVOU_CLI_TEST_IMA_CLIENT_ID_UNSET",
        "--api-key-stdin",
    ])
    .expect_err("missing client id env must fail before reading stdin");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("not set"), "{error}");
    assert!(
        error
            .to_string()
            .contains("PINVOU_CLI_TEST_IMA_CLIENT_ID_UNSET"),
        "{error}"
    );
}
