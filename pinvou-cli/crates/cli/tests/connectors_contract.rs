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
    /// The extra process-global state both constructor forms isolate (npm
    /// prefixes resolve through these outside PATH).
    isolated: Option<Vec<(&'static str, Option<OsString>)>>,
    empty_home: Option<PathBuf>,
}

impl VendorCliGuard {
    /// Isolation shared by both constructor forms: vendor-CLI resolution
    /// also consults the npm global prefixes outside PATH (a real
    /// `~/.npm-global/bin/dws` on the dev machine would defeat a fake
    /// staged on PATH), so `HOME` is pointed at a throwaway directory and
    /// the npm prefix envs are cleared. Returns the throwaway home (removed
    /// on drop) and the state list `Drop` re-applies.
    fn isolate_home_and_npm_prefixes() -> (PathBuf, Vec<(&'static str, Option<OsString>)>) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let empty_home = std::env::temp_dir().join(format!(
            "pinvou-cli-connectors-empty-home-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&empty_home).unwrap();
        let isolated = vec![
            ("HOME", Some(empty_home.clone().into_os_string())),
            ("NPM_CONFIG_PREFIX", None),
            ("npm_config_prefix", None),
        ];
        for (key, value) in &isolated {
            match value {
                // SAFETY: the caller holds ENV_LOCK for the whole test.
                Some(value) => unsafe { std::env::set_var(key, value) },
                // SAFETY: the caller holds ENV_LOCK for the whole test.
                None => unsafe { std::env::remove_var(key) },
            }
        }
        (empty_home, isolated)
    }

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
        let (empty_home, isolated) = Self::isolate_home_and_npm_prefixes();
        let previous = std::env::var_os("PATH");
        // SAFETY: the caller holds ENV_LOCK for the whole test.
        unsafe { std::env::set_var("PATH", &empty_bin) };
        Self {
            previous,
            empty_bin,
            isolated: Some(isolated),
            empty_home: Some(empty_home),
        }
    }

    /// Points `PATH` at a caller-provided directory — the fake-vendor-CLI
    /// tests stage scripted stand-ins there (unix only). Isolates `HOME` and
    /// the npm prefix envs exactly like `new()`: a dev machine's
    /// `~/.npm-global/bin/dws` or `$NPM_CONFIG_PREFIX/bin/tmeet` must not
    /// bypass the staged fake.
    fn new_at(bin: PathBuf) -> Self {
        let (empty_home, isolated) = Self::isolate_home_and_npm_prefixes();
        let previous = std::env::var_os("PATH");
        // SAFETY: the caller holds ENV_LOCK for the whole test.
        unsafe { std::env::set_var("PATH", &bin) };
        Self {
            previous,
            empty_bin: bin,
            isolated: Some(isolated),
            empty_home: Some(empty_home),
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
        if let Some(isolated) = self.isolated.take() {
            for (key, value) in isolated {
                match value {
                    // SAFETY: ENV_LOCK is held by the owning test.
                    Some(value) => unsafe { std::env::set_var(key, value) },
                    // SAFETY: ENV_LOCK is held by the owning test.
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
        if let Some(home) = self.empty_home.take() {
            let _ = std::fs::remove_dir_all(&home);
        }
        let _ = std::fs::remove_dir_all(&self.empty_bin);
    }
}

/// Sets one environment variable for the duration of the test and restores
/// the previous value on drop (panic-safe, unlike an inline restore after the
/// assertions). Must be constructed while ENV_LOCK is held.
#[cfg(unix)]
struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

#[cfg(unix)]
impl EnvVarGuard {
    fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: the caller holds ENV_LOCK for the whole test.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
}

#[cfg(unix)]
impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            // SAFETY: ENV_LOCK is held by the owning test.
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            // SAFETY: ENV_LOCK is held by the owning test.
            None => unsafe { std::env::remove_var(self.key) },
        }
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
        vec!["pinvou", "connectors", "logout", "tmeet", "--yes"],
        vec!["pinvou", "connectors", "ima", "logout", "--yes"],
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
        assert_eq!(
            entry["installed"], false,
            "uninstalled entries report installed: false"
        );
        assert_eq!(
            entry["connected"], false,
            "uninstalled entries report connected: false"
        );
        assert_eq!(entry["enabled"], true, "catalog entries default to enabled");
        assert_eq!(
            entry["skills_applied"], false,
            "uninstalled entries report skills_applied: false"
        );
        assert_eq!(entry["ok"], false, "uninstalled entries report ok: false");
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

    // Logout destroys stored credentials, so it follows the destructive
    // convention: without --yes it is a usage error before anything runs.
    for id in ["feishu", "wecom"] {
        let error = run(&["pinvou", "connectors", "logout", id])
            .expect_err("logout without --yes must be refused");
        assert_eq!(error.exit_code(), ExitCode::Usage, "{id}");
        assert!(error.to_string().contains("--yes"), "{error}");
    }

    // With --yes, feishu logout shells out to `lark-cli auth logout`; without
    // the binary that is a clean host failure naming the install hint.
    let error = run(&["pinvou", "connectors", "logout", "feishu", "--yes"])
        .expect_err("logout without lark-cli must fail");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("lark-cli"), "{error}");
    assert!(error.to_string().contains("ensure-cli feishu"), "{error}");

    // apply-skills mirrors the GUI: an unprobeable vendor CLI counts as
    // "not connected" and the command succeeds with skills hidden (the same
    // degraded outcome as the GUI's disconnected state).
    let outcome = run(&["pinvou", "connectors", "apply-skills", "feishu"])
        .expect("apply-skills treats an uninstalled CLI as not connected");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(
        outcome.stdout.contains("connected: no"),
        "apply-skills should report the vendor CLI as not connected"
    );
    assert!(
        outcome.stdout.contains("skills should show: no"),
        "apply-skills should hide skills when not connected"
    );

    // dingtalk/tmeet treat "CLI not installed" as already logged out.
    for id in ["dingtalk", "tmeet"] {
        let value = run_json(&["pinvou", "connectors", "logout", id, "--yes"]);
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
            // The resolution error fires before spawn: neither a managed
            // install nor a PATH candidate exists.
            error.to_string().contains("was not found"),
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

// ── fake vendor CLI coverage ────────────────────────────────────────────────
// The empty-PATH tests above prove the absent-CLI behavior; these tests stage
// scripted vendor stand-ins so the real spawn paths (argv construction, live
// login-link surfacing, the wecom QR note) execute against actual child
// processes. Unix only: the stand-ins are /bin/sh scripts.

#[cfg(unix)]
fn write_fake_cli(bin: &std::path::Path, name: &str, version: &str, body: &str) {
    write_fake_cli_logged(bin, name, version, body, None);
}

/// Writes a scripted vendor CLI stand-in. Every invocation appends its argv
/// to `args_log` (when given) before dispatching, so tests can pin the exact
/// argv each spawn receives; `--version` is answered by the prelude.
#[cfg(unix)]
fn write_fake_cli_logged(
    bin: &std::path::Path,
    name: &str,
    version: &str,
    body: &str,
    args_log: Option<&std::path::Path>,
) {
    use std::os::unix::fs::PermissionsExt as _;
    let log_line = match args_log {
        Some(path) => format!("printf '%s\\n' \"$@\" >> {}\n", path.display()),
        None => String::new(),
    };
    let script = format!(
        "#!/bin/sh\n{log_line}if [ \"$1\" = \"--version\" ]; then echo \"{version}\"; exit 0; fi\n{body}\nexit 1\n"
    );
    let path = bin.join(name);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
#[cfg(unix)]
fn vendor_cli_argv_is_passed_exactly_once() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("argv-once");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-connectors-fake-bin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    let args_file = bin.join("seen-args.txt");
    // Every spawn appends its argv to the log; doubled argv (the regression
    // this pins) would repeat entries. The status response reports feishu as
    // ready so the whole probe chain runs.
    write_fake_cli_logged(
        &bin,
        "lark-cli",
        "lark-cli 1.2.3",
        "if [ \"$1\" = \"auth\" ]; then echo '{\"identities\":{\"user\":{\"status\":\"ready\"}}}'; exit 0; fi\n",
        Some(&args_file),
    );
    let _path = VendorCliGuard::new_at(bin.clone());

    let outcome = run(&[
        "pinvou",
        "connectors",
        "status",
        "feishu",
        "--output",
        "json",
    ])
    .expect("status with a fake vendor CLI must succeed");
    let status: serde_json::Value =
        serde_json::from_str(&outcome.stdout).expect("single-line JSON status");
    assert_eq!(
        status["connectors"][0]["connected"],
        serde_json::json!(true)
    );

    let seen = std::fs::read_to_string(&args_file).unwrap();
    let arguments: Vec<&str> = seen.lines().collect();
    assert_eq!(
        arguments,
        vec!["--version", "auth", "status", "--json"],
        "vendor argv must be passed exactly once per spawn"
    );
    let _ = std::fs::remove_dir_all(&bin);
}

#[test]
#[cfg(unix)]
fn connect_failure_carries_the_captured_login_link() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("connect-note");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-connectors-fake-bin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    // The fake login child prints the authorize link and then blocks (like
    // the real vendor CLIs, which stay alive until the user authorizes);
    // the connect budget expires first and the error must carry the link.
    write_fake_cli(
        &bin,
        "dws",
        "dws version 1.0.0",
        "if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"status\" ]; then echo '{\"authenticated\": false}'; exit 0; fi\nif [ \"$1\" = \"auth\" ] && [ \"$2\" = \"login\" ]; then echo \"visit https://login.dingtalk.com/oauth/authorize?x=1 to continue\"; sleep 30; exit 0; fi\n",
    );
    let _path = VendorCliGuard::new_at(bin.clone());

    let error = run(&[
        "pinvou",
        "connectors",
        "connect",
        "dingtalk",
        "--timeout",
        "2",
    ])
    .expect_err("the fake login never completes");
    let message = error.to_string();
    assert!(
        message.contains("https://login.dingtalk.com/oauth/authorize?x=1"),
        "the failure must surface the captured login link: {message}"
    );
    let _ = std::fs::remove_dir_all(&bin);
}

#[test]
#[cfg(unix)]
fn connect_does_not_wait_out_the_url_window_when_no_user_code_arrives() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("connect-no-code");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-connectors-fake-bin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    // dws prints only the authorize link and exits 0 — no user code line, the
    // feishu/tmeet/wecom situation. The old `while url.is_none() ||
    // user_code.is_none()` wait burned the whole 60s `login_url_wait_secs`
    // window here even though the link was already captured; the GUI loops
    // break on the first URL (feishu.rs / tmeet.rs), so the CLI must move
    // straight on to the exit status and the auth probe. The probe reports
    // unauthenticated and the failure must still carry the captured link.
    write_fake_cli(
        &bin,
        "dws",
        "dws version 1.0.0",
        "if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"login\" ]; then echo \"visit https://login.dingtalk.com/oauth/authorize?x=1 to continue\"; exit 0; fi\nif [ \"$1\" = \"auth\" ] && [ \"$2\" = \"status\" ]; then echo '{\"authenticated\": false}'; exit 0; fi\n",
    );
    let _path = VendorCliGuard::new_at(bin.clone());

    let started = std::time::Instant::now();
    let error = run(&["pinvou", "connectors", "connect", "dingtalk"])
        .expect_err("the fake never authenticates");
    // Old behavior waited out the full 60s URL window before even checking
    // the child exit; fixed behavior finishes in a small fraction of that.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "connect must not wait out login_url_wait_secs once the URL is captured"
    );
    let message = error.to_string();
    assert!(
        message.contains("https://login.dingtalk.com/oauth/authorize?x=1"),
        "the failure must surface the captured login link: {message}"
    );
    assert!(
        message.contains("exited before authorization completed"),
        "{message}"
    );
    let _ = std::fs::remove_dir_all(&bin);
}

#[test]
#[cfg(unix)]
fn connect_keeps_draining_the_dingtalk_user_code_after_a_bare_url() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("connect-dingtalk-code");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-connectors-fake-bin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    // dingtalk's login page needs the `user_code` the vendor CLI prints on a
    // separate line after the authorize link; the GUI loop keeps draining
    // after the bare URL until both halves are in hand. The CLI must do the
    // same while the vendor CLI is still running — the old break-on-first-URL
    // loop abandoned the code line in the channel and surfaced a link whose
    // page cannot be completed without the code. The URL-only-and-exited
    // case above stays fast: once the child exits no code can arrive.
    write_fake_cli(
        &bin,
        "dws",
        "dws version 1.0.0",
        "if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"login\" ]; then echo \"visit https://login.dingtalk.com/oauth/authorize?x=1 to continue\"; sleep 1; echo \"user code: DTK123\"; sleep 300; exit 0; fi\nif [ \"$1\" = \"auth\" ] && [ \"$2\" = \"status\" ]; then echo '{\"authenticated\": false}'; exit 0; fi\n",
    );
    let _path = VendorCliGuard::new_at(bin.clone());

    let started = std::time::Instant::now();
    let error = run(&[
        "pinvou",
        "connectors",
        "connect",
        "dingtalk",
        "--timeout",
        "6",
    ])
    .expect_err("the fake never authorizes");
    // The code line arrives ~1s after the link; the drain must have kept
    // going instead of breaking at the URL, and the budget (6s) is what
    // ends the run — not the URL window (60s).
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "connect must end on the connect budget, not the URL window"
    );
    let message = error.to_string();
    assert!(
        message.contains("user code: DTK123"),
        "the failure must surface the captured user code: {message}"
    );
    assert!(
        message.contains("https://login.dingtalk.com/oauth/authorize?x=1"),
        "the failure must surface the captured login link: {message}"
    );
    let _ = std::fs::remove_dir_all(&bin);
}

#[test]
#[cfg(unix)]
fn wecom_connect_surfaces_the_qr_file_while_it_exists() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("wecom-qr");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-connectors-fake-bin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    // The vendor writes `qr.png` into its working directory around the time
    // it prints the (landing-page) URL, then blocks waiting for the scan.
    write_fake_cli(
        &bin,
        "wecom-cli",
        "wecom-cli 1.9.9 (build 1)",
        "if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"init\" ]; then printf 'png' > qr.png; echo \"login at https://work.weixin.qq.com/landing?x=1\"; sleep 30; exit 0; fi\n",
    );
    let _path = VendorCliGuard::new_at(bin.clone());

    let error = run(&["pinvou", "connectors", "connect", "wecom", "--timeout", "3"])
        .expect_err("the fake login never completes");
    let message = error.to_string();
    assert!(
        message.contains("scan-qr-file:"),
        "the failure must surface the QR file path: {message}"
    );
    assert!(
        message.contains("https://work.weixin.qq.com/landing?x=1"),
        "the failure must surface the captured login link: {message}"
    );
    let _ = std::fs::remove_dir_all(&bin);
}

#[test]
#[cfg(unix)]
fn logout_runs_the_real_auth_logout_for_a_below_minimum_tmeet() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("logout-old-tmeet");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-connectors-fake-bin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    let args_file = bin.join("seen-args.txt");
    // tmeet 1.0.10 parses but sits below the 1.0.15 install gate: status /
    // ensure-cli treat it as upgrade_required, yet the vendor credentials it
    // holds are real — logout must still run `tmeet auth logout` (GUI
    // parity: tmeet_logout gates on "the version parses", not on the
    // minimum). Without that, logout reports success while the credentials
    // stay on disk.
    write_fake_cli_logged(
        &bin,
        "tmeet",
        "tmeet version 1.0.10",
        "if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"logout\" ]; then echo \"Logged out\"; exit 0; fi\n",
        Some(&args_file),
    );
    let _path = VendorCliGuard::new_at(bin.clone());

    let value = run_json(&["pinvou", "connectors", "logout", "tmeet", "--yes"]);
    assert_eq!(value["ok"], true);
    assert_eq!(value["id"], "tmeet");
    assert_eq!(
        value["installed"], true,
        "a below-minimum install must not be reported as not-installed"
    );

    let seen = std::fs::read_to_string(&args_file).unwrap();
    let arguments: Vec<&str> = seen.lines().collect();
    assert_eq!(
        arguments,
        vec!["--version", "auth", "logout"],
        "logout must run the real `tmeet auth logout` even below the minimum version"
    );
    let _ = std::fs::remove_dir_all(&bin);
}

#[test]
#[cfg(unix)]
fn logout_success_leg_spawns_auth_logout_once_and_flags_the_store_disconnected() {
    use pinvou3_lib::features::marketplace::store::{BundleRecord, BundleSource, BundleStore};

    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("logout-success");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-connectors-fake-bin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    let args_file = bin.join("seen-args.txt");
    // Usable-version fakes: `--version` answers through the prelude, the
    // logout subcommand exits 0 like a real logged-out vendor CLI.
    write_fake_cli_logged(
        &bin,
        "dws",
        "dws version 1.0.0",
        "if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"logout\" ]; then exit 0; fi\n",
        Some(&args_file),
    );
    write_fake_cli_logged(
        &bin,
        "tmeet",
        "tmeet version 1.0.15",
        "if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"logout\" ]; then exit 0; fi\n",
        Some(&args_file),
    );
    let _path = VendorCliGuard::new_at(bin.clone());

    // Seed connected records first: mark_degraded is a no-op on a missing
    // record, so only a seeded record proves the logout mirror flipped it.
    let store = BundleStore::new();
    for id in ["dingtalk", "tmeet"] {
        store
            .upsert(BundleRecord::installed_now(id, BundleSource::Builtin))
            .expect("seed the bundle store record");
    }

    for id in ["dingtalk", "tmeet"] {
        let value = run_json(&["pinvou", "connectors", "logout", id, "--yes"]);
        assert_eq!(value["ok"], true, "{id}");
        assert_eq!(value["id"], id, "{id}");
        assert_eq!(value["installed"], true, "{id}");
    }

    // Exactly one `--version` probe + one `auth logout [--yes]` spawn per
    // connector, in call order (dingtalk carries `--yes`, tmeet does not).
    let seen = std::fs::read_to_string(&args_file).unwrap();
    let arguments: Vec<&str> = seen.lines().collect();
    assert_eq!(
        arguments,
        vec![
            "--version",
            "auth",
            "logout",
            "--yes",
            "--version",
            "auth",
            "logout",
        ],
        "each success-leg logout must spawn auth logout exactly once"
    );

    // The store mirror must flip the seeded records to the disconnected
    // state (degraded set to the shared bundle_store_on_disconnected reason
    // the GUI renders — the store's "connected=false").
    for id in ["dingtalk", "tmeet"] {
        let record = store
            .get(id)
            .expect("store read must succeed")
            .expect("seeded record must be kept");
        assert_eq!(
            record.degraded.as_deref(),
            Some("已断开授权：配套技能已随断开移除，重新连接即可恢复"),
            "{id}: logout must mark the store record disconnected"
        );
    }
    let _ = std::fs::remove_dir_all(&bin);
}

#[test]
#[cfg(unix)]
fn status_finds_a_gui_installed_npm_prefix_cli() {
    use std::os::unix::fs::PermissionsExt as _;
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("npm-prefix");
    // The GUI's tmeet install applies the user npm prefix, so a GUI-installed
    // tmeet lives in <prefix>/bin/tmeet — typically not on PATH. Resolution
    // must find it there exactly like the GUI's connector_cli_program.
    let prefix = std::env::temp_dir().join(format!(
        "pinvoy-cli-connectors-npm-prefix-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(prefix.join("bin")).unwrap();
    let script = prefix.join("bin").join("tmeet");
    std::fs::write(
        &script,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"1.0.15\"; exit 0; fi\nif [ \"$1\" = \"auth\" ]; then echo \"Logged in as someone@example.com\"; exit 0; fi\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Panic-safe restore: without the guard a failing assert would leak the
    // fixture prefix into the remaining tests in this process.
    let _prefix = EnvVarGuard::set("NPM_CONFIG_PREFIX", prefix.as_os_str());

    let outcome = run(&[
        "pinvou",
        "connectors",
        "status",
        "tmeet",
        "--output",
        "json",
    ])
    .expect("status must resolve the npm-prefix install");
    let status: serde_json::Value =
        serde_json::from_str(&outcome.stdout).expect("single-line JSON status");
    assert_eq!(
        status["connectors"][0]["connected"],
        serde_json::json!(true),
        "a GUI-installed npm-prefix CLI must be visible to the CLI: {status}"
    );

    let _ = std::fs::remove_dir_all(&prefix);
}
