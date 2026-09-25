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

/// Panic-safe restore for an env var an opt-in test sets (same pattern as
/// models_contract.rs): without the Drop guard a failing assert would leak
/// the variable into every later test in the binary.
struct RestoreEnvVar(&'static str, Option<std::ffi::OsString>);

impl Drop for RestoreEnvVar {
    fn drop(&mut self) {
        // SAFETY: the file-scoped ENV_LOCK is held for the whole test.
        match self.1.take() {
            Some(value) => unsafe { std::env::set_var(self.0, value) },
            None => unsafe { std::env::remove_var(self.0) },
        }
    }
}

fn run_ok(args: &[&str]) -> String {
    let parsed = parse_args(args.to_vec()).expect("valid command");
    let outcome = execute(parsed).expect("execute succeeds");
    // Diagnostic messages deliberately stay free of command output: the
    // shared CliOutcome can carry command-shaped strings (session ids,
    // credential echoes), so tests report only the exit codes — assert_eq!
    // already prints both sides on failure.
    assert_eq!(outcome.exit_code, ExitCode::Success);
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
    serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("json output did not parse: {error}"))
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
    assert!(
        stdout.contains(FIXTURE_DIR_SKILL),
        "skills list should mention the imported directory skill"
    );
    assert!(
        stdout.contains("kind=skill"),
        "skills list should mark the entry kind=skill"
    );

    let zip = fixture_skill_zip(home.path());
    let stdout = run_ok(&["pinvoy", "plugins", "import", zip.to_str().unwrap()]);
    assert!(
        stdout.contains(FIXTURE_ZIP_SKILL),
        "skills list should mention the imported zip skill"
    );

    let human = run_ok(&["pinvoy", "plugins", "skills", "list"]);
    assert!(human.contains(FIXTURE_DIR_SKILL));
    assert!(human.contains(FIXTURE_ZIP_SKILL));
    assert!(
        human.contains("uploaded"),
        "skills list should mark imported skills uploaded"
    );

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
    assert!(
        stdout.contains("contract-md-skill"),
        "import output should mention the skill id from the file name"
    );

    // No frontmatter name: the stem is sanitized into the fallback id
    // ("plain notes" → "plain-notes"); import without an injected name would
    // fail with "SKILL.md lacks name", so success proves the injection ran.
    let plain = home.path().join("plain notes.md");
    std::fs::write(&plain, "just some body text").unwrap();
    let stdout = run_ok(&["pinvoy", "plugins", "import", plain.to_str().unwrap()]);
    assert!(
        stdout.contains("plain-notes"),
        "import output should mention the sanitized fallback id"
    );
}

/// Regression: two directories whose names sanitize to the generic "skill"
/// fallback (pure non-ASCII names) must not collapse onto one constant id.
/// The wrapper's anti-collision branch hashes the RAW name (GUI FNV collision
/// defense); feeding it the pre-sanitized name hashed the constant "skill"
/// instead, so the second import failed with an id-exists error.
#[test]
fn import_directories_with_non_ascii_names_get_distinct_fallback_ids() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("import-nonascii-dirs");

    let mut ids = Vec::new();
    for name in ["技能一", "技能二"] {
        let dir = home.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        // No frontmatter name: the fallback id must derive from the raw
        // directory name, so the bodies stay distinct too.
        std::fs::write(
            dir.join("SKILL.md"),
            format!("body for {name} without a frontmatter name"),
        )
        .unwrap();
        let value = run_json(&["pinvoy", "plugins", "import", dir.to_str().unwrap()]);
        ids.push(value["id"].as_str().expect("string id").to_owned());
    }
    assert_ne!(ids[0], ids[1], "distinct names must yield distinct ids");
    assert!(
        ids[0].starts_with("skill-") && ids[1].starts_with("skill-"),
        "pure non-ASCII names must land on the skill-<hash> fallback form"
    );
    let _ = home;
}

/// Regression for the sanitizer × hash interaction: a name carrying
/// invisible characters sanitizes to the same label as its clean spelling
/// ("技能\u{200B}" cleans to "技能"), so hashing the CLEANED form would
/// collapse the two onto one id (and diverge from the GUI's raw-stem hash).
/// The raw name must feed the hash; the cleaned form is only the label.
#[test]
fn import_names_sanitizing_to_the_same_label_get_distinct_fallback_ids() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("import-invisible-dirs");

    let mut ids = Vec::new();
    for name in ["技能", "技能\u{200B}"] {
        let dir = home.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), format!("body for {name}")).unwrap();
        let value = run_json(&["pinvoy", "plugins", "import", dir.to_str().unwrap()]);
        ids.push(value["id"].as_str().expect("string id").to_owned());
    }
    assert_ne!(
        ids[0], ids[1],
        "names sanitizing to the same label must keep distinct ids"
    );
    let _ = home;
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

/// Creates a sparse file of exactly `len` bytes (no data is written, so the
/// 200 MiB-scale fixtures below cost no disk space and no read time unless
/// the import actually reads them).
fn write_sparse(path: &Path, len: u64) {
    let file = std::fs::File::create(path).expect("create sparse fixture");
    file.set_len(len).expect("set sparse fixture length");
}

fn import_package_limit() -> u64 {
    pinvou3_lib::features::marketplace::plugin_import::MAX_PLUGIN_SIZE_BYTES
}

/// A single `.md` skill file over the product import limit is rejected by the
/// CLI's pre-wrap read (the unified pipeline would enforce the same limit
/// later, but only after the whole file had been read into memory and
/// re-copied into the wrapper zip). One byte over the limit is rejected; the
/// boundary itself (`== limit`) is the pipeline's `>` comparison and would
/// import a 200 MiB package, which is too heavy to execute in a contract
/// test.
#[test]
fn import_rejects_oversize_markdown_file() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("import-oversize-md");

    let oversized = home.path().join("oversize.md");
    write_sparse(&oversized, import_package_limit() + 1);
    let (message, code) = run_err(&["pinvoy", "plugins", "import", oversized.to_str().unwrap()]);
    assert_eq!(code, ExitCode::Failed);
    assert!(
        message.contains("exceeds the 200 MiB import limit"),
        "message: {message}"
    );
}

/// The cumulative cap cannot be dodged by splitting the payload across files
/// that are each under the per-file limit: a directory whose files sum over
/// the product import limit is rejected during the walk.
#[test]
fn import_rejects_directory_over_cumulative_limit() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("import-oversize-dir");
    let dir = home.path().join("oversize-dir");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "---\nname: oversize-dir\n---\nbody").unwrap();
    // The first file reads fully (sparse, so it is fast) and stays under the
    // per-file limit; the second trips the cumulative check from metadata
    // alone.
    write_sparse(&dir.join("a.bin"), import_package_limit() - 1024);
    write_sparse(&dir.join("b.bin"), 2048);
    let (message, code) = run_err(&["pinvoy", "plugins", "import", dir.to_str().unwrap()]);
    assert_eq!(code, ExitCode::Failed);
    assert!(
        message.contains("exceeds the 200 MiB import limit"),
        "message: {message}"
    );
}

/// The root SKILL.md is walked and charged once. A regression re-read it
/// against the same cumulative budget, so a directory whose true content
/// was within the limit was rejected by the pre-wrap read itself (content +
/// SKILL.md > limit). The fixture sits in exactly that window (assets +
/// 2×SKILL.md over the limit while assets + SKILL.md stays under it) at the
/// smallest asymmetric sizes: the pre-wrap stage must hand the package to
/// the unified pipeline — which then applies its own budget — instead of
/// rejecting it with the CLI's own limit error.
#[test]
fn import_directory_counts_root_skill_md_once() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("import-skill-md-once");
    let dir = home.path().join("boundary-dir");
    std::fs::create_dir_all(&dir).unwrap();
    let skill_md = dir.join("SKILL.md");
    std::fs::write(&skill_md, "---\nname: boundary-skill\n---\n").unwrap();
    // Extend SKILL.md with zero-filled sparse bytes (valid UTF-8, no real
    // disk cost) so a second charge would tip the budget while the true
    // total stays under the limit.
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&skill_md)
        .unwrap();
    file.set_len(8192).unwrap();
    drop(file);
    write_sparse(&dir.join("a.bin"), import_package_limit() - 12288);

    let parsed = parse_args(vec!["pinvoy", "plugins", "import", dir.to_str().unwrap()])
        .expect("valid command");
    match execute(parsed) {
        Ok(outcome) => {
            assert_eq!(outcome.exit_code, ExitCode::Success);
        }
        Err(error) => assert!(
            !error
                .to_string()
                .contains("exceeds the 200 MiB import limit"),
            "pre-wrap read charged the root SKILL.md twice: {error}"
        ),
    }
}

/// A FIFO shaped like a skill file is rejected from its metadata, before any
/// read: the old `read_to_string` path would block on `open` until an
/// unrelated writer appeared (a hang, not an error). Unix-only because FIFOs
/// are a unix special file type.
#[cfg(unix)]
#[test]
fn import_rejects_fifo_skill_file_without_reading() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("import-fifo");

    let fifo = home.path().join("pipe.md");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo failed");
    let (message, code) = run_err(&["pinvoy", "plugins", "import", fifo.to_str().unwrap()]);
    assert_eq!(code, ExitCode::Failed);
    assert!(message.contains("not a regular file"), "message: {message}");
}

/// The `.zip` channel gets the same pre-flight: the shared importer's
/// `File::open` would block on a FIFO exactly like the wrap reads, and its
/// budget bounds decompressed content only, so an oversized package file is
/// rejected from metadata (matching the GUI upload path's cap on the
/// package file itself) instead of after a full open.
#[test]
fn import_rejects_oversize_zip_package() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("import-oversize-zip");

    let oversized = home.path().join("oversize.zip");
    write_sparse(&oversized, import_package_limit() + 1);
    let (message, code) = run_err(&["pinvoy", "plugins", "import", oversized.to_str().unwrap()]);
    assert_eq!(code, ExitCode::Failed);
    assert!(
        message.contains("exceeds the 200 MiB import limit"),
        "message: {message}"
    );
}

#[cfg(unix)]
#[test]
fn import_rejects_fifo_zip_package_without_opening() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("import-fifo-zip");

    let fifo = home.path().join("pipe.zip");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo failed");
    let (message, code) = run_err(&["pinvoy", "plugins", "import", fifo.to_str().unwrap()]);
    assert_eq!(code, ExitCode::Failed);
    assert!(message.contains("not a regular file"), "message: {message}");
}

/// Non-regular entries inside a directory tree stay skipped (only regular,
/// non-hidden files feed the wrapper zip), so a stray socket or FIFO in the
/// skill directory neither hangs the import nor fails it.
#[cfg(unix)]
#[test]
fn import_directory_skips_fifo_entries() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("import-fifo-dir");
    let dir = home.path().join("fifo-dir");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "---\nname: fifo-dir-skill\n---\nbody").unwrap();
    let status = std::process::Command::new("mkfifo")
        .arg(dir.join("pipe"))
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo failed");

    let stdout = run_ok(&["pinvoy", "plugins", "import", dir.to_str().unwrap()]);
    assert!(
        stdout.contains("fifo-dir-skill"),
        "import output should mention the skill from the fifo dir"
    );
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
    assert!(
        stdout.contains("display meta"),
        "meta output should confirm the display meta update"
    );

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
    assert_eq!(file["scopes"]["plain"], serde_json::json!(["weather"]));

    run_ok(&["pinvoy", "plugins", "disable", "weather", "--scope", "code"]);
    let file = disabled_bundles_json(home.path());
    // Uninitialized code scope defaults to deny-all (builtin CLI connector
    // ids included); disabling weather freezes that list plus weather.
    let code = file["scopes"]["code"].as_array().unwrap();
    assert!(
        code.contains(&serde_json::json!("weather")),
        "disabled code scope should record weather"
    );
    assert!(
        code.contains(&serde_json::json!("feishu")),
        "disabled code scope should keep the builtin feishu connector"
    );

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
    assert!(
        stdout.contains("scope=code"),
        "enable output should confirm scope=code"
    );
}

// `ima-skills` is claimed by the `ima` package inside the storage layer, so
// toggling the raw skill id must persist (and verify against) `ima`, not the
// raw id. This is the companion-id remap shape: a raw-id comparison reported
// a false `persistence_verified: true` on enable while the owner package
// stayed disabled.
#[test]
fn toggles_verify_against_the_owner_package_for_remapped_ids() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("scope-companion-remap");

    let disabled = run_json(&[
        "pinvoy",
        "plugins",
        "disable",
        "ima-skills",
        "--scope",
        "plain",
    ]);
    assert_eq!(
        disabled["persistence_verified"],
        serde_json::json!(true),
        "disable of a remapped id is exactly verifiable"
    );
    assert_eq!(
        disabled_bundles_json(home.path())["scopes"]["plain"],
        serde_json::json!(["ima"]),
        "the owner package id is what lands in the store"
    );

    let enabled = run_json(&[
        "pinvoy",
        "plugins",
        "enable",
        "ima-skills",
        "--scope",
        "plain",
    ]);
    assert_eq!(
        enabled["persistence_verified"],
        serde_json::json!(true),
        "enable of a remapped id is exactly verifiable"
    );
    assert_eq!(
        disabled_bundles_json(home.path())["scopes"]["plain"],
        serde_json::json!([]),
        "the owner package entry must actually be removed"
    );
}

#[test]
fn project_skills_round_trip() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("project-skills");

    assert!(!disabled_bundles_json_exists(home.path()));
    let stdout = run_ok(&["pinvoy", "plugins", "project-skills", "on"]);
    assert!(
        stdout.contains("enabled"),
        "project-skills on should confirm enabled"
    );
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
        assert!(human.contains(known), "catalog should list {known}");
    }
    assert!(!human.contains("installed\t"), "nothing installed yet");

    let value = run_json(&["pinvoy", "plugins", "tools", "list"]);
    let tools = value["tools"].as_array().expect("tools array");
    assert!(tools.len() >= 11, "embedded catalog has 11 packages");
    let weather = tools.iter().find(|tool| tool["id"] == "weather").unwrap();
    // JSON mirrors the GUI MarketplaceToolInfo DTO. Presentation-only
    // fields (icon/color/category) stay in the frontend overlay on main,
    // so the CLI's catalog view does not carry them.
    for field in [
        "id",
        "name",
        "description",
        "version",
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
        "installed-only list must be empty before any install"
    );
}

#[test]
fn tools_install_uninstall_round_trip_is_hermetic_for_manifest_only_packages() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("tools-roundtrip");

    let stdout = run_ok(&["pinvoy", "plugins", "tools", "install", "qcc"]);
    assert!(
        stdout.contains("installed qcc"),
        "tools install output should confirm installed qcc"
    );

    let installed_only = run_ok(&["pinvoy", "plugins", "tools", "list", "--installed-only"]);
    assert!(
        installed_only.contains("qcc"),
        "tools list should include installed qcc"
    );

    // Destructive uninstall requires --yes (exit 2 before touching state).
    let (message, code) = run_err(&["pinvoy", "plugins", "tools", "uninstall", "qcc"]);
    assert_eq!(code, ExitCode::Usage);
    assert!(message.contains("--yes"), "message: {message}");

    run_ok(&["pinvoy", "plugins", "tools", "uninstall", "qcc", "--yes"]);
    let installed_only = run_ok(&["pinvoy", "plugins", "tools", "list", "--installed-only"]);
    assert!(
        !installed_only.contains("qcc"),
        "tools list --installed-only should drop uninstalled qcc"
    );
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
    assert!(
        stdout.contains("no active oauth login"),
        "oauth-cancel output should report no active login"
    );
}

/// OPT-IN (network + system credential store): `tools install weather` reads
/// the secret from the environment and persists it via the OS keyring.
/// Run with: cargo test -p pinvou-cli --test plugins_contract -- --ignored
#[test]
#[ignore]
fn tools_install_with_secret_persists_credential() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("tools-secret");
    let _amap = RestoreEnvVar(
        "PINVOU_CLI_TEST_AMAP_KEY",
        std::env::var_os("PINVOU_CLI_TEST_AMAP_KEY"),
    );
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
    assert!(
        stdout.contains("installed weather"),
        "tools install output should confirm installed weather"
    );

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
    let _patsnap = RestoreEnvVar(
        "PINVOU_CLI_TEST_PATSNAP_KEY",
        std::env::var_os("PINVOU_CLI_TEST_PATSNAP_KEY"),
    );
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
        "tools install output should confirm installed patsnap-search"
    );
    assert!(
        stdout.contains("validation skipped"),
        "tools install output should report validation skipped"
    );
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
    assert!(
        !installed_only.contains("visualizer"),
        "tools list --installed-only should drop uninstalled visualizer"
    );
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
    assert!(
        stdout.contains("exported"),
        "export output should confirm the export"
    );
    assert!(dest.is_file());
    assert!(std::fs::metadata(&dest).unwrap().len() > 0);

    // An existing destination is refused, not overwritten (exit 1).
    let (message, code) = run_err(&[
        "pinvoy",
        "plugins",
        "export",
        FIXTURE_DIR_SKILL,
        "--output",
        dest.to_str().unwrap(),
    ]);
    assert_eq!(code, ExitCode::Failed);
    assert!(
        message.contains("refusing to overwrite"),
        "message: {message}"
    );
    assert!(
        std::fs::metadata(&dest).unwrap().len() > 0,
        "the existing destination must be untouched"
    );

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
    assert_eq!(entries.len(), 1, "exactly one recycled entry");
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
    assert!(
        human.contains("weather"),
        "readiness output should mention the weather tool"
    );

    let value = run_json(&["pinvoy", "plugins", "readiness"]);
    let bundles = value["bundles"].as_array().expect("bundles array");
    assert!(!bundles.is_empty(), "registry lists the embedded catalog");
    for bundle in bundles {
        assert_eq!(
            bundle["installed"],
            serde_json::json!(false),
            "zero state: catalog bundles are uninstalled"
        );
        // Registry readiness for credential-free packages is `ready` even
        // when uninstalled (GUI bundle_readiness derives the same value);
        // packages with required credentials report missing_credentials.
        if bundle["ready"] == serde_json::json!(true) {
            assert_eq!(bundle["reason"], serde_json::Value::Null);
        } else {
            assert!(
                bundle["reason"].is_string(),
                "not-ready bundles carry a reason"
            );
        }
        for field in ["bundle_id", "kind", "installed", "ready", "reason", "probe"] {
            assert!(bundle.get(field).is_some(), "missing {field}");
        }
        // The probe marker is what makes a row self-describing: `registry`
        // rows are the same verdict the GUI computes, `unavailable_in_cli`
        // rows are the ones the CLI could not settle. A `cli`-kind row is
        // always the latter — the GUI decides it from a live `*_status`
        // probe, so the headless surface must never claim `ready: true` for
        // one (a logged-out connector would look ready).
        if bundle["kind"] == serde_json::json!("cli") {
            assert_eq!(
                bundle["probe"],
                serde_json::json!("unavailable_in_cli"),
                "cli readiness is not decidable headlessly"
            );
            assert_eq!(
                bundle["ready"],
                serde_json::json!(false),
                "a cli bundle is never reported ready without a live probe"
            );
        } else {
            assert_eq!(
                bundle["probe"],
                serde_json::json!("registry"),
                "non-cli readiness is fully computed from the registry"
            );
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
        "weather requires AMAP_KEY"
    );
    // Credential-free remote MCP is registry-ready though uninstalled.
    let canva = bundles
        .iter()
        .find(|bundle| bundle["bundle_id"] == "canva-mcp")
        .expect("canva-mcp listed");
    assert_eq!(canva["ready"], serde_json::json!(true));
    assert_eq!(canva["probe"], serde_json::json!("registry"));
    // A cli-kind row in the zero state: "not installed" IS a registry-visible
    // fact, so the reason names it rather than the headless limitation — the
    // probe marker still records that no connection check was performed.
    let feishu = bundles
        .iter()
        .find(|bundle| bundle["bundle_id"] == "feishu")
        .expect("feishu listed");
    assert_eq!(feishu["kind"], serde_json::json!("cli"));
    assert_eq!(feishu["reason"], serde_json::json!("cli_not_installed"));
    assert_eq!(feishu["probe"], serde_json::json!("unavailable_in_cli"));
    // The human table carries the probe column too (6 tab-separated fields).
    let feishu_line = human
        .lines()
        .find(|line| line.starts_with("feishu\t"))
        .expect("feishu human row");
    assert_eq!(
        feishu_line.split('\t').count(),
        6,
        "human row: id, kind, installed, ready, reason, probe"
    );
    assert!(
        feishu_line.ends_with("\tunavailable_in_cli"),
        "human row discloses the probe: {feishu_line}"
    );
}

#[test]
fn plugins_reject_a_flag_looking_id() {
    // A `--`-prefixed "id" is a mistyped flag (e.g. `plugins disable --scope
    // <scope>` with the id forgotten); recording it into
    // disabled_bundles.json would disable nothing and confuse later reads.
    // Same guard as the connectors family's require_connector.
    let error = parse_args(["pinvou", "plugins", "disable", "--scope", "all"].to_vec())
        .expect_err("a flag-looking id must be a usage error");
    assert_eq!(error.exit_code(), ExitCode::Usage);
}

/// Seeds `marketplace/bundles.json` with one hand-written record so the
/// execution-level readiness arms (which need an *installed* record) can be
/// exercised; the registry-only zero state never reaches them.
fn seed_bundle_record(home: &std::path::Path, record_json: &str) {
    let marketplace = home.join("marketplace");
    std::fs::create_dir_all(&marketplace).unwrap();
    std::fs::write(
        marketplace.join("bundles.json"),
        format!(r#"{{"schema_version":1,"records":[{record_json}]}}"#),
    )
    .unwrap();
}

/// Execution-level readiness for a degraded CLI record. `degraded` is the
/// store's "registered but its assets are missing" flag, and CLI records reach
/// it two ways whose fixes are OPPOSITE:
/// - a routine disconnect — both `bundle_store_on_disconnected` implementations
///   (desktop `connectors::connector_cli`, and `connectors logout` in this
///   binary) call `mark_degraded` with the "已断开授权…" reason on every logout,
///   which makes it by far the common cause; the fix is re-authorizing;
/// - a legacy binary that fails its SHA-256 check against the lock table
///   (`store::legacy_cli_records`, a one-off first-boot import path); the fix
///   is re-downloading.
/// Collapsing both onto `cli_assets_mismatch` sent an operator who had merely
/// logged out off to re-download a binary — the exact misdirection the reason
/// code exists to prevent.
#[test]
fn readiness_separates_cli_disconnect_from_cli_asset_damage() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // Cause 1: the routine disconnect reason both writers store verbatim.
    {
        let home = SandboxHome::new("readiness-disconnected");
        seed_bundle_record(
            home.path(),
            r#"{"id":"feishu","source":"builtin","installed":true,"installed_at":"2026-09-23T00:00:00Z","degraded":"已断开授权：配套技能已随断开移除，重新连接即可恢复"}"#,
        );
        let value = run_json(&["pinvoy", "plugins", "readiness"]);
        let rows = value["bundles"].as_array().expect("bundles array");
        let feishu = rows
            .iter()
            .find(|row| row["bundle_id"] == "feishu")
            .expect("feishu row");
        assert_eq!(feishu["installed"], serde_json::json!(true));
        assert_eq!(feishu["ready"], serde_json::json!(false));
        assert_eq!(
            feishu["reason"],
            serde_json::json!("cli_disconnected"),
            "a logged-out connector must not be reported as a damaged binary"
        );
        assert_eq!(feishu["probe"], serde_json::json!("unavailable_in_cli"));
    }

    // Cause 2: the legacy asset check.
    {
        let home = SandboxHome::new("readiness-degraded");
        seed_bundle_record(
            home.path(),
            r#"{"id":"feishu","source":"builtin","installed":true,"installed_at":"2026-09-23T00:00:00Z","degraded":"CLI 二进制 SHA-256 与 lock 表不符，待重新下载"}"#,
        );
        let value = run_json(&["pinvoy", "plugins", "readiness"]);
        let rows = value["bundles"].as_array().expect("bundles array");
        let feishu = rows
            .iter()
            .find(|row| row["bundle_id"] == "feishu")
            .expect("feishu row");
        assert_eq!(feishu["ready"], serde_json::json!(false));
        assert_eq!(feishu["reason"], serde_json::json!("cli_assets_mismatch"));
        assert_ne!(
            feishu["reason"],
            serde_json::json!("not_connected"),
            "asset damage must not borrow the desktop's live-probe verdict either"
        );
        assert_eq!(feishu["probe"], serde_json::json!("unavailable_in_cli"));
    }
}

/// Execution-level readiness for a healthy, installed CLI record — the case
/// the registry cannot decide. Everything headless can check passes, but
/// "ready" for a connector means "logged in", and only the desktop's live
/// `*_status` probe knows that. The row must therefore NOT claim `ready:
/// true` (a logged-out feishu is indistinguishable here); it discloses the
/// limitation in `reason` + `probe` instead.
#[test]
fn readiness_never_claims_an_unprobed_cli_record_is_ready() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("readiness-cli-healthy");
    seed_bundle_record(
        home.path(),
        r#"{"id":"feishu","source":"builtin","installed":true,"installed_at":"2026-09-23T00:00:00Z"}"#,
    );

    let value = run_json(&["pinvoy", "plugins", "readiness"]);
    let rows = value["bundles"].as_array().expect("bundles array");
    let feishu = rows
        .iter()
        .find(|row| row["bundle_id"] == "feishu")
        .expect("feishu row");
    assert_eq!(feishu["installed"], serde_json::json!(true));
    assert_eq!(
        feishu["ready"],
        serde_json::json!(false),
        "no live probe ran, so readiness must not be asserted"
    );
    assert_eq!(
        feishu["reason"],
        serde_json::json!("connection_unknown_in_cli"),
        "the reason names the limitation, not an observed fault"
    );
    assert_eq!(feishu["probe"], serde_json::json!("unavailable_in_cli"));
}

/// Execution-level readiness for a degraded NON-CLI package. `degraded` means
/// the package is registered but its resources are missing, which is true
/// regardless of kind, so every non-CLI kind is demoted the same way.
///
/// The demotion is derived in the CLI's own row builder, NOT in
/// `readiness_for`: the desktop readiness card consumes that function verbatim
/// through `bundle_readiness`'s `_` arm, so demoting there would have flipped a
/// GUI card nobody asked to change. `bundle.rs`'s unit test pins the app-side
/// verdict as unchanged (`Ready`); this one pins the headless verdict.
#[test]
fn readiness_derives_assets_missing_for_a_degraded_package() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("readiness-degraded-pkg");
    seed_bundle_record(
        home.path(),
        r#"{"id":"canva-mcp","source":"builtin","installed":true,"installed_at":"2026-09-23T00:00:00Z","degraded":"resources missing"}"#,
    );

    let value = run_json(&["pinvoy", "plugins", "readiness"]);
    let rows = value["bundles"].as_array().expect("bundles array");
    let canva = rows
        .iter()
        .find(|row| row["bundle_id"] == "canva-mcp")
        .expect("canva-mcp row");
    assert_eq!(canva["kind"], serde_json::json!("mcp"));
    assert_eq!(canva["installed"], serde_json::json!(true));
    assert_eq!(
        canva["ready"],
        serde_json::json!(false),
        "a package whose resources are missing cannot serve requests"
    );
    assert_eq!(canva["reason"], serde_json::json!("assets_missing"));
    // Fully decided from registry state: no live probe is involved.
    assert_eq!(canva["probe"], serde_json::json!("registry"));
}

/// `tools auth` is a status READ: a damaged `mcp.json` must not turn it into
/// exit 1. The GUI's `get_marketplace_tool_auth_status` logs the read/parse
/// failure and continues with `mcp_configured = false`, which lands on
/// `auth_pending` for an installed OAuth tool; the CLI used to propagate the
/// parse error and exit 1 instead.
///
/// The second half pins the typing decision: the GUI deserializes the file
/// into a typed `McpConfig`, so a server entry that is structurally present
/// but carries a wrong field type makes the whole parse fail there. An untyped
/// `servers[name]` presence check reported `mcp_configured: true` for exactly
/// those files.
#[test]
fn tools_auth_degrades_on_a_damaged_mcp_json() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("tools-auth-corrupt-mcp");
    run_ok(&["pinvoy", "plugins", "tools", "install", "qcc"]);
    let mcp = home.path().join("bundle").join("mcp.json");
    assert!(mcp.is_file(), "installing qcc should write mcp.json");

    // Unparseable bytes: degrade, do not fail.
    std::fs::write(&mcp, "{ this is not json").unwrap();
    let value = run_json(&["pinvoy", "plugins", "tools", "auth", "qcc"]);
    assert_eq!(value["installed"], serde_json::json!(true));
    assert_eq!(
        value["mcp_configured"],
        serde_json::json!(false),
        "an unreadable MCP config must read as not configured, like the GUI"
    );
    assert_eq!(value["status"], serde_json::json!("auth_pending"));

    // Structurally present, type-invalid entry: `args` must be a string list.
    std::fs::write(
        &mcp,
        r#"{"servers":{"qcc-company":{"url":"https://example.invalid","args":"not-a-list"}}}"#,
    )
    .unwrap();
    let value = run_json(&["pinvoy", "plugins", "tools", "auth", "qcc"]);
    assert_eq!(
        value["mcp_configured"],
        serde_json::json!(false),
        "a server entry the GUI's typed parse rejects must not read as configured"
    );
    assert_eq!(value["status"], serde_json::json!("auth_pending"));

    // The same entry with the declared types parses on both surfaces.
    std::fs::write(
        &mcp,
        r#"{"servers":{"qcc-company":{"url":"https://example.invalid","args":[]}}}"#,
    )
    .unwrap();
    let value = run_json(&["pinvoy", "plugins", "tools", "auth", "qcc"]);
    assert_eq!(value["mcp_configured"], serde_json::json!(true));
    assert_eq!(
        value["status"],
        serde_json::json!("config_installed_auth_pending")
    );
}

/// The degradation above is scoped to the status read. `oauth-login` is about
/// to ACT on the config, and its GUI counterpart
/// (`start_marketplace_tool_oauth_login`) propagates the parse failure, so the
/// CLI keeps the strict reader there.
#[test]
fn oauth_login_propagates_a_damaged_mcp_json() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("oauth-login-corrupt-mcp");
    run_ok(&["pinvoy", "plugins", "tools", "install", "qcc"]);
    std::fs::write(home.path().join("bundle").join("mcp.json"), "{ nope").unwrap();

    let (message, code) = run_err(&["pinvoy", "plugins", "tools", "oauth-login", "qcc"]);
    assert_eq!(code, ExitCode::Failed);
    assert!(
        message.contains("mcp.json"),
        "the acting command must name the unusable config: {message}"
    );
}

/// The stored display name must be the one the GUI would store for the same
/// file: `sanitize_display_name` drops `/` and `\` and caps at 128 CHARS. The
/// CLI additionally drops zero-width/bidi code points, which only ever removes
/// characters, so the two surfaces stay convergent. Without the cap the same
/// file imported from the two surfaces got two different names in
/// `bundles.json` (the CLI's was unbounded).
#[test]
fn import_display_name_matches_the_gui_sanitizer() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("import-display-name");

    // 60 + 80 significant chars once the ZWSP and the backslash are dropped,
    // i.e. 140 > 128: the cap has to bite. A backslash is a legal file-name
    // character on unix, so both filter rules are exercised from one name.
    let raw = format!("{}\u{200B}\\{}.md", "a".repeat(60), "b".repeat(80));
    let file = home.path().join(&raw);
    std::fs::write(&file, "---\nname: long-name-skill\n---\nbody").unwrap();
    run_ok(&["pinvoy", "plugins", "import", file.to_str().unwrap()]);

    let bundles: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.path().join("marketplace/bundles.json")).unwrap(),
    )
    .unwrap();
    let record = bundles["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["id"] == "long-name-skill")
        .expect("bundle record for the imported skill");
    let expected = format!("upload:{}{}", "a".repeat(60), "b".repeat(68));
    assert_eq!(
        record["source"],
        serde_json::json!(expected),
        "the stored display name must be sanitized and capped at 128 chars"
    );
}

/// `wrap_markdown_skill` PREPENDS a frontmatter block, so the bytes that go
/// into the package are larger than the SKILL.md the walk charged. Pushing the
/// wrapper in without re-charging let a directory sitting exactly on
/// `MAX_PLUGIN_SIZE_BYTES` ship a package over it by the frontmatter's size.
/// The fixture lands exactly on the limit before wrapping (the walk's own
/// check is `>`), so only the re-charge can reject it.
#[test]
fn import_charges_the_wrapped_skill_md_against_the_limit() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = SandboxHome::new("import-wrapped-charge");
    let dir = home.path().join("wrapped-charge");
    std::fs::create_dir_all(&dir).unwrap();
    // No frontmatter name, so the wrapper runs.
    let body = "body without a frontmatter name";
    std::fs::write(dir.join("SKILL.md"), body).unwrap();
    write_sparse(
        &dir.join("a.bin"),
        import_package_limit() - body.len() as u64,
    );

    let (message, code) = run_err(&["pinvoy", "plugins", "import", dir.to_str().unwrap()]);
    assert_eq!(code, ExitCode::Failed);
    assert!(
        message.contains("exceeds the 200 MiB import limit"),
        "the injected frontmatter must be charged against the budget: {message}"
    );
}

/// `--scope both` is two single-scope writes and the storage layer offers no
/// two-scope transaction, so the payload names the scopes that actually
/// landed instead of implying an all-or-nothing apply. The human line also
/// carries the hot-refresh caveat: the GUI runs `hot_refresh` after a scope
/// change, the CLI hosts no engine pool, so a desktop app running alongside
/// keeps its live engines on the previous whitelist.
#[test]
fn scope_toggle_reports_applied_scopes_and_the_missing_hot_refresh() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = SandboxHome::new("scope-reporting");

    let value = run_json(&["pinvoy", "plugins", "disable", "weather", "--scope", "both"]);
    assert_eq!(
        value["scopes_applied"],
        serde_json::json!(["plain", "code"]),
        "both scopes landed, in write order"
    );
    assert_eq!(value["persistence_verified"], serde_json::json!(true));

    let value = run_json(&["pinvoy", "plugins", "enable", "weather", "--scope", "code"]);
    assert_eq!(
        value["scopes_applied"],
        serde_json::json!(["code"]),
        "a single-scope run reports only that scope"
    );

    let stdout = run_ok(&["pinvoy", "plugins", "enable", "weather", "--scope", "plain"]);
    assert!(
        stdout.contains("no hot-refresh broadcast"),
        "the toggle must disclose that live engines keep the stale whitelist: {stdout}"
    );
}
