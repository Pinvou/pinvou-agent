//! Contract tests for the `models` + `settings` families (`pinvou models`,
//! `pinvou settings` alias). Parse-level coverage runs against pure parser
//! state; execute-level coverage uses a temporary `PINVOU3_HOME` so no test
//! touches real user data. Network paths (connection probes) are `#[ignore]`
//! only — see the bottom of this file.

use std::sync::Mutex;

use pinvou_cli::{ExitCode, execute, parse_args};
use pinvou3_lib::features::sessions::SerializableMode;
use pinvou3_lib::platform::prefs::{ColorScheme, ModelPreset, SearchProvider, Theme, UserPrefs};

/// Serialises tests that mutate the process-global `PINVOU3_HOME` environment
/// variable, preventing data races when the parallel test runner executes
/// them concurrently (same pattern as cli_contract.rs).
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Creates a unique temporary product home and points `PINVOU3_HOME` at it.
/// Restores the previous value and removes the directory on drop so an
/// assertion failure cannot leak environment state into other tests.
struct SandboxHome {
    root: std::path::PathBuf,
    previous: Option<std::ffi::OsString>,
}

impl SandboxHome {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pinvou-cli-models-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create temporary PINVOU3_HOME");
        let previous = std::env::var_os("PINVOU3_HOME");
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
        Self { root, previous }
    }
}

impl Drop for SandboxHome {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Restores an environment variable to its saved value on drop, so a panic
/// between `set_var` and the assertions cannot leak state into other tests
/// (same pattern as `RestoreHome` in sessions_contract.rs).
struct RestoreEnvVar(&'static str, Option<std::ffi::OsString>);

impl Drop for RestoreEnvVar {
    fn drop(&mut self) {
        // SAFETY: ENV_LOCK is held by the owning test.
        match self.1.take() {
            Some(value) => unsafe { std::env::set_var(self.0, value) },
            None => unsafe { std::env::remove_var(self.0) },
        }
    }
}

fn usage_error(args: &[&str]) -> String {
    let error = parse_args(args.to_vec()).expect_err("expected a usage error");
    assert_eq!(error.exit_code(), ExitCode::Usage, "message: {error}");
    error.to_string()
}

fn run_ok(args: &[&str]) -> String {
    let parsed = parse_args(args.to_vec()).expect("valid command");
    let outcome = execute(parsed).expect("execute succeeds");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    outcome.stdout
}

fn run_err(args: &[&str]) -> (String, ExitCode) {
    let parsed = parse_args(args.to_vec()).expect("valid command");
    let error = execute(parsed).expect_err("execute fails");
    (error.to_string(), error.exit_code())
}

/// A command that COMPLETES with a non-success exit code (the probe families:
/// the result is a payload on stdout and the exit code is the verdict), as
/// opposed to `run_err`'s commands that fail before producing one.
fn run_outcome(args: &[&str]) -> (String, ExitCode) {
    let parsed = parse_args(args.to_vec()).expect("valid command");
    let outcome = execute(parsed).expect("a completed probe is an outcome, not an error");
    (outcome.stdout, outcome.exit_code)
}

fn load_prefs() -> UserPrefs {
    UserPrefs::load()
}

const ADD_ARGS: &[&str] = &[
    "pinvoy",
    "models",
    "add",
    "--preset",
    "deepseek",
    "--name",
    "DeepSeek test",
    "--model",
    "deepseek-v4-pro",
    "--base-url",
    "https://api.deepseek.com",
];

// ---------------------------------------------------------------------------
// parse level: models subcommands
// ---------------------------------------------------------------------------

#[test]
fn parses_every_models_subcommand() {
    for args in [
        vec!["pinvoy", "models", "list"],
        vec![
            "pinvoy",
            "models",
            "add",
            "--preset",
            "local_vllm",
            "--name",
            "Local",
            "--model",
            "qwen36_35b_256k",
            "--base-url",
            "http://127.0.0.1:8000/v1",
        ],
        // Every optional GUI-form field the model editor writes.
        vec![
            "pinvoy",
            "models",
            "add",
            "--preset",
            "openai_compatible",
            "--name",
            "Compatible",
            "--model",
            "glm-4.7",
            "--base-url",
            "https://example.invalid/api/paas/v4",
            "--alias",
            "GLM",
            "--provider-kind",
            "custom",
            "--vendor",
            "glm",
            "--endpoint-mode",
            "full_chat_completions",
            "--vision-model-id",
            "m_other",
        ],
        vec!["pinvoy", "models", "edit", "m1", "--name", "Renamed"],
        vec!["pinvoy", "models", "edit", "m1", "--alias", "none"],
        vec!["pinvoy", "models", "edit", "m1", "--clear-api-key"],
        vec!["pinvoy", "models", "edit", "m1", "--api-key-stdin"],
        vec!["pinvoy", "models", "edit", "m1", "--set-active"],
        vec!["pinvoy", "models", "remove", "m1", "--yes"],
        vec!["pinvoy", "models", "remove", "m1"],
        vec!["pinvoy", "models", "use", "m1"],
        vec!["pinvoy", "models", "show", "m1"],
        vec!["pinvoy", "models", "show", "m1", "--reveal-key"],
        vec!["pinvoy", "models", "test", "m1"],
        vec!["pinvoy", "models", "probe-local"],
        vec![
            "pinvoy",
            "models",
            "probe-local",
            "--url",
            "http://127.0.0.1:8000/v1",
        ],
        vec![
            "pinvoy",
            "models",
            "probe-local",
            "--url",
            "http://127.0.0.1:8000/v1",
            "--model",
            "m1",
        ],
    ] {
        parse_args(args).unwrap_or_else(|error| panic!("valid command rejected: {error}"));
    }
}

#[test]
fn models_without_subcommand_is_a_usage_error() {
    let message = usage_error(&["pinvoy", "models"]);
    assert!(message.contains("models"), "unexpected message: {message}");
    assert!(message.contains("list"), "usage must name subcommands");
}

#[test]
fn models_rejects_unknown_subcommands_and_options() {
    assert!(usage_error(&["pinvoy", "models", "bogus"]).contains("usage"));
    assert!(usage_error(&["pinvoy", "models", "list", "extra"]).contains("no arguments"));
    assert!(usage_error(&["pinvoy", "models", "list", "--json"]).contains("unknown option"));
    assert!(usage_error(&["pinvoy", "models", "use"]).contains("requires an id"));
    assert!(usage_error(&["pinvoy", "models", "show", "a", "b"]).contains("one id"));
}

/// Duplicate boolean flags exit 2 like duplicate value flags and like every
/// `support::parse_family_flags` family.
#[test]
fn models_reject_duplicate_boolean_flags() {
    let message = usage_error(&["pinvoy", "models", "remove", "m1", "--yes", "--yes"]);
    assert!(
        message.contains("--yes"),
        "the duplicate-flag usage error must name the flag"
    );
}

#[test]
fn models_add_rejects_bad_presets_and_efforts() {
    let message = usage_error(&[
        "pinvoy",
        "models",
        "add",
        "--preset",
        "nope",
        "--name",
        "N",
        "--model",
        "M",
        "--base-url",
        "https://example.invalid/v1",
    ]);
    assert!(message.contains("nope"), "message names the bad value");
    assert!(
        message.contains("local_vllm") && message.contains("anthropic"),
        "usage names valid presets: {message}"
    );

    let message = usage_error(&[
        "pinvoy",
        "models",
        "add",
        "--preset",
        "deepseek",
        "--name",
        "N",
        "--model",
        "M",
        "--base-url",
        "https://example.invalid/v1",
        "--reasoning-effort",
        "turbo",
    ]);
    assert!(
        message.contains("off, low, medium, high, max"),
        "usage names valid efforts: {message}"
    );
}

#[test]
fn models_add_requires_core_fields() {
    for missing in [
        vec![
            "pinvoy",
            "models",
            "add",
            "--name",
            "N",
            "--model",
            "M",
            "--base-url",
            "U",
        ],
        vec![
            "pinvoy",
            "models",
            "add",
            "--preset",
            "deepseek",
            "--model",
            "M",
            "--base-url",
            "U",
        ],
        vec![
            "pinvoy",
            "models",
            "add",
            "--preset",
            "deepseek",
            "--name",
            "N",
            "--base-url",
            "U",
        ],
        vec![
            "pinvoy", "models", "add", "--preset", "deepseek", "--name", "N", "--model", "M",
        ],
    ] {
        let error = parse_args(missing).expect_err("missing required field");
        assert_eq!(error.exit_code(), ExitCode::Usage);
        assert!(error.to_string().contains("is required"), "{error}");
    }
}

#[test]
fn models_add_rejects_bad_numbers_and_conflicting_key_sources() {
    for args in [
        vec![
            "pinvoy",
            "models",
            "add",
            "--preset",
            "deepseek",
            "--name",
            "N",
            "--model",
            "M",
            "--base-url",
            "U",
            "--context-window",
            "abc",
        ],
        vec![
            "pinvoy",
            "models",
            "add",
            "--preset",
            "deepseek",
            "--name",
            "N",
            "--model",
            "M",
            "--base-url",
            "U",
            "--context-window",
            "0",
        ],
        vec![
            "pinvoy",
            "models",
            "add",
            "--preset",
            "deepseek",
            "--name",
            "N",
            "--model",
            "M",
            "--base-url",
            "U",
            "--max-output",
            "-3",
        ],
        vec![
            "pinvoy",
            "models",
            "add",
            "--preset",
            "deepseek",
            "--name",
            "N",
            "--model",
            "M",
            "--base-url",
            "U",
            "--api-key-env",
            "A",
            "--api-key-stdin",
        ],
    ] {
        let error = parse_args(args).expect_err("invalid add options");
        assert_eq!(error.exit_code(), ExitCode::Usage, "{error}");
    }
}

// ---------------------------------------------------------------------------
// parse level: settings subcommands
// ---------------------------------------------------------------------------

#[test]
fn parses_settings_get_set_and_search() {
    for args in [
        vec!["pinvoy", "settings", "get"],
        vec!["pinvoy", "settings", "get", "theme"],
        vec!["pinvoy", "settings", "set", "theme", "liquid-dark"],
        // snake_case alias of the canonical kebab-case value
        vec!["pinvoy", "settings", "set", "theme", "liquid_dark"],
        vec!["pinvoy", "settings", "set", "color_scheme", "system"],
        vec!["pinvoy", "settings", "set", "language", "zh-Hans"],
        vec!["pinvoy", "settings", "set", "memory_enabled", "true"],
        vec![
            "pinvoy",
            "settings",
            "set",
            "notifications.enabled",
            "false",
        ],
        vec![
            "pinvoy",
            "settings",
            "set",
            "notifications.task_completed",
            "true",
        ],
        vec![
            "pinvoy",
            "settings",
            "set",
            "sidebar.date_grouping",
            "false",
        ],
        vec!["pinvoy", "settings", "set", "mode_defaults.work", "plan"],
        vec!["pinvoy", "settings", "set", "mode_defaults.work", "none"],
        vec![
            "pinvoy",
            "settings",
            "set",
            "code_permission.last_mode",
            "yolo",
        ],
        vec!["pinvoy", "settings", "set", "advanced.allow_shell", "none"],
        vec![
            "pinvoy",
            "settings",
            "set",
            "voice_shortcut_enabled",
            "false",
        ],
        vec!["pinvoy", "settings", "set", "pet.enabled", "true"],
        vec!["pinvoy", "settings", "search", "list"],
        vec![
            "pinvoy",
            "settings",
            "search",
            "set",
            "--provider",
            "metaso",
            "--api-key-env",
            "METASO_API_KEY",
        ],
        vec![
            "pinvoy",
            "settings",
            "search",
            "set",
            "--provider",
            "tavily",
            "--clear",
        ],
        vec![
            "pinvoy",
            "settings",
            "search",
            "set",
            "--provider",
            "tavily",
            "--clear",
            "--yes",
        ],
        vec!["pinvoy", "settings", "search", "test", "bing"],
    ] {
        parse_args(args).unwrap_or_else(|error| panic!("valid command rejected: {error}"));
    }
}

#[test]
fn settings_rejects_unknown_subcommands_and_keys() {
    let message = usage_error(&["pinvoy", "settings"]);
    assert!(
        message.contains("settings"),
        "unexpected message: {message}"
    );
    assert!(usage_error(&["pinvoy", "settings", "bogus"]).contains("get|set|search"));
    let message = usage_error(&["pinvoy", "settings", "get", "bogus_key"]);
    assert!(message.contains("bogus_key") && message.contains("theme"));
    let message = usage_error(&["pinvoy", "settings", "set", "bogus_key", "true"]);
    assert!(message.contains("bogus_key") && message.contains("pet.enabled"));
}

#[test]
fn settings_set_rejects_bad_values_naming_valid_ones() {
    // dark is a color_scheme value, not a theme
    let message = usage_error(&["pinvoy", "settings", "set", "theme", "dark"]);
    assert!(
        message.contains("genesis") && message.contains("liquid-light"),
        "{message}"
    );
    let message = usage_error(&["pinvoy", "settings", "set", "color_scheme", "blue"]);
    assert!(message.contains("light") && message.contains("dark") && message.contains("system"));
    let message = usage_error(&["pinvoy", "settings", "set", "language", "zh"]);
    assert!(message.contains("zh-Hans") && message.contains("en") && message.contains("ja"));
    let message = usage_error(&["pinvoy", "settings", "set", "memory_enabled", "yes"]);
    assert!(
        message.contains("true") && message.contains("false"),
        "{message}"
    );
    let message = usage_error(&["pinvoy", "settings", "set", "mode_defaults.work", "auto"]);
    assert!(message.contains("plan") && message.contains("yolo") && message.contains("none"));
    let message = usage_error(&["pinvoy", "settings", "set", "advanced.allow_shell", "maybe"]);
    assert!(
        message.contains("true") && message.contains("none"),
        "{message}"
    );
    // type mismatch: bool key with an enum value
    let message = usage_error(&["pinvoy", "settings", "set", "pet.enabled", "plan"]);
    assert!(
        message.contains("true") && message.contains("false"),
        "{message}"
    );
}

#[test]
fn settings_set_requires_key_and_value() {
    assert!(usage_error(&["pinvoy", "settings", "set"]).contains("<key> <value>"));
    assert!(usage_error(&["pinvoy", "settings", "set", "theme"]).contains("<key> <value>"));
}

#[test]
fn settings_search_rejects_bad_providers_and_conflicting_sources() {
    assert!(usage_error(&["pinvoy", "settings", "search"]).contains("list|set|test"));
    assert!(usage_error(&["pinvoy", "settings", "search", "bogus"]).contains("list|set|test"));
    let message = usage_error(&["pinvoy", "settings", "search", "set"]);
    assert!(message.contains("--provider"), "{message}");
    let message = usage_error(&["pinvoy", "settings", "search", "set", "--provider", "nope"]);
    assert!(
        message.contains("bing") && message.contains("tavily"),
        "{message}"
    );
    let message = usage_error(&[
        "pinvoy",
        "settings",
        "search",
        "set",
        "--provider",
        "metaso",
        "--api-key-env",
        "A",
        "--clear",
    ]);
    assert!(message.contains("--clear"), "{message}");
    let message = usage_error(&["pinvoy", "settings", "search", "test", "google"]);
    assert!(
        message.contains("bing") && message.contains("metaso"),
        "{message}"
    );
}

/// Binding rule: the `settings` alias only accepts settings subcommands, so
/// `settings list` must stay a usage error even though the stub parser
/// accepted any word.
#[test]
fn settings_alias_only_accepts_settings_subcommands() {
    let message = usage_error(&["pinvoy", "settings", "list"]);
    assert!(message.contains("get|set|search"), "{message}");
    let message = usage_error(&["pinvoy", "settings", "probe-local"]);
    assert!(message.contains("get|set|search"), "{message}");
}

#[test]
fn models_token_only_accepts_models_subcommands() {
    let message = usage_error(&["pinvoy", "models", "get"]);
    assert!(message.contains("models"), "{message}");
    assert!(message.contains("probe-local"), "{message}");
    assert!(
        message.contains("edit"),
        "the usage must name the in-place edit subcommand: {message}"
    );
}

// ---------------------------------------------------------------------------
// execute level (temporary PINVOU3_HOME, no network, no secrets)
// ---------------------------------------------------------------------------

#[test]
fn models_list_reports_fresh_default_model() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("list-fresh");
    let json = run_ok(&["pinvoy", "--output", "json", "models", "list"]);
    let value: serde_json::Value = serde_json::from_str(&json).expect("single-line json");
    let models = value["models"].as_array().expect("models array");
    assert_eq!(models.len(), 1, "fresh home migrates one default model");
    assert_eq!(models[0]["id"], "default");
    assert_eq!(models[0]["preset"], default_preset_str());
    assert_eq!(models[0]["active"], true);
    assert_eq!(models[0]["has_secret"], false);
    // The GUI list DTO's credential_state: a keyless default model is
    // "missing" after the safe-prefs refresh.
    assert_eq!(models[0]["credential_state"], "missing");
    assert!(json.contains("has_secret"), "field present");
    assert!(
        !json.to_lowercase().contains("api_key"),
        "no key field in list"
    );

    let human = run_ok(&["pinvoy", "models", "list"]);
    assert!(
        human.contains("*default"),
        "models list should mark the active model"
    );
    assert!(
        human.contains("credential_state=missing"),
        "models list human output must carry the credential_state column"
    );

    // credential_state mirrors the GUI across states: a non-empty
    // DEEPSEEK_API_KEY env override marks every model env_override in the
    // list JSON without touching the OS keychain (the prefs layer's
    // refresh_credential_states_with_store short-circuits on it).
    let _restore_deepseek_key =
        RestoreEnvVar("DEEPSEEK_API_KEY", std::env::var_os("DEEPSEEK_API_KEY"));
    unsafe { std::env::set_var("DEEPSEEK_API_KEY", "pinvou-cli-contract-override") };
    let json = run_ok(&["pinvoy", "--output", "json", "models", "list"]);
    let human = run_ok(&["pinvoy", "models", "list"]);
    let value: serde_json::Value = serde_json::from_str(&json).expect("single-line json");
    assert_eq!(
        value["models"][0]["credential_state"], "env_override",
        "a set DEEPSEEK_API_KEY must mark models env_override in list json"
    );
    assert!(
        human.contains("credential_state=env_override"),
        "models list human output must carry the env_override state"
    );
}

fn default_preset_str() -> String {
    ModelPreset::default().as_str().to_owned()
}

#[test]
fn settings_get_all_and_single_key_round_trip() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("settings-roundtrip");

    // All-settings get is JSON even in human mode.
    let all = run_ok(&["pinvoy", "settings", "get"]);
    let value: serde_json::Value = serde_json::from_str(&all).expect("json settings dump");
    assert!(value.get("theme").is_some());
    assert!(value.get("notifications").is_some());
    assert!(value.get("advanced").is_some());

    // set -> prefs layer persists -> get reflects it; assert through
    // UserPrefs::load to prove the write went through the prefs layer.
    run_ok(&["pinvoy", "settings", "set", "theme", "liquid-dark"]);
    assert_eq!(load_prefs().theme, Theme::LiquidDark);
    let single = run_ok(&["pinvoy", "--output", "json", "settings", "get", "theme"]);
    assert_eq!(single, r#"{"theme":"liquid-dark"}"#);
    let human = run_ok(&["pinvoy", "settings", "get", "theme"]);
    assert_eq!(human, "theme = liquid-dark");

    run_ok(&["pinvoy", "settings", "set", "color_scheme", "dark"]);
    assert_eq!(load_prefs().color_scheme, ColorScheme::Dark);
    run_ok(&["pinvoy", "settings", "set", "language", "ja"]);
    assert_eq!(load_prefs().language.locale_tag(), "ja");
    run_ok(&[
        "pinvoy",
        "settings",
        "set",
        "notifications.task_completed",
        "false",
    ]);
    assert!(!load_prefs().notifications.task_completed);
    run_ok(&[
        "pinvoy",
        "settings",
        "set",
        "sidebar.date_grouping",
        "false",
    ]);
    assert!(!load_prefs().sidebar.date_grouping);
    run_ok(&[
        "pinvoy",
        "settings",
        "set",
        "voice_shortcut_enabled",
        "true",
    ]);
    assert!(load_prefs().voice_shortcut_enabled);
    run_ok(&["pinvoy", "settings", "set", "pet.enabled", "true"]);
    assert!(load_prefs().pet.enabled);
    run_ok(&["pinvoy", "settings", "set", "mode_defaults.work", "yolo"]);
    assert_eq!(
        load_prefs().mode_defaults.work,
        Some(SerializableMode::Yolo)
    );
    run_ok(&[
        "pinvoy",
        "settings",
        "set",
        "code_permission.last_mode",
        "plan",
    ]);
    assert_eq!(
        load_prefs().code_permission.last_mode,
        Some(SerializableMode::Plan)
    );
    run_ok(&["pinvoy", "settings", "set", "advanced.allow_shell", "true"]);
    assert_eq!(load_prefs().advanced.allow_shell, Some(true));
    run_ok(&["pinvoy", "settings", "set", "advanced.allow_shell", "none"]);
    assert_eq!(load_prefs().advanced.allow_shell, None);
}

#[test]
fn settings_enforces_the_memory_locale_policy_through_the_prefs_layer() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("memory-locale");

    // Non-zh-Hans language does not support memory: the prefs layer
    // normalizes memory_enabled back to false on save.
    run_ok(&["pinvoy", "settings", "set", "language", "en"]);
    run_ok(&["pinvoy", "settings", "set", "memory_enabled", "true"]);
    assert!(!load_prefs().memory_enabled, "locale policy must win");

    // zh-Hans supports memory: the value sticks.
    run_ok(&["pinvoy", "settings", "set", "language", "zh-Hans"]);
    run_ok(&["pinvoy", "settings", "set", "memory_enabled", "true"]);
    assert!(load_prefs().memory_enabled);
}

#[test]
fn settings_json_get_reports_a_bad_key_as_usage_error() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("settings-bad-key");
    let parsed = parse_args(["pinvoy", "settings", "get", "nope"].to_vec()).unwrap_err();
    assert_eq!(parsed.exit_code(), ExitCode::Usage);
}

#[test]
fn models_add_use_show_remove_round_trip_without_secrets() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("model-lifecycle");

    // Add without a key: must not touch the credential store.
    let stdout = run_ok(ADD_ARGS);
    let added_id = stdout
        .strip_prefix("id: ")
        .expect("prints the new id")
        .trim()
        .to_owned();
    assert!(
        added_id.starts_with("m_"),
        "models add should print an m_-prefixed GUI id"
    );

    // list shows the entry with the requested limits.
    let json = run_ok(&["pinvoy", "--output", "json", "models", "list"]);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let entry = value["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["id"] == added_id.as_str())
        .expect("added model listed")
        .clone();
    assert_eq!(entry["preset"], "deepseek");
    assert_eq!(entry["model"], "deepseek-v4-pro");
    assert_eq!(entry["base_url"], "https://api.deepseek.com");
    assert_eq!(entry["has_secret"], false);
    assert_eq!(entry["credential_state"], "missing");
    assert_eq!(entry["active"], false);

    // use -> active marker flips and prefs state matches.
    assert_eq!(
        run_ok(&["pinvoy", "models", "use", &added_id]),
        format!("active: {added_id}")
    );
    assert_eq!(
        load_prefs().advanced.active_model_id.as_deref(),
        Some(added_id.as_str())
    );
    let json = run_ok(&["pinvoy", "--output", "json", "models", "list"]);
    let value: serde_json::Value = serde_json::from_str(&json).expect("single-line json");
    let active_entry = value["models"]
        .as_array()
        .expect("models array")
        .iter()
        .find(|model| model["id"] == added_id.as_str())
        .expect("added model listed")
        .clone();
    assert_eq!(
        active_entry["active"], true,
        "models list json should mark the model active"
    );

    // show prints config, never a key.
    let human = run_ok(&["pinvoy", "models", "show", &added_id]);
    assert!(
        human.contains("preset: deepseek"),
        "models show should print the preset line"
    );
    assert!(
        human.contains("has_secret: false"),
        "models show should report has_secret: false"
    );
    assert!(
        !human.contains("api_key"),
        "no api_key line without --reveal-key"
    );

    // remove enforces --yes and the min-1 rule.
    let (message, code) = run_err(&["pinvoy", "models", "remove", &added_id]);
    assert_eq!(code, ExitCode::Usage);
    assert!(
        message.contains("--yes"),
        "the --yes refusal must name the flag"
    );
    assert!(
        load_prefs().model_by_id(&added_id).is_some(),
        "not removed yet"
    );

    assert_eq!(
        run_ok(&["pinvoy", "models", "remove", &added_id, "--yes"]),
        format!("removed: {added_id}")
    );
    assert!(load_prefs().model_by_id(&added_id).is_none());

    // Fresh home still has the migrated default model: removing it violates
    // the GUI's min-1 rule.
    let (message, code) = run_err(&["pinvoy", "models", "remove", "default", "--yes"]);
    assert_eq!(code, ExitCode::Usage);
    assert!(
        message.contains("last remaining model"),
        "the min-1 refusal must keep the GUI wording"
    );
    assert!(load_prefs().model_by_id("default").is_some());
}

/// The min-1-model rule is classified structurally before the remove
/// transaction: removing the only model is refused with a usage error and
/// nothing is written.
#[test]
fn models_remove_refuses_the_only_model_without_changes() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("remove-only-model");
    let before = std::fs::read_to_string(home_settings_path()).unwrap_or_default();
    let (message, code) = run_err(&["pinvoy", "models", "remove", "default", "--yes"]);
    assert_eq!(
        code,
        ExitCode::Usage,
        "the min-1 rule must be a usage error"
    );
    assert!(
        message.contains("cannot remove the last"),
        "the refusal must reuse the GUI min-1 wording"
    );
    assert!(
        load_prefs().model_by_id("default").is_some(),
        "the only model must survive the refused removal"
    );
    let after = std::fs::read_to_string(home_settings_path()).unwrap_or_default();
    assert_eq!(
        before, after,
        "a refused removal must not write settings.json"
    );
}

#[test]
fn models_add_set_active_and_unknown_ids() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("model-set-active");

    let stdout = run_ok(&[
        "pinvoy",
        "models",
        "add",
        "--preset",
        "kimi",
        "--name",
        "Kimi",
        "--model",
        "kimi-k3",
        "--base-url",
        "https://api.moonshot.cn/v1",
        "--context-window",
        "262144",
        "--max-output",
        "8192",
        "--reasoning-effort",
        "high",
        "--set-active",
    ]);
    let id = stdout.strip_prefix("id: ").unwrap().trim().to_owned();
    let prefs = load_prefs();
    assert_eq!(prefs.advanced.active_model_id.as_deref(), Some(id.as_str()));
    let model = prefs.model_by_id(&id).unwrap();
    assert_eq!(model.context_window_tokens, Some(262_144));
    assert_eq!(model.max_output_tokens, Some(8_192));
    assert_eq!(model.reasoning_effort.as_deref(), Some("high"));

    for command in [
        vec!["pinvoy", "models", "use", "missing-id"],
        vec!["pinvoy", "models", "show", "missing-id"],
        vec!["pinvoy", "models", "remove", "missing-id", "--yes"],
    ] {
        let (message, code) = run_err(&command);
        // Unknown ids exit 1 like every other family: a lookup miss against
        // the live store is a runtime failure, not argv misuse.
        assert_eq!(
            code,
            ExitCode::Failed,
            "unknown model ids must exit 1 (host failure)"
        );
        assert!(
            message.contains("model not found"),
            "unknown model ids must be reported as model not found"
        );
    }
}

#[test]
fn models_add_reports_missing_secret_env_as_host_failure() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("missing-secret-env");
    let (message, code) = run_err(&[
        "pinvoy",
        "models",
        "add",
        "--preset",
        "deepseek",
        "--name",
        "N",
        "--model",
        "M",
        "--base-url",
        "https://api.deepseek.com",
        "--api-key-env",
        "PINVOU_CLI_TEST_DEFINITELY_MISSING_KEY",
    ]);
    assert_eq!(code, ExitCode::Failed);
    assert_eq!(
        message,
        "secret environment variable PINVOU_CLI_TEST_DEFINITELY_MISSING_KEY is not set"
    );
    // Failed resolution must not have touched settings.
    let written = std::fs::read_to_string(home_settings_path()).unwrap_or_default();
    assert!(
        !written.contains("deepseek-v4-pro"),
        "no model may be added when secret resolution fails"
    );
}

fn home_settings_path() -> std::path::PathBuf {
    std::env::var_os("PINVOU3_HOME")
        .map(std::path::PathBuf::from)
        .expect("PINVOU3_HOME set by SandboxHome")
        .join("settings.json")
}

#[test]
fn probe_local_refuses_non_loopback_urls_with_usage_error() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("probe-local-guard");

    for url in [
        "https://api.deepseek.com/v1",
        "http://10.0.0.7:8000/v1",
        "http://192.168.1.5:8000/v1",
        "http://example.invalid:8000/v1",
    ] {
        let (message, code) = run_err(&["pinvoy", "models", "probe-local", "--url", url]);
        assert_eq!(
            code,
            ExitCode::Usage,
            "non-loopback probe-local urls must be usage errors"
        );
        assert!(
            message.contains("loopback"),
            "the refusal must name the loopback rule"
        );
    }

    // Malformed URL is a usage error too.
    let (message, code) = run_err(&["pinvoy", "models", "probe-local", "--url", "not a url"]);
    assert_eq!(code, ExitCode::Usage);
    assert!(
        message.contains("not a valid url"),
        "a malformed probe-local url must be reported as invalid"
    );

    // Usage validation precedes env resolution: a broken --api-key-env must
    // not downgrade the non-loopback refusal from usage (2) to failed (1).
    let (message, code) = run_err(&[
        "pinvoy",
        "models",
        "probe-local",
        "--url",
        "https://api.deepseek.com/v1",
        "--api-key-env",
        "PINVOU_CLI_TEST_DEFINITELY_MISSING_KEY",
    ]);
    assert_eq!(
        code,
        ExitCode::Usage,
        "the loopback refusal must win over the env failure"
    );
    assert!(message.contains("loopback"), "message: {message}");
}

/// The no-`--url` branch obeys the same orderings as the `--url` branch.
///
/// Two separate defects lived here, both invisible to the test above because
/// it only ever passed `--url`:
///
/// 1. Ordering. The `--url` branch checks the loopback constraint (usage,
///    exit 2) before resolving `--api-key-env` (host failure, exit 1). The
///    stored-`base_url` branch did the opposite — it resolved the env var
///    first — so `probe-local --api-key-env MISSING` against a non-loopback
///    active model exited 1 where the contract says 2. Resolving the target
///    first for BOTH branches makes the ordering a property of the function.
///
/// 2. Classification. A malformed `base_url` read out of `settings.json` was
///    reported as a USAGE error (exit 2), but argv was well-formed — the
///    host's own stored state is broken, which is exit 1. `models test`
///    already classifies exactly this condition that way
///    (`{"code":"invalid_url"}`), so the two commands disagreed about the
///    same fact. probe-local now matches `models test`.
///
/// Neither case reaches the network: both refusals happen before any request.
#[test]
fn probe_local_without_url_orders_usage_before_env_and_reports_stored_url_failures() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("probe-local-active-model");

    // No key is passed, so `models add` never touches the credential store.
    let stdout = run_ok(&[
        "pinvoy",
        "models",
        "add",
        "--preset",
        "deepseek",
        "--name",
        "Remote",
        "--model",
        "deepseek-v4-pro",
        "--base-url",
        "https://api.deepseek.com/v1",
        "--set-active",
    ]);
    assert!(stdout.starts_with("id: "), "{stdout}");

    // Ordering: the stored base_url is non-loopback AND the env var is
    // missing. Usage (2) must win, exactly as it does on the --url branch.
    // Before the fix this exits 1 with the env-var message.
    let (message, code) = run_err(&[
        "pinvoy",
        "models",
        "probe-local",
        "--api-key-env",
        "PINVOU_CLI_TEST_DEFINITELY_MISSING_KEY",
    ]);
    assert_eq!(
        code,
        ExitCode::Usage,
        "the stored-base_url loopback refusal must win over the env failure: {message}"
    );
    assert!(message.contains("loopback"), "message: {message}");

    // Same refusal without the broken env override, so the ordering fix is
    // not the only reason the assertion above passes.
    let (message, code) = run_err(&["pinvoy", "models", "probe-local"]);
    assert_eq!(code, ExitCode::Usage, "message: {message}");
    assert!(message.contains("loopback"), "message: {message}");

    // Classification: a malformed stored base_url is a host failure (1),
    // matching `models test`'s invalid_url, not a usage error about argv.
    // Before the fix this exits 2.
    let stdout = run_ok(&[
        "pinvoy",
        "models",
        "add",
        "--preset",
        "openai_compatible",
        "--name",
        "Broken",
        "--model",
        "local",
        "--base-url",
        "not a url",
        "--set-active",
    ]);
    assert!(stdout.starts_with("id: "), "{stdout}");
    let (message, code) = run_err(&["pinvoy", "models", "probe-local"]);
    assert_eq!(
        code,
        ExitCode::Failed,
        "a corrupt stored base_url is a host failure, not a usage error: {message}"
    );
    assert!(
        message.contains("invalid_url"),
        "the message must use the same code `models test` reports: {message}"
    );
    // A malformed value supplied on the command line stays a usage error:
    // the classification follows where the url came from, not its shape.
    let (_, code) = run_err(&["pinvoy", "models", "probe-local", "--url", "not a url"]);
    assert_eq!(code, ExitCode::Usage);
}

/// An empty value for a valued option is a missing value in the `models`
/// family too, matching `support::parse_family_flags` and `memory`.
///
/// `models` rejected a value that merely *looked* like a flag
/// (`starts_with("--")`) but accepted `""`, so `--name ""` reached the store
/// and `--api-key-env ""` reached `std::env::var("")`, each failing later
/// with a message about the store or the environment rather than about the
/// command line. Without the `value.is_empty()` guard every case below
/// parses.
#[test]
fn models_rejects_empty_option_values() {
    for args in [
        vec![
            "pinvoy",
            "models",
            "add",
            "--preset",
            "deepseek",
            "--name",
            "",
            "--model",
            "M",
            "--base-url",
            "https://example.com",
        ],
        vec![
            "pinvoy",
            "models",
            "add",
            "--preset",
            "deepseek",
            "--name",
            "N",
            "--model",
            "M",
            "--base-url",
            "https://example.com",
            "--api-key-env",
            "",
        ],
        vec!["pinvoy", "models", "probe-local", "--url", ""],
        vec!["pinvoy", "settings", "search", "set", "--provider", ""],
    ] {
        let error = parse_args(args.clone()).expect_err("an empty option value is a usage error");
        assert_eq!(error.exit_code(), ExitCode::Usage, "{args:?}: {error}");
        assert!(
            error.to_string().contains("requires a value"),
            "{args:?}: {error}"
        );
    }
}

/// `settings get` with no key is always JSON, and the usage text says so.
///
/// `--output` is a global flag accepted on every subcommand, so a reader can
/// reasonably expect `--output human` to change this dump; it cannot, because
/// the payload is the whole nested `UserPrefs` document with no `key = value`
/// rendering. Silently ignoring the flag is the thing under test: the
/// behaviour is fine, leaving it undocumented was not.
///
/// This also covers the other half of the same defect — the dump used to end
/// in `serde_json::to_string(&value).unwrap_or_default()`, so a serialization
/// failure printed an EMPTY line and exited 0. It now propagates as a host
/// failure. `UserPrefs` cannot actually fail to serialize, so the propagation
/// itself is pinned by the type system (`?` on a `Result`) rather than by a
/// test that would need an unserializable settings document to exist.
#[test]
fn settings_get_without_a_key_is_always_json_and_documented_as_such() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("settings-get-all");

    for mode in [
        vec!["pinvoy", "settings", "get"],
        vec!["pinvoy", "--output", "human", "settings", "get"],
        vec!["pinvoy", "--output", "json", "settings", "get"],
    ] {
        let stdout = run_ok(&mode);
        assert!(
            !stdout.trim().is_empty(),
            "{mode:?}: the settings dump must never be an empty successful line"
        );
        let value: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|error| panic!("{mode:?} must print JSON: {error}: {stdout}"));
        assert!(
            value.is_object(),
            "{mode:?}: the dump is the whole settings object: {value}"
        );
    }

    // The always-JSON rule is stated where a caller looks for it rather than
    // left to be discovered from output that did not change.
    let message = usage_error(&["pinvoy", "settings"]);
    assert!(
        message.contains("always prints JSON") && message.contains("--output"),
        "the settings usage must disclose that the keyless dump ignores --output: {message}"
    );
}

#[test]
fn settings_search_list_reports_defaults() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("search-list");
    let human = run_ok(&["pinvoy", "settings", "search", "list"]);
    assert!(
        human.contains("provider: bing"),
        "search list should default to provider bing"
    );
    let json = run_ok(&["pinvoy", "--output", "json", "settings", "search", "list"]);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["provider"], "bing");
    assert_eq!(value["enabled_providers"][0], "bing");
}

/// The non-Bing `settings search test` lane had NO execution coverage at all,
/// and under it the command reported `{"ok":true,"code":"configured"}` for
/// four of the five providers as soon as a non-empty credential existed,
/// without contacting anything — so a revoked, expired or garbage key passed
/// a command called `test`.
///
/// This pins the half of the contract that can be asserted without a network:
/// with no credential anywhere, the command must fail, must say it verified
/// only credential presence, and must never emit the `configured` code that
/// used to stand in for a successful test. The live-probe half is covered by
/// the `#[ignore]` opt-in tests at the bottom of this file and by
/// `search_api_requests_carry_the_key_for_every_api_provider` in
/// `src/models.rs`, which pins the request shapes.
#[test]
fn settings_search_test_without_a_key_verifies_only_credential_presence() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("search-test-no-key");
    // The env tier of the credential resolution must be empty for the
    // providers that have env names, or the test would try to reach them.
    let _restore_metaso = RestoreEnvVar("METASO_API_KEY", std::env::var_os("METASO_API_KEY"));
    let _restore_baidu = RestoreEnvVar(
        "BAIDU_SEARCH_API_KEY",
        std::env::var_os("BAIDU_SEARCH_API_KEY"),
    );
    unsafe {
        std::env::remove_var("METASO_API_KEY");
        std::env::remove_var("BAIDU_SEARCH_API_KEY");
    }

    for provider in ["metaso", "bocha", "baidu", "tavily"] {
        let (stdout, code) = run_outcome(&[
            "pinvoy", "--output", "json", "settings", "search", "test", provider,
        ]);
        let value: serde_json::Value =
            serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{provider}: {e}: {stdout}"));
        assert_eq!(
            code,
            ExitCode::Failed,
            "{provider}: an unconfigured provider must not pass its own test: {stdout}"
        );
        assert_eq!(value["provider"], provider);
        assert_eq!(value["ok"], false, "{provider}: {stdout}");
        assert_eq!(value["code"], "no_api_key", "{provider}: {stdout}");
        assert_eq!(
            value["verified"], "credential_presence",
            "{provider}: the row must say nothing was contacted: {stdout}"
        );

        let (human, _) = run_outcome(&["pinvoy", "settings", "search", "test", provider]);
        assert!(
            human.contains("verified: credential_presence"),
            "{provider}: the human line must carry the same disclosure: {human}"
        );
        // `configured` was the code that used to stand in for a passed test.
        assert!(
            !human.contains("code: configured") && !human.contains("ok: true"),
            "{provider}: credential presence must never be rendered as a passed test: {human}"
        );
    }
}

/// `settings search set --provider P --clear` is a credential operation on P,
/// not a selection of P. It used to run `prefs.search.provider = provider`
/// unconditionally, so clearing a NON-active provider's key silently moved
/// the user's search backend onto it.
///
/// Touches no keyring: the provider has never been configured, so there is no
/// credential reference to delete.
#[test]
fn settings_search_set_clear_does_not_switch_the_active_provider() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("search-clear-active");
    assert_eq!(
        load_prefs().search.provider,
        SearchProvider::Bing,
        "a fresh home starts on the default provider"
    );

    let stdout = run_ok(&[
        "pinvoy",
        "--output",
        "json",
        "settings",
        "search",
        "set",
        "--provider",
        "tavily",
        "--clear",
        "--yes",
    ]);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("single-line json");
    assert_eq!(
        value["provider"], "tavily",
        "the credential that was cleared"
    );
    assert_eq!(value["credential"], "cleared");
    assert_eq!(
        value["active_provider"], "bing",
        "clearing a key must not select its provider: {stdout}"
    );
    assert_eq!(
        load_prefs().search.provider,
        SearchProvider::Bing,
        "the persisted active provider must be untouched by a clear"
    );

    // Selecting is still selecting: the non-clear form does switch. Bing
    // takes no api key, so this exercises the selection path without any
    // credential-store write.
    run_ok(&["pinvoy", "settings", "search", "set", "--provider", "bing"]);
    assert_eq!(load_prefs().search.provider, SearchProvider::Bing);
}

/// Round-18 finding: credential deletions ran without `--yes`. In the
/// models family that is `models edit --clear-api-key` — it deletes the
/// keyring entry and marks the model `missing`, i.e. the same destructive
/// credential loss `models remove` already gates with `require_yes`. Without
/// `--yes` the command must refuse with the family's confirmation-refusal
/// shape (usage error naming `--yes`), with `--yes` it must proceed.
///
/// Both cases drive the real store under a sandboxed `PINVOU3_HOME`. The
/// model is never configured, so the clear path runs end to end without
/// touching any keyring entry (the keyring deletion is gated on the model's
/// stored `credential_ref`).
#[test]
fn models_edit_clear_api_key_requires_yes() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("edit-clear-api-key-yes");

    // The bare form must refuse like `models remove`: a usage error whose
    // message names --yes, with nothing written.
    let before = std::fs::read_to_string(home_settings_path()).unwrap_or_default();
    let (message, code) = run_err(&["pinvoy", "models", "edit", "default", "--clear-api-key"]);
    assert_eq!(
        code,
        ExitCode::Usage,
        "the confirmation refusal is a usage error, like models remove: {message}"
    );
    assert!(
        message.contains("--yes"),
        "the refusal must name the flag: {message}"
    );
    assert_eq!(
        std::fs::read_to_string(home_settings_path()).unwrap_or_default(),
        before,
        "a refused clear must not write settings.json"
    );

    // With --yes the same command proceeds and reports the credential as
    // cleared.
    let stdout = run_ok(&[
        "pinvoy",
        "models",
        "edit",
        "default",
        "--clear-api-key",
        "--yes",
    ]);
    assert!(stdout.contains("credential: cleared"), "{stdout}");
    let prefs = load_prefs();
    let model = prefs.model_by_id("default").expect("default model");
    assert!(
        !model.has_secret && model.credential_ref.is_none(),
        "the cleared model must be recorded as secretless: {:?}",
        model.credential_state
    );

    // --yes composes with a field change on the same command (the refusal
    // happens at execute, exactly like `models remove`).
    parse_args(
        [
            "pinvoy",
            "models",
            "edit",
            "m1",
            "--clear-api-key",
            "--yes",
            "--name",
            "Renamed",
        ]
        .to_vec(),
    )
    .expect("clear-api-key with --yes and a field change must parse");
}

/// Round-18 finding (same class): `settings search set --provider P --clear`
/// wipes provider P's stored credential — the search-family sibling of
/// `models edit --clear-api-key` — and ran without `--yes`. Same contract:
/// without `--yes` refuse with the family's confirmation-refusal shape and
/// write nothing; with `--yes` proceed (and still not switch the active
/// provider). Drives the real store under a sandboxed `PINVOU3_HOME`; the
/// provider is never configured, so no keyring entry is ever touched.
#[test]
fn settings_search_set_clear_requires_yes() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("search-set-clear-yes");

    let before = std::fs::read_to_string(home_settings_path()).unwrap_or_default();
    let (message, code) = run_err(&[
        "pinvoy",
        "settings",
        "search",
        "set",
        "--provider",
        "tavily",
        "--clear",
    ]);
    assert_eq!(
        code,
        ExitCode::Usage,
        "the confirmation refusal is a usage error, like models remove: {message}"
    );
    assert!(
        message.contains("--yes"),
        "the refusal must name the flag: {message}"
    );
    assert_eq!(
        std::fs::read_to_string(home_settings_path()).unwrap_or_default(),
        before,
        "a refused clear must not write settings.json"
    );

    // With --yes the clear proceeds (and still must not switch the active
    // provider — that contract is pinned separately in
    // settings_search_set_clear_does_not_switch_the_active_provider).
    let stdout = run_ok(&[
        "pinvoy",
        "--output",
        "json",
        "settings",
        "search",
        "set",
        "--provider",
        "tavily",
        "--clear",
        "--yes",
    ]);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("single-line json");
    assert_eq!(value["credential"], "cleared", "{stdout}");
    assert_eq!(value["active_provider"], "bing", "{stdout}");
    assert_eq!(
        load_prefs().search.provider,
        SearchProvider::Bing,
        "the persisted active provider must be untouched by a clear"
    );
}

/// Human rows must not be forgeable. `models.rs` was the last family still
/// interpolating untrusted cells raw, and `parse_add`'s `.trim()` only strips
/// the EDGES — so `--name $'ok\n*m_fake\tEvil'` injected a line
/// indistinguishable from a real active-model row.
#[test]
fn models_list_and_show_collapse_control_characters_in_human_rows() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("models-row-hygiene");
    let hostile = "ok\n*m_fake\tEvil";
    let stdout = run_ok(&[
        "pinvoy",
        "models",
        "add",
        "--preset",
        "deepseek",
        "--name",
        hostile,
        "--model",
        "deepseek-v4-pro",
        "--base-url",
        "https://api.deepseek.com",
    ]);
    let id = stdout.strip_prefix("id: ").unwrap().trim().to_owned();

    let human = run_ok(&["pinvoy", "models", "list"]);
    assert_eq!(
        human.lines().count(),
        2,
        "one row per model, no forged line: {human:?}"
    );
    assert!(
        human.lines().all(|line| !line.starts_with("*m_fake")),
        "a name must not be able to forge an active-model row: {human:?}"
    );
    let row = human
        .lines()
        .find(|line| line.contains(&id))
        .expect("the added model has a row");
    assert!(
        row.contains("ok *m_fake Evil"),
        "the control characters must be collapsed to spaces, not dropped: {row:?}"
    );

    let shown = run_ok(&["pinvoy", "models", "show", &id]);
    assert!(
        shown.contains("name: ok *m_fake Evil"),
        "models show must collapse the same cells: {shown:?}"
    );

    // JSON output still carries the original untouched — the hygiene is a
    // rendering concern, not a storage one.
    let json = run_ok(&["pinvoy", "--output", "json", "models", "show", &id]);
    let value: serde_json::Value = serde_json::from_str(&json).expect("single-line json");
    assert_eq!(value["name"], hostile);
}

/// `--reveal-key` on an `EnvOverride` model printed `api_key: (not stored)`,
/// which is false: that model HAS a key, supplied by the environment. The CLI
/// refuses to echo a value it does not own (mirroring `reveal_model_api_key`),
/// but it must say WHY rather than report the key as absent.
#[test]
fn models_show_reveal_key_names_the_credential_source() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("reveal-key-source");
    let _restore_deepseek_key =
        RestoreEnvVar("DEEPSEEK_API_KEY", std::env::var_os("DEEPSEEK_API_KEY"));
    unsafe { std::env::remove_var("DEEPSEEK_API_KEY") };

    // No credential reference at all: genuinely nothing stored.
    let human = run_ok(&["pinvoy", "models", "show", "default", "--reveal-key"]);
    assert!(human.contains("api_key_source: none"), "{human}");
    assert!(human.contains("api_key: (not stored)"), "{human}");

    // The env override is what used to be misreported.
    unsafe { std::env::set_var("DEEPSEEK_API_KEY", "pinvou-cli-contract-override") };
    let human = run_ok(&["pinvoy", "models", "show", "default", "--reveal-key"]);
    assert!(
        human.contains("api_key_source: environment"),
        "an env-overridden model must name the environment as the source: {human}"
    );
    assert!(
        !human.contains("api_key: (not stored)"),
        "an env-overridden model has a key; reporting it as absent is false: {human}"
    );
    assert!(
        human.contains("DEEPSEEK_API_KEY"),
        "the line must name the variable that supplies it: {human}"
    );
    let json = run_ok(&[
        "pinvoy",
        "--output",
        "json",
        "models",
        "show",
        "default",
        "--reveal-key",
    ]);
    let value: serde_json::Value = serde_json::from_str(&json).expect("single-line json");
    assert_eq!(value["api_key_source"], "environment");
    assert!(
        value["api_key"].is_null(),
        "the env-supplied value is still never echoed: {json}"
    );
}

/// `models add` used to hardcode the five optional GUI-form fields to `None`,
/// so a CLI-created model was not expressible: `vendor` in particular stays
/// `None` and is read for reasoning-protocol routing
/// (`features/assistant/platform/bridge.rs`).
#[test]
fn models_add_writes_the_gui_form_metadata() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("models-add-metadata");
    let stdout = run_ok(&[
        "pinvoy",
        "models",
        "add",
        "--preset",
        "openai_compatible",
        "--name",
        "Compatible",
        "--model",
        "glm-4.7",
        "--base-url",
        "https://example.invalid/api/paas/v4",
        "--alias",
        "GLM",
        "--provider-kind",
        "custom",
        "--vendor",
        "glm",
        "--endpoint-mode",
        "full_chat_completions",
    ]);
    let id = stdout.strip_prefix("id: ").unwrap().trim().to_owned();
    let prefs = load_prefs();
    let model = prefs.model_by_id(&id).expect("added model");
    assert_eq!(model.alias.as_deref(), Some("GLM"));
    assert_eq!(model.provider_kind.as_deref(), Some("custom"));
    assert_eq!(model.vendor.as_deref(), Some("glm"));
    assert_eq!(
        model.endpoint_mode.as_deref(),
        Some("full_chat_completions")
    );

    // A vision fallback must name a model that exists, or it is a routing
    // preference that silently never fires.
    let (message, code) = run_err(&[
        "pinvoy",
        "models",
        "add",
        "--preset",
        "deepseek",
        "--name",
        "Vision",
        "--model",
        "deepseek-v4-pro",
        "--base-url",
        "https://api.deepseek.com",
        "--vision-model-id",
        "m_does_not_exist",
    ]);
    assert_eq!(code, ExitCode::Failed);
    assert!(message.contains("vision model not found"), "{message}");

    // An unrecognized provider_kind is a typo, not a value the prefs layer
    // will keep.
    let message = usage_error(&[
        "pinvoy",
        "models",
        "add",
        "--preset",
        "deepseek",
        "--name",
        "N",
        "--model",
        "M",
        "--base-url",
        "https://api.deepseek.com",
        "--provider-kind",
        "nope",
    ]);
    assert!(
        message.contains("official_api") && message.contains("custom"),
        "{message}"
    );
}

/// The gap that silently breaks other features: rotating a key or fixing a
/// `base_url` used to require `remove` + `add`, which mints a NEW id and
/// therefore orphans per-session model bindings and scheduled-task model
/// pins. `models edit` must change everything EXCEPT the id.
#[test]
fn models_edit_mutates_in_place_and_preserves_the_id() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("models-edit");
    let stdout = run_ok(&[
        "pinvoy",
        "models",
        "add",
        "--preset",
        "deepseek",
        "--name",
        "Before",
        "--model",
        "deepseek-v4-pro",
        "--base-url",
        "https://api.deepseek.com",
        "--context-window",
        "131072",
        "--reasoning-effort",
        "high",
        "--alias",
        "DS",
    ]);
    let id = stdout.strip_prefix("id: ").unwrap().trim().to_owned();

    let json = run_ok(&[
        "pinvoy",
        "--output",
        "json",
        "models",
        "edit",
        &id,
        "--name",
        "After",
        "--base-url",
        "https://api.deepseek.com/v1",
        "--vendor",
        "deepseek",
        // `none` clears an optional field.
        "--alias",
        "none",
        "--context-window",
        "none",
    ]);
    let value: serde_json::Value = serde_json::from_str(&json).expect("single-line json");
    assert_eq!(value["id"], id, "an edit must never mint a new id");
    assert_eq!(value["updated"], true);
    assert_eq!(
        value["credential"], "unchanged",
        "no credential flag means the stored secret bookkeeping is carried over"
    );

    let prefs = load_prefs();
    assert_eq!(
        prefs.advanced.saved_models.len(),
        2,
        "edit updates in place; it must not append a second record"
    );
    let model = prefs.model_by_id(&id).expect("the same id still resolves");
    assert_eq!(model.name, "After");
    assert_eq!(model.base_url, "https://api.deepseek.com/v1");
    assert_eq!(model.vendor.as_deref(), Some("deepseek"));
    assert_eq!(model.alias, None, "`none` clears an optional field");
    assert_eq!(model.context_window_tokens, None);
    assert_eq!(
        model.reasoning_effort.as_deref(),
        Some("high"),
        "a field whose flag was not given must be left alone"
    );

    // --set-active works through edit too, still on the same id.
    run_ok(&["pinvoy", "models", "edit", &id, "--set-active"]);
    assert_eq!(load_prefs().advanced.active_model_id.as_deref(), Some(&*id));
}

#[test]
fn models_edit_rejects_no_op_unknown_ids_and_conflicting_credential_flags() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("models-edit-guards");

    // No field flag at all: a mistyped command, not a successful no-op write.
    let message = usage_error(&["pinvoy", "models", "edit", "default"]);
    assert!(message.contains("--base-url"), "{message}");

    // Three mutually exclusive credential intents.
    let message = usage_error(&[
        "pinvoy",
        "models",
        "edit",
        "default",
        "--api-key-stdin",
        "--clear-api-key",
    ]);
    assert!(message.contains("--clear-api-key"), "{message}");

    // Identity fields are not clearable; the model would stop being a model.
    let message = usage_error(&["pinvoy", "models", "edit", "default", "--name", "   "]);
    assert!(message.contains("--name"), "{message}");

    // A model cannot be its own vision fallback.
    let message = usage_error(&[
        "pinvoy",
        "models",
        "edit",
        "default",
        "--vision-model-id",
        "default",
    ]);
    assert!(message.contains("different model"), "{message}");

    // Unknown ids exit 1 like every other family.
    let (message, code) = run_err(&["pinvoy", "models", "edit", "missing-id", "--name", "X"]);
    assert_eq!(code, ExitCode::Failed);
    assert!(message.contains("model not found"), "{message}");
}

/// `probe-local --url` without a credential silently misclassified an
/// authenticated local server: `--api-key-env` was the only way to name one,
/// and the GUI's counterpart resolves a SAVED key through its `model_id`
/// parameter. `--model ID` is that parameter.
///
/// Reaches no network: every case below is refused before the first request.
#[test]
fn probe_local_model_flag_names_a_saved_credential() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("probe-local-model");

    // --model is about WHICH saved credential to present, so it needs the
    // endpoint to present it to. Without --url the active model's own
    // credential is already used, and naming another model there would send
    // that model's key to a different model's endpoint.
    let message = usage_error(&["pinvoy", "models", "probe-local", "--model", "default"]);
    assert!(message.contains("--url"), "{message}");

    // Two credential sources at once would leave the choice to evaluation
    // order.
    let message = usage_error(&[
        "pinvoy",
        "models",
        "probe-local",
        "--url",
        "http://127.0.0.1:8000/v1",
        "--model",
        "default",
        "--api-key-env",
        "SOME_VAR",
    ]);
    assert!(message.contains("--model"), "{message}");

    // An unknown id FAILS instead of degrading to an anonymous probe, which
    // is how the misclassification comes back.
    let (message, code) = run_err(&[
        "pinvoy",
        "models",
        "probe-local",
        "--url",
        "http://127.0.0.1:8000/v1",
        "--model",
        "missing-id",
    ]);
    assert_eq!(code, ExitCode::Failed);
    assert!(message.contains("model not found"), "{message}");

    // A known model with no stored key cannot present one either: say so
    // rather than probe anonymously and report a kind.
    let (message, code) = run_err(&[
        "pinvoy",
        "models",
        "probe-local",
        "--url",
        "http://127.0.0.1:8000/v1",
        "--model",
        "default",
    ]);
    assert_eq!(code, ExitCode::Failed);
    assert!(
        message.contains("no stored api key"),
        "the refusal must name the missing credential: {message}"
    );
}

// ---------------------------------------------------------------------------
// Network paths: opt-in only, never run by default (AGENTS.md rule).
// ---------------------------------------------------------------------------

/// Opt-in: `cargo test -p pinvoy-cli --test models_contract -- --ignored bing_probe_hits_live_endpoint`
/// Requires internet access. The GUI ships no search-provider test at all
/// (see the `models` module doc), so this is the only automated check of the
/// live Bing lane.
#[test]
#[ignore = "network: run with `pinvoy settings search test bing` against bing.com"]
fn bing_probe_hits_live_endpoint() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("ignored-bing-test");
    let stdout = run_ok(&["pinvoy", "settings", "search", "test", "bing"]);
    assert!(
        stdout.contains("ok: true"),
        "search test should report ok: true"
    );
}

/// Opt-in: the API-provider lane now sends a REAL search request, so the key
/// it reports on is actually exercised. Requires internet access AND a
/// configured credential for the provider under test (environment variable or
/// `settings search set`), and spends one search against that provider's
/// quota — which is the cost of the command meaning what its name says.
///
/// `cargo test -p pinvoy-cli --test models_contract -- --ignored search_api_probe_validates_a_live_key`
#[test]
#[ignore = "network + quota: run with a real key, e.g. METASO_API_KEY=... pinvoy settings search test metaso"]
fn search_api_probe_validates_a_live_key() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let provider =
        std::env::var("PINVOU_CLI_SEARCH_TEST_PROVIDER").unwrap_or_else(|_| "metaso".to_owned());
    let (stdout, code) = run_outcome(&[
        "pinvoy", "--output", "json", "settings", "search", "test", &provider,
    ]);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("single-line json");
    assert_eq!(
        value["verified"], "live_probe",
        "an API provider with a key must be contacted, never reported from presence alone: {stdout}"
    );
    assert_eq!(
        code,
        ExitCode::Success,
        "a valid key must pass its own test: {stdout}"
    );
}

/// Opt-in against a local vLLM/Ollama/LM Studio server:
/// `cargo test -p pinvoy-cli --test models_contract -- --ignored probe_local_identifies_local_server`
#[test]
#[ignore = "network (loopback): run with a local inference server on 127.0.0.1:8000"]
fn probe_local_identifies_local_server() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("ignored-probe-local");
    let stdout = run_ok(&[
        "pinvoy",
        "models",
        "probe-local",
        "--url",
        "http://127.0.0.1:8000/v1",
    ]);
    assert!(
        stdout.contains("kind:"),
        "probe-local output should include a kind line"
    );
}

/// The search provider enum surface the CLI validates against must stay the
/// GUI's five providers (parse-time rejection depends on it).
#[test]
fn search_provider_surface_matches_gui() {
    assert_eq!(SearchProvider::default().as_str(), "bing");
    for provider in ["metaso", "bocha", "baidu", "tavily"] {
        // parse accepts every documented provider spelling
        parse_args(["pinvoy", "settings", "search", "test", provider].to_vec())
            .unwrap_or_else(|error| panic!("provider {provider} must parse: {error}"));
    }
}
