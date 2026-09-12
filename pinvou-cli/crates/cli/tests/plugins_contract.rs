//! Contract tests for the `plugins` family (`pinvou plugins ...`). Parse-level
//! coverage runs against pure parser state; execute-level coverage uses a
//! temporary `PINVOU3_HOME` so no test touches real user data.
//!
//! Hermeticity notes:
//! - Imports (zip / .md / SKILL.md directory), preset skill install/update,
//!   embedded-catalog MCP tool install (manifests without `pip_dependencies`
//!   or required secrets), recycle-bin flows, export, meta, readiness, and
//!   scope toggles are pure file operations — safe as default tests.
//! - Tool installs whose manifest declares `pip_dependencies` (pptx, gongwen,
//!   tencent-docs skills runtime) or required secret config fields (weather,
//!   iwencai, patsnap-search, wecom-bot — the secret lands in the system
//!   credential store) would download packages or touch the OS keyring: they
//!   are `#[ignore]` only. The MCP OAuth login flow itself lives in the
//!   foundation crate and is not reachable headless; the CLI surfaces that as
//!   a deterministic exit-1 error which the default tests cover.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pinvou_cli::{ExitCode, execute, parse_args};

/// Serialises tests that mutate the process-global `PINVOU3_HOME` environment
/// variable, preventing data races when the parallel test runner executes
/// them concurrently (same pattern as cli_contract.rs / models_contract.rs).
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Creates a unique temporary product home and points `PINVOU3_HOME` at it.
/// Restores the previous value and removes the directory on drop so an
/// assertion failure cannot leak environment state into other tests.
struct SandboxHome {
    root: PathBuf,
    previous: Option<std::ffi::OsString>,
}

impl SandboxHome {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "pinvou-cli-plugins-{label}-{}-{}",
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

    fn path(&self) -> &Path {
        &self.root
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

fn usage_error(args: &[&str]) -> String {
    let error = parse_args(args.to_vec()).expect_err("expected a usage error");
    assert_eq!(error.exit_code(), ExitCode::Usage, "message: {error}");
    error.to_string()
}

fn run_ok(args: &[&str]) -> String {
    let parsed = parse_args(args.to_vec()).expect("valid command");
    let outcome = execute(parsed).expect("execute succeeds");
    assert_eq!(outcome.exit_code, ExitCode::Success, "stdout: {outcome:?}");
    outcome.stdout
}

fn run_err(args: &[&str]) -> (String, ExitCode) {
    let parsed = parse_args(args.to_vec()).expect("valid command");
    let error = execute(parsed).expect_err("execute fails");
    (error.to_string(), error.exit_code())
}

/// Runs `args` with `--output json` inserted after argv[0] and parses the
/// single-line JSON object from stdout.
fn run_json(args: &[&str]) -> serde_json::Value {
    let mut owned = args.to_vec();
    owned.insert(1, "--output");
    owned.insert(2, "json");
    let stdout = run_ok(&owned);
    serde_json::from_str(&stdout).unwrap_or_else(|error| panic!("json output: {error}: {stdout}"))
}

fn disabled_bundles_json(home: &Path) -> serde_json::Value {
    let path = home.join("disabled_bundles.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&content).expect("disabled_bundles.json parses")
}

// ---------------------------------------------------------------------------
// fixture helpers
// ---------------------------------------------------------------------------

const FIXTURE_DIR_SKILL: &str = "contract-fixture-skill";
const FIXTURE_ZIP_SKILL: &str = "contract-zip-skill";

/// Minimal STORED (uncompressed) zip writer — the CLI test graph has no zip
/// crate dependency, so the archive is assembled with std only. Mirrors the
/// layout the implementation's own wrapper produces (local headers, central
/// directory, EOCD; CRC-32 over entry bytes).
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn zip_entry(out: &mut Vec<u8>, central: &mut Vec<u8>, name: &str, data: &[u8]) {
    let offset = out.len() as u32;
    let checksum = crc32(data);
    let size = data.len() as u32;
    out.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
    out.extend_from_slice(&20_u16.to_le_bytes());
    out.extend_from_slice(&0x0800_u16.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&0x0021_u16.to_le_bytes());
    out.extend_from_slice(&checksum.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(data);
    central.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
    central.extend_from_slice(&20_u16.to_le_bytes());
    central.extend_from_slice(&20_u16.to_le_bytes());
    central.extend_from_slice(&0x0800_u16.to_le_bytes());
    central.extend_from_slice(&0_u16.to_le_bytes());
    central.extend_from_slice(&0_u16.to_le_bytes());
    central.extend_from_slice(&0x0021_u16.to_le_bytes());
    central.extend_from_slice(&checksum.to_le_bytes());
    central.extend_from_slice(&size.to_le_bytes());
    central.extend_from_slice(&size.to_le_bytes());
    central.extend_from_slice(&(name.len() as u16).to_le_bytes());
    central.extend_from_slice(&0_u16.to_le_bytes());
    central.extend_from_slice(&0_u16.to_le_bytes());
    central.extend_from_slice(&0_u16.to_le_bytes());
    central.extend_from_slice(&0_u16.to_le_bytes());
    central.extend_from_slice(&0_u32.to_le_bytes());
    central.extend_from_slice(&offset.to_le_bytes());
    central.extend_from_slice(name.as_bytes());
}

fn build_stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        zip_entry(&mut out, &mut central, name, data);
    }
    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out
}

fn skill_md(name: &str) -> String {
    format!("---\nname: {name}\n---\n# {name}\n\nFixture skill body for CLI contract tests.\n")
}

/// Creates a SKILL.md directory fixture and returns its path.
fn fixture_skill_dir(home: &Path) -> PathBuf {
    let dir = home.join("fixtures").join(FIXTURE_DIR_SKILL);
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("SKILL.md"), skill_md(FIXTURE_DIR_SKILL)).unwrap();
    std::fs::write(dir.join("docs").join("note.md"), "fixture note").unwrap();
    dir
}

/// Creates a zip skill-package fixture and returns its path.
fn fixture_skill_zip(home: &Path) -> PathBuf {
    let path = home
        .join("fixtures")
        .join(format!("{FIXTURE_ZIP_SKILL}.zip"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes = build_stored_zip(&[
        ("SKILL.md", skill_md(FIXTURE_ZIP_SKILL).as_bytes()),
        ("extra.txt", b"zip fixture payload".as_slice()),
    ]);
    std::fs::write(&path, bytes).unwrap();
    path
}

// ---------------------------------------------------------------------------
// parse level
// ---------------------------------------------------------------------------

#[test]
fn parses_every_plugins_subcommand() {
    for args in [
        vec!["pinvoy", "plugins", "tools", "list"],
        vec!["pinvoy", "plugins", "tools", "list", "--installed-only"],
        vec!["pinvoy", "plugins", "tools", "install", "weather"],
        vec![
            "pinvoy",
            "plugins",
            "tools",
            "install",
            "weather",
            "--secret",
            "AMAP_KEY=MY_AMAP_ENV",
            "--secret",
            "OTHER=MY_OTHER_ENV",
        ],
        vec![
            "pinvoy",
            "plugins",
            "tools",
            "uninstall",
            "weather",
            "--yes",
        ],
        vec!["pinvoy", "plugins", "tools", "auth", "qcc"],
        vec!["pinvoy", "plugins", "tools", "oauth-login", "qcc"],
        vec![
            "pinvoy",
            "plugins",
            "tools",
            "oauth-login",
            "qcc",
            "--timeout",
            "30",
        ],
        vec!["pinvoy", "plugins", "tools", "oauth-cancel", "qcc"],
        vec!["pinvoy", "plugins", "skills", "list"],
        vec!["pinvoy", "plugins", "skills", "list", "--installed-only"],
        vec!["pinvoy", "plugins", "skills", "install", "visualizer"],
        vec!["pinvoy", "plugins", "skills", "update", "visualizer"],
        vec![
            "pinvoy",
            "plugins",
            "skills",
            "uninstall",
            "visualizer",
            "--yes",
        ],
        vec!["pinvoy", "plugins", "import", "/tmp/skill.zip"],
        vec!["pinvoy", "plugins", "export", "my-pkg"],
        vec![
            "pinvoy",
            "plugins",
            "export",
            "my-pkg",
            "--output",
            "/tmp/out.zip",
        ],
        vec!["pinvoy", "plugins", "meta", "my-pkg", "--name", "New Name"],
        vec![
            "pinvoy",
            "plugins",
            "meta",
            "my-pkg",
            "--description",
            "New description",
        ],
        vec!["pinvoy", "plugins", "recycle", "list"],
        vec!["pinvoy", "plugins", "recycle", "restore", "my-pkg"],
        vec!["pinvoy", "plugins", "recycle", "purge", "my-pkg", "--yes"],
        vec![
            "pinvoy", "plugins", "recycle", "export", "my-pkg", "--output", "x.zip",
        ],
        vec!["pinvoy", "plugins", "readiness"],
        vec!["pinvoy", "plugins", "enable", "weather"],
        vec!["pinvoy", "plugins", "enable", "weather", "--scope", "plain"],
        vec!["pinvoy", "plugins", "disable", "weather", "--scope", "code"],
        vec!["pinvoy", "plugins", "disable", "weather", "--scope", "both"],
        vec!["pinvoy", "plugins", "project-skills", "on"],
        vec!["pinvoy", "plugins", "project-skills", "off"],
    ] {
        parse_args(args).unwrap_or_else(|error| panic!("valid command rejected: {error}"));
    }
}

#[test]
fn plugins_usage_errors_exit_two() {
    // missing / unknown subcommands
    assert!(usage_error(&["pinvoy", "plugins"]).contains("usage"));
    assert!(usage_error(&["pinvoy", "plugins", "bogus"]).contains("usage"));
    assert!(usage_error(&["pinvoy", "plugins", "tools"]).contains("usage"));
    assert!(usage_error(&["pinvoy", "plugins", "tools", "bogus"]).contains("usage"));
    assert!(usage_error(&["pinvoy", "plugins", "skills", "bogus"]).contains("usage"));
    assert!(usage_error(&["pinvoy", "plugins", "recycle", "bogus"]).contains("usage"));
    // missing ids
    assert!(usage_error(&["pinvoy", "plugins", "tools", "install"]).contains("id"));
    assert!(usage_error(&["pinvoy", "plugins", "tools", "uninstall"]).contains("id"));
    assert!(usage_error(&["pinvoy", "plugins", "tools", "auth"]).contains("id"));
    assert!(usage_error(&["pinvoy", "plugins", "tools", "oauth-login"]).contains("id"));
    assert!(usage_error(&["pinvoy", "plugins", "tools", "oauth-cancel"]).contains("id"));
    assert!(usage_error(&["pinvoy", "plugins", "skills", "install"]).contains("id"));
    assert!(usage_error(&["pinvoy", "plugins", "skills", "uninstall"]).contains("id"));
    assert!(usage_error(&["pinvoy", "plugins", "export"]).contains("id"));
    assert!(usage_error(&["pinvoy", "plugins", "meta"]).contains("id"));
    assert!(usage_error(&["pinvoy", "plugins", "recycle", "restore"]).contains("id"));
    assert!(usage_error(&["pinvoy", "plugins", "recycle", "purge"]).contains("id"));
    assert!(usage_error(&["pinvoy", "plugins", "recycle", "export"]).contains("id"));
    assert!(usage_error(&["pinvoy", "plugins", "enable"]).contains("id"));
    assert!(usage_error(&["pinvoy", "plugins", "disable"]).contains("id"));
    // unsupported / malformed options
    assert!(usage_error(&["pinvoy", "plugins", "tools", "list", "--bogus"]).contains("--bogus"));
    assert!(
        usage_error(&[
            "pinvoy",
            "plugins",
            "tools",
            "install",
            "w",
            "--secret",
            "NO_EQUALS"
        ])
        .contains("KEY=ENV_VAR_NAME")
    );
    assert!(
        usage_error(&["pinvoy", "plugins", "tools", "install", "w", "--secret"])
            .contains("KEY=ENV_VAR_NAME")
    );
    assert!(
        usage_error(&[
            "pinvoy", "plugins", "tools", "install", "w", "--secret", "=ENV"
        ])
        .contains("non-empty KEY")
    );
    assert!(
        usage_error(&[
            "pinvoy",
            "plugins",
            "tools",
            "oauth-login",
            "q",
            "--timeout",
            "0"
        ])
        .contains("positive integer")
    );
    assert!(
        usage_error(&[
            "pinvoy",
            "plugins",
            "tools",
            "oauth-login",
            "q",
            "--timeout",
            "abc"
        ])
        .contains("positive integer")
    );
    assert!(
        usage_error(&["pinvoy", "plugins", "tools", "auth", "q", "--full"])
            .contains("accepts no options")
    );
    assert!(
        usage_error(&[
            "pinvoy",
            "plugins",
            "skills",
            "list",
            "--installed-only",
            "--archived"
        ])
        .contains("--archived")
    );
    // meta requires at least one editable field
    assert!(usage_error(&["pinvoy", "plugins", "meta", "p"]).contains("--name"));
    // scope values are validated with the valid values named
    assert!(
        usage_error(&["pinvoy", "plugins", "enable", "w", "--scope", "global"]).contains("plain")
    );
    assert!(
        usage_error(&["pinvoy", "plugins", "disable", "w", "--scope", "CODE"]).contains("plain")
    );
    // project-skills only accepts on|off
    assert!(usage_error(&["pinvoy", "plugins", "project-skills", "true"]).contains("on or off"));
    assert!(usage_error(&["pinvoy", "plugins", "project-skills"]).contains("on or off"));
    // import requires a path; readiness accepts nothing
    assert!(usage_error(&["pinvoy", "plugins", "import"]).contains("PATH"));
    assert!(
        usage_error(&["pinvoy", "plugins", "readiness", "extra"]).contains("accepts no arguments")
    );
}

// ---------------------------------------------------------------------------
// execute level: import / skills
// ---------------------------------------------------------------------------

#[test]
fn import_directory_and_zip_show_up_in_skills_list() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("import-list");
    let dir = fixture_skill_dir(home.path());
    let stdout = run_ok(&["pinvoy", "plugins", "import", dir.to_str().unwrap()]);
    assert!(stdout.contains(FIXTURE_DIR_SKILL), "stdout: {stdout}");
    assert!(stdout.contains("kind=skill"), "stdout: {stdout}");

    let zip = fixture_skill_zip(home.path());
    let stdout = run_ok(&["pinvoy", "plugins", "import", zip.to_str().unwrap()]);
    assert!(stdout.contains(FIXTURE_ZIP_SKILL), "stdout: {stdout}");

    let human = run_ok(&["pinvoy", "plugins", "skills", "list"]);
    assert!(human.contains(FIXTURE_DIR_SKILL));
    assert!(human.contains(FIXTURE_ZIP_SKILL));
    assert!(human.contains("uploaded"), "human: {human}");

    let installed_only = run_ok(&["pinvoy", "plugins", "skills", "list", "--installed-only"]);
    assert!(installed_only.contains(FIXTURE_DIR_SKILL));
    assert!(installed_only.contains(FIXTURE_ZIP_SKILL));

    let value = run_json(&["pinvoy", "plugins", "skills", "list"]);
    let skills = value["skills"].as_array().expect("skills array");
    let fixture = skills
        .iter()
        .find(|skill| skill["id"] == FIXTURE_ZIP_SKILL)
        .expect("zip fixture listed");
    assert_eq!(fixture["installed"], serde_json::json!(true));
    assert_eq!(fixture["user_uploaded"], serde_json::json!(true));
}

#[test]
fn import_md_file_with_and_without_frontmatter() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("import-md");

    let named = home.path().join("contract-md-skill.md");
    std::fs::write(&named, "---\nname: contract-md-skill\n---\nbody").unwrap();
    let stdout = run_ok(&["pinvoy", "plugins", "import", named.to_str().unwrap()]);
    assert!(stdout.contains("contract-md-skill"), "stdout: {stdout}");

    // No frontmatter name: the stem is sanitized into the fallback id
    // ("plain notes" → "plain-notes"); import without an injected name would
    // fail with "SKILL.md lacks name", so success proves the injection ran.
    let plain = home.path().join("plain notes.md");
    std::fs::write(&plain, "just some body text").unwrap();
    let stdout = run_ok(&["pinvoy", "plugins", "import", plain.to_str().unwrap()]);
    assert!(stdout.contains("plain-notes"), "stdout: {stdout}");
}

#[test]
fn import_rejects_missing_path_and_unsupported_extension() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("import-errors");

    let missing = home.path().join("nope.zip");
    let (message, code) = run_err(&["pinvoy", "plugins", "import", missing.to_str().unwrap()]);
    assert_eq!(code, ExitCode::Failed);
    assert!(message.contains("does not exist"), "message: {message}");

    let txt = home.path().join("file.txt");
    std::fs::write(&txt, "x").unwrap();
    let (message, code) = run_err(&["pinvoy", "plugins", "import", txt.to_str().unwrap()]);
    assert_eq!(code, ExitCode::Usage);
    assert!(message.contains(".zip"), "message: {message}");

    // Directory without a root SKILL.md is rejected as a failed import.
    let bare = home.path().join("bare-dir");
    std::fs::create_dir_all(&bare).unwrap();
    let (message, code) = run_err(&["pinvoy", "plugins", "import", bare.to_str().unwrap()]);
    assert_eq!(code, ExitCode::Failed);
    assert!(message.contains("SKILL.md"), "message: {message}");
}

#[test]
fn meta_updates_upload_package_and_rejects_preset() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("meta");
    let dir = fixture_skill_dir(home.path());
    run_ok(&["pinvoy", "plugins", "import", dir.to_str().unwrap()]);

    let stdout = run_ok(&[
        "pinvoy",
        "plugins",
        "meta",
        FIXTURE_DIR_SKILL,
        "--name",
        "Renamed Fixture",
        "--description",
        "Updated description",
    ]);
    assert!(stdout.contains("display meta"), "stdout: {stdout}");

    let bundles: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.path().join("marketplace/bundles.json")).unwrap(),
    )
    .unwrap();
    let record = bundles["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["id"] == FIXTURE_DIR_SKILL)
        .expect("bundle record for imported skill");
    // extra is serde-flattened into the record JSON.
    assert_eq!(record["display_name"], serde_json::json!("Renamed Fixture"));
    assert_eq!(
        record["display_description"],
        serde_json::json!("Updated description")
    );

    // Preset/embedded packages refuse display-meta overrides (GUI parity).
    let (message, code) = run_err(&["pinvoy", "plugins", "meta", "visualizer", "--name", "Nope"]);
    assert_eq!(code, ExitCode::Failed);
    assert!(message.contains("visualizer"), "message: {message}");
}

// ---------------------------------------------------------------------------
// execute level: enable / disable / project-skills
// ---------------------------------------------------------------------------

#[test]
fn disable_enable_scope_round_trip_persists_disabled_bundles_json() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("scope-roundtrip");

    run_ok(&[
        "pinvoy", "plugins", "disable", "weather", "--scope", "plain",
    ]);
    let file = disabled_bundles_json(home.path());
    assert_eq!(
        file["scopes"]["plain"],
        serde_json::json!(["weather"]),
        "file: {file}"
    );

    run_ok(&["pinvoy", "plugins", "disable", "weather", "--scope", "code"]);
    let file = disabled_bundles_json(home.path());
    // Uninitialized code scope defaults to deny-all (builtin CLI connector
    // ids included); disabling weather freezes that list plus weather.
    let code = file["scopes"]["code"].as_array().unwrap();
    assert!(code.contains(&serde_json::json!("weather")), "file: {file}");
    assert!(code.contains(&serde_json::json!("feishu")), "file: {file}");

    // Default scope is `both`.
    run_ok(&["pinvoy", "plugins", "disable", "obsidian"]);
    let file = disabled_bundles_json(home.path());
    assert_eq!(file["scopes"]["plain"].as_array().unwrap().len(), 2);
    let code = file["scopes"]["code"].as_array().unwrap();
    assert!(code.contains(&serde_json::json!("obsidian")));

    run_ok(&["pinvoy", "plugins", "enable", "weather", "--scope", "both"]);
    let file = disabled_bundles_json(home.path());
    let plain = file["scopes"]["plain"].as_array().unwrap();
    let code = file["scopes"]["code"].as_array().unwrap();
    assert!(!plain.contains(&serde_json::json!("weather")));
    assert!(!code.contains(&serde_json::json!("weather")));
    assert!(plain.contains(&serde_json::json!("obsidian")));

    let stdout = run_ok(&["pinvoy", "plugins", "enable", "obsidian", "--scope", "code"]);
    assert!(stdout.contains("scope=code"), "stdout: {stdout}");
}

#[test]
fn project_skills_round_trip() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("project-skills");

    assert!(!disabled_bundles_json_exists(home.path()));
    let stdout = run_ok(&["pinvoy", "plugins", "project-skills", "on"]);
    assert!(stdout.contains("enabled"), "stdout: {stdout}");
    assert_eq!(
        disabled_bundles_json(home.path())["project_skills_enabled"],
        serde_json::json!(true)
    );

    run_ok(&["pinvoy", "plugins", "project-skills", "off"]);
    assert_eq!(
        disabled_bundles_json(home.path())["project_skills_enabled"],
        serde_json::json!(false)
    );
}

fn disabled_bundles_json_exists(home: &Path) -> bool {
    home.join("disabled_bundles.json").exists()
}

// ---------------------------------------------------------------------------
// execute level: tools
// ---------------------------------------------------------------------------

#[test]
fn tools_list_shows_embedded_catalog_with_installed_state() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("tools-list");

    let human = run_ok(&["pinvoy", "plugins", "tools", "list"]);
    for known in ["weather", "qcc", "obsidian", "pptx"] {
        assert!(
            human.contains(known),
            "catalog should list {known}: {human}"
        );
    }
    assert!(!human.contains("installed\t"), "nothing installed yet");

    let value = run_json(&["pinvoy", "plugins", "tools", "list"]);
    let tools = value["tools"].as_array().expect("tools array");
    assert!(tools.len() >= 11, "embedded catalog has 11 packages");
    let weather = tools.iter().find(|tool| tool["id"] == "weather").unwrap();
    // JSON mirrors the GUI MarketplaceToolInfo DTO.
    for field in [
        "id",
        "name",
        "description",
        "version",
        "icon",
        "category",
        "installed",
        "companion_skills",
        "source",
        "exportable",
    ] {
        assert!(weather.get(field).is_some(), "missing field {field}");
    }
    assert_eq!(weather["installed"], serde_json::json!(false));

    let installed_only = run_ok(&["pinvoy", "plugins", "tools", "list", "--installed-only"]);
    assert!(
        installed_only.is_empty(),
        "nothing installed: {installed_only}"
    );
}

#[test]
fn tools_install_uninstall_round_trip_is_hermetic_for_manifest_only_packages() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("tools-roundtrip");

    let stdout = run_ok(&["pinvoy", "plugins", "tools", "install", "qcc"]);
    assert!(stdout.contains("installed qcc"), "stdout: {stdout}");

    let installed_only = run_ok(&["pinvoy", "plugins", "tools", "list", "--installed-only"]);
    assert!(installed_only.contains("qcc"), "{installed_only}");

    // Destructive uninstall requires --yes (exit 2 before touching state).
    let (message, code) = run_err(&["pinvoy", "plugins", "tools", "uninstall", "qcc"]);
    assert_eq!(code, ExitCode::Usage);
    assert!(message.contains("--yes"), "message: {message}");

    run_ok(&["pinvoy", "plugins", "tools", "uninstall", "qcc", "--yes"]);
    let installed_only = run_ok(&["pinvoy", "plugins", "tools", "list", "--installed-only"]);
    assert!(!installed_only.contains("qcc"), "{installed_only}");
    assert!(!home.path().join("bundles").join("qcc").exists());
}

#[test]
fn tools_auth_reports_installed_and_oauth_states() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("tools-auth");

    // Non-OAuth tool, not installed.
    let value = run_json(&["pinvoy", "plugins", "tools", "auth", "obsidian"]);
    assert_eq!(value["installed"], serde_json::json!(false));
    assert_eq!(value["oauth_required"], serde_json::json!(false));
    assert_eq!(value["status"], serde_json::json!("not_installed"));

    // Remote OAuth tool, not installed: server declared, no mcp.json yet.
    let value = run_json(&["pinvoy", "plugins", "tools", "auth", "qcc"]);
    assert_eq!(value["oauth_required"], serde_json::json!(true));
    assert_eq!(value["server_name"], serde_json::json!("qcc-company"));
    assert_eq!(value["mcp_configured"], serde_json::json!(false));
    assert_eq!(value["status"], serde_json::json!("not_installed"));

    // Installed OAuth tool: MCP config present, token presence not verifiable
    // headless (foundation token store) — conservative pending status.
    run_ok(&["pinvoy", "plugins", "tools", "install", "qcc"]);
    let value = run_json(&["pinvoy", "plugins", "tools", "auth", "qcc"]);
    assert_eq!(value["installed"], serde_json::json!(true));
    assert_eq!(value["mcp_configured"], serde_json::json!(true));
    assert_eq!(
        value["status"],
        serde_json::json!("config_installed_auth_pending")
    );
    assert_eq!(
        value["token_check"],
        serde_json::json!("unavailable_in_cli")
    );

    // Installed non-OAuth tool reports connected.
    run_ok(&["pinvoy", "plugins", "tools", "install", "obsidian"]);
    let value = run_json(&["pinvoy", "plugins", "tools", "auth", "obsidian"]);
    assert_eq!(value["status"], serde_json::json!("connected"));
}

#[test]
fn oauth_login_guards_and_cancel_behaviour() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("oauth-login");

    // Tool without a remote OAuth declaration → the feature's own error.
    let (message, code) = run_err(&["pinvoy", "plugins", "tools", "oauth-login", "obsidian"]);
    assert_eq!(code, ExitCode::Failed);
    assert!(
        message.contains("does not declare a remote MCP OAuth login"),
        "message: {message}"
    );

    // OAuth tool whose server is not in mcp.json yet (not installed).
    let (message, code) = run_err(&["pinvoy", "plugins", "tools", "oauth-login", "qcc"]);
    assert_eq!(code, ExitCode::Failed);
    assert!(message.contains("mcp.json"), "message: {message}");

    // Installed OAuth tool: the interactive OAuth flow lives in the
    // foundation crate the CLI does not link — deterministic, documented
    // exit-1 instead of a half-working reimplementation.
    run_ok(&["pinvoy", "plugins", "tools", "install", "qcc"]);
    let (message, code) = run_err(&[
        "pinvoy",
        "plugins",
        "tools",
        "oauth-login",
        "qcc",
        "--timeout",
        "5",
    ]);
    assert_eq!(code, ExitCode::Failed);
    assert!(
        message.contains("oauth_login_unavailable_in_cli"),
        "message: {message}"
    );

    // Cancel: a CLI process never owns an in-flight login.
    let stdout = run_ok(&["pinvoy", "plugins", "tools", "oauth-cancel", "qcc"]);
    assert!(stdout.contains("no active oauth login"), "stdout: {stdout}");
}

/// OPT-IN (network + system credential store): `tools install weather` reads
/// the secret from the environment and persists it via the OS keyring.
/// Run with: cargo test -p pinvou-cli --test plugins_contract -- --ignored
#[test]
#[ignore]
fn tools_install_with_secret_persists_credential() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("tools-secret");
    unsafe { std::env::set_var("PINVOU_CLI_TEST_AMAP_KEY", "test-secret-value") };

    let stdout = run_ok(&[
        "pinvoy",
        "plugins",
        "tools",
        "install",
        "weather",
        "--secret",
        "AMAP_KEY=PINVOU_CLI_TEST_AMAP_KEY",
    ]);
    assert!(stdout.contains("installed weather"), "stdout: {stdout}");

    let (message, code) = run_err(&["pinvoy", "plugins", "tools", "install", "iwencai"]);
    assert_eq!(code, ExitCode::Failed);
    assert!(
        message.contains("secret environment variable"),
        "message: {message}"
    );
}

/// OPT-IN (network): `tools install patsnap-search` declares
/// `validate_on_install`; the CLI installs locally and warns that the remote
/// MCP handshake was skipped instead of performing it.
/// Run with: cargo test -p pinvou-cli --test plugins_contract -- --ignored
#[test]
#[ignore]
fn tools_install_warns_when_remote_validation_is_skipped() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("tools-validate");
    unsafe { std::env::set_var("PINVOU_CLI_TEST_PATSNAP_KEY", "test-secret-value") };

    let stdout = run_ok(&[
        "pinvoy",
        "plugins",
        "tools",
        "install",
        "patsnap-search",
        "--secret",
        "PATSNAP_API_KEY=PINVOU_CLI_TEST_PATSNAP_KEY",
    ]);
    assert!(
        stdout.contains("installed patsnap-search"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("validation skipped"), "stdout: {stdout}");
}

// ---------------------------------------------------------------------------
// execute level: skills (preset) + export / recycle
// ---------------------------------------------------------------------------

#[test]
fn skills_preset_install_update_uninstall_round_trip() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("skills-preset");

    // Update guard: only installed preset skills can be updated.
    let (message, code) = run_err(&["pinvoy", "plugins", "skills", "update", "visualizer"]);
    assert_eq!(code, ExitCode::Failed);
    assert!(
        message.contains("not an installed preset skill"),
        "message: {message}"
    );

    run_ok(&["pinvoy", "plugins", "skills", "install", "visualizer"]);
    let value = run_json(&["pinvoy", "plugins", "skills", "list"]);
    let visualizer = value["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["id"] == "visualizer")
        .expect("preset listed")
        .clone();
    assert_eq!(visualizer["installed"], serde_json::json!(true));
    assert_eq!(visualizer["user_uploaded"], serde_json::json!(false));

    // Unknown skill id → feature's own error (exit 1).
    let (message, code) = run_err(&["pinvoy", "plugins", "skills", "install", "no-such-skill"]);
    assert_eq!(code, ExitCode::Failed);
    assert!(message.contains("no-such-skill"), "message: {message}");

    run_ok(&["pinvoy", "plugins", "skills", "update", "visualizer"]);

    let (message, code) = run_err(&["pinvoy", "plugins", "skills", "uninstall", "visualizer"]);
    assert_eq!(code, ExitCode::Usage);
    assert!(message.contains("--yes"), "message: {message}");

    run_ok(&[
        "pinvoy",
        "plugins",
        "skills",
        "uninstall",
        "visualizer",
        "--yes",
    ]);
    let installed_only = run_ok(&["pinvoy", "plugins", "skills", "list", "--installed-only"]);
    assert!(!installed_only.contains("visualizer"), "{installed_only}");
}

#[test]
fn export_installed_package_writes_zip_and_preset_is_rejected() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("export");

    let dir = fixture_skill_dir(home.path());
    run_ok(&["pinvoy", "plugins", "import", dir.to_str().unwrap()]);

    let dest = home.path().join("exports").join("fixture.zip");
    let stdout = run_ok(&[
        "pinvoy",
        "plugins",
        "export",
        FIXTURE_DIR_SKILL,
        "--output",
        dest.to_str().unwrap(),
    ]);
    assert!(stdout.contains("exported"), "stdout: {stdout}");
    assert!(dest.is_file());
    assert!(std::fs::metadata(&dest).unwrap().len() > 0);

    // Embedded preset packages refuse export (feature's own error, exit 1).
    let (message, code) = run_err(&["pinvoy", "plugins", "export", "pptx"]);
    assert_eq!(code, ExitCode::Failed);
    assert!(message.contains("pptx"), "message: {message}");

    // Unknown id → not installed.
    let (message, code) = run_err(&["pinvoy", "plugins", "export", "never-installed"]);
    assert_eq!(code, ExitCode::Failed);
    assert!(message.contains("never-installed"), "message: {message}");
}

#[test]
fn recycle_round_trip_via_fixture() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("recycle");

    let zip = fixture_skill_zip(home.path());
    run_ok(&["pinvoy", "plugins", "import", zip.to_str().unwrap()]);

    // Uploaded package uninstall = soft delete into the recycle bin.
    run_ok(&[
        "pinvoy",
        "plugins",
        "skills",
        "uninstall",
        FIXTURE_ZIP_SKILL,
        "--yes",
    ]);
    let value = run_json(&["pinvoy", "plugins", "recycle", "list"]);
    let entries = value["recycled"].as_array().expect("recycled array");
    assert_eq!(entries.len(), 1, "value: {value}");
    assert_eq!(entries[0]["id"], FIXTURE_ZIP_SKILL);
    assert_eq!(entries[0]["kind"], "skill");
    assert_eq!(entries[0]["package_missing"], serde_json::json!(false));

    // Export while recycled: writes a re-importable zip.
    let dest = home.path().join("recycled-export.zip");
    run_ok(&[
        "pinvoy",
        "plugins",
        "recycle",
        "export",
        FIXTURE_ZIP_SKILL,
        "--output",
        dest.to_str().unwrap(),
    ]);
    assert!(dest.is_file());
    assert!(std::fs::metadata(&dest).unwrap().len() > 0);

    // Restore puts the skill back on the market list.
    run_ok(&["pinvoy", "plugins", "recycle", "restore", FIXTURE_ZIP_SKILL]);
    let value = run_json(&["pinvoy", "plugins", "skills", "list"]);
    let restored = value["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["id"] == FIXTURE_ZIP_SKILL)
        .expect("restored skill listed");
    assert_eq!(restored["installed"], serde_json::json!(true));
    let value = run_json(&["pinvoy", "plugins", "recycle", "list"]);
    assert!(value["recycled"].as_array().unwrap().is_empty());

    // Uninstall again, then purge destroys the entry permanently.
    run_ok(&[
        "pinvoy",
        "plugins",
        "skills",
        "uninstall",
        FIXTURE_ZIP_SKILL,
        "--yes",
    ]);
    let (message, code) = run_err(&["pinvoy", "plugins", "recycle", "purge", FIXTURE_ZIP_SKILL]);
    assert_eq!(code, ExitCode::Usage);
    assert!(message.contains("--yes"), "message: {message}");

    run_ok(&[
        "pinvoy",
        "plugins",
        "recycle",
        "purge",
        FIXTURE_ZIP_SKILL,
        "--yes",
    ]);
    let value = run_json(&["pinvoy", "plugins", "recycle", "list"]);
    assert!(value["recycled"].as_array().unwrap().is_empty());
    assert!(
        !home.path().join("bundles").join(FIXTURE_ZIP_SKILL).exists(),
        "purged package must be gone"
    );

    // Unknown ids surface the feature's own errors (exit 1).
    let (_, code) = run_err(&["pinvoy", "plugins", "recycle", "restore", "no-such-id"]);
    assert_eq!(code, ExitCode::Failed);
    let (_, code) = run_err(&[
        "pinvoy",
        "plugins",
        "recycle",
        "purge",
        "no-such-id",
        "--yes",
    ]);
    assert_eq!(code, ExitCode::Failed);
    let (_, code) = run_err(&["pinvoy", "plugins", "recycle", "export", "no-such-id"]);
    assert_eq!(code, ExitCode::Failed);
}

// ---------------------------------------------------------------------------
// execute level: readiness
// ---------------------------------------------------------------------------

#[test]
fn readiness_zero_state_reports_uninstalled_catalog() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("readiness");

    let human = run_ok(&["pinvoy", "plugins", "readiness"]);
    assert!(human.contains("weather"), "human: {human}");

    let value = run_json(&["pinvoy", "plugins", "readiness"]);
    let bundles = value["bundles"].as_array().expect("bundles array");
    assert!(!bundles.is_empty(), "registry lists the embedded catalog");
    for bundle in bundles {
        assert_eq!(
            bundle["installed"],
            serde_json::json!(false),
            "zero state: {bundle}"
        );
        // Registry readiness for credential-free packages is `ready` even
        // when uninstalled (GUI bundle_readiness derives the same value);
        // packages with required credentials report missing_credentials.
        if bundle["ready"] == serde_json::json!(true) {
            assert_eq!(bundle["reason"], serde_json::Value::Null, "{bundle}");
        } else {
            assert!(
                bundle["reason"].is_string(),
                "not-ready bundles carry a reason: {bundle}"
            );
        }
        for field in ["bundle_id", "kind", "installed", "ready", "reason"] {
            assert!(bundle.get(field).is_some(), "missing {field}: {bundle}");
        }
    }
    let weather = bundles
        .iter()
        .find(|bundle| bundle["bundle_id"] == "weather")
        .expect("weather listed");
    assert_eq!(weather["kind"], serde_json::json!("mcp"));
    assert_eq!(
        weather["reason"],
        serde_json::json!("missing_credentials"),
        "weather requires AMAP_KEY: {weather}"
    );
    // Credential-free remote MCP is registry-ready though uninstalled.
    let canva = bundles
        .iter()
        .find(|bundle| bundle["bundle_id"] == "canva-mcp")
        .expect("canva-mcp listed");
    assert_eq!(canva["ready"], serde_json::json!(true));
}
