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

// PermissionsExt imports live inside the #[cfg(unix)] tests that need them
// (755, 1289, 1345, 1503): a module-level import compiles on unix even when
// none of those tests is included, which is how this became an unused name.
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

    /// The LEGACY `<id>_disabled` marker file `platform::connector_state`
    /// reads. Nothing writes it any more — the app retired that writer
    /// together with the `set_*_enabled` commands — so tests plant it by
    /// hand to pin the read-and-heal contract.
    fn disabled_marker(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}_disabled"))
    }

    /// The unified scope state that IS the connector switch on both
    /// surfaces (`set_disabled_connectors` in the GUI,
    /// the same plain-scope save here).
    fn disabled_bundles_file(&self) -> PathBuf {
        self.root.join("disabled_bundles.json")
    }

    /// `~/.pinvou3/bundles/<id>/skills` — the root the app's
    /// `apply_connector_skills` unpacks into and deletes from, and the one
    /// the CLI's hide direction must clear.
    fn connector_skills_dir(&self, id: &str) -> PathBuf {
        self.root.join("bundles").join(id).join("skills")
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
fn connectors_enable_disable_switch_through_scope_state_only() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("enable-disable");
    let _path = VendorCliGuard::new();

    assert!(!home.disabled_marker("feishu").exists());

    // The switch persists through the unified scope state — the same
    // `disabled_bundles.json` the GUI's `set_disabled_connectors` writes and
    // the execpolicy CLI hard-block / skill materialization read. It must
    // NOT create a `<id>_disabled` marker: the app retired that writer with
    // the `set_*_enabled` commands, so a marker written here could never be
    // cleared from any GUI surface and would pin the app into deleting the
    // connector's skill directories forever.
    let value = run_json(&["pinvou", "connectors", "disable", "feishu"]);
    assert_eq!(value["ok"], true);
    assert_eq!(value["id"], "feishu");
    assert_eq!(value["action"], "disabled");
    assert_eq!(value["enabled"], false);
    assert_eq!(value["connected"], false);
    assert_eq!(value["skills_should_show"], false);
    // The hide direction needs no embedded bundle, so the CLI performs it.
    assert_eq!(value["skills_removed"], true);
    assert_eq!(value["skills_refresh"], "removed");
    assert!(
        !home.disabled_marker("feishu").exists(),
        "disable must not write a marker no GUI surface can clear"
    );
    assert!(
        std::fs::read_to_string(home.disabled_bundles_file())
            .unwrap()
            .contains("feishu")
    );

    // status reads the switch, not the marker: a connector switched off in
    // the scope state must report enabled=false (before this contract it
    // read only the marker and reported a GUI-disabled connector as on).
    let value = run_json(&["pinvou", "connectors", "status", "feishu"]);
    assert_eq!(value["connectors"][0]["enabled"], false);
    assert_eq!(value["connectors"][0]["legacy_disabled_marker"], false);
    let outcome = run(&["pinvou", "connectors", "status", "feishu"]).expect("human status");
    assert!(outcome.stdout.contains("enabled=no"));
    assert!(!outcome.stdout.contains("legacy_disabled_marker"));

    // enable removes the bridge entry again.
    let value = run_json(&["pinvou", "connectors", "enable", "feishu"]);
    assert_eq!(value["action"], "enabled");
    assert_eq!(value["enabled"], true);
    assert!(
        !std::fs::read_to_string(home.disabled_bundles_file())
            .unwrap()
            .contains("feishu")
    );
    let value = run_json(&["pinvou", "connectors", "status", "feishu"]);
    assert_eq!(value["connectors"][0]["enabled"], true);

    // Both directions are idempotent.
    let value = run_json(&["pinvou", "connectors", "enable", "feishu"]);
    assert_eq!(value["enabled"], true);
    let value = run_json(&["pinvou", "connectors", "disable", "feishu"]);
    assert_eq!(value["enabled"], false);
    let value = run_json(&["pinvou", "connectors", "disable", "feishu"]);
    assert_eq!(value["enabled"], false);
    assert!(!home.disabled_marker("feishu").exists());
}

#[test]
fn connectors_status_surfaces_a_legacy_marker_and_enable_heals_it() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("legacy-marker");
    let _path = VendorCliGuard::new();

    // A marker left by an older CLI build (or by a hand-edited home). The
    // app's gate really does keep deleting the connector's skill dirs while
    // this file exists, and no GUI action can remove it.
    std::fs::write(home.disabled_marker("wecom"), b"1").unwrap();

    let value = run_json(&["pinvou", "connectors", "status", "wecom"]);
    assert_eq!(
        value["connectors"][0]["enabled"], false,
        "a legacy marker really hides the skills, so enabled must report false"
    );
    assert_eq!(
        value["connectors"][0]["legacy_disabled_marker"], true,
        "the marker must be surfaced distinctly from the scope switch"
    );
    let outcome = run(&["pinvou", "connectors", "status", "wecom"]).expect("human status");
    assert!(
        outcome.stdout.contains("legacy_disabled_marker=yes"),
        "{}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains("pinvou connectors enable wecom"),
        "the human row must name the one command that clears it: {}",
        outcome.stdout
    );

    // enable is the only writer left and it only ever removes.
    let value = run_json(&["pinvou", "connectors", "enable", "wecom"]);
    assert_eq!(value["enabled"], true);
    assert!(
        !home.disabled_marker("wecom").exists(),
        "enable must heal a stale legacy marker"
    );
    let value = run_json(&["pinvou", "connectors", "status", "wecom"]);
    assert_eq!(value["connectors"][0]["enabled"], true);
    assert_eq!(value["connectors"][0]["legacy_disabled_marker"], false);
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
    // The PNG is a one-scan login grant that the failure path deliberately
    // keeps (the user still needs it), so the note must say so and say that
    // deleting it is theirs to do.
    assert!(
        message.contains("one-scan login grant") && message.contains("delete it"),
        "the QR note must disclose the credential-equivalent leftover: {message}"
    );
    let _ = std::fs::remove_dir_all(&bin);
}

#[test]
#[cfg(unix)]
fn connect_failure_surfaces_the_redacted_vendor_output_tail() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("connect-vendor-tail");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-connectors-fake-bin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    // The single commonest dingtalk onboarding blocker: the organization has
    // not enabled CLI data access, `dws auth login` says so on its own
    // output and exits. That output is piped (the drainer needs it to find
    // the login link), so before the bounded tail the headless user got
    // "login exited before authorization completed" and nothing else. The
    // credential line in front of it pins the redaction: the tail passes
    // through the CLI's mirror of the GUI's `safe_auth_log_line`.
    write_fake_cli(
        &bin,
        "dws",
        "dws version 1.0.0",
        "if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"status\" ]; then echo '{\"authenticated\": false}'; exit 0; fi\nif [ \"$1\" = \"auth\" ] && [ \"$2\" = \"login\" ]; then echo \"access_token=SUPERSECRETVALUE\"; echo \"Error: CLI data access is not enabled\"; exit 1; fi\n",
    );
    let _path = VendorCliGuard::new_at(bin.clone());

    let error = run(&[
        "pinvou",
        "connectors",
        "connect",
        "dingtalk",
        "--timeout",
        "5",
    ])
    .expect_err("the fake login refuses to authorize");
    let message = error.to_string();
    assert!(
        message.contains("CLI data access is not enabled"),
        "the failure must carry the vendor's own reason: {message}"
    );
    assert!(
        !message.contains("SUPERSECRETVALUE"),
        "credential material must never reach the error text: {message}"
    );
    assert!(
        message.contains("[redacted credential line]"),
        "the credential line must be replaced, not silently dropped: {message}"
    );
    let _ = std::fs::remove_dir_all(&bin);
}

#[test]
#[cfg(unix)]
fn logout_and_apply_skills_remove_the_companion_skill_directories() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("logout-skill-hide");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-connectors-fake-bin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    // `auth logout` succeeds; every other subcommand (notably `auth status`)
    // falls through to exit 1, so the connector reads as not connected.
    write_fake_cli(
        &bin,
        "dws",
        "dws version 1.0.0",
        "if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"logout\" ]; then exit 0; fi\n",
    );
    let _path = VendorCliGuard::new_at(bin.clone());

    // Plant the skill tree exactly as the app's bundle unpack leaves it:
    // one directory per `DINGTALK_SKILL_DIRS` entry plus the connector's
    // NOTICE file. The unrelated sibling pins that the hide direction is
    // table-driven (the app's `apply_connector_skills` removes the listed
    // dirs, not the whole skills root).
    let skills = home.connector_skills_dir("dingtalk");
    std::fs::create_dir_all(skills.join("dws").join("references")).unwrap();
    std::fs::write(skills.join("dws").join("SKILL.md"), "# dws").unwrap();
    std::fs::write(skills.join("NOTICE-dingtalk.md"), "notice").unwrap();
    std::fs::create_dir_all(skills.join("unrelated")).unwrap();

    let value = run_json(&["pinvou", "connectors", "logout", "dingtalk", "--yes"]);
    assert_eq!(value["ok"], true);
    assert_eq!(value["installed"], true);
    assert_eq!(
        value["skills_removed"], true,
        "the GUI logout is logout + apply_skills; the CLI must do the hide half too"
    );
    assert!(
        !skills.join("dws").exists(),
        "logout must remove the companion skill directories"
    );
    assert!(
        !skills.join("NOTICE-dingtalk.md").exists(),
        "logout must remove the connector NOTICE file the unpack dropped"
    );
    assert!(
        skills.join("unrelated").is_dir(),
        "the hide direction must not wipe unlisted entries in the skills root"
    );

    // `apply-skills` on a disconnected connector resolves to the same hide
    // direction and is idempotent on an already-clean tree.
    std::fs::create_dir_all(skills.join("dws")).unwrap();
    std::fs::write(skills.join("dws").join("SKILL.md"), "# dws").unwrap();
    let value = run_json(&["pinvou", "connectors", "apply-skills", "dingtalk"]);
    assert_eq!(value["visible"], false);
    assert_eq!(value["skills_removed"], true);
    assert!(!skills.join("dws").exists());
    let value = run_json(&["pinvou", "connectors", "apply-skills", "dingtalk"]);
    assert_eq!(value["skills_removed"], true);

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
    // tmeet 1.0.10 parses but sits below the install gate: status /
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

/// A below-minimum install is `upgrade_required`, but the vendor credentials
/// it holds are as real as a usable install's: the GUI DTOs (`wecom_status` /
/// `tmeet_status`) gate on `*_cli_version` (the parse gate, not the minimum),
/// so an overly old install still gets its status-probe spawn and reports
/// `connected` from the live probe. The pre-fix CLI arm hard-coded
/// `connected:false, ok:false` for every below-minimum install, lying "logged
/// out" for a credential the vendor still holds. Pin the probe-backed
/// verdict: an authorized below-minimum wecom reports `connected:true` +
/// `upgrade_required:true` at once, and a probe-failing below-minimum tmeet
/// reports the honest `connected:false`.
#[test]
#[cfg(unix)]
fn status_probes_a_below_minimum_install_like_the_gui_dtos() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("upgrade-gate-probe");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-connectors-fake-bin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    // wecom 1.2.0 parses but sits below the 1.2.1 baseline; its status probe
    // answers the whole line `authorized` with exit 0 like a connected one.
    write_fake_cli(
        &bin,
        "wecom-cli",
        "wecom-cli 1.2.0",
        "if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"show\" ]; then echo \"authorized\"; exit 0; fi\n",
    );
    // tmeet 1.0.10 sits below the 1.0.18 baseline; its `auth status` falls
    // through to the script's trailing exit 1 with no "Logged in" line, so
    // the probe reports not-logged-in.
    write_fake_cli(&bin, "tmeet", "tmeet version 1.0.10", "");
    let _path = VendorCliGuard::new_at(bin.clone());

    let value = run_json(&["pinvou", "connectors", "status", "wecom"]);
    let entry = &value["connectors"][0];
    assert_eq!(entry["installed"], true, "{entry}");
    assert_eq!(entry["upgrade_required"], true, "{entry}");
    assert_eq!(entry["version"], "wecom-cli 1.2.0", "{entry}");
    assert_eq!(
        entry["connected"], true,
        "an authorized below-minimum install is still connected: {entry}"
    );
    assert_eq!(
        entry["ok"], true,
        "the probe exit status must be reported, not hard-coded: {entry}"
    );
    assert!(
        entry.get("note").is_none(),
        "a healthy probe must not degrade to a note: {entry}"
    );
    // The human row carries both facts at once: connected AND upgrade.
    let outcome = run(&["pinvou", "connectors", "status", "wecom"]).expect("human status");
    assert!(
        outcome.stdout.contains("connected=yes"),
        "the human row must keep the live connected verdict: {}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains("upgrade_required=yes"),
        "the human row must still demand the upgrade: {}",
        outcome.stdout
    );
    assert!(
        outcome.stdout.contains("installed=yes(wecom-cli 1.2.0)"),
        "the human row must name the below-minimum version: {}",
        outcome.stdout
    );

    // The mirror case: a probe that fails reports connected:false (honest
    // "not logged in"), never a fabricated connection either.
    let value = run_json(&["pinvou", "connectors", "status", "tmeet"]);
    let entry = &value["connectors"][0];
    assert_eq!(entry["installed"], true, "{entry}");
    assert_eq!(entry["upgrade_required"], true, "{entry}");
    assert_eq!(
        entry["connected"], false,
        "a failing probe must not fabricate a connection: {entry}"
    );
    assert_eq!(entry["ok"], false, "{entry}");
    assert!(entry.get("note").is_none(), "{entry}");

    let _ = std::fs::remove_dir_all(&bin);
}

/// A vendor CLI that writes past the 8 MiB drain cap must still complete.
/// `take(cap).read_to_end` returns EOF to the drainer at the cap while the
/// child keeps writing, so the pipe fills, the child blocks in write(2), the
/// probe wait expires, and a healthy exit-0 child was misreported as "timed
/// out" and group-SIGKILLed. The drain keeps at most the cap but reads — and
/// discards — until true EOF (the voice.rs `drain_capped` / code.rs
/// `read_capped_to_eof` discipline). The fake prints the tmeet "Logged in"
/// marker first (so the connected predicate has its signal) and then
/// ~9.5 MiB of padding — cap + a pipe-buffer worth + margin — and exits 0.
#[test]
#[cfg(unix)]
fn a_vendor_cli_that_outproduces_the_drain_cap_still_completes() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("drain-past-cap");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-connectors-fake-bin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    // `yes x | head -c 9961472` produces 9,961,472 bytes (8 MiB cap
    // 8,388,608 + ~1.5 MiB margin, far more than a 64 KiB pipe buffer) and
    // then exits 0 like a chatty-but-healthy vendor CLI.
    write_fake_cli(
        &bin,
        "tmeet",
        "tmeet version 1.0.18",
        "if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"status\" ]; then echo \"Logged in as someone@example.com\"; yes x | head -c 9961472; exit 0; fi\n",
    );
    let _path = VendorCliGuard::new_at(bin.clone());

    let started = std::time::Instant::now();
    let value = run_json(&["pinvou", "connectors", "status", "tmeet"]);
    let took = started.elapsed();
    let entry = &value["connectors"][0];
    assert_eq!(
        entry["connected"], true,
        "a child blocked mid-write must not be misreported as a probe timeout: {entry}"
    );
    assert_eq!(entry["installed"], true, "{entry}");
    assert_eq!(entry["ok"], true, "{entry}");
    assert!(
        entry.get("note").is_none(),
        "must not degrade to a probe-failure note: {entry}"
    );
    // Under the take() bug the child stalls until the 60 s probe budget
    // burns; the fixed drain completes as fast as the child writes.
    assert!(
        took < std::time::Duration::from_secs(30),
        "a 9.5 MiB write must not stall into the probe timeout: {took:?}"
    );
    let outcome = run(&["pinvou", "connectors", "status", "tmeet"]).expect("human status");
    assert!(
        outcome.stdout.contains("connected=yes"),
        "{}",
        outcome.stdout
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
        "tmeet version 1.0.18",
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
fn logout_with_a_failing_version_probe_errors_without_touching_the_store() {
    use pinvou3_lib::features::marketplace::store::{BundleRecord, BundleSource, BundleStore};

    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("logout-broken-probe");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-connectors-fake-bin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    let args_file = bin.join("seen-args.txt");
    // A CLI that resolves but whose every invocation exits non-zero (npm
    // shim with node removed, transient hang-then-fail): the login state is
    // UNCONFIRMED. Logout must error without running `auth logout` and
    // without flipping the store — reporting `installed:false` here would
    // skip the real logout while the vendor token stays on disk (GUI
    // `logout_probe_verdict` parity).
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\nexit 1\n",
        args_file.display()
    );
    let fake = bin.join("dws");
    std::fs::write(&fake, script).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _path = VendorCliGuard::new_at(bin.clone());

    let store = BundleStore::new();
    store
        .upsert(BundleRecord::installed_now(
            "dingtalk",
            BundleSource::Builtin,
        ))
        .expect("seed the bundle store record");

    let error = run(&["pinvou", "connectors", "logout", "dingtalk", "--yes"])
        .expect_err("a failing version probe must not report a clean logout");
    let message = error.to_string();
    assert!(
        message.contains("unconfirmed"),
        "the error must say the login state is unconfirmed: {message}"
    );

    let record = store
        .get("dingtalk")
        .expect("store read must succeed")
        .expect("seeded record must be kept");
    assert_eq!(
        record.degraded.as_deref(),
        None,
        "the store record must stay connected when the probe is unconfirmed"
    );
    // Only the `--version` probe may have run; the real `auth logout` must
    // be untouched so the token survives for a retry.
    let seen = std::fs::read_to_string(&args_file).unwrap();
    assert_eq!(
        seen.lines().collect::<Vec<_>>(),
        vec!["--version"],
        "no auth logout may run when the probe is unconfirmed"
    );
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
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"1.0.18\"; exit 0; fi\nif [ \"$1\" = \"auth\" ]; then echo \"Logged in as someone@example.com\"; exit 0; fi\nexit 1\n",
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

// ── connect post-connection side effects ────────────────────────────────────

/// The GUI's connect does not stop at `bundle_store_on_connected`: its
/// frontend invokes `*_apply_skills` the moment the login lands
/// (`ToolStoreView.jsx`'s connect-completion handler), whose `show` branch
/// runs `sync_deny_all_scopes_after_install` — the write that re-applies the
/// "code sessions default external capabilities off" policy to a freshly
/// connected connector. Before this contract the CLI's `connect` skipped it,
/// so a new connection stayed usable from code sessions until some GUI-side
/// apply-skills happened to run. The pin seeds an **initialized** code scope
/// (the write is a no-op on an uninitialized one, by design) and asserts the
/// connector's package id landed in the persisted code list after `connect`.
#[test]
#[cfg(unix)]
fn connect_reapplies_the_deny_all_code_scope_after_a_fresh_connection() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("connect-scope-sync");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-connectors-fake-bin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    // dingtalk (dws): a lock-table connector (unlike tmeet's npm lane — it
    // has no lock-table entry, so the asset-pin mutation below would be a
    // no-op there). `--version` answers, `auth login` prints the login URL
    // plus the user-code line, `auth status` reports authenticated — so
    // `connect` completes through the post-exit grace probe.
    write_fake_cli(
        &bin,
        "dws",
        "dws version 1.0.0",
        "if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"login\" ]; then echo \"User Code: ZXCV1234\"; echo \"open https://login.dingtalk.com/oauth/authorize?user_code=ZXCV1234 to authorize\"; exit 0; fi\nif [ \"$1\" = \"auth\" ] && [ \"$2\" = \"status\" ]; then echo '{\"authenticated\":true}'; exit 0; fi\nexit 1\n",
    );
    let _path = VendorCliGuard::new_at(bin.clone());

    // Seed an initialized code scope: the sync targets every initialized
    // DenyAll scope (`PackDefaultPolicy::DenyAll` ⇒ the code mode), and
    // writes the connector's package id into its persisted list.
    let disabled = home.disabled_bundles_file();
    if let Some(parent) = disabled.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &disabled,
        r#"{"scopes":{"code":[]},"hidden_scopes":{},"initialized":["code"]}"#,
    )
    .unwrap();

    let value = run_json(&[
        "pinvou",
        "connectors",
        "connect",
        "dingtalk",
        "--timeout",
        "30",
    ]);
    assert_eq!(value["connected"], true, "{value}");

    let persisted: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&disabled).unwrap())
            .expect("disabled_bundles.json stays valid JSON");
    let dingtalk_entry = persisted["scopes"]["code"]
        .as_array()
        .expect("code scope list persists")
        .iter()
        .any(|id| id.as_str() == Some("dingtalk"));
    assert!(
        dingtalk_entry,
        "a fresh connect must re-apply the DenyAll code-scope default (the GUI's \
         post-connect apply-skills write): {persisted}"
    );

    // And the divergent GUI-mirror write this round removes: no asset pins
    // on the connect write — the GUI's own write is a bare `installed_now`
    // record with `source=Builtin`. dingtalk is a lock-table connector
    // (`cli_bundle_bin("dingtalk")` → "dws"), so the pre-fix code's
    // `artifact_pin("dws")` lookup really fires on this arm.
    let record = pinvou3_lib::features::marketplace::store::BundleStore::new()
        .get("dingtalk")
        .expect("store read succeeds")
        .expect("connect registers the record");
    assert!(
        record.assets.is_empty(),
        "connect must not fabricate asset pins the GUI never writes: {:?}",
        record.assets
    );

    let _ = std::fs::remove_dir_all(&bin);
}

/// The feishu poll judges readiness by a status probe on every tick; the
/// probe is a fresh spawn whose failure is NOT the login's failure. The GUI
/// folds probe errors to "not connected yet" and keeps polling (feishu.rs
/// `is_user_ready`); this pin holds the CLI to the same fold — before the
/// fix, `connect` propagated the probe's error (`?`) as a hard connect
/// failure and skipped the post-connect side effects, so a single probe
/// hiccup (spawn failure, probe timeout) after a successful login aborted
/// the whole command.
///
/// Hermetic construction: the fake lark-cli answers the login phases, then
/// on the FIRST `auth status` probe schedules its own disappearance — a
/// detached subshell hides the script one second later and restores it two
/// seconds after that. The probe timeline inside `connect --timeout 30`
/// (3 s poll period) is then:
/// - probe 1 (~t=3): runs, exits non-zero with no JSON → `Ok(false)`
///   (a non-zero probe exit alone never errors — the fold is not what this
///   exercises);
/// - probe 2 (~t=6): the script is hidden → resolution fails → the probe
///   call returns `Err` — exactly the error the pre-fix `?` turned into a
///   hard connect failure;
/// - probe 3 (~t=9): the script is back and reports ready → connected.
#[test]
#[cfg(unix)]
fn feishu_connect_survives_a_failed_status_probe_and_still_completes() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HomeGuard::new("connect-probe-hiccup");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-connectors-fake-bin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    let hidden = bin.join("lark-cli.off");
    let counter = bin.join("status-probes");
    let script = bin.join("lark-cli");
    write_fake_cli(
        &bin,
        "lark-cli",
        "lark-cli 1.2.3",
        &format!(
            "if [ \"$1\" = \"config\" ]; then echo \"open https://open.feishu.cn/app?ticket=reg\"; exit 0; fi\n\
             if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"login\" ] && [ \"$3\" = \"--device-code\" ]; then exit 0; fi\n\
             if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"login\" ]; then echo '{{\"verification_uri_complete\":\"https://accounts.feishu.cn/authorize?x=1\",\"device_code\":\"DEV123\"}}'; exit 0; fi\n\
             if [ \"$1\" = \"auth\" ] && [ \"$2\" = \"status\" ]; then\n\
             \x20 if [ -f \"{counter}\" ]; then echo '{{\"identities\":{{\"user\":{{\"status\":\"ready\"}}}}}}'; exit 0; fi\n\
             \x20 : > \"{counter}\"\n\
             \x20 ( sleep 1; mv -f \"{script}\" \"{hidden}\" 2>/dev/null; sleep 3; mv -f \"{hidden}\" \"{script}\" 2>/dev/null ) >/dev/null 2>&1 &\n\
             \x20 exit 1\n\
             fi\n",
            counter = counter.display(),
            script = script.display(),
            hidden = hidden.display(),
        ),
    );
    let _path = VendorCliGuard::new_at(bin.clone());

    // Seed an initialized code scope so the post-connect DenyAll sync has
    // something to write into (the connect must reach its side effects).
    let disabled = home.disabled_bundles_file();
    if let Some(parent) = disabled.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &disabled,
        r#"{"scopes":{"code":[]},"hidden_scopes":{},"initialized":["code"]}"#,
    )
    .unwrap();

    let value = run_json(&[
        "pinvou",
        "connectors",
        "connect",
        "feishu",
        "--timeout",
        "30",
    ]);
    assert_eq!(
        value["connected"], true,
        "a failed status probe must fold to 'not yet' and keep polling, not abort the connect: {value}"
    );

    // The connect ran its side effects: the store mirror flipped to
    // connected and the DenyAll code-scope sync landed.
    let record = pinvou3_lib::features::marketplace::store::BundleStore::new()
        .get("feishu")
        .expect("store read succeeds")
        .expect("the connect must still register the store record after a probe error");
    assert!(
        record.installed,
        "the store record must read as connected: {record:?}"
    );
    let persisted: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&disabled).unwrap())
            .expect("disabled_bundles.json stays valid JSON");
    let feishu_entry = persisted["scopes"]["code"]
        .as_array()
        .expect("code scope list persists")
        .iter()
        .any(|id| id.as_str() == Some("feishu"));
    assert!(
        feishu_entry,
        "the connect must still run the DenyAll code-scope sync after a probe error: {persisted}"
    );

    let _ = std::fs::remove_dir_all(&bin);
}

/// `ensure-cli`'s user-facing claim is "present and executes". The final
/// presence check used to be gated on `installed &&` — skipped whenever
/// `ensure_native_cli` reported the destination hash already matched (`Ok
/// (false)`), so the command printed success for a binary the OS refuses to
/// execute (lost exec bit, quarantine attribute) — exactly the state a user
/// runs `ensure-cli` to fix. The regression pin would need the hash-matched
/// skip, which requires bytes whose SHA-256 equals the compiled-in lock
/// entry's — not fabricatable hermetically. What IS hermetically pinnable
/// is the same command's refuse-to-succeed half: with the binary present
/// but unexecutable and every spawn failing, `ensure-cli` must never print
/// success — the pre-fix code could reach `installed=false` (hash did not
/// match) and then, had the download lane failed, would have surfaced the
/// download error; the post-fix code surfaces the clearer repair message.
/// The tmeet arm of the same command is npm-shaped and stays out of scope
/// like the download lane.
#[test]
#[cfg(unix)]
fn ensure_cli_never_reports_success_for_a_binary_that_cannot_execute() {
    use std::os::unix::fs::PermissionsExt as _;
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = HomeGuard::new("ensure-cli-unexecutable");
    let bin = std::env::temp_dir().join(format!(
        "pinvou-cli-connectors-fake-bin-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bin).unwrap();
    // A lark-cli that exists but is not executable: resolution finds it on
    // PATH (existence checks only), every spawn fails with EACCES, so
    // `cli_installed` reports Missing. Whatever the install lanes do next,
    // the command's own verification must catch it — the observable
    // contract tested here is: no exit 0 while the binary cannot run.
    std::fs::write(bin.join("lark-cli"), "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(bin.join("lark-cli"), std::fs::Permissions::from_mode(0o644)).unwrap();
    let _path = VendorCliGuard::new_at(bin.clone());

    let outcome = run(&["pinvou", "connectors", "ensure-cli", "feishu"]);
    match outcome {
        Ok(success) => panic!(
            "ensure-cli must not report success while the binary cannot execute: {}",
            success.stdout
        ),
        Err(error) => {
            assert_eq!(error.exit_code(), ExitCode::Failed);
            // Either failure is honest; both name the real state. The
            // post-fix wording is preferred but the download lane may fail
            // first (no network in CI), so accept the lane error too.
            let message = error.to_string();
            assert!(
                message.contains("will not execute")
                    || message.contains("checksum mismatch")
                    || message.contains("download")
                    || message.contains("tar"),
                "the error must name the actual failure, not a fabricated success: {message}"
            );
        }
    }

    let _ = std::fs::remove_dir_all(&bin);
}
