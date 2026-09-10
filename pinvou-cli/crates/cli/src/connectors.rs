//! `connectors` family: vendor CLI connector status / lifecycle, mirroring
//! `pinvou3-app/src-tauri/src/app/commands/connectors.rs` and the feature
//! functions it forwards to under `features/connectors/`.
//!
//! Visibility note: the GUI's `features::connectors` submodules are
//! `pub(crate)`, so this module cannot call them directly. Every behavior
//! below mirrors the exact GUI feature function over the public
//! `pinvou3_lib` building blocks the feature itself uses:
//! - enable/disable marker files (`<id>_disabled` in `PINVOU3_HOME`) and
//!   skills-visibility reads → `pinvou3_lib::platform::connector_state`
//!   (the same marker semantics as the `ConnectorSkillGate` default impls
//!   and `set_<connector>_enabled` in feishu/wecom/dingtalk/tmeet.rs);
//! - scope / disabled-set sync → `pinvou3_lib::features::marketplace::
//!   sync_disabled_bundles_for_connector_switch` /
//!   `sync_deny_all_scopes_after_install` (the calls the GUI command layer
//!   makes around the feature calls);
//! - bundle-store mirror on connect/logout → `pinvou3_lib::features::
//!   marketplace::store::{BundleStore, BundleRecord}` +
//!   `platform::connector_lock` (mirror of `connector_cli::
//!   bundle_store_on_connected` / `bundle_store_on_disconnected`);
//! - vendor CLI probing / spawning → the same `--version` / `auth ...`
//!   commands, output parsers, auth-domain URL extractors and version gates
//!   as feishu.rs / wecom.rs / dingtalk.rs / tmeet.rs;
//! - ima → `pinvou3_lib::platform::credential_store` +
//!   `features::marketplace::skill_marketplace::SkillMarketplaceManager`
//!   (the exact pieces `features/connectors/ima.rs` is built on).
//!
//! Headless deviations (disclosed in command output, not silent):
//! - `apply-skills` / enable-disable cannot materialize the bundled skill
//!   directories: the unpack source is the desktop app's embedded bundle
//!   (`features::runtime_bundle`, crate-private). The CLI recomputes
//!   visibility + scope sync and reports `skills_refresh: "app-only"`.
//! - The execpolicy ruleset hot-refresh after connect / apply runs on the
//!   GUI's live `EnginePool`; the CLI has no engine pool and reports it.
//! - `connect` prints the login / QR URL instead of rendering the QR image
//!   (spec: QR image display is GUI-bound).
//! - `ensure-cli` downloads the lock-table-pinned archive with `reqwest`
//!   and extracts with the system `tar` (the CLI workspace has no tar/zip
//!   crates); tmeet installs through npm like the GUI does.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::support::{render, resolve_secret, sandbox_home, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::marketplace::skill_marketplace::SkillMarketplaceManager;
use pinvou3_lib::features::marketplace::store::{BundleRecord, BundleSource, BundleStore};
use pinvou3_lib::platform::connector_lock::{artifact_pin, executable_name, file_sha256_hex};
use pinvou3_lib::platform::connector_skills::WECOM_SKILL_DIRS;
use pinvou3_lib::platform::connector_state::skills_visible_for;
use pinvou3_lib::platform::credential_store::{
    CredentialReference, CredentialStore, SystemCredentialStore,
};
use pinvou3_lib::platform::paths::{
    assets_cli_dir, assets_staging_dir, bundles_root, pinvou3_home,
};

const USAGE: &str =
    "usage: pinvou connectors <status|ensure-cli|enable|disable|logout|apply-skills|connect|ima>";

/// Semantic-version floor per connector, mirroring the `*_MIN_VERSION`
/// gates in wecom.rs (1.1.0 command-model baseline) and tmeet.rs (1.0.15).
const WECOM_MIN_VERSION: (u64, u64, u64) = (1, 1, 0);
const TMEET_MIN_VERSION: (u64, u64, u64) = (1, 0, 15);
const TMEET_NPM_SPEC: &str = "@tencentcloud/tmeet@1.0.15";

/// Mirror of `runtime_bundle::platform::LARK_SKILL_DIRS` (crate-private):
/// the bundle skill directory names whose presence defines "skills applied"
/// for feishu. wecom comes from the public `platform::connector_skills`
/// single-source-of-truth table; dingtalk/tmeet are single mono skills.
const LARK_SKILL_DIRS: &[&str] = &[
    "lark-shared",
    "lark-calendar",
    "lark-doc",
    "lark-drive",
    "lark-sheets",
    "lark-im",
    "lark-task",
    "lark-wiki",
    "lark-base",
];
const DINGTALK_SKILL_DIRS: &[&str] = &["dws"];
const TMEET_SKILL_DIRS: &[&str] = &["tmeet-skill"];

/// ima skill installed by `ima_connect` (mirror of ima.rs `IMA_SKILL_ID`).
const IMA_SKILL_ID: &str = "ima-skills";
const IMA_SKILL_VERSION: &str = "1.1.8";
/// Stored degraded reason reused verbatim from
/// `connector_cli::bundle_store_on_disconnected` so GUI-rendered store data
/// stays identical regardless of which surface wrote it.
const DISCONNECTED_REASON: &str = "已断开授权：配套技能已随断开移除，重新连接即可恢复";

/// Archive size cap, mirroring `native_installer::MAX_ARCHIVE_BYTES`.
const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
/// Per-process wait for the vendor CLI to produce a login URL, mirroring the
/// 40s `rx.recv_timeout` in the GUI connect flows.
const LOGIN_URL_TIMEOUT_SECS: u64 = 40;
/// Default overall connect timeout, mirroring feishu's 5-minute authorize
/// window; `connect --timeout SECS` overrides it.
const CONNECT_DEFAULT_TIMEOUT_SECS: u64 = 300;
/// npm install timeout for tmeet, mirroring `install_tmeet_cli`.
const NPM_INSTALL_TIMEOUT_SECS: u64 = 180;

// ───────────────────────────── connector kinds ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectorKind {
    Feishu,
    Wecom,
    Dingtalk,
    Tmeet,
}

const ALL_CONNECTOR_IDS: &str = "feishu, wecom, dingtalk, tmeet";

impl ConnectorKind {
    fn parse(value: &str) -> Result<Self, CliError> {
        match value {
            "feishu" => Ok(Self::Feishu),
            "wecom" => Ok(Self::Wecom),
            "dingtalk" => Ok(Self::Dingtalk),
            "tmeet" => Ok(Self::Tmeet),
            other => Err(CliError::usage(format!(
                "unknown connector '{other}' (valid: {ALL_CONNECTOR_IDS})"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Feishu => "feishu",
            Self::Wecom => "wecom",
            Self::Dingtalk => "dingtalk",
            Self::Tmeet => "tmeet",
        }
    }

    fn spec(self) -> &'static VendorSpec {
        const FEISHU: VendorSpec = VendorSpec {
            id: "feishu",
            display_name: "Feishu",
            cli_bin: "lark-cli",
            envs: &[("LARK_CLI_NO_PROXY", "1")],
            auth_domains: &["feishu", "larksuite"],
            disabled_filename: "feishu_disabled",
            min_version: None,
        };
        const WECOM: VendorSpec = VendorSpec {
            id: "wecom",
            display_name: "WeCom",
            cli_bin: "wecom-cli",
            envs: &[],
            auth_domains: &["work.weixin.qq.com", "weixin.qq.com"],
            disabled_filename: "wecom_disabled",
            min_version: Some(WECOM_MIN_VERSION),
        };
        const DINGTALK: VendorSpec = VendorSpec {
            id: "dingtalk",
            display_name: "DingTalk",
            cli_bin: "dws",
            envs: &[],
            auth_domains: &["dingtalk.com", "login.dingtalk.com", "oauth.dingtalk.com"],
            disabled_filename: "dingtalk_disabled",
            min_version: None,
        };
        const TMEET: VendorSpec = VendorSpec {
            id: "tmeet",
            display_name: "Tencent Meeting",
            cli_bin: "tmeet",
            envs: &[],
            auth_domains: &["tencent", "qq.com"],
            disabled_filename: "tmeet_disabled",
            min_version: Some(TMEET_MIN_VERSION),
        };
        match self {
            Self::Feishu => &FEISHU,
            Self::Wecom => &WECOM,
            Self::Dingtalk => &DINGTALK,
            Self::Tmeet => &TMEET,
        }
    }

    fn skill_dirs(self) -> &'static [&'static str] {
        match self {
            Self::Feishu => LARK_SKILL_DIRS,
            // Same 14-directory table the runtime bundle gate iterates; the
            // app keeps it in `platform::connector_skills` (public).
            Self::Wecom => &WECOM_SKILL_DIRS,
            Self::Dingtalk => DINGTALK_SKILL_DIRS,
            Self::Tmeet => TMEET_SKILL_DIRS,
        }
    }
}

struct VendorSpec {
    id: &'static str,
    display_name: &'static str,
    cli_bin: &'static str,
    envs: &'static [(&'static str, &'static str)],
    auth_domains: &'static [&'static str],
    disabled_filename: &'static str,
    /// `Some(min)` = installs below this version count as not-installed and
    /// `status` reports `upgrade_required` (wecom/tmeet version gates).
    min_version: Option<(u64, u64, u64)>,
}

// ─────────────────────────────── commands ───────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectorsCommand {
    Status {
        connector: Option<ConnectorKind>,
    },
    EnsureCli {
        connector: ConnectorKind,
    },
    Enable {
        connector: ConnectorKind,
    },
    Disable {
        connector: ConnectorKind,
    },
    Logout {
        connector: ConnectorKind,
    },
    ApplySkills {
        connector: ConnectorKind,
    },
    Connect {
        connector: ConnectorKind,
        timeout: u64,
    },
    Ima(ImaCommand),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImaCommand {
    Status,
    Connect {
        client_id_env: String,
        api_key_env: Option<String>,
        api_key_stdin: bool,
    },
    Logout,
}

/// Flags that carry a value, per subcommand.
const CONNECT_OPTIONS: &[&str] = &["--timeout"];
const IMA_CONNECT_OPTIONS: &[&str] = &["--client-id-env", "--api-key-env"];

/// Boolean (valueless) flags, per subcommand.
const IMA_CONNECT_FLAGS: &[&str] = &["--api-key-stdin"];

pub fn parse(values: &[String]) -> Result<ConnectorsCommand, CliError> {
    let subcommand = values.get(1).ok_or_else(|| CliError::usage(USAGE))?;
    let rest = &values[2..];
    match subcommand.as_str() {
        "status" => {
            let connector = match rest.first() {
                None => None,
                Some(first) if first.starts_with("--") => None,
                Some(first) => Some(ConnectorKind::parse(first)?),
            };
            Ok(ConnectorsCommand::Status { connector })
        }
        "ensure-cli" | "enable" | "disable" | "logout" | "apply-skills" => {
            let connector = require_connector(rest, subcommand)?;
            if rest.len() > 1 {
                return Err(CliError::usage(format!(
                    "connectors {subcommand} accepts no options"
                )));
            }
            Ok(match subcommand.as_str() {
                "ensure-cli" => ConnectorsCommand::EnsureCli { connector },
                "enable" => ConnectorsCommand::Enable { connector },
                "disable" => ConnectorsCommand::Disable { connector },
                "logout" => ConnectorsCommand::Logout { connector },
                _ => ConnectorsCommand::ApplySkills { connector },
            })
        }
        "connect" => {
            let connector = require_connector(rest, "connect")?;
            let (options, _) = parse_flags(&rest[1..], CONNECT_OPTIONS, &[])?;
            let timeout = match option(&options, "--timeout") {
                None => CONNECT_DEFAULT_TIMEOUT_SECS,
                Some(value) => value
                    .parse::<u64>()
                    .ok()
                    .filter(|secs| *secs > 0)
                    .ok_or_else(|| {
                        CliError::usage("connectors connect --timeout must be a positive integer")
                    })?,
            };
            Ok(ConnectorsCommand::Connect { connector, timeout })
        }
        "ima" => {
            let action = rest.first().ok_or_else(|| {
                CliError::usage("usage: pinvou connectors ima <status|connect|logout>")
            })?;
            let rest = &rest[1..];
            match action.as_str() {
                "status" => {
                    if !rest.is_empty() {
                        return Err(CliError::usage("connectors ima status accepts no options"));
                    }
                    Ok(ConnectorsCommand::Ima(ImaCommand::Status))
                }
                "connect" => {
                    let (options, flags) =
                        parse_flags(rest, IMA_CONNECT_OPTIONS, IMA_CONNECT_FLAGS)?;
                    let client_id_env = option(&options, "--client-id-env")
                        .ok_or_else(|| {
                            CliError::usage(
                                "connectors ima connect requires --client-id-env VAR (secrets are never accepted as argv values)",
                            )
                        })?
                        .to_owned();
                    Ok(ConnectorsCommand::Ima(ImaCommand::Connect {
                        client_id_env,
                        api_key_env: option(&options, "--api-key-env").map(str::to_owned),
                        api_key_stdin: flags.contains(&"--api-key-stdin"),
                    }))
                }
                "logout" => {
                    if !rest.is_empty() {
                        return Err(CliError::usage("connectors ima logout accepts no options"));
                    }
                    Ok(ConnectorsCommand::Ima(ImaCommand::Logout))
                }
                other => Err(CliError::usage(format!(
                    "unknown ima action '{other}' (valid: status, connect, logout)"
                ))),
            }
        }
        _ => Err(CliError::usage(USAGE)),
    }
}

fn require_connector(values: &[String], subcommand: &str) -> Result<ConnectorKind, CliError> {
    let value = values.first().ok_or_else(|| {
        CliError::usage(format!(
            "connectors {subcommand} requires a connector ({ALL_CONNECTOR_IDS})"
        ))
    })?;
    if value.starts_with("--") {
        return Err(CliError::usage(format!(
            "connectors {subcommand} requires a connector ({ALL_CONNECTOR_IDS})"
        )));
    }
    ConnectorKind::parse(value)
}

/// Mirrors the pair-based flag parser used by the sessions family: every
/// token must be a known value flag (followed by a non-empty value) or a
/// known boolean flag.
fn parse_flags<'a>(
    values: &'a [String],
    value_flags: &[&str],
    boolean_flags: &[&str],
) -> Result<(Vec<(&'a str, &'a str)>, Vec<&'a str>), CliError> {
    let mut options = Vec::new();
    let mut flags = Vec::new();
    let mut index = 0;
    while index < values.len() {
        let token = values[index].as_str();
        if boolean_flags.contains(&token) {
            if flags.contains(&token) {
                return Err(CliError::usage(format!(
                    "duplicate connectors option {token}"
                )));
            }
            flags.push(token);
            index += 1;
            continue;
        }
        if !value_flags.contains(&token) {
            return Err(CliError::usage(format!(
                "unsupported connectors option: {token}"
            )));
        }
        if options.iter().any(|(name, _)| *name == token) {
            return Err(CliError::usage(format!(
                "duplicate connectors option {token}"
            )));
        }
        let value = values.get(index + 1).ok_or_else(|| {
            CliError::usage(format!("connectors option {token} requires a value"))
        })?;
        if value.is_empty() || value.starts_with("--") {
            return Err(CliError::usage(format!(
                "connectors option {token} requires a value"
            )));
        }
        options.push((token, value.as_str()));
        index += 2;
    }
    Ok((options, flags))
}

fn option<'a>(options: &'a [(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    options
        .iter()
        .find_map(|(candidate, value)| (*candidate == name).then_some(*value))
}

// ─────────────────────── vendor CLI subprocess helpers ───────────────────────

/// Runs `<cli> <args>` capturing `(success, stdout, stderr)` — mirror of
/// `connector_cli::run` (adds the connector envs from the spec).
fn run_cli(spec: &VendorSpec, args: &[&str]) -> Result<(bool, String, String), CliError> {
    let mut cmd = Command::new(spec.cli_bin);
    for (key, value) in spec.envs {
        cmd.env(key, value);
    }
    cmd.args(args);
    let outcome = cmd.output().map_err(|error| {
        CliError::failed(format!(
            "{} could not be executed: {error} (install it first: pinvou connectors ensure-cli {})",
            spec.cli_bin, spec.id
        ))
    })?;
    Ok((
        outcome.status.success(),
        String::from_utf8_lossy(&outcome.stdout).into_owned(),
        String::from_utf8_lossy(&outcome.stderr).into_owned(),
    ))
}

/// First embedded JSON object in mixed CLI output — mirror of
/// `connector_cli::parse_json` (spinners / hint lines around `--json`).
fn parse_json(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&text[start..=end]).ok()
}

/// Three-segment semantic version from the first numeric run — mirror of
/// `connector_cli::parse_semver3` (missing segments pad with 0).
fn parse_semver3(text: &str) -> Option<(u64, u64, u64)> {
    let mut nums = text
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u64>().ok());
    let major = nums.next()?;
    Some((major, nums.next().unwrap_or(0), nums.next().unwrap_or(0)))
}

/// Per-connector `--version` parsing, mirroring each feature file:
/// - wecom: parse the token following the one containing `wecom-cli`
///   (`parse_wecom_version` contract: skip build-timestamp noise);
/// - tmeet: skip to the `version` marker and strip a `v` prefix
///   (`parse_tmeet_version`);
/// - feishu / dingtalk: whole-output three-segment parse, display only (the
///   GUI gates them on `--version` success, not on a version number).
fn cli_semver(spec: &VendorSpec, stdout: &str, stderr: &str) -> Option<(u64, u64, u64)> {
    let source = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    match spec.id {
        "wecom" => {
            let tokens: Vec<&str> = source.split_whitespace().collect();
            let index = tokens
                .iter()
                .position(|token| token.contains("wecom-cli"))?;
            parse_semver3(tokens.get(index + 1).copied().unwrap_or(""))
        }
        "tmeet" => {
            let lower = source.to_ascii_lowercase();
            let marker = lower
                .find("version")
                .map(|index| index + "version".len())
                .unwrap_or(0);
            let tail = &source[marker..];
            let start = tail
                .char_indices()
                .find(|(_, c)| c.is_ascii_digit() || *c == 'v')?
                .0;
            parse_semver3(tail[start..].trim_start_matches(['v', 'V']))
        }
        _ => parse_semver3(source),
    }
}

/// `(semver, first non-empty version line)` from one `--version` invocation.
fn probe_cli_version(spec: &VendorSpec) -> Option<((u64, u64, u64), String)> {
    let (ok, stdout, stderr) = run_cli(spec, &["--version"]).ok()?;
    if !ok {
        return None;
    }
    let semver = cli_semver(spec, &stdout, &stderr)?;
    let raw = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    let raw = raw
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_owned();
    Some((semver, raw))
}

/// Installation gate per connector, mirroring `*_cli_present`:
/// feishu/dingtalk count any working `--version`; wecom/tmeet also require
/// the minimum version (older installs must be replaced, not used).
fn cli_installed(spec: &VendorSpec) -> bool {
    match probe_cli_version(spec) {
        Some((semver, _)) => spec.min_version.map_or(true, |min| semver >= min),
        None => false,
    }
}

/// Runs the connector's status subcommand and returns `(exit_ok, stdout,
/// stderr)` — one spawn shared by the `ok` DTO field and the connected
/// predicate.
fn run_status_probe(spec: &VendorSpec) -> Result<(bool, String, String), CliError> {
    match spec.id {
        "feishu" => run_cli(spec, &["auth", "status", "--json"]),
        "wecom" => run_cli(spec, &["auth", "show", "--status"]),
        "dingtalk" => run_cli(spec, &["auth", "status", "--format", "json"]),
        "tmeet" => run_cli(spec, &["auth", "status"]),
        _ => Ok((false, String::new(), String::new())),
    }
}

/// Per-connector connected predicate over the status subcommand output,
/// mirroring the feature files:
/// - feishu: JSON `identities/user/status == "ready"`;
/// - wecom: exit ok and the whole line `authorized` (case-insensitive);
/// - dingtalk: JSON `authenticated: true`;
/// - tmeet: output contains `Logged in`.
fn cli_connected(spec: &VendorSpec) -> Result<bool, CliError> {
    let (ok, stdout, stderr) = run_status_probe(spec)?;
    Ok(match spec.id {
        "feishu" => parse_json(&stdout)
            .or_else(|| parse_json(&stderr))
            .and_then(|value| {
                value
                    .pointer("/identities/user/status")
                    .and_then(Value::as_str)
                    .map(|status| status == "ready")
            })
            .unwrap_or(false),
        "wecom" => ok && (authorized_line(&stdout) || authorized_line(&stderr)),
        "dingtalk" => authenticated_json(&stdout) || authenticated_json(&stderr),
        "tmeet" => stdout.contains("Logged in") || stderr.contains("Logged in"),
        _ => false,
    })
}

/// Mirror of wecom `status_is_authorized`: the whole trimmed line must be
/// `authorized` (case-insensitive); `unauthorized` shares the prefix and
/// must not match.
fn authorized_line(text: &str) -> bool {
    text.trim().eq_ignore_ascii_case("authorized")
}

/// Mirror of dingtalk `auth_is_authenticated_str`: only the official JSON
/// `authenticated: true` counts.
fn authenticated_json(text: &str) -> bool {
    parse_json(text)
        .and_then(|value| value.get("authenticated").and_then(Value::as_bool))
        .unwrap_or(false)
}

// ────────────────────────── marker / skills state ──────────────────────────

/// Disabled marker read via the app's own platform helper (marker file
/// `<id>_disabled` under `PINVOU3_HOME`), same as `ConnectorSkillGate::
/// is_disabled` / `platform::connector_state`.
fn is_disabled(kind: ConnectorKind) -> bool {
    !skills_visible_for(kind.as_str())
}

/// Enabled/disabled marker write, mirroring `ConnectorSkillGate::
/// set_disabled_flag` (write-on-disable, remove-on-enable, idempotent,
/// failures propagate).
fn set_disabled_flag(spec: &VendorSpec, disabled: bool) -> Result<(), CliError> {
    let path = pinvou3_home().join(spec.disabled_filename);
    if disabled {
        std::fs::write(&path, b"1").map_err(|error| {
            CliError::failed(format!(
                "cannot save {} skill disabled state: {error}",
                spec.display_name
            ))
        })?;
    } else if path.exists() {
        std::fs::remove_file(&path).map_err(|error| {
            CliError::failed(format!(
                "cannot clear {} skill disabled state: {error}",
                spec.display_name
            ))
        })?;
    }
    Ok(())
}

/// Skills-applied state, mirroring `cached_*_skills_visible`: the enable
/// marker is clear AND every bundle skill directory carries its SKILL.md
/// under `~/.pinvou3/bundles/<id>/skills/`.
fn skills_applied(kind: ConnectorKind) -> bool {
    if is_disabled(kind) {
        return false;
    }
    let target = bundles_root().join(kind.as_str()).join("skills");
    kind.skill_dirs()
        .iter()
        .all(|dir| target.join(dir).join("SKILL.md").is_file())
}

/// Mirror of `connector_cli::bundle_store_on_disconnected`: logout marks the
/// marketplace record degraded (record stays installed; reconnect repairs).
fn bundle_store_on_disconnected(id: &str) {
    let _ = BundleStore::new().mark_degraded(id, DISCONNECTED_REASON);
}

/// Mirror of `connector_cli::bundle_store_on_connected`: register the CLI
/// package (`source=Builtin`) pinned to the lock-table version/SHA-256 and
/// clear `degraded`. Mirror-write failures never fail the main operation
/// (the GUI only logs them).
fn bundle_store_on_connected(id: &str) {
    use pinvou3_lib::features::marketplace::store::{ASSET_KIND_CLI, AssetRef};
    let mut record = BundleRecord::installed_now(id, BundleSource::Builtin);
    if let Some(pin) = artifact_pin(id) {
        record.assets.push(AssetRef {
            kind: ASSET_KIND_CLI.to_owned(),
            name: id.to_owned(),
            version: pin.version,
            sha256: pin.binary_sha256,
        });
    }
    let _ = BundleStore::new().upsert_preserving(record);
}

// ─────────────────────────────── execute ───────────────────────────────

pub fn execute(command: ConnectorsCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    match command {
        ConnectorsCommand::Status { connector } => status(connector, output),
        ConnectorsCommand::EnsureCli { connector } => ensure_cli(connector, output),
        ConnectorsCommand::Enable { connector } => set_enabled(connector, true, output),
        ConnectorsCommand::Disable { connector } => set_enabled(connector, false, output),
        ConnectorsCommand::Logout { connector } => logout(connector, output),
        ConnectorsCommand::ApplySkills { connector } => apply_skills(connector, output),
        ConnectorsCommand::Connect { connector, timeout } => connect(connector, timeout, output),
        ConnectorsCommand::Ima(action) => execute_ima(action, output),
    }
}

fn status(connector: Option<ConnectorKind>, output: OutputMode) -> Result<CliOutcome, CliError> {
    let kinds = match connector {
        Some(kind) => vec![kind],
        None => vec![
            ConnectorKind::Feishu,
            ConnectorKind::Wecom,
            ConnectorKind::Dingtalk,
            ConnectorKind::Tmeet,
        ],
    };
    let mut entries = Vec::new();
    for kind in kinds {
        entries.push(vendor_status_entry(kind)?);
    }
    entries.push(ima_status_entry()?);

    let human = entries
        .iter()
        .map(|entry| {
            let mut line = format!("{}\t", entry["id"].as_str().unwrap_or("-"));
            if entry.get("credentials_present").is_some() {
                line.push_str(&format!(
                    "connected={}\tcredentials={}\tskill_installed={}",
                    yes_no(bool_field(entry, "connected")),
                    yes_no(bool_field(entry, "credentials_present")),
                    yes_no(bool_field(entry, "skill_installed")),
                ));
            } else {
                let installed = if bool_field(entry, "installed") {
                    match entry.get("version").and_then(Value::as_str) {
                        Some(version) => format!("yes({version})"),
                        None => "yes".to_owned(),
                    }
                } else {
                    "no".to_owned()
                };
                line.push_str(&format!(
                    "installed={}\tconnected={}\tenabled={}\tskills={}",
                    installed,
                    yes_no(bool_field(entry, "connected")),
                    yes_no(bool_field(entry, "enabled")),
                    yes_no(bool_field(entry, "skills_applied")),
                ));
                if bool_field(entry, "upgrade_required") {
                    line.push_str("\tupgrade_required=yes");
                }
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(success(render(
        output,
        human,
        &json!({ "connectors": entries }),
    )))
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// `Value` boolean read defaulting to false (JSON round-trips of DTO fields).
fn bool_field(entry: &Value, key: &str) -> bool {
    entry.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// One status object per connector, mirroring the GUI feature DTO fields
/// (`feishu_status` / `wecom_status` / `dingtalk_status` / `tmeet_status`)
/// plus the enable marker and skills-applied state the composer reads via
/// `platform::connector_state` and the bundle skill directories.
fn vendor_status_entry(kind: ConnectorKind) -> Result<Value, CliError> {
    let spec = kind.spec();
    let mut entry = json!({
        "id": spec.id,
        "enabled": !is_disabled(kind),
        "skills_applied": skills_applied(kind),
    });
    match probe_cli_version(spec) {
        None => {
            entry["ok"] = json!(false);
            entry["connected"] = json!(false);
            entry["installed"] = json!(false);
            if spec.min_version.is_some() {
                entry["upgrade_required"] = json!(false);
            }
        }
        Some((semver, raw)) if spec.min_version.map_or(false, |min| semver < min) => {
            // Installed but below the command-model baseline: mirror the
            // `upgrade_required` three-state the wecom/tmeet DTOs report.
            entry["ok"] = json!(false);
            entry["connected"] = json!(false);
            entry["installed"] = json!(true);
            entry["upgrade_required"] = json!(true);
            entry["version"] = json!(raw);
        }
        Some((_, raw)) => {
            entry["installed"] = json!(true);
            entry["upgrade_required"] = json!(false);
            entry["version"] = json!(raw);
            let (ok, stdout, stderr) = run_status_probe(spec)?;
            let connected = match spec.id {
                "feishu" => {
                    let parsed = parse_json(&stdout).or_else(|| parse_json(&stderr));
                    let connected = parsed
                        .as_ref()
                        .and_then(|value| value.pointer("/identities/user/status"))
                        .and_then(Value::as_str)
                        .map(|status| status == "ready")
                        .unwrap_or(false);
                    // Mirror `feishu_status`'s extra `configured` flag
                    // (non-empty appId in the auth status payload).
                    let configured = parsed
                        .as_ref()
                        .and_then(|value| value.get("appId"))
                        .and_then(Value::as_str)
                        .map(|app_id| !app_id.is_empty())
                        .unwrap_or(false);
                    entry["configured"] = json!(configured);
                    connected
                }
                "wecom" => ok && (authorized_line(&stdout) || authorized_line(&stderr)),
                "dingtalk" => authenticated_json(&stdout) || authenticated_json(&stderr),
                "tmeet" => stdout.contains("Logged in") || stderr.contains("Logged in"),
                _ => false,
            };
            entry["ok"] = json!(ok);
            entry["connected"] = json!(connected);
        }
    }
    Ok(entry)
}

fn set_enabled(
    kind: ConnectorKind,
    enabled: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let spec = kind.spec();
    sandbox_home()?;
    set_disabled_flag(spec, !enabled)?;
    // Mirror of the `set_<connector>_enabled` scope bridge: the marker file
    // alone only gates skill directories inside the app; the execpolicy CLI
    // hard-block and skill materialization read `disabled_bundles.json`.
    pinvou3_lib::features::marketplace::sync_disabled_bundles_for_connector_switch(
        spec.id, enabled,
    );
    let connected = cli_connected(spec).unwrap_or(false);
    let skills_should_show = connected && !is_disabled(kind);
    let action = if enabled { "enabled" } else { "disabled" };
    let human = format!(
        "{action} {}\nconnected: {}\nskills should show: {}\nskills refresh: deferred to the desktop app (embedded bundle unpack is app-only)",
        spec.id,
        yes_no(connected),
        yes_no(skills_should_show),
    );
    let value = json!({
        "ok": true,
        "id": spec.id,
        "action": action,
        "enabled": enabled,
        "connected": connected,
        "skills_should_show": skills_should_show,
        "skills_refresh": "app-only",
    });
    Ok(success(render(output, human, &value)))
}

/// Mirror of `<connector>_apply_skills`: recompute `should_show = !disabled
/// && connected`, sync the DenyAll scopes on newly-visible connectors, and
/// disclose that the actual skill unpack (embedded bundle) and the execpolicy
/// ruleset hot-refresh (live engine pool) are app-side.
fn apply_skills(kind: ConnectorKind, output: OutputMode) -> Result<CliOutcome, CliError> {
    let spec = kind.spec();
    let connected = cli_connected(spec)?;
    let visible = connected && !is_disabled(kind);
    if visible {
        pinvou3_lib::features::marketplace::sync_deny_all_scopes_after_install(spec.id);
    }
    let human = format!(
        "{} skills should show: {}\nconnected: {}\nskill unpack: requires the desktop app (embedded bundle)\nruleset refresh: requires the GUI engine pool",
        spec.id,
        yes_no(visible),
        yes_no(connected),
    );
    let value = json!({
        "id": spec.id,
        "visible": visible,
        "connected": connected,
        "skills_unpack": "app-only",
        "ruleset_refresh": "gui-only",
    });
    Ok(success(render(output, human, &value)))
}

fn logout(kind: ConnectorKind, output: OutputMode) -> Result<CliOutcome, CliError> {
    let spec = kind.spec();
    let value = match spec.id {
        // Mirror `feishu_logout`: `lark-cli auth logout` clears the token.
        "feishu" => {
            let (ok, _, _) = run_cli(spec, &["auth", "logout"])?;
            if !ok {
                return Err(CliError::failed(format!(
                    "{} CLI logout failed",
                    spec.display_name
                )));
            }
            bundle_store_on_disconnected(spec.id);
            json!({ "ok": true, "id": spec.id })
        }
        // Mirror `wecom_logout`: no logout subcommand exists; delete the
        // credential directory `~/.config/wecom` (real user home, exactly
        // like the GUI — PINVOU3_HOME does not relocate vendor credentials).
        "wecom" => {
            let dir = wecom_config_dir();
            let existed = dir.exists();
            let _ = std::fs::remove_dir_all(&dir);
            bundle_store_on_disconnected(spec.id);
            json!({ "ok": true, "id": spec.id, "removed": existed })
        }
        // Mirror `dingtalk_logout` / `tmeet_logout`: already logged out when
        // the CLI is not installed; otherwise `auth logout [--yes]` must
        // succeed before the store mirror is updated.
        _ => {
            let args: &[&str] = if spec.id == "dingtalk" {
                &["auth", "logout", "--yes"]
            } else {
                &["auth", "logout"]
            };
            if probe_cli_version(spec).is_none() {
                bundle_store_on_disconnected(spec.id);
                json!({ "ok": true, "id": spec.id, "installed": false })
            } else {
                let (ok, _, _) = run_cli(spec, args)?;
                if !ok {
                    return Err(CliError::failed(format!(
                        "{} CLI logout failed",
                        spec.display_name
                    )));
                }
                bundle_store_on_disconnected(spec.id);
                json!({ "ok": true, "id": spec.id, "installed": true })
            }
        }
    };
    Ok(success(render(
        output,
        format!("logged out {}", spec.id),
        &value,
    )))
}

/// Mirror of `wecom_config_dir` (real home, not PINVOU3_HOME).
fn wecom_config_dir() -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    Path::new(&home).join(".config").join("wecom")
}

// ─────────────────────────────── ensure-cli ───────────────────────────────

/// Platform lock table — the same pinned artifacts
/// `features/connectors/native_installer.rs` reads (URL + archive/binary
/// SHA-256). Included from the app resources because the installer is
/// crate-private to the app.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const LOCK_JSON: &str = include_str!(
    "../../../../pinvou3-app/src-tauri/resources/platforms/linux/aarch64/bundle/connectors/connectors.lock.json"
);
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const LOCK_JSON: &str = include_str!(
    "../../../../pinvou3-app/src-tauri/resources/platforms/linux/x86_64/bundle/connectors/connectors.lock.json"
);
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const LOCK_JSON: &str = include_str!(
    "../../../../pinvou3-app/src-tauri/resources/platforms/macos/aarch64/bundle/connectors/connectors.lock.json"
);
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const LOCK_JSON: &str = include_str!(
    "../../../../pinvou3-app/src-tauri/resources/platforms/macos/x86_64/bundle/connectors/connectors.lock.json"
);
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const LOCK_JSON: &str = include_str!(
    "../../../../pinvou3-app/src-tauri/resources/platforms/windows/x86_64/bundle/connectors/connectors.lock.json"
);
#[cfg(not(any(
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "windows", target_arch = "x86_64"),
)))]
const LOCK_JSON: &str = "";

/// The lock table has no `serde` derive here (the CLI crate depends on
/// `serde_json` only), so artifacts are pulled out as `Value` records.
struct LockArtifact {
    name: String,
    version: String,
    url: String,
    archive_sha256: String,
    binary_sha256: String,
}

fn load_lock() -> Result<(String, Vec<LockArtifact>), CliError> {
    const NO_LOCK: &str = "this platform has no reviewed connector CLI install records";
    if LOCK_JSON.is_empty() {
        return Err(CliError::failed(NO_LOCK));
    }
    let table: Value = serde_json::from_str(LOCK_JSON)
        .map_err(|_| CliError::failed("connector CLI lock table is invalid"))?;
    let expected = pinvou3_lib::platform::paths::connector_platform_dir(
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
    .unwrap_or_default();
    let platform = table
        .get("platform")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if platform != expected {
        return Err(CliError::failed(NO_LOCK));
    }
    let artifacts = table
        .get("artifacts")
        .and_then(Value::as_array)
        .map(|artifacts| {
            artifacts
                .iter()
                .map(|artifact| LockArtifact {
                    name: string_field(artifact, "name"),
                    version: string_field(artifact, "version"),
                    url: string_field(artifact, "url"),
                    archive_sha256: string_field(artifact, "archiveSha256"),
                    binary_sha256: string_field(artifact, "binarySha256"),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok((platform, artifacts))
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn ensure_cli(kind: ConnectorKind, output: OutputMode) -> Result<CliOutcome, CliError> {
    let spec = kind.spec();
    if cli_installed(spec) {
        let value = json!({ "ok": true, "id": spec.id, "already": true });
        return success_or_render(output, spec.id, "already installed", value);
    }
    let installed = match spec.id {
        // Mirror `install_tmeet_cli`: npm global install of the pinned spec.
        "tmeet" => {
            if !run_npm_install(spec)? {
                return Err(CliError::failed(format!(
                    "{} CLI install failed, see ~/.pinvou3/cli-install.log",
                    spec.display_name
                )));
            }
            true
        }
        // Mirror `native_installer::ensure_native_cli`: download the pinned
        // archive, verify both hashes, stage the executable into the
        // versioned asset directory.
        _ => ensure_native_cli(spec)?,
    };
    if installed && !cli_installed(spec) {
        return Err(CliError::failed(format!(
            "{} CLI install finished but the binary will not execute; retry",
            spec.display_name
        )));
    }
    let value = json!({ "ok": true, "id": spec.id, "already": false });
    success_or_render(output, spec.id, "installed", value)
}

fn success_or_render(
    output: OutputMode,
    id: &str,
    action: &str,
    value: Value,
) -> Result<CliOutcome, CliError> {
    Ok(success(render(
        output,
        format!("{id} cli {action}"),
        &value,
    )))
}

/// Mirror of `install_tmeet_cli` + `connector_cli::run_with_timeout`:
/// stdin nulled (installers hang on an inherited TTY-less stdin), output
/// captured to `~/.pinvou3/cli-install.log`, hard timeout.
fn run_npm_install(spec: &VendorSpec) -> Result<bool, CliError> {
    let log_path = pinvou3_home().join("cli-install.log");
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut cmd = Command::new("npm");
    cmd.args(["install", "-g", TMEET_NPM_SPEC]);
    for (key, value) in spec.envs {
        cmd.env(key, value);
    }
    let (out, err) = match std::fs::File::create(&log_path) {
        Ok(file) => match file.try_clone() {
            Ok(clone) => (Stdio::from(file), Stdio::from(clone)),
            Err(_) => (Stdio::null(), Stdio::null()),
        },
        Err(_) => (Stdio::null(), Stdio::null()),
    };
    cmd.stdin(Stdio::null()).stdout(out).stderr(err);
    let mut child = cmd
        .spawn()
        .map_err(|error| CliError::failed(format!("npm install failed to start: {error}")))?;
    let start = Instant::now();
    loop {
        match child
            .try_wait()
            .map_err(|error| CliError::failed(format!("wait: {error}")))?
        {
            Some(status) => return Ok(status.success()),
            None => {
                if start.elapsed() > Duration::from_secs(NPM_INSTALL_TIMEOUT_SECS) {
                    let _ = child.kill();
                    return Err(CliError::failed(format!(
                        "CLI install timed out after {NPM_INSTALL_TIMEOUT_SECS}s (network or proxy blocked; log at {})",
                        log_path.display()
                    )));
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
}

/// Headless mirror of `native_installer::ensure_native_cli` for the
/// lock-table CLIs (lark-cli / wecom-cli / dws). Differences (disclosed in
/// the module doc): extraction uses the system `tar` and the license
/// side-files the app writes are skipped; both SHA-256 verifications are
/// identical to the GUI path.
fn ensure_native_cli(spec: &VendorSpec) -> Result<bool, CliError> {
    let (platform, artifacts) = load_lock()?;
    let artifact = artifacts
        .iter()
        .find(|artifact| artifact.name == spec.cli_bin)
        .ok_or_else(|| {
            CliError::failed(format!(
                "no reviewed install record for {} on this platform",
                spec.cli_bin
            ))
        })?;
    let filename = executable_name(&artifact.name);
    let version_dir = assets_cli_dir(&artifact.name, &artifact.version);
    let destination = version_dir.join(&filename);
    if file_is_sha256(&destination, &artifact.binary_sha256) {
        // Present on disk (but not resolvable through PATH): the GUI's
        // `connector_cli_command` resolves this path at spawn time.
        return Ok(false);
    }

    std::fs::create_dir_all(&version_dir).map_err(|error| {
        CliError::failed(format!("cannot create connector asset directory: {error}"))
    })?;
    let staging_dir = assets_staging_dir().join(&platform);
    std::fs::create_dir_all(&staging_dir)
        .map_err(|error| CliError::failed(format!("cannot create staging directory: {error}")))?;
    let archive_ext = if artifact.url.ends_with(".zip") {
        "zip"
    } else {
        "tar.gz"
    };
    let archive = staging_dir.join(format!(
        "{}-{}.{}",
        artifact.name, artifact.version, archive_ext
    ));
    if !file_is_sha256(&archive, &artifact.archive_sha256) {
        download_https(&artifact.url, &archive)?;
        if !file_is_sha256(&archive, &artifact.archive_sha256) {
            let _ = std::fs::remove_file(&archive);
            return Err(CliError::failed(format!(
                "{} archive checksum mismatch",
                artifact.name
            )));
        }
    }

    let extract_dir = staging_dir.join(format!("{}-extract-{}", artifact.name, std::process::id()));
    let _ = std::fs::remove_dir_all(&extract_dir);
    std::fs::create_dir_all(&extract_dir)
        .map_err(|error| CliError::failed(format!("cannot create extract directory: {error}")))?;
    if let Err(error) = extract_member(&archive, &filename, &extract_dir) {
        let _ = std::fs::remove_dir_all(&extract_dir);
        return Err(error);
    }
    let extracted = extract_dir.join(&filename);
    if !file_is_sha256(&extracted, &artifact.binary_sha256) {
        let _ = std::fs::remove_dir_all(&extract_dir);
        return Err(CliError::failed(format!(
            "{} executable checksum mismatch",
            artifact.name
        )));
    }

    let staging = version_dir.join(format!(".{filename}.installing-{}", std::process::id()));
    let _ = std::fs::remove_file(&staging);
    std::fs::rename(&extracted, &staging)
        .map_err(|error| CliError::failed(format!("cannot stage connector binary: {error}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).map_err(
            |error| CliError::failed(format!("cannot set executable permissions: {error}")),
        )?;
    }
    if destination.exists() {
        std::fs::remove_file(&destination).map_err(|error| {
            CliError::failed(format!("cannot replace old connector binary: {error}"))
        })?;
    }
    std::fs::rename(&staging, &destination)
        .map_err(|error| CliError::failed(format!("cannot finish connector install: {error}")))?;
    let _ = std::fs::remove_dir_all(&extract_dir);
    Ok(true)
}

fn file_is_sha256(path: &Path, expected: &str) -> bool {
    file_sha256_hex(path)
        .map(|actual| actual == expected)
        .unwrap_or(false)
}

/// HTTPS-only download capped at the app's archive limit.
fn download_https(url: &str, destination: &Path) -> Result<(), CliError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| CliError::failed("connector download URL is invalid"))?;
    if parsed.scheme() != "https" {
        return Err(CliError::failed("connector download URL must be https"));
    }
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|error| CliError::failed(format!("cannot build download client: {error}")))?
        .get(parsed)
        .send()
        .map_err(|error| CliError::failed(format!("connector download failed: {error}")))?;
    if !response.status().is_success() {
        return Err(CliError::failed(format!(
            "connector download failed with HTTP {}",
            response.status()
        )));
    }
    let mut reader = response.take(MAX_ARCHIVE_BYTES + 1);
    let mut file = std::fs::File::create(destination)
        .map_err(|error| CliError::failed(format!("cannot create archive file: {error}")))?;
    std::io::copy(&mut reader, &mut file)
        .map_err(|error| CliError::failed(format!("connector download failed: {error}")))?;
    let size = file
        .metadata()
        .map(|meta| meta.len())
        .map_err(|error| CliError::failed(format!("cannot stat archive file: {error}")))?;
    if size > MAX_ARCHIVE_BYTES {
        return Err(CliError::failed("connector archive exceeds the size cap"));
    }
    Ok(())
}

/// Extract one archive member by exact file name using the system `tar`
/// (bsdtar also reads zip, matching the GUI's tar.gz/zip split).
fn extract_member(archive: &Path, member: &str, target: &Path) -> Result<(), CliError> {
    let list = Command::new("tar")
        .arg("-tf")
        .arg(archive)
        .output()
        .map_err(|error| CliError::failed(format!("cannot list archive with tar: {error}")))?;
    if !list.status.success() {
        return Err(CliError::failed("cannot read connector archive"));
    }
    let listing = String::from_utf8_lossy(&list.stdout);
    let entry = listing
        .lines()
        .find(|entry| {
            Path::new(entry.trim())
                .file_name()
                .map(|name| name.to_string_lossy() == member)
                .unwrap_or(false)
        })
        .map(str::trim)
        .ok_or_else(|| CliError::failed(format!("connector archive does not contain {member}")))?;
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(target)
        .arg(entry)
        .status()
        .map_err(|error| CliError::failed(format!("cannot extract archive with tar: {error}")))?;
    if !status.success() {
        return Err(CliError::failed(format!(
            "cannot extract {member} from the connector archive"
        )));
    }
    Ok(())
}

// ─────────────────────────────── connect ───────────────────────────────

/// Headless connect flows. The GUI spawns the same commands in background
/// threads and drives the UI through `<id>:qr` / `<id>:connected` events;
/// the CLI runs them in the foreground, prints the login URL(s) (no QR
/// image render — spec: GUI-bound), and polls the connected predicate until
/// success or `--timeout`.
fn connect(kind: ConnectorKind, timeout: u64, output: OutputMode) -> Result<CliOutcome, CliError> {
    let spec = kind.spec();
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let mut notes: Vec<String> = Vec::new();
    match spec.id {
        "feishu" => {
            // Phase 1 (register app): `config init --new` until exit.
            let (url, status_ok) =
                spawn_and_capture_url(spec, &["config", "init", "--new"], &mut notes)?;
            if let Some(url) = url {
                notes.push(format!("register-url: {url}"));
            }
            if !status_ok {
                return Err(CliError::failed(
                    "feishu app registration did not complete (cancelled or timed out)",
                ));
            }
            // Phase 2 (authorize user): device-code login + polling.
            let (ok, stdout, stderr) = run_cli(
                spec,
                &["auth", "login", "--no-wait", "--json", "--recommend"],
            )?;
            if !ok {
                return Err(CliError::failed("feishu auth login did not return a link"));
            }
            let payload = parse_json(&stdout).or_else(|| parse_json(&stderr));
            let url = [
                "verification_uri_complete",
                "verification_url",
                "verificationUrl",
                "url",
            ]
            .iter()
            .find_map(|key| {
                payload
                    .as_ref()
                    .and_then(|p| p.get(*key))
                    .and_then(Value::as_str)
            })
            .map(str::to_owned)
            .ok_or_else(|| CliError::failed("feishu auth login did not return a link"))?;
            let device_code = ["device_code", "deviceCode"]
                .iter()
                .find_map(|key| {
                    payload
                        .as_ref()
                        .and_then(|p| p.get(*key))
                        .and_then(Value::as_str)
                })
                .map(str::to_owned)
                .ok_or_else(|| {
                    CliError::failed("feishu auth login did not return a device code")
                })?;
            notes.push(format!("authorize-url: {url}"));
            loop {
                if Instant::now() >= deadline {
                    return Err(CliError::failed(
                        "feishu authorization timed out before the scan completed",
                    ));
                }
                std::thread::sleep(Duration::from_secs(3));
                // This call may block until completion or return pending;
                // readiness is judged by the auth status probe either way.
                let _ = run_cli(
                    spec,
                    &["auth", "login", "--device-code", &device_code, "--json"],
                );
                if cli_connected(spec)? {
                    bundle_store_on_connected(spec.id);
                    break;
                }
            }
        }
        "wecom" => {
            let qr_dir = std::env::temp_dir().join(format!(
                "pinvou-cli-wecom-qr-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&qr_dir).map_err(|error| {
                CliError::failed(format!("cannot create the scan QR temp dir: {error}"))
            })?;
            let flow = spawn_and_capture_url(
                spec,
                &[
                    "auth",
                    "init",
                    "--noninteractive",
                    "--no-browser",
                    "--output-qrcode",
                    "qr.png",
                ],
                &mut notes,
            );
            let outcome = flow.and_then(|(url, status_ok)| {
                if let Some(url) = url {
                    notes.push(format!("authorize-url: {url}"));
                }
                let _ = std::fs::remove_dir_all(&qr_dir);
                if !status_ok || !cli_connected(spec)? {
                    return Err(CliError::failed(
                        "wecom authorization did not complete (cancelled or timed out)",
                    ));
                }
                Ok(())
            });
            outcome?;
            bundle_store_on_connected(spec.id);
        }
        "dingtalk" | "tmeet" => {
            let args: &[&str] = if spec.id == "dingtalk" {
                &["auth", "login", "--device"]
            } else {
                &["auth", "login", "--no-browser"]
            };
            let (url, _status_ok) = spawn_and_capture_url(spec, args, &mut notes)?;
            if let Some(url) = url {
                notes.push(format!("authorize-url: {url}"));
            }
            loop {
                if cli_connected(spec)? {
                    bundle_store_on_connected(spec.id);
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(CliError::failed(format!(
                        "{} authorization timed out before login completed",
                        spec.display_name
                    )));
                }
                std::thread::sleep(Duration::from_millis(400));
            }
        }
        _ => return Err(CliError::usage("unknown connector")),
    }
    notes.push(format!("{} connected", spec.id));
    let value = json!({
        "id": spec.id,
        "connected": true,
        "notes": notes,
    });
    Ok(success(render(output, notes.join("\n"), &value)))
}

/// Spawns a long-running login command, captures the first authorization URL
/// from its output (auth-domain filtered, mirror of `drain_for_url` /
/// `CliCtx::extract_url`), waits for exit and returns `(url, exit_success)`.
/// stdin is nulled like the GUI flows.
fn spawn_and_capture_url(
    spec: &VendorSpec,
    args: &[&str],
    notes: &mut Vec<String>,
) -> Result<(Option<String>, bool), CliError> {
    let mut cmd = Command::new(spec.cli_bin);
    for (key, value) in spec.envs {
        cmd.env(key, value);
    }
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|error| {
        CliError::failed(format!(
            "{} {} failed to start: {error} (install the connector CLI first: pinvou connectors ensure-cli {})",
            spec.cli_bin,
            args.join(" "),
            spec.id
        ))
    })?;
    let (tx, rx) = mpsc::channel::<String>();
    if let Some(stdout) = child.stdout.take() {
        drain_for_url(spec, stdout, tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        drain_for_url(spec, stderr, tx.clone());
    }
    drop(tx);
    let url = match rx.recv_timeout(Duration::from_secs(LOGIN_URL_TIMEOUT_SECS)) {
        Ok(url) => Some(url),
        Err(_) => {
            notes.push(format!(
                "no login link within {LOGIN_URL_TIMEOUT_SECS}s (check network / proxy); still waiting for the CLI to exit"
            ));
            None
        }
    };
    let status = child.wait().map_err(|error| {
        CliError::failed(format!("waiting for {} failed: {error}", spec.cli_bin))
    })?;
    Ok((url, status.success()))
}

/// Background pipe drainer capturing the first URL whose host matches the
/// connector's auth domains — mirror of `connector_cli::drain_for_url` and
/// `CliCtx::extract_url` (truncate at whitespace, keep query strings).
fn drain_for_url<R: std::io::Read + Send + 'static>(
    spec: &VendorSpec,
    reader: R,
    tx: mpsc::Sender<String>,
) -> std::thread::JoinHandle<()> {
    let domains = spec.auth_domains;
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            let line = match line {
                Ok(line) => line,
                Err(_) => break,
            };
            let Some(index) = line.find("https://") else {
                continue;
            };
            let url: String = line[index..]
                .chars()
                .take_while(|c| !c.is_whitespace())
                .collect();
            if domains.iter().any(|domain| url.contains(domain)) && tx.send(url).is_err() {
                break;
            }
        }
    })
}

// ─────────────────────────────── ima ───────────────────────────────

fn execute_ima(action: ImaCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    match action {
        ImaCommand::Status => {
            let entry = ima_status_entry()?;
            let human = format!(
                "ima\tconnected={}\tcredentials={}\tskill_installed={}",
                yes_no(bool_field(&entry, "connected")),
                yes_no(bool_field(&entry, "credentials_present")),
                yes_no(bool_field(&entry, "skill_installed")),
            );
            Ok(success(render(output, human, &entry)))
        }
        ImaCommand::Connect {
            client_id_env,
            api_key_env,
            api_key_stdin,
        } => ima_connect(&client_id_env, api_key_env, api_key_stdin, output),
        ImaCommand::Logout => ima_logout(output),
    }
}

/// Mirror of `features/connectors/ima.rs::status_with_store` over the pub
/// credential store + skill marketplace: connected = both credentials
/// present AND the ima-skills package installed.
fn ima_status_entry() -> Result<Value, CliError> {
    let store = SystemCredentialStore::new();
    let client_id = store
        .get(&ima_secret_ref("client_id"))
        .map_err(credential_error)?
        .filter(|value| !value.trim().is_empty());
    let api_key = store
        .get(&ima_secret_ref("api_key"))
        .map_err(credential_error)?
        .filter(|value| !value.trim().is_empty());
    let credentials_present = client_id.is_some() && api_key.is_some();
    let skill_installed = SkillMarketplaceManager::new()
        .list_skills()
        .into_iter()
        .any(|skill| skill.id == IMA_SKILL_ID && skill.installed);
    Ok(json!({
        "id": "ima",
        "connected": credentials_present && skill_installed,
        "credentials_present": credentials_present,
        "skill_installed": skill_installed,
    }))
}

fn ima_secret_ref(name: &str) -> CredentialReference {
    CredentialReference::for_ima_secret(name)
}

fn credential_error(error: impl std::fmt::Display) -> CliError {
    CliError::failed(format!("ima credential store unavailable: {error}"))
}

/// Mirror of `ima_connect`: network-validate the credentials against the ima
/// OpenAPI (check_skill_update), store both secrets, install ima-skills and
/// sync the DenyAll scopes. Network + system keyring: exercised only by
/// `#[ignore]` tests.
fn ima_connect(
    client_id_env: &str,
    api_key_env: Option<String>,
    api_key_stdin: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let client_id = std::env::var(client_id_env).map_err(|_| {
        CliError::failed(format!(
            "client id environment variable {client_id_env} is not set"
        ))
    })?;
    let api_key = resolve_secret(&api_key_env, api_key_stdin)?.ok_or_else(|| {
        CliError::usage("connectors ima connect requires --api-key-env VAR or --api-key-stdin")
    })?;
    if client_id.trim().is_empty() || api_key.trim().is_empty() {
        return Err(CliError::failed(
            "ima client id and api key must both be non-empty",
        ));
    }
    validate_ima_credentials(client_id.trim(), api_key.trim())?;

    let store = SystemCredentialStore::new();
    let client_ref = ima_secret_ref("client_id");
    let api_ref = ima_secret_ref("api_key");
    let previous_client_id = store.get(&client_ref).map_err(credential_error)?;
    let previous_api_key = store.get(&api_ref).map_err(credential_error)?;
    let result = (|| -> Result<(), CliError> {
        store
            .set(&client_ref, client_id.trim())
            .map_err(|error| credential_error(error.user_message()))?;
        store
            .set(&api_ref, api_key.trim())
            .map_err(|error| credential_error(error.user_message()))?;
        SkillMarketplaceManager::new()
            .install(IMA_SKILL_ID)
            .map_err(|error| CliError::failed(format!("ima skill install failed: {error}")))?;
        pinvou3_lib::features::marketplace::skill_scope::sync_deny_all_scopes_after_skill_install(
            IMA_SKILL_ID,
        );
        Ok(())
    })();
    if let Err(error) = result {
        // Mirror `rollback_secret`: restore the previous credential state
        // before surfacing the failure.
        match previous_client_id {
            Some(value) => {
                let _ = store.set(&client_ref, &value);
            }
            None => {
                let _ = store.delete(&client_ref);
            }
        }
        match previous_api_key {
            Some(value) => {
                let _ = store.set(&api_ref, &value);
            }
            None => {
                let _ = store.delete(&api_ref);
            }
        }
        return Err(error);
    }
    let value = json!({ "ok": true, "id": "ima", "connected": true });
    Ok(success(render(output, "ima connected".to_owned(), &value)))
}

/// Mirror of `ima.rs::validate_credentials` + `request_ima`: one POST to the
/// allowlisted `openapi/check_skill_update` path; `code == 0` means the
/// credentials are accepted.
fn validate_ima_credentials(client_id: &str, api_key: &str) -> Result<(), CliError> {
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| CliError::failed(format!("cannot build ima client: {error}")))?
        .post("https://ima.qq.com/openapi/check_skill_update")
        .timeout(Duration::from_secs(30))
        .header("ima-openapi-clientid", client_id)
        .header("ima-openapi-apikey", api_key)
        .header(
            "ima-openapi-ctx",
            format!("skill_version={IMA_SKILL_VERSION}"),
        )
        .json(&json!({ "version": IMA_SKILL_VERSION }))
        .send()
        .map_err(|error| {
            CliError::failed(format!(
                "cannot reach ima.qq.com (check network / proxy): {error}"
            ))
        })?;
    if !response.status().is_success() {
        return Err(CliError::failed(format!(
            "ima request failed with HTTP {}",
            response.status()
        )));
    }
    let payload: Value = response
        .json()
        .map_err(|_| CliError::failed("ima response is not valid JSON"))?;
    let code = payload
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(|| CliError::failed("ima validation response has no code field"))?;
    if code == 0 {
        return Ok(());
    }
    Err(CliError::failed(
        payload
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("ima OpenAPI authentication failed; check client id / api key"),
    ))
}

/// Mirror of `ima_logout`: delete both secrets, uninstall ima-skills and
/// remove it from the per-scope disabled sets.
fn ima_logout(output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = SystemCredentialStore::new();
    let client_result = store.delete(&ima_secret_ref("client_id"));
    let api_result = store.delete(&ima_secret_ref("api_key"));
    let _ = SkillMarketplaceManager::new().uninstall(IMA_SKILL_ID);
    pinvou3_lib::features::marketplace::skill_scope::remove_skill_from_disabled_scopes(
        IMA_SKILL_ID,
    );
    client_result.map_err(|error| credential_error(error.user_message()))?;
    api_result.map_err(|error| credential_error(error.user_message()))?;
    let value = json!({ "ok": true, "id": "ima", "connected": false });
    Ok(success(render(output, "ima logged out".to_owned(), &value)))
}
