//! `connectors` family: vendor CLI connector status / lifecycle, mirroring
//! `pinvou3-app/src-tauri/src/app/commands/connectors.rs` and the feature
//! functions it forwards to under `features/connectors/`.
//!
//! Visibility note: the GUI's `features::connectors` submodules are
//! `pub(crate)`, so this module cannot call them directly. Every behavior
//! below mirrors the exact GUI feature function over the public
//! `pinvou3_lib` building blocks the feature itself uses:
//! - the connector switch → the plain-scope `load_disabled_bundles_for` /
//!   `save_disabled_bundles_for` pair the GUI toggle uses, and
//!   `sync_deny_all_scopes_after_install` after a connect's skill apply. The unified scope state
//!   (`disabled_bundles.json`) is the switch on BOTH surfaces: the GUI's
//!   toggle is `set_disabled_connectors` → `apply_disabled_connectors_for`,
//!   and `features/connectors/skill_gate.rs` records that the marker's
//!   write side "was removed together with the retired `set_*_enabled`
//!   commands … so the gate only reads the flag". The CLI therefore never
//!   writes a `<id>_disabled` marker either — a marker no GUI surface can
//!   clear would pin the app into deleting the skill dirs forever. The
//!   marker survives only as a LEGACY READ: `status` reports the connector
//!   off while one exists (the app's gate really does hide the skills
//!   then), names it in the output, and `enable` removes it. That removal
//!   is the CLI's only write to the marker path;
//! - skills-visibility reads → `pinvou3_lib::platform::connector_state`
//!   (the same marker read as `ConnectorGate::is_disabled`);
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
//! - Only the SHOW direction of the skill gate is app-only: materializing
//!   the skill directories unpacks the desktop app's embedded bundle
//!   (`features::runtime_bundle`, crate-private), so the CLI recomputes
//!   visibility + scope sync and reports `skills_unpack: "app-only"`. The
//!   HIDE direction needs no bundle at all — the app's own
//!   `apply_connector_skills` hide branch (runtime_bundle/platform/
//!   extraction.rs) is a bare `remove_dir_all` of each `*_SKILL_DIRS` entry
//!   plus the connector's NOTICE file under `bundles/<id>/skills/`. The CLI
//!   performs exactly that removal itself (`hide_connector_skills`), because
//!   the GUI logout is two calls — `<id>_logout` then `<id>_apply_skills`,
//!   whose `skills_should_show()` is false right after a logout — and a CLI
//!   `logout` that skipped it would leave a fully connected-looking skill
//!   tree on disk after the credentials are gone. Unlike the app's `let _ =`
//!   the CLI reports removal failures instead of swallowing them.
//! - The execpolicy ruleset hot-refresh after connect / apply runs on the
//!   GUI's live `EnginePool`; the CLI has no engine pool and reports it.
//! - `connect` prints the login / QR URL instead of rendering the QR image
//!   (spec: QR image display is GUI-bound).
//! - `ensure-cli` downloads the lock-table-pinned archive with `reqwest`
//!   and extracts with the system `tar` (the CLI workspace has no tar/zip
//!   crates); tmeet installs through npm like the GUI does. Both lanes
//!   serialize concurrent installs through the shared
//!   `locks/connector-install.lock` (blocking wait, native-lane
//!   discipline).
//! - `status` and `ima status` read the OS keyring for the ima entry on
//!   every run — macOS may prompt, exactly as `plugins readiness`
//!   documents for its own reads; this module touches the same credential
//!   store.

use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wait_timeout::ChildExt;

use crate::support::{render, require_yes, resolve_secret, sandbox_home, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::marketplace::bundle::CLI_DISCONNECTED_DEGRADED_REASON;
use pinvou3_lib::features::marketplace::skill_marketplace::SkillMarketplaceManager;
use pinvou3_lib::features::marketplace::store::{BundleRecord, BundleSource, BundleStore};
use pinvou3_lib::platform::connector_lock::{executable_name, locked_cli_path};
use pinvou3_lib::platform::connector_skills::{
    DINGTALK_SKILL_DIRS, LARK_SKILL_DIRS, TMEET_SKILL_DIRS, WECOM_SKILL_DIRS,
};
use pinvou3_lib::platform::sha256_file;

use pinvou3_lib::platform::connector_state::skills_visible_for;
use pinvou3_lib::platform::credential_store::{
    CredentialReference, CredentialStore, SystemCredentialStore, redact_secret,
};
use pinvou3_lib::platform::paths::{
    assets_cli_dir, assets_staging_dir, bundles_root, pinvou3_home,
};

const USAGE: &str =
    "usage: pinvou connectors <status|ensure-cli|enable|disable|logout|apply-skills|connect|ima>";

/// Semantic-version floor per connector, mirroring the `*_MIN_VERSION`
/// gates in wecom.rs (1.2.1 skill baseline) and tmeet.rs (1.0.18 npm spec).
const WECOM_MIN_VERSION: (u64, u64, u64) = (1, 2, 1);
const TMEET_MIN_VERSION: (u64, u64, u64) = (1, 0, 18);
const TMEET_NPM_SPEC: &str = "@tencentcloud/tmeet@1.0.18";

/// ima skill installed by `ima_connect` (mirror of ima.rs `IMA_SKILL_ID`).
const IMA_SKILL_ID: &str = "ima-skills";
const IMA_SKILL_VERSION: &str = "1.1.8";

/// Archive size cap, mirroring `native_installer::MAX_ARCHIVE_BYTES`.
const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;

/// Per-stream byte cap for the `run_cli_bounded` output drains: the
/// deadlines bound the process lifetimes, not the bytes — a chatty or
/// hostile vendor CLI (or a descendant that inherited the pipes) must not be
/// able to balloon the CLI's memory. Bytes past the cap are read and
/// discarded, not left in the pipe; see [`drain_vendor_output`].
const MAX_VENDOR_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;

/// Per-stream byte cap for the login-output drainer (`drain_for_url`),
/// matching `run_cli_bounded`'s `MAX_VENDOR_OUTPUT_BYTES`: line processing
/// and the `Log` events (the caller's bounded failure tail and its event
/// queue) keep within the cap, while bytes past it are still read — and
/// discarded — so the vendor can never block on a full pipe.
const LOGIN_DRAIN_CAP_BYTES: u64 = 8 * 1024 * 1024;

/// Default overall connect timeout, mirroring feishu's 5-minute authorize
/// window; `connect --timeout SECS` overrides it.
const CONNECT_DEFAULT_TIMEOUT_SECS: u64 = 300;
/// Upper bound for `connect --timeout`: bounds the deadline arithmetic
/// (`Instant + Duration` would panic on absurd values) at one week — the
/// same ceiling the agent family enforces.
const CONNECT_MAX_TIMEOUT_SECS: u64 = 7 * 24 * 60 * 60;
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
            // Registrable domains, NOT the GUI's bare labels: the GUI's
            // `extract_url` matches by substring (`url.contains("feishu")`),
            // while `url_host_matches_auth_domain` below matches by host
            // suffix — under bare labels nothing can ever match
            // (open.feishu.cn does not end in "feishu"), and the phase-1
            // registration URL the connect flow depends on was silently
            // dropped.
            auth_domains: &["feishu.cn", "larksuite.com"],
            disabled_filename: "feishu_disabled",
            min_version: None,
            login_url_wait_secs: 40,
        };
        const WECOM: VendorSpec = VendorSpec {
            id: "wecom",
            display_name: "WeCom",
            cli_bin: "wecom-cli",
            envs: &[],
            auth_domains: &["work.weixin.qq.com", "weixin.qq.com"],
            disabled_filename: "wecom_disabled",
            min_version: Some(WECOM_MIN_VERSION),
            login_url_wait_secs: 40,
        };
        const DINGTALK: VendorSpec = VendorSpec {
            id: "dingtalk",
            display_name: "DingTalk",
            cli_bin: "dws",
            envs: &[],
            auth_domains: &[
                "dingtalk.com",
                "login.dingtalk.com",
                "oauth.dingtalk.com",
                "open.dingtalk.com",
            ],
            disabled_filename: "dingtalk_disabled",
            min_version: None,
            login_url_wait_secs: 60,
        };
        const TMEET: VendorSpec = VendorSpec {
            id: "tmeet",
            display_name: "Tencent Meeting",
            cli_bin: "tmeet",
            // Mirror of the GUI's `TMEET_CTX` (features/connectors/tmeet.rs),
            // which sets both envs on every tmeet invocation.
            envs: &[("TMEET_AGENT", "Pinvou"), ("TMEET_MODEL", "Pinvou")],
            auth_domains: &["meeting.tencent.com"],
            disabled_filename: "tmeet_disabled",
            min_version: Some(TMEET_MIN_VERSION),
            login_url_wait_secs: 60,
        };
        match self {
            Self::Feishu => &FEISHU,
            Self::Wecom => &WECOM,
            Self::Dingtalk => &DINGTALK,
            Self::Tmeet => &TMEET,
        }
    }

    fn skill_dirs(self) -> &'static [&'static str] {
        // The same tables the runtime bundle gate iterates, imported from the
        // app's single source of truth so the two surfaces cannot drift.
        match self {
            Self::Feishu => &LARK_SKILL_DIRS,
            Self::Wecom => &WECOM_SKILL_DIRS,
            Self::Dingtalk => &DINGTALK_SKILL_DIRS,
            Self::Tmeet => &TMEET_SKILL_DIRS,
        }
    }

    /// Provenance NOTICE file each connector's bundle unpack drops next to
    /// its skill directories — the `notice_file` argument the app passes to
    /// `Pinvou3Bundle::apply_connector_skills` (runtime_bundle/platform/
    /// extraction.rs). feishu owns the bare `NOTICE.md`; the other three
    /// carry an id suffix so four unpacks into sibling roots cannot
    /// overwrite each other. The hide direction must remove it alongside
    /// the directories or a logged-out connector keeps a stray attribution
    /// file the app would never leave behind. There is no exported constant
    /// for these on the app side (the strings are literals at the four call
    /// sites), so this table is a mirror, not an import.
    fn skills_notice_file(self) -> &'static str {
        match self {
            Self::Feishu => "NOTICE.md",
            Self::Wecom => "NOTICE-wecom.md",
            Self::Dingtalk => "NOTICE-dingtalk.md",
            Self::Tmeet => "NOTICE-tmeet.md",
        }
    }
}

struct VendorSpec {
    id: &'static str,
    display_name: &'static str,
    cli_bin: &'static str,
    envs: &'static [(&'static str, &'static str)],
    auth_domains: &'static [&'static str],
    /// Legacy `<id>_disabled` marker file name (`ConnectorGate::
    /// disabled_filename`). Read-only on both surfaces now; the CLI's only
    /// write is the removal `enable` performs to heal a stale one.
    disabled_filename: &'static str,
    /// `Some(min)` = installs below this version fail the presence gates
    /// (`cli_installed` / the GUI's `*_cli_present` count them as not
    /// installed, so they must be replaced, not used), while `status`
    /// reports them as `installed: true` + `upgrade_required: true` and
    /// still runs the status probe for them (wecom/tmeet version gates).
    min_version: Option<(u64, u64, u64)>,
    /// Per-connector wait for the first login URL, mirroring the GUI
    /// (feishu/wecom `rx.recv_timeout(40s)`, dingtalk/tmeet 60s).
    login_url_wait_secs: u64,
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
        yes: bool,
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
    Logout {
        yes: bool,
    },
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
                Some(first) => Some(ConnectorKind::parse(first)?),
            };
            if rest.len() > 1 {
                return Err(CliError::usage(
                    "connectors status accepts at most one connector id",
                ));
            }
            Ok(ConnectorsCommand::Status { connector })
        }
        "ensure-cli" | "enable" | "disable" | "apply-skills" => {
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
                _ => ConnectorsCommand::ApplySkills { connector },
            })
        }
        // Logout destroys stored credentials (wecom removes the whole
        // credential directory), so it follows the destructive-action
        // convention and requires --yes.
        "logout" => {
            let connector = require_connector(rest, subcommand)?;
            let (_, flags) = parse_flags(&rest[1..], &[], &["--yes"])?;
            Ok(ConnectorsCommand::Logout {
                connector,
                yes: flags.contains(&"--yes"),
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
                    .filter(|secs| *secs > 0 && *secs <= CONNECT_MAX_TIMEOUT_SECS)
                    .ok_or_else(|| {
                        CliError::usage(
                            "connectors connect --timeout must be between 1 and 604800 seconds",
                        )
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
                    let (_, flags) = parse_flags(rest, &[], &["--yes"])?;
                    Ok(ConnectorsCommand::Ima(ImaCommand::Logout {
                        yes: flags.contains(&"--yes"),
                    }))
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

/// Shared implementation in `support::parse_family_flags`; `family`
/// only names this family in error messages.
fn parse_flags<'a>(
    values: &'a [String],
    value_flags: &[&str],
    boolean_flags: &[&str],
) -> Result<(Vec<(&'a str, &'a str)>, Vec<&'a str>), CliError> {
    crate::support::parse_family_flags(values, value_flags, boolean_flags, "connectors")
}

fn option<'a>(options: &'a [(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    crate::support::family_option(options, name)
}

// ─────────────────────── vendor CLI subprocess helpers ───────────────────────

/// Resolves the vendor CLI executable in the GUI's per-platform order
/// (`connector_cli_program` / `windows_npm_shim`): the managed assets install
/// (`locked_cli_path` — what `ensure-cli` installs and what the GUI installs
/// both live there), the legacy managed bin dir, the npm global prefixes
/// `npm install -g` writes into (where a GUI-installed tmeet lands), and only
/// then PATH with the platform binary-name candidates. Skipping the npm
/// prefixes would make CLI-installed and GUI-installed npm CLIs invisible to
/// each other.
fn resolve_vendor_cli(spec: &VendorSpec) -> Option<PathBuf> {
    if let Some(path) = locked_cli_path(spec.cli_bin) {
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(bin_dir) = pinvou3_lib::platform::paths::managed_connector_bin_dir() {
        let candidate = if cfg!(windows) {
            bin_dir.join(pinvou3_lib::platform::connector_lock::executable_name(
                spec.cli_bin,
            ))
        } else {
            bin_dir.join(&spec.cli_bin)
        };
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    for candidate in npm_prefix_candidates(spec.cli_bin) {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let path = std::env::var_os("PATH")?;
    let candidates = crate::support::binary_candidates(spec.cli_bin);
    std::env::split_paths(&path)
        .flat_map(|dir| candidates.iter().map(move |candidate| dir.join(candidate)))
        .find(|candidate| candidate.is_file())
}

/// The npm global prefixes a GUI/CLI `npm install -g` writes binaries into,
/// in the GUI's order: `$NPM_CONFIG_PREFIX`/`npm_config_prefix` first, then
/// the platform default prefix (`~/.npm-global` on Unix, `%APPDATA%\npm` on
/// Windows), plus the GUI's `~/.local/bin` Unix candidate. Unix layout is
/// `<prefix>/bin/<program>`; Windows npm shims live at the prefix root.
fn npm_prefix_candidates(program: &str) -> Vec<PathBuf> {
    let mut prefixes: Vec<PathBuf> = Vec::new();
    for key in ["NPM_CONFIG_PREFIX", "npm_config_prefix"] {
        if let Some(prefix) = std::env::var_os(key) {
            let prefix = PathBuf::from(&prefix);
            if !prefix.as_os_str().is_empty() && !prefixes.contains(&prefix) {
                prefixes.push(prefix);
            }
        }
    }
    let default_prefix = if cfg!(windows) {
        std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join("npm"))
    } else {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".npm-global"))
    };
    if let Some(prefix) = default_prefix {
        if !prefixes.contains(&prefix) {
            prefixes.push(prefix);
        }
    }
    let mut candidates = Vec::new();
    for prefix in &prefixes {
        if cfg!(windows) {
            candidates.push(prefix.join(format!("{program}.cmd")));
        } else {
            candidates.push(prefix.join("bin").join(program));
        }
    }
    if !cfg!(windows) {
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(PathBuf::from(home).join(".local").join("bin").join(program));
        }
    }
    candidates
}

/// Mirrors the GUI's `apply_user_npm_prefix`: an `npm install -g` without an
/// explicit prefix writes into the global npm prefix (`/usr/local` on stock
/// macOS), which fails without sudo. Default the prefix to the user-writable
/// `~/.npm-global` (Unix) / `%APPDATA%\npm` (Windows, with a local npm cache)
/// and prepend the directory npm actually puts executables in to the child's
/// PATH.
///
/// The two platform arms mirror two different GUI functions, and they differ
/// on purpose:
/// - unix (`platform/os/posix.rs::apply_user_npm_prefix`): when the caller
///   already exported `NPM_CONFIG_PREFIX` / `npm_config_prefix` the GUI does
///   nothing at all — the user picked that prefix, and rewriting PATH around
///   it is not ours to do. The prefix's `bin` (not the prefix itself) is what
///   holds the executables in npm's unix layout, so that is the entry
///   prepended.
/// - windows (`platform/os/windows/windows_path.rs::apply_user_npm_prefix`):
///   npm shims live at the prefix root, so the prefix itself goes on PATH,
///   and the env prefix only suppresses the env WRITE, not the PATH entry.
fn apply_user_npm_prefix(cmd: &mut std::process::Command) {
    let prefix_from_env = ["NPM_CONFIG_PREFIX", "npm_config_prefix"]
        .into_iter()
        .find_map(std::env::var_os)
        .filter(|value| !value.is_empty());
    if !cfg!(windows) && prefix_from_env.is_some() {
        return;
    }
    let prefix = prefix_from_env.clone().map(PathBuf::from).or_else(|| {
        if cfg!(windows) {
            std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join("npm"))
        } else {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".npm-global"))
        }
    });
    let Some(prefix) = prefix else {
        return;
    };
    // The GUI creates the exact directory it then puts on PATH (posix creates
    // `<prefix>/bin`), so a first install does not hand the child a PATH entry
    // that does not exist yet.
    let path_entry = if cfg!(windows) {
        prefix.clone()
    } else {
        prefix.join("bin")
    };
    let _ = std::fs::create_dir_all(&path_entry);
    if prefix_from_env.is_none() {
        cmd.env("NPM_CONFIG_PREFIX", &prefix)
            .env("npm_config_prefix", &prefix);
    }
    if cfg!(windows) {
        // The GUI also keeps npm's cache out of the shared default on
        // Windows (npm otherwise writes to the roaming profile).
        if std::env::var_os("NPM_CONFIG_CACHE").is_none()
            && std::env::var_os("npm_config_cache").is_none()
        {
            if let Some(cache) = std::env::var_os("LOCALAPPDATA") {
                let cache = PathBuf::from(cache).join("pinvou3").join("npm-cache");
                let _ = std::fs::create_dir_all(&cache);
                cmd.env("NPM_CONFIG_CACHE", &cache)
                    .env("npm_config_cache", &cache);
            }
        }
    }
    let mut paths = vec![path_entry];
    if let Some(current) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&current));
    }
    if let Ok(joined) = std::env::join_paths(&paths) {
        cmd.env("PATH", joined);
    }
}

/// Upper bound for one status/version probe. The GUI does bound its probes
/// too — `run_probe` wraps every one in
/// `output_with_timeout_and_kill_tree(cmd, 30s)` (features/connectors/
/// connector_cli.rs), the same class of protection against a hung npm-shim
/// CLI. The CLI deliberately allows twice that here (no UI to keep alive,
/// and the same history of shim hang-ups to survive); what stays
/// non-negotiable on both surfaces is that a hung vendor CLI must not hang
/// the command forever.
const PROBE_TIMEOUT_SECS: u64 = 60;

/// Runs `<cli> <args>` capturing `(success, stdout, stderr)` — mirror of
/// `connector_cli::run` (adds the connector envs from the spec), bounded so
/// every blocking vendor-CLI phase honors a timeout.
fn run_cli(spec: &VendorSpec, args: &[&str]) -> Result<(bool, String, String), CliError> {
    run_cli_bounded(
        spec,
        args,
        Instant::now() + Duration::from_secs(PROBE_TIMEOUT_SECS),
    )
}

/// Best-effort argv redaction for timeout errors: exact-value strip of the
/// values of pairing flags the vendor CLIs pass (`--device-code`), before
/// the heuristic `redact_secret` pass — a short device code survives the
/// heuristic otherwise (the same exact-value-first pattern the ima errors
/// use).
fn redact_argv_pairing(args: &[&str]) -> String {
    let mut joined = args.join(" ");
    let mut index = 0;
    while index + 1 < args.len() {
        if args[index] == "--device-code" && !args[index + 1].is_empty() {
            joined = joined.replace(args[index + 1], "[REDACTED]");
        }
        index += 1;
    }
    pinvou3_lib::platform::credential_store::redact_secret(&joined)
}

/// Runs `<cli> <args>` capturing `(success, stdout, stderr)` — like
/// [`run_cli`], with a hard kill at `deadline`: the GUI cancels through its
/// host, so the CLI must enforce the `--timeout` budget itself on every
/// blocking phase.
fn run_cli_bounded(
    spec: &VendorSpec,
    args: &[&str],
    deadline: Instant,
) -> Result<(bool, String, String), CliError> {
    let Some(executable) = resolve_vendor_cli(spec) else {
        return Err(CliError::failed(format!(
            "{} was not found (managed install or PATH); install it first: pinvou connectors \
             ensure-cli {}",
            spec.cli_bin, spec.id
        )));
    };
    // `build_command` already carries `args`; `Command::args` appends, so a
    // second call here would double the argv and break every vendor parser.
    let mut cmd = crate::support::build_command(&executable, args);
    for (key, value) in spec.envs {
        cmd.env(key, value);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::support::set_process_group(&mut cmd);
    // `spawn_supervised` is spawn + registration with the interrupt signals
    // blocked across the two: a SIGINT landing between a bare pair would
    // forward to nothing and re-raise with this child absent from the
    // registry — the orphan the bracket exists to prevent. The same kill
    // guard `code login` uses: this lane's children (login/connect/probe)
    // run for minutes-to-hours, and the registration is what a terminal
    // SIGINT uses to take them down with the CLI. Paired with a forget on
    // every exit below, so the pgid is never left registered for the OS to
    // recycle onto an unrelated process.
    let mut child = crate::support::supervise::spawn_supervised(&mut cmd).map_err(|error| {
        CliError::failed(format!(
            "{} could not be executed: {error} (install it first: pinvou connectors ensure-cli {})",
            spec.cli_bin, spec.id
        ))
    })?;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    // Lossy like the GUI's `String::from_utf8_lossy`: vendor CLIs (wecom /
    // dingtalk / tmeet on Windows especially) can emit non-UTF-8 output, and
    // a strict `read_to_string` errors on the first bad byte and discards
    // the ENTIRE stream — status reads then lie "not connected" and logout
    // claims success without running. Read bytes and decode lossily. The
    // results come back through channels so the drain can be bounded (see
    // below) instead of joined unconditionally.
    let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
    // Size-bound KEEP, unbounded READ: the bytes past the cap are read and
    // discarded rather than left in the pipe. A `take(cap)` that simply stops
    // reading returns EOF to the drainer while the child keeps writing, so
    // the pipe fills (64 KiB), the child blocks in write(2), the deadline
    // below expires, and a healthy child is misreported as "timed out" and
    // group-SIGKILLed. Same read-and-discard-to-EOF discipline as voice.rs's
    // `drain_capped` (engine capture) and code.rs's `read_capped_to_eof`
    // (git output). Normal output is far below the cap, so behavior for it
    // is unchanged.
    std::thread::spawn(move || {
        let _ = stdout_tx.send(drain_vendor_output(stdout_pipe));
    });
    std::thread::spawn(move || {
        let _ = stderr_tx.send(drain_vendor_output(stderr_pipe));
    });
    let remaining = deadline.saturating_duration_since(Instant::now());
    let status = match child.wait_timeout(remaining) {
        Ok(Some(status)) => status,
        Ok(None) => {
            // The CLI was spawned as a process-group leader, so the group kill
            // takes its npm/shell/node descendants with it instead of
            // orphaning them.
            crate::support::kill_process_tree(&mut child);
            crate::support::supervise::forget_child_group(child.id());
            return Err(CliError::failed(format!(
                "{} {} timed out",
                spec.cli_bin,
                // args can carry pairing/device material; the shared
                // redactor only catches secret-shaped tokens, so pairing
                // flag values are stripped by exact value first. One known
                // exception passes a code in argv on purpose: the feishu
                // poll's `--device-code <CODE>` (GUI parity, feishu.rs) —
                // short-lived single-use material inherited from the GUI
                // flow, redacted best-effort here.
                redact_argv_pairing(args)
            )));
        }
        Err(error) => {
            // An OS-level wait error leaves the child's fate unknown — it may
            // still be running — so it goes down with the group before the
            // registration is released (same convention as `run_npm_install`
            // below). The child was never reaped by us, so the kill cannot
            // race a recycled pid.
            crate::support::kill_process_tree(&mut child);
            crate::support::supervise::forget_child_group(child.id());
            return Err(CliError::failed(format!(
                "waiting for {} failed: {error}",
                spec.cli_bin
            )));
        }
    };
    // The child is reaped by the wait above, so an interrupt from here on
    // must not signal a group the OS may have already recycled: release the
    // registration BEFORE the drain grace, which can hold for up to 2×5s.
    crate::support::supervise::forget_child_group(child.id());
    // A descendant that inherited the write end can keep EOF away forever —
    // the deadline above only bounds the direct child. Bound the drain like
    // `code`/`voice`: give the pipes a short grace to deliver EOF, then
    // proceed with the bytes that arrived. A straggler is deliberately left
    // alone — the child is already reaped here, so killing its process group
    // would race a reused pid (the timeout branch above is the only place a
    // group kill is safe).
    const DRAIN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);
    let drain = |rx: std::sync::mpsc::Receiver<String>| -> String {
        rx.recv_timeout(DRAIN_GRACE).unwrap_or_default()
    };
    let stdout = drain(stdout_rx);
    let stderr = drain(stderr_rx);
    Ok((status.success(), stdout, stderr))
}

/// Drains one `run_cli_bounded` output pipe: KEEP at most
/// [`MAX_VENDOR_OUTPUT_BYTES`] and decode lossily (the non-UTF-8 hazards the
/// caller documents), but keep reading — and DISCARDING — until true EOF, so
/// a child that outproduces the cap can still finish instead of blocking on
/// a full pipe. Same loop as voice.rs's `drain_capped` and code.rs's
/// `read_capped_to_eof`.
fn drain_vendor_output<R: std::io::Read>(pipe: Option<R>) -> String {
    let mut kept: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let Some(mut pipe) = pipe else {
        return String::new();
    };
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                let room = MAX_VENDOR_OUTPUT_BYTES.saturating_sub(kept.len() as u64) as usize;
                if room > 0 {
                    kept.extend_from_slice(&chunk[..read.min(room)]);
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
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
    // Try stdout first, then stderr (noisy stdout must not hide a stderr
    // version — the GUI parses both streams in order).
    semver_from_stream(spec, stdout).or_else(|| semver_from_stream(spec, stderr))
}

fn semver_from_stream(spec: &VendorSpec, source: &str) -> Option<(u64, u64, u64)> {
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

/// `(semver if parseable, first non-empty version line)` from one
/// `--version` invocation. Presence is judged by the exit status only (the
/// GUI's `*_cli_present`); a version line without a parseable number means
/// "installed, version unknown" for the connectors that do not gate on one.
fn probe_cli_version(spec: &VendorSpec) -> Option<(Option<(u64, u64, u64)>, String)> {
    let (ok, stdout, stderr) = run_cli(spec, &["--version"]).ok()?;
    if !ok {
        return None;
    }
    let semver = cli_semver(spec, &stdout, &stderr);
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

/// Installation gate per connector, mirroring `*_cli_present` plus the
/// wecom/tmeet minimum-version replacement gate (status / ensure-cli / the
/// connect-time presence checks; `logout` judges through `logout_probe`
/// instead, whose not-installed claim must survive the stricter verdict):
/// - feishu/dingtalk count any working `--version` (an unparseable version
///   line must not turn an installed CLI into a reinstall loop);
/// - wecom/tmeet also require the minimum version (older or unparseable
///   installs must be replaced, not used).
enum VersionGate {
    Missing,
    Upgrade { raw: String },
    Usable { raw: String },
}

fn version_gate(spec: &VendorSpec) -> VersionGate {
    match probe_cli_version(spec) {
        None => VersionGate::Missing,
        Some((semver, raw)) => match (spec.min_version, semver) {
            (Some(min), Some(found)) if found < min => VersionGate::Upgrade { raw },
            (Some(_), None) => VersionGate::Missing,
            _ => VersionGate::Usable { raw },
        },
    }
}

fn cli_installed(spec: &VendorSpec) -> bool {
    matches!(version_gate(spec), VersionGate::Usable { .. })
}

/// Logout gate for the dingtalk/tmeet arm, mirror of the GUI's
/// `logout_probe_verdict` (`connector_cli.rs`): only a CLI that cannot be
/// resolved at all may claim "not installed" and skip the real logout; a CLI
/// that exists but answers `--version` with a failure or an unparseable
/// version (tmeet, whose install gate is version-based) leaves the token
/// state UNCONFIRMED. Skipping `auth logout` in that state would report a
/// clean logout while the vendor token stays on disk — the exact hazard the
/// GUI folded into its verdict function.
enum LogoutGate {
    NotInstalled,
    Unconfirmed(String),
    Installed,
}

fn logout_probe(spec: &VendorSpec, deadline: Instant) -> LogoutGate {
    if resolve_vendor_cli(spec).is_none() {
        return LogoutGate::NotInstalled;
    }
    let (ok, stdout, stderr) = match run_cli_bounded(spec, &["--version"], deadline) {
        Ok(result) => result,
        Err(error) => {
            return LogoutGate::Unconfirmed(format!(
                "{} CLI is installed but the version probe failed ({error}); the login state is \
                 unconfirmed, so the stored connection was not changed",
                spec.display_name
            ));
        }
    };
    if !ok {
        return LogoutGate::Unconfirmed(format!(
            "{} CLI is installed but `--version` failed; the login state is unconfirmed, so the \
             stored connection was not changed",
            spec.display_name
        ));
    }
    // tmeet additionally requires a parseable version (its GUI probe folds
    // an unparseable version into the same Unconfirmed verdict); dingtalk is
    // version-ungated presence.
    if spec.id == "tmeet" && cli_semver(spec, &stdout, &stderr).is_none() {
        return LogoutGate::Unconfirmed(format!(
            "{} CLI is installed but its version could not be parsed; the login state is \
             unconfirmed, so the stored connection was not changed",
            spec.display_name
        ));
    }
    LogoutGate::Installed
}

/// Runs the connector's status subcommand and returns `(exit_ok, stdout,
/// stderr)` — one spawn shared by the `ok` DTO field and the connected
/// predicate.
fn run_status_probe(spec: &VendorSpec) -> Result<(bool, String, String), CliError> {
    run_status_probe_bounded(
        spec,
        Instant::now() + Duration::from_secs(PROBE_TIMEOUT_SECS),
    )
}

/// The status probe under a caller's deadline: the connect poll loops reuse
/// it so their final tick cannot add a fresh `PROBE_TIMEOUT_SECS` on top of
/// the connect budget a wrapper scripted (`connect --timeout 30` used to be
/// able to observe ~90 s).
fn run_status_probe_bounded(
    spec: &VendorSpec,
    deadline: Instant,
) -> Result<(bool, String, String), CliError> {
    match spec.id {
        "feishu" => run_cli_bounded(spec, &["auth", "status", "--json"], deadline),
        "wecom" => run_cli_bounded(spec, &["auth", "show", "--status"], deadline),
        "dingtalk" => run_cli_bounded(spec, &["auth", "status", "--format", "json"], deadline),
        "tmeet" => run_cli_bounded(spec, &["auth", "status"], deadline),
        _ => Ok((false, String::new(), String::new())),
    }
}

/// [`cli_connected`] under the caller's budget: the probe still gets its own
/// full window when the budget allows, but never runs past `deadline`.
fn cli_connected_bounded(spec: &VendorSpec, deadline: Instant) -> bool {
    run_status_probe_bounded(spec, deadline)
        .map(|(ok, stdout, stderr)| connected_from_probe(spec, ok, &stdout, &stderr))
        .unwrap_or(false)
}

/// Per-connector connected predicate over the status subcommand output,
/// mirroring the feature files:
/// - feishu: JSON `identities/user/status == "ready"`;
/// - wecom: exit ok and the whole line `authorized` (case-insensitive);
/// - dingtalk: JSON `authenticated: true`;
/// - tmeet: output contains `Logged in`.
fn cli_connected(spec: &VendorSpec) -> Result<bool, CliError> {
    let (ok, stdout, stderr) = run_status_probe(spec)?;
    Ok(connected_from_probe(spec, ok, &stdout, &stderr))
}

/// The per-connector `connected` verdict over one status-probe capture —
/// the single authority both `status` entries and the connect/login gates
/// read, so the four vendor branches cannot drift apart.
fn connected_from_probe(spec: &VendorSpec, ok: bool, stdout: &str, stderr: &str) -> bool {
    match spec.id {
        "feishu" => parse_json(stdout)
            .or_else(|| parse_json(stderr))
            .and_then(|value| {
                value
                    .pointer("/identities/user/status")
                    .and_then(Value::as_str)
                    .map(|status| status == "ready")
            })
            .unwrap_or(false),
        "wecom" => ok && (authorized_line(stdout) || authorized_line(stderr)),
        "dingtalk" => authenticated_json(stdout) || authenticated_json(stderr),
        "tmeet" => stdout.contains("Logged in") || stderr.contains("Logged in"),
        _ => false,
    }
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

/// Legacy disabled-marker read via the app's own platform helper (marker
/// file `<id>_disabled` under `PINVOU3_HOME`), same as
/// `ConnectorGate::is_disabled` / `platform::connector_state`.
///
/// "Legacy" because nothing writes this file any more: `skill_gate.rs`
/// records that the write side was retired together with the
/// `set_*_enabled` commands, and there is no GUI surface that can create OR
/// clear it. A marker left behind by an older build still makes the app's
/// gate hide the connector's skills forever, so the CLI must keep READING
/// it (`status` would otherwise claim a connector is on while the app keeps
/// deleting its skill dirs) and offers `enable` as the one way to clear it.
fn legacy_disabled_marker(kind: ConnectorKind) -> bool {
    !skills_visible_for(kind.as_str())
}

/// The connector switch as the GUI persists it: the unified scope state
/// (`disabled_bundles.json`, plain scope) the GUI's `set_disabled_connectors`
/// → `apply_disabled_connectors_for` writes and `set_enabled` below writes the
/// same way.
///
/// Read through the app's own `load_disabled_bundles` (plain scope) rather
/// than off the raw file: it applies the same read-time package-id
/// normalization the GUI sees, and it is deliberately the PLAIN scope only —
/// `sync_deny_all_scopes_after_install` adds a freshly connected connector to
/// the *code* scope by design ("code sessions default external capabilities
/// off"), so an any-scope read would report a perfectly enabled connector as
/// switched off right after `apply-skills`.
fn scope_state_disabled(kind: ConnectorKind) -> bool {
    let package_id = pinvou3_lib::features::marketplace::scope::package_id_for(kind.as_str());
    pinvou3_lib::features::marketplace::load_disabled_bundles()
        .iter()
        .any(|id| id == &package_id || id == kind.as_str())
}

/// The on-disk skill gate, mirroring `features/connectors/skill_gate.rs::
/// ConnectorGate::skills_should_show` exactly: `!legacy_marker && ready_probe`
/// — the LEGACY `<id>_disabled` marker plus the connection state. The
/// plain-scope switch state is NOT part of the gate: the GUI's toggle
/// (`set_disabled_connectors`) governs the composite materialization and tool
/// gating and never deletes `bundles/<id>/skills`, so the CLI's hide
/// direction must not fire on the switch alone either — only the marker or
/// the absence of a connection does. A probe error counts as not-connected,
/// exactly like the GUI's `ready_probe` folds `run_probe` errors to false
/// (feishu.rs `is_user_ready`, wecom.rs `is_ready`, …).
fn gui_skill_gate_shows(kind: ConnectorKind) -> bool {
    !legacy_disabled_marker(kind) && cli_connected(kind.spec()).unwrap_or(false)
}

/// Removes a legacy `<id>_disabled` marker if one is present. This is the
/// CLI's ONLY write to that path and it only ever removes: `enable` heals a
/// marker an older CLI build (or a hand-edited home) left behind, which no
/// GUI action can do. Idempotent; a failed removal propagates, because
/// silently keeping the marker would leave `enable` reporting a switch the
/// app will not honor.
fn clear_legacy_disabled_marker(spec: &VendorSpec) -> Result<(), CliError> {
    let path = pinvou3_home().join(spec.disabled_filename);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CliError::failed(format!(
            "connectors {}: cannot clear the legacy disabled marker {}: {error}",
            spec.id,
            path.display()
        ))),
    }
}

/// Skills-applied state, mirroring `cached_*_skills_visible`: the connector
/// is not switched off AND every bundle skill directory carries its SKILL.md
/// under `~/.pinvou3/bundles/<id>/skills/`. `disabled` is threaded in by the
/// caller because resolving the scope state touches the filesystem and the
/// status path already knows the answer.
fn skills_applied(kind: ConnectorKind, disabled: bool) -> bool {
    if disabled {
        return false;
    }
    let target = connector_skills_dir(kind);
    kind.skill_dirs()
        .iter()
        .all(|dir| target.join(dir).join("SKILL.md").is_file())
}

/// `~/.pinvou3/bundles/<id>/skills` — the app's
/// `Pinvou3Bundle::connector_package_skills_dir`, the single root both the
/// show (app-only) and hide (CLI-capable) directions operate on.
fn connector_skills_dir(kind: ConnectorKind) -> PathBuf {
    bundles_root().join(kind.as_str()).join("skills")
}

/// The HIDE direction of the app's `apply_connector_skills`, performed
/// headlessly: remove every `*_SKILL_DIRS` entry and the connector's NOTICE
/// file under `bundles/<id>/skills/`. Idempotent (removing what is not there
/// is not an error), exactly like the app's branch.
///
/// Deviation from the app, on purpose and for the same reason as the wecom
/// credential-directory removal in `logout`: the app writes `let _ =` on
/// every removal, but the CLI's caller reports a logout as complete, and a
/// skill tree that survived the logout is precisely the state the user must
/// be told about. Every failure is collected (one unremovable directory must
/// not hide the next) and reported together.
fn hide_connector_skills(kind: ConnectorKind) -> Result<(), CliError> {
    let target = connector_skills_dir(kind);
    let mut failures: Vec<String> = Vec::new();
    for dir in kind.skill_dirs() {
        let path = target.join(dir);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => failures.push(format!("{}: {error}", path.display())),
        }
    }
    let notice = target.join(kind.skills_notice_file());
    match std::fs::remove_file(&notice) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => failures.push(format!("{}: {error}", notice.display())),
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(CliError::failed(format!(
            "connectors {}: the companion skill files could not be removed ({}); the skills \
             stay visible to the engine until they are gone",
            kind.as_str(),
            failures.join("; ")
        )))
    }
}

/// Mirror of `connector_cli::bundle_store_on_disconnected`: logout marks the
/// marketplace record degraded (record stays installed; reconnect repairs).
/// The reason text is the marketplace module's published
/// `CLI_DISCONNECTED_DEGRADED_REASON` — the single source both write sides
/// (desktop `bundle_store_on_disconnected`, this mirror) import, so
/// GUI-rendered store data stays identical regardless of which surface
/// wrote it.
fn bundle_store_on_disconnected(id: &str) {
    // Mirror-write failures never fail the main operation (GUI parity), but
    // they are disclosed: the module's own rule is "headless deviations are
    // disclosed in command output, not silent", and a silent mirror failure
    // leaves a store record that still reads connected.
    if let Err(error) = BundleStore::new().mark_degraded(id, CLI_DISCONNECTED_DEGRADED_REASON) {
        note!(
            "pinvou: warning: connectors logout: the marketplace record could not be marked degraded: {error}"
        );
    }
}

/// Mirror of `connector_cli::bundle_store_on_connected`: register the CLI
/// package and clear `degraded` (reconnect repairs the record). Mirror-write
/// failures never fail the main operation (the GUI only logs them).
fn bundle_store_on_connected(id: &str) {
    // Byte-for-byte the app's own write: `installed_now` with
    // `source=Builtin` and NO asset pins. The GUI never writes `assets` on a
    // connect — pins only exist via the first-boot import path
    // (`store::legacy_cli_records`), which reads the real files on disk
    // rather than trusting a record — and `upsert_preserving` keeps an
    // existing record's `assets` verbatim, so pins written here would (a)
    // create first-connect records the GUI would never create and (b) be
    // silently dropped on every reconnect anyway. Content integrity on this
    // lane is the download's own SHA-256 verification, same as the GUI.
    let record = BundleRecord::installed_now(id, BundleSource::Builtin);
    // Same disclose-don't-swallow rule as the disconnected mirror above.
    if let Err(error) = BundleStore::new().upsert_preserving(record) {
        note!(
            "pinvou: warning: connectors connect: the marketplace record could not be updated: {error}"
        );
    }
}

// ─────────────────────────────── execute ───────────────────────────────

pub fn execute(command: ConnectorsCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    // Connector state (bundle mirror, disabled markers, install locks/logs)
    // lives under the product data root; a relative PINVOU3_HOME would
    // silently resolve against the cwd.
    crate::support::sandbox_home()?;
    match command {
        ConnectorsCommand::Status { connector } => status(connector, output),
        ConnectorsCommand::EnsureCli { connector } => ensure_cli(connector, output),
        ConnectorsCommand::Enable { connector } => set_enabled(connector, true, output),
        ConnectorsCommand::Disable { connector } => set_enabled(connector, false, output),
        ConnectorsCommand::Logout { connector, yes } => logout(connector, yes, output),
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
    // Each entry costs two vendor spawns (`--version` + the status probe),
    // each with its own `PROBE_TIMEOUT_SECS` ceiling. Run sequentially, one
    // hung npm shim stalls the whole overview for minutes while the other
    // three connectors sit idle; the GUI avoids that by giving every
    // connector its own `spawn_blocking`. Scoped threads give the same
    // overlap without a new dependency and without moving `kind` into a
    // 'static closure. Results are joined in the input order, so the output
    // stays byte-for-byte deterministic regardless of which probe finishes
    // first, and each entry keeps its own degrade-to-`note` behavior below.
    let mut entries: Vec<Value> = std::thread::scope(|scope| {
        let handles: Vec<_> = kinds
            .iter()
            .map(|kind| {
                let kind = *kind;
                scope.spawn(move || vendor_status_entry(kind))
            })
            .collect();
        kinds
            .iter()
            .zip(handles)
            .map(|(kind, handle)| {
                // One broken vendor CLI (a corrupt shim, a probe timeout)
                // must not fail the whole overview and discard the other
                // entries: degrade it to a note like the ima entry below. A
                // panicking probe thread degrades the same way instead of
                // resuming the unwind through the whole command.
                let note = match handle.join() {
                    Ok(Ok(entry)) => return entry,
                    Ok(Err(error)) => error.to_string(),
                    Err(_) => "the status probe panicked".to_owned(),
                };
                // A panicked probe knows NOTHING about this connector — not
                // even whether it is installed — so the degrade must not
                // fabricate `installed/connected: false`: that is exactly
                // the "hide a wedged shim behind an install recommendation"
                // outcome the Missing arm and `degrade_probe_failure`
                // refuse. The fields stay absent for a JSON consumer (an
                // absent field is unknown, not false) and the note carries
                // the reason; the human row's bool cells render absent as
                // "no" (the renderer has no unknown state), which the note
                // printed alongside disambiguates.
                json!({
                    "id": kind.spec().id,
                    "ok": false,
                    "note": note,
                    "degraded": "panic",
                })
            })
            .collect::<Vec<_>>()
    });
    // ima is part of the default overview only; a filtered `status <id>`
    // must not report unrelated connectors. A credential-store failure must
    // degrade the ima entry, not fail the whole overview — the four vendor
    // entries were computed fine and the GUI surfaces per-connector too.
    if connector.is_none() {
        match ima_status_entry() {
            Ok(entry) => entries.push(entry),
            Err(error) => entries.push(json!({
                "id": "ima",
                "ok": false,
                // Unknown is not false: a credential-store failure says
                // nothing about connectivity, stored credentials, or skill
                // installation — fabricating `false` here is the exact
                // "unknown rendered as a clean-looking row" outcome the
                // vendor panic arm refuses. The fields stay null for a JSON
                // consumer (null is not false); the human row renders them
                // "no" with this note alongside, and the dedicated
                // `connectors ima status` still fails loudly with the real
                // error.
                "connected": Value::Null,
                "credentials_present": Value::Null,
                "skill_installed": Value::Null,
                "note": error.to_string(),
                "degraded": "credential_store",
            })),
        }
    }

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
                if let Some(note) = entry.get("note").and_then(Value::as_str) {
                    line.push_str(&format!("\t({note})"));
                }
            } else {
                let installed = if bool_field(entry, "installed") {
                    match entry.get("version").and_then(Value::as_str) {
                        // The version string comes straight from the vendor
                        // CLI's `--version` output; a control character in it
                        // would break the tab-separated column structure of
                        // this row (same reason the sessions/projects
                        // families collapse their vendor- and user-supplied
                        // cells). JSON keeps the raw string.
                        Some(version) => format!(
                            "yes({})",
                            crate::support::collapse_control_characters(version)
                        ),
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
                // A legacy marker is the one "disabled" reason no GUI surface
                // can explain or undo, so it is named separately from the
                // plain `enabled=no` and carries its own remedy.
                if bool_field(entry, "legacy_disabled_marker") {
                    let id = entry["id"].as_str().unwrap_or("-");
                    line.push_str(&format!(
                        "\tlegacy_disabled_marker=yes (run `pinvou connectors enable {id}` to clear it)"
                    ));
                }
                if let Some(note) = entry.get("note").and_then(Value::as_str) {
                    line.push_str(&format!("\t({note})"));
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
/// plus the switch and skills-applied state the composer reads via the
/// unified scope state and the bundle skill directories.
///
/// `enabled` is the truthful answer, not just the marker read: a connector
/// the user switched off in the GUI lives in `disabled_bundles.json` and
/// never had a marker written for it, while a legacy marker left by an older
/// build hides the skills just as effectively. Both are reported, and the
/// marker additionally surfaces under its own key so the user can tell the
/// two apart (only `pinvou connectors enable <id>` clears the marker).
fn vendor_status_entry(kind: ConnectorKind) -> Result<Value, CliError> {
    let spec = kind.spec();
    let legacy_marker = legacy_disabled_marker(kind);
    let disabled = scope_state_disabled(kind) || legacy_marker;
    let mut entry = json!({
        "id": spec.id,
        "enabled": !disabled,
        "legacy_disabled_marker": legacy_marker,
        "skills_applied": skills_applied(kind, disabled),
    });
    match version_gate(spec) {
        VersionGate::Missing => {
            entry["ok"] = json!(false);
            entry["connected"] = json!(false);
            entry["installed"] = json!(false);
            // Distinguish the two ways a probe can report Missing: no
            // binary anywhere (a genuine not-installed), or a resolvable
            // binary whose `--version` spawn failed or timed out. Silently
            // rendering the second as `installed: false` disagrees with the
            // status-lane note below and hides a wedged shim behind an
            // install recommendation; the note keeps the verdict (and the
            // remedy) while naming the uncertainty.
            if resolve_vendor_cli(spec).is_some() {
                entry["probe"] = json!(format!(
                    "unanswerable: the CLI is present but its --version probe failed or timed \
                     out; `pinvou connectors ensure-cli {}` re-checks the install",
                    spec.id
                ));
                // The human row renders only `note` (same channel as
                // `degrade_probe_failure` below), so the verdict and its
                // remedy must land there too — stderr is invisible to a
                // `--output json`-style consumer of the human table.
                entry["note"] = json!(format!(
                    "the CLI is present but its --version probe failed or timed out \
                     (`pinvou connectors ensure-cli {}` re-checks the install)",
                    spec.id
                ));
                note!(
                    "[connectors] {} is present on disk or PATH, but its --version probe \
                     failed or timed out — reporting it as not installed would be a guess; \
                     run `pinvou connectors ensure-cli {}` to re-check",
                    spec.cli_bin,
                    spec.id
                );
            }
            // The GUI's uninstalled feishu DTO reports `configured: false`
            // rather than omitting the field (feishu.rs); JSON consumers
            // read booleans, not absent keys.
            if spec.id == "feishu" {
                entry["configured"] = json!(false);
            }
            if spec.min_version.is_some() {
                entry["upgrade_required"] = json!(false);
            }
        }
        VersionGate::Upgrade { raw } => {
            // Installed but below the command-model baseline. The status
            // probe still runs here, GUI parity: `wecom_status` /
            // `tmeet_status` gate on `*_cli_version` (parse gate, not the
            // minimum), so an overly old install gets the same `auth …`
            // spawn as a usable one and reports `connected = ok &&
            // authorized` / "Logged in" from the live probe. A pre-fix hard
            // `connected:false, ok:false` lied "logged out" for a credential
            // the vendor still holds. `upgrade_required: true` is the
            // CLI-only extra the GUI handles through ensure_cli's version
            // gate instead.
            entry["installed"] = json!(true);
            entry["upgrade_required"] = json!(true);
            entry["version"] = json!(raw);
            let (ok, stdout, stderr) = match run_status_probe(spec) {
                Ok(result) => result,
                Err(error) => {
                    degrade_probe_failure(spec, &mut entry, &error);
                    return Ok(entry);
                }
            };
            let connected = connected_from_probe(spec, ok, &stdout, &stderr);
            entry["ok"] = json!(ok);
            entry["connected"] = json!(connected);
        }
        VersionGate::Usable { raw } => {
            entry["installed"] = json!(true);
            entry["upgrade_required"] = json!(false);
            entry["version"] = json!(raw);
            let (ok, stdout, stderr) = match run_status_probe(spec) {
                Ok(result) => result,
                Err(error) => {
                    degrade_probe_failure(spec, &mut entry, &error);
                    return Ok(entry);
                }
            };
            let connected = connected_from_probe(spec, ok, &stdout, &stderr);
            if spec.id == "feishu" {
                // Mirror `feishu_status`'s extra `configured` flag (non-empty
                // appId in the auth status payload).
                let configured = parse_json(&stdout)
                    .or_else(|| parse_json(&stderr))
                    .as_ref()
                    .and_then(|value| value.get("appId"))
                    .and_then(Value::as_str)
                    .map(|app_id| !app_id.is_empty())
                    .unwrap_or(false);
                entry["configured"] = json!(configured);
            }
            entry["ok"] = json!(ok);
            entry["connected"] = json!(connected);
        }
    }
    Ok(entry)
}

/// Degrades a failed status probe on an entry whose version gate already
/// answered: the connector IS installed and its version is known, so the
/// entry keeps those facts and carries the failure as a `note` (rendered on
/// the human row too) instead of letting the caller's degrade path
/// fabricate `installed: false` with the reason visible only in JSON — the
/// same "hide a wedged shim behind an install recommendation" outcome the
/// `Missing` arm above refuses to produce.
fn degrade_probe_failure(spec: &VendorSpec, entry: &mut Value, error: &CliError) {
    entry["ok"] = json!(false);
    entry["connected"] = json!(false);
    entry["note"] = json!(format!(
        "status probe failed: {error} (`pinvou connectors ensure-cli {}` re-checks the install)",
        spec.id
    ));
    note!(
        "[connectors] {} is installed (its --version probe answered), but its status probe \
         failed: {error} — reporting connected=no with the reason; \
         run `pinvou connectors ensure-cli {}` to re-check",
        spec.cli_bin,
        spec.id
    );
}

fn set_enabled(
    kind: ConnectorKind,
    enabled: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let spec = kind.spec();
    sandbox_home()?;
    // The switch itself is the unified scope state below — the CLI does NOT
    // write a `<id>_disabled` marker. The app retired that writer together
    // with the `set_*_enabled` commands (`skill_gate.rs`), so a marker
    // written here would be unclearable from every GUI surface and would
    // make the app delete the connector's skill directories forever.
    //
    // `enable` still removes a marker an older build may have left: it is
    // the only path that can heal one, and it only ever removes.
    if enabled {
        clear_legacy_disabled_marker(spec)?;
    }
    // The GUI switch (`set_disabled_connectors` → `apply_disabled_connectors_for`)
    // writes the plain scope's list and nothing else: hidden lists and the
    // DenyAll code scope are separate decisions (docs/capability-governance.md),
    // and `status` reads the plain scope. Same load-modify-save as the GUI and
    // `plugins enable/disable --scope plain`; a failed write fails the command.
    let package_id = pinvou3_lib::features::marketplace::scope::package_id_for(spec.id);
    let scope = pinvou3_lib::features::marketplace::ConnectorScope::Plain;
    let mut ids = pinvou3_lib::features::marketplace::load_disabled_bundles_for(scope);
    if enabled {
        ids.retain(|id| id != &package_id);
    } else if !ids.iter().any(|id| id == &package_id) {
        ids.push(package_id.clone());
    }
    pinvou3_lib::features::marketplace::save_disabled_bundles_for(scope, &ids).map_err(
        |error| {
            CliError::failed(format!(
                "connectors {}: could not update the plain-scope disabled set: {error}",
                spec.id
            ))
        },
    )?;
    let connected = cli_connected(spec).unwrap_or(false);
    // GUI parity (round-18 finding 1): the on-disk skill gate looks ONLY at
    // the connection state + the legacy `<id>_disabled` marker
    // (`skill_gate.rs::ConnectorGate::skills_should_show`); the plain-scope
    // write above is the connector SWITCH — it governs the composite
    // materialization and tool gating in the GUI, which never deletes
    // `bundles/<id>/skills`. Tearing the tree down on the switch alone would
    // silently renege on a connection the user still has, with no surface
    // able to restore it (the show direction needs the app's embedded
    // bundle). A probe error counts as not-connected here exactly like the
    // GUI's ready probes fold `run_probe` errors to false.
    let skills_should_show = gui_skill_gate_shows(kind);
    // The hide direction is the CLI's to perform (see `hide_connector_skills`)
    // — a `disable` that left the skill tree on disk would keep the engine
    // offering commands the switch just turned off, until the desktop app
    // happens to run its gate refresh. The show direction still needs the
    // app's embedded bundle.
    let skills_removed = if skills_should_show {
        false
    } else {
        hide_connector_skills(kind)?;
        true
    };
    let action = if enabled { "enabled" } else { "disabled" };
    let skills_refresh = if skills_removed {
        "removed"
    } else {
        "app-only"
    };
    let refresh_line = if skills_removed {
        "skills refresh: companion skill files removed".to_owned()
    } else if skills_should_show {
        // Nothing was pending: enabling a connected connector (or disabling
        // one that already kept its skills) leaves the skill files exactly
        // as they are, and the "deferred" phrasing would imply pending work.
        "skills refresh: nothing to do (skill files match the current state)".to_owned()
    } else {
        "skills refresh: deferred to the desktop app (embedded bundle unpack is app-only)"
            .to_owned()
    };
    let human = format!(
        "{action} {}\nconnected: {}\nskills should show: {}\n{refresh_line}",
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
        "skills_removed": skills_removed,
        "skills_refresh": skills_refresh,
    });
    Ok(success(render(output, human, &value)))
}

/// Mirror of `<connector>_apply_skills` (`ConnectorGate::apply_skills_command`
/// → `skills_should_show()` = marker read + `ready_probe()`), unchanged from
/// the GUI: recompute `should_show`, then apply it. The gate deliberately
/// does NOT consult the plain-scope switch state — the GUI's toggle governs
/// composite materialization and never deletes `bundles/<id>/skills`, so the
/// CLI's hide direction fires only on the marker or the absence of a
/// connection (round-18 finding 1).
///
/// The `false` half is applied for real here — it is the app's plain
/// `remove_dir_all` of the skill dirs plus the NOTICE file, which needs no
/// embedded bundle (see the module header). Only the `true` half stays
/// app-side (the unpack source is the app's compiled-in bundle), along with
/// the execpolicy ruleset hot-refresh (live engine pool).
fn apply_skills(kind: ConnectorKind, output: OutputMode) -> Result<CliOutcome, CliError> {
    let spec = kind.spec();
    // Mirror the GUI: a vendor CLI that cannot even be probed counts as
    // "not connected" (the visible outcome is the same: skills stay hidden),
    // instead of failing the whole command on a missing binary.
    let connected = cli_connected(spec).unwrap_or(false);
    let visible = gui_skill_gate_shows(kind);
    if visible {
        // The GUI's `apply_skills` (skill_gate) fails the command with the
        // shared consent-failure marker instead of letting the connector go
        // live with zero consent; the CLI mirrors that contract. The scope
        // layer logs nothing on write failure, so the error carries it.
        pinvou3_lib::features::marketplace::sync_deny_all_scopes_after_install(spec.id).map_err(
            |error| {
                CliError::failed(format!(
                    "{} applied, but {}: new sessions will enable it by default — \
turn it off in the tools list: {error}",
                    spec.id,
                    pinvou3_lib::features::marketplace::scope::CONSENT_SYNC_FAILURE_MARKER
                ))
            },
        )?;
    } else {
        hide_connector_skills(kind)?;
    }
    let applied_line = if visible {
        "skill unpack: requires the desktop app (embedded bundle)"
    } else {
        "skill removal: done (companion skill files removed)"
    };
    let human = format!(
        "{} skills should show: {}\nconnected: {}\n{applied_line}\nruleset refresh: requires the GUI engine pool",
        spec.id,
        yes_no(visible),
        yes_no(connected),
    );
    let value = json!({
        "ok": true,
        "id": spec.id,
        "visible": visible,
        "connected": connected,
        // The show direction remains app-only; the hide direction ran here.
        "skills_unpack": "app-only",
        "skills_removed": !visible,
        "ruleset_refresh": "gui-only",
    });
    Ok(success(render(output, human, &value)))
}

fn logout(kind: ConnectorKind, yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    let spec = kind.spec();
    let logout_deadline = Instant::now() + Duration::from_secs(CONNECT_DEFAULT_TIMEOUT_SECS);
    let mut value = match spec.id {
        // Mirror `feishu_logout`: `lark-cli auth logout` clears the token.
        "feishu" => {
            let (ok, _, _) = run_cli_bounded(spec, &["auth", "logout"], logout_deadline)?;
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
            let dir = wecom_config_dir()?;
            let existed = dir.exists();
            // A failed removal must not be reported as a clean logout: the
            // directory holds vendor credentials, so the error propagates
            // instead of being ignored (NotFound just means nothing was
            // stored). Intentional parity deviation from the GUI's `let _ =`
            // in wecom_logout: a destructive operation must be honest about
            // failing.
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(CliError::failed(format!(
                        "connectors: cannot remove the wecom credential directory: {error}"
                    )));
                }
            }
            bundle_store_on_disconnected(spec.id);
            json!({ "ok": true, "id": spec.id, "removed": existed })
        }
        // Mirror `dingtalk_logout` / `tmeet_logout`: already logged out when
        // the CLI is not installed; otherwise `auth logout [--yes]` must
        // succeed before the store mirror is updated. The gate judges
        // through `logout_probe` (GUI `logout_probe_verdict` parity): a CLI
        // that exists but fails its version probe leaves the login state
        // unconfirmed, and the error surfaces without touching the store —
        // claiming `installed:false` there would skip the real logout while
        // the vendor token stays on disk.
        _ => {
            let args: &[&str] = if spec.id == "dingtalk" {
                &["auth", "logout", "--yes"]
            } else {
                &["auth", "logout"]
            };
            match logout_probe(spec, logout_deadline) {
                LogoutGate::Unconfirmed(note) => {
                    return Err(CliError::failed(format!("connectors logout: {note}")));
                }
                LogoutGate::NotInstalled => {
                    bundle_store_on_disconnected(spec.id);
                    json!({ "ok": true, "id": spec.id, "installed": false })
                }
                LogoutGate::Installed => {
                    let (ok, _, _) = run_cli_bounded(spec, args, logout_deadline)?;
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
        }
    };
    // The GUI logout is two invokes, not one: `ToolStoreView.jsx` calls
    // `cfg.commands.logout` and then `cfg.commands.applySkills`, whose
    // `skills_should_show()` is necessarily false right after the
    // credentials are gone — so the desktop flow removes the companion skill
    // trees as part of every logout. Doing the vendor logout without this
    // leaves a fully connected-looking skill tree on disk, advertising
    // commands that can no longer authenticate.
    //
    // Runs only after a successful vendor logout (every failure branch above
    // returned already): skills must not be torn down while the credentials
    // are still in place.
    if let Err(error) = hide_connector_skills(kind) {
        return Err(CliError::failed(format!(
            "connectors logout: {} was logged out, but its companion skills are still on disk: \
             {error}. Fix the permissions and run `pinvou connectors apply-skills {}`",
            spec.id, spec.id
        )));
    }
    value["skills_removed"] = json!(true);
    Ok(success(render(
        output,
        format!(
            "logged out {}\ncompanion skills removed from {}",
            spec.id,
            connector_skills_dir(kind).display()
        ),
        &value,
    )))
}

/// Mirror of `wecom_config_dir` (real home, not PINVOU3_HOME), resolved the
/// way the GUI's `user_home_dir` resolves it per platform: `HOME` on unix —
/// never `USERPROFILE`, which WSL interop exports pointing at the Windows
/// profile and which would send the destructive `remove_dir_all` below at a
/// tree neither the vendor CLI nor the GUI ever wrote while reporting the
/// real tokens still live — and `USERPROFILE` first on Windows. An unset
/// home is an error, not an empty path: the directory feeds a destructive
/// `remove_dir_all`, and `Path::new("").join(...)` would resolve against
/// the process cwd.
#[cfg(unix)]
fn wecom_config_dir() -> Result<PathBuf, CliError> {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return Err(CliError::failed(
            "connectors: cannot locate the wecom credential directory: HOME is not set",
        ));
    }
    Ok(Path::new(&home).join(".config").join("wecom"))
}

/// Windows half of the split above (see the unix doc for the home-policy
/// rationale): `USERPROFILE` first, falling back to `HOME`, matching the
/// precedence the GUI's Windows `user_home_dir` leads with.
#[cfg(windows)]
fn wecom_config_dir() -> Result<PathBuf, CliError> {
    let home = std::env::var("USERPROFILE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_default();
    if home.trim().is_empty() {
        return Err(CliError::failed(
            "connectors: cannot locate the wecom credential directory: neither USERPROFILE nor HOME is set",
        ));
    }
    Ok(Path::new(&home).join(".config").join("wecom"))
}

// ─────────────────────────────── ensure-cli ───────────────────────────────

/// Platform lock table — the same pinned artifacts
/// `features/connectors/native_installer.rs` reads (URL + archive/binary
/// SHA-256). Read through the app crate's `connector_lock::lock_json()`
/// rather than a second `include_str!` copy: a hand-maintained duplicate of
/// a cfg-gated table is exactly the drift the rest of this module guards
/// against, and the app export returns byte-identical content (empty on
/// platforms with no reviewed artifacts, which keeps the `NO_LOCK` error
/// path below intact).

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
    let lock_json = pinvou3_lib::platform::connector_lock::lock_json();
    if lock_json.is_empty() {
        return Err(CliError::failed(NO_LOCK));
    }
    let table: Value = serde_json::from_str(lock_json)
        .map_err(|_| CliError::failed("connector CLI lock table is invalid"))?;
    if table.get("schemaVersion").and_then(Value::as_i64) != Some(1) {
        return Err(CliError::failed(
            "connector CLI lock table schema is not supported",
        ));
    }
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
    match spec.id {
        // Mirror `install_tmeet_cli`: npm global install of the pinned spec.
        "tmeet" => {
            if !run_npm_install(spec)? {
                // The hint only names the log that actually exists: npm's
                // stdio falls back to null when the log file cannot be
                // created, and pointing at a file that was never written
                // sends the user hunting a empty trail.
                let log_path = pinvou3_home().join("cli-install.log");
                let hint = if log_path.is_file() {
                    format!(", see {}", log_path.display())
                } else {
                    String::new()
                };
                return Err(CliError::failed(format!(
                    "{} CLI install failed{hint}",
                    spec.display_name
                )));
            }
        }
        // Mirror `native_installer::ensure_native_cli`: download the pinned
        // archive, verify both hashes, stage the executable into the
        // versioned asset directory.
        _ => {
            ensure_native_cli(spec)?;
        }
    };
    // The user-facing claim of this command is "the CLI is present and
    // executes", not "some bytes landed": a hash-matched destination that
    // the OS refuses to execute (a staged file losing its exec bit through a
    // filesystem that ignores the 0755, a quarantine attribute, a shim
    // resolving to a broken interpreter) must surface here instead of the
    // next status probe. `cli_installed` runs the real `--version`, and the
    // check is NOT skipped when `ensure_native_cli` reports the hash already
    // matched: that means the file verified without this call installing
    // anything, so an unexecutable pre-existing binary is caught on the
    // repair path too — exactly the state a user runs `ensure-cli` to fix.
    if !cli_installed(spec) {
        return Err(CliError::failed(format!(
            "{} CLI is present on disk but will not execute; retry with `connectors ensure-cli` \
             after repairing the file's permissions",
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
    // Same cross-process serialization as the native lane's lock (see
    // `ensure_native_cli`): concurrent `ensure-cli` runs — npm and native
    // alike — must not race (here on npm's global prefix tree and the shared
    // install log). Blocking wait is fine, same discipline as the native
    // lane: installs are rare and the loser just installs over the finished
    // tree. The guard releases when this function returns.
    let install_lock_dir = pinvou3_home().join("locks");
    std::fs::create_dir_all(&install_lock_dir).map_err(|error| {
        CliError::failed(format!(
            "cannot create the connector lock directory: {error}"
        ))
    })?;
    let install_lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(install_lock_dir.join("connector-install.lock"))
        .map_err(|error| CliError::failed(format!("cannot open the install lock: {error}")))?;
    let mut install_lock = fd_lock::RwLock::new(install_lock_file);
    let _install_guard = install_lock
        .write()
        .map_err(|error| CliError::failed(format!("cannot acquire the install lock: {error}")))?;
    let log_path = pinvou3_home().join("cli-install.log");
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // npm is an npm.cmd shim on Windows; resolve the candidate and wrap the
    // spawn the same way every other vendor CLI child is wrapped.
    let npm = crate::support::binary_candidates("npm")
        .into_iter()
        .find_map(|candidate| {
            std::env::split_paths(&std::env::var_os("PATH")?)
                .map(|dir| dir.join(&candidate))
                .find(|path| path.is_file())
        })
        .ok_or_else(|| CliError::failed("npm was not found on PATH; install Node.js first"))?;
    let mut cmd = crate::support::build_command(&npm, &["install", "-g", TMEET_NPM_SPEC]);
    crate::support::set_process_group(&mut cmd);
    // The GUI's tmeet install applies the user npm prefix (the shared
    // apply_user_npm_prefix helper), so `ensure-cli tmeet` must not try to
    // write /usr/local without sudo.
    apply_user_npm_prefix(&mut cmd);
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
    // Same bracket as every other long-running vendor child: the npm
    // install can block for minutes, and a terminal SIGINT must take it
    // down with this CLI instead of orphaning it in its own group.
    // `spawn_supervised` also closes the spawn→register window (see
    // `run_cli_bounded`).
    let mut child = crate::support::supervise::spawn_supervised(&mut cmd)
        .map_err(|error| CliError::failed(format!("npm install failed to start: {error}")))?;
    let start = Instant::now();
    loop {
        // An OS-level wait error leaves the child's state unknown, and a
        // plain `?` would both orphan it and leave its group registered —
        // so it goes down with the group (it may still be running) and the
        // registration is released, the same convention as the timeout arm.
        let polled = match child.try_wait() {
            Ok(polled) => polled,
            Err(error) => {
                crate::support::kill_process_tree(&mut child);
                crate::support::supervise::forget_child_group(child.id());
                return Err(CliError::failed(format!("wait: {error}")));
            }
        };
        match polled {
            Some(status) => {
                // Reaped here, so the group must leave the registry before
                // the OS can recycle the pgid.
                crate::support::supervise::forget_child_group(child.id());
                return Ok(status.success());
            }
            None => {
                if start.elapsed() > Duration::from_secs(NPM_INSTALL_TIMEOUT_SECS) {
                    crate::support::kill_process_tree(&mut child);
                    crate::support::supervise::forget_child_group(child.id());
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
    // Serialize concurrent CLI installs (two `ensure-cli` processes would
    // race on the shared staging archive and destination); blocking wait is
    // fine — installs are rare and the loser just re-verifies the hash.
    // Scope note: this lock gates CLI×CLI only. A GUI-triggered install of
    // the same connector takes the desktop's in-process mutex, not this
    // file lock, and GUI×CLI interleaving stays a disclosed residual (the
    // payloads are SHA-256-pinned and installed by atomic rename, so the
    // outcome remains correct) — see docs/pinvou-cli.md Known limitations.
    let install_lock_dir = pinvou3_home().join("locks");
    std::fs::create_dir_all(&install_lock_dir).map_err(|error| {
        CliError::failed(format!(
            "cannot create the connector lock directory: {error}"
        ))
    })?;
    let install_lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(install_lock_dir.join("connector-install.lock"))
        .map_err(|error| CliError::failed(format!("cannot open the install lock: {error}")))?;
    let mut install_lock = fd_lock::RwLock::new(install_lock_file);
    let _install_guard = install_lock
        .write()
        .map_err(|error| CliError::failed(format!("cannot acquire the install lock: {error}")))?;

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
        // GUI parity (`native_installer::ensure_native_cli`): a matching hash
        // says the bytes on disk are right; only the presence check the
        // caller then runs tells the user the CLI binary actually executes.
        // Mirrors the GUI, where `install()` is a no-op exactly when the
        // hash matched and `present()` still runs in
        // `ensure_cli_with`/"安装完成但无法执行" — so install=true here
        // regardless of which branch produced the bytes (round-18 finding 3).
        return Ok(true);
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
    // tar preserves the member's internal directory layout, so the binary
    // may land in a nested dir; locate it by name before verifying.
    let extracted = match find_file_by_name(&extract_dir, &filename) {
        Some(path) => path,
        None => {
            let _ = std::fs::remove_dir_all(&extract_dir);
            return Err(CliError::failed(format!(
                "{} executable not found in the extracted archive",
                artifact.name
            )));
        }
    };
    if !file_is_sha256(&extracted, &artifact.binary_sha256) {
        let _ = std::fs::remove_dir_all(&extract_dir);
        return Err(CliError::failed(format!(
            "{} executable checksum mismatch",
            artifact.name
        )));
    }

    let staging = version_dir.join(format!(".{filename}.installing-{}", std::process::id()));
    let _ = std::fs::remove_file(&staging);
    // The staging-to-destination section cleans up after itself on every
    // error exit, like the extraction section above: a best-effort failure
    // must not leave the `.installing-<pid>` file or the extract directory
    // behind (a later run only cleans its own pid's leftovers).
    let staging_result = (|| -> Result<(), CliError> {
        std::fs::rename(&extracted, &staging)
            .map_err(|error| CliError::failed(format!("cannot stage connector binary: {error}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).map_err(
                |error| CliError::failed(format!("cannot set executable permissions: {error}")),
            )?;
        }
        // On posix `rename` replaces the destination atomically — the pre-`remove_file`
        // this site used to do opened a window where a concurrent spawner resolved
        // a MISSING binary (the Known limitations copy already promised "installed
        // by atomic rename"). The Windows arm keeps the explicit remove for GUI
        // parity (`native_installer.rs` does the same remove-then-rename
        // unconditionally) — std's Windows `rename` maps to MoveFileExW with
        // REPLACE_EXISTING and would replace by itself, so the remove is
        // redundant rather than required; it trades a brief missing-binary
        // window (crash between remove and rename recovers by re-running
        // ensure-cli) for byte-for-byte behaviour parity with the desktop
        // installer, and installs are file-lock serialized CLI×CLI. Both arms
        // are typed `Result<(), CliError>`: the closure's tail expression must
        // not let the raw `io::Error` leak into the closure's inferred type
        // (the `?` above then demands a `From<CliError> for io::Error` that
        // does not exist — the Windows target did not compile before this was
        // explicit).
        #[cfg(unix)]
        let replace = std::fs::rename(&staging, &destination)
            .map_err(|error| CliError::failed(format!("cannot finish connector install: {error}")));
        #[cfg(windows)]
        let replace = (|| -> Result<(), CliError> {
            if destination.exists() {
                std::fs::remove_file(&destination).map_err(|error| {
                    CliError::failed(format!("cannot replace old connector binary: {error}"))
                })?;
            }
            std::fs::rename(&staging, &destination).map_err(|error| {
                CliError::failed(format!("cannot finish connector install: {error}"))
            })
        })();
        replace
    })();
    // The staging file is only removed on the failure exits here: after a
    // successful rename it no longer exists, and on an error exit a later
    // run would otherwise only clean its own pid's leftovers.
    if staging_result.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    let _ = std::fs::remove_dir_all(&extract_dir);
    staging_result?;
    Ok(true)
}

fn file_is_sha256(path: &Path, expected: &str) -> bool {
    sha256_file(path)
        .map(|actual| actual == expected)
        .unwrap_or(false)
}

/// Finds the first regular file with `name` under `dir` (the walker does not
/// follow symlinks).
fn find_file_by_name(dir: &Path, name: &str) -> Option<PathBuf> {
    if dir.is_file() {
        return None;
    }
    for entry in std::fs::read_dir(dir).ok()? {
        // Skip an unreadable entry instead of propagating `None` out of the
        // whole walk: with `?` here a single EACCES on one sibling aborted
        // the search and surfaced as "executable not found in the extracted
        // archive", pointing the user at the archive instead of at the
        // permission problem.
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            if let Some(found) = find_file_by_name(&path, name) {
                return Some(found);
            }
        } else if path.file_name().map(|n| n == name).unwrap_or(false) {
            return Some(path);
        }
    }
    None
}

/// HTTPS-only download capped at the app's archive limit, staged through a
/// `.part` sibling so a crashed download never leaves a truncated file at the
/// real path (rename is atomic on the same filesystem).
fn download_https(url: &str, destination: &Path) -> Result<(), CliError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| CliError::failed("connector download URL is invalid"))?;
    if parsed.scheme() != "https" {
        return Err(CliError::failed("connector download URL must be https"));
    }
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(30))
        // Same https-only redirect policy as the GUI's `download_verified`
        // (features/connectors/native_installer.rs): content integrity is
        // pinned by sha256, but a scheme-downgrading redirect must not leak
        // the URL to a plaintext hop either.
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 || attempt.url().scheme() != "https" {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .user_agent(concat!("pinvou-cli/", env!("CARGO_PKG_VERSION")))
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
    let part = destination.with_extension("part");
    let mut reader = response.take(MAX_ARCHIVE_BYTES + 1);
    let mut file = std::fs::File::create(&part)
        .map_err(|error| CliError::failed(format!("cannot create archive file: {error}")))?;
    let copied = match std::io::copy(&mut reader, &mut file) {
        Ok(copied) => copied,
        Err(error) => {
            let _ = std::fs::remove_file(&part);
            return Err(CliError::failed(format!(
                "connector download failed: {error}"
            )));
        }
    };
    if copied > MAX_ARCHIVE_BYTES {
        let _ = std::fs::remove_file(&part);
        return Err(CliError::failed("connector archive exceeds the size cap"));
    }
    drop(file);
    if let Err(error) = std::fs::rename(&part, destination) {
        let _ = std::fs::remove_file(&part);
        return Err(CliError::failed(format!(
            "cannot finish the archive download: {error}"
        )));
    }
    Ok(())
}

/// Extract one archive member by exact file name using the system `tar`
/// (bsdtar also reads zip, matching the GUI's tar.gz/zip split).
/// The tar lanes are bounded like every other vendor spawn: process group,
/// deadline, tree-kill, and a capped listing. (The archive is sha256-pinned
/// against the reviewed lock table before this runs, so these bounds are
/// belt-and-braces — but an unbounded, deadline-free spawn is exactly the
/// drift the vendor-spawn consolidation exists to prevent.)
const TAR_TIMEOUT: Duration = Duration::from_secs(120);
const TAR_LIST_CAP: u64 = 8 * 1024 * 1024;
/// How long the tar lanes wait for their drain threads to finish after the
/// child is reaped — the same grace `run_cli_bounded` gives its drains. The
/// kill closes tar's pipe ends, but a descendant that escaped the group
/// could still hold stdout open, so collecting the drain must stay bounded.
const TAR_DRAIN_GRACE: Duration = Duration::from_secs(5);
/// Hard bound on the UNCOMPRESSED member, mirroring
/// `native_installer::MAX_BINARY_BYTES` (the GUI reads the member through
/// `read_limited(&mut entry, MAX_BINARY_BYTES)`). `MAX_ARCHIVE_BYTES` bounds
/// only the download, so without this a pinned-looking archive whose
/// compressed size fits could still inflate without limit on extraction.
/// Defence in depth: the archive's SHA-256 is verified against the
/// compiled-in lock table before any of this runs.
const MAX_MEMBER_BYTES: u64 = 128 * 1024 * 1024;

/// Mirror of the safety half of `native_installer::normalized_path_eq`: the
/// GUI matches members by exact normalized path and maps anything that is
/// not a plain name component (`..`, a root, a Windows drive prefix) to
/// `<unsafe>`, which can never equal the expected name. The CLI matches by
/// file name inside a nested layout, so the same rejection is applied
/// explicitly — a member path that walks upwards must never be handed to
/// `tar` as an extraction target.
///
/// Windows-style separators deserve their own rule on non-Windows hosts:
/// `Path::components` on unix treats `C:\Windows` as ONE Normal component
/// (backslash is not a separator there), so the components check alone
/// would pass a Windows-shaped traversal through. Refuse any member
/// carrying a backslash or a drive-letter prefix outright — connector
/// archives never use either form (their members are unix-style paths or
/// bare file names), so nothing legitimate is excluded.
fn member_path_is_safe(entry: &str) -> bool {
    if entry.is_empty() || entry.contains('\\') {
        return false;
    }
    Path::new(entry)
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn extract_member(archive: &Path, member: &str, target: &Path) -> Result<(), CliError> {
    // `--` ends option parsing on every tar the CLI runs on (bsdtar and GNU
    // tar alike): without it, a member name that begins with a dash would be
    // parsed as tar options. The wanted name comes from the listing and the
    // archive path from a staging name the CLI itself built, but the
    // convention is enforced unconditionally so the lane cannot depend on
    // those two facts — `-xOf` is the one flag-taking-operand form where a
    // misplaced `--` changes the parse (`-xOf -- archive member` opens an
    // archive named "--"), so it is placed as a separator after the archive
    // operand in both child constructions (see `tar_separated_operands`).
    let mut list = Command::new("tar");
    list.arg("-tf")
        .arg(archive)
        .arg("--")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::support::set_process_group(&mut list);
    // Short-lived and piped, but the same bracket discipline: until it is
    // reaped, a terminal SIGINT takes the group down with the CLI.
    // `spawn_supervised` closes the spawn→register window (see
    // `run_cli_bounded`).
    let mut child = crate::support::supervise::spawn_supervised(&mut list)
        .map_err(|error| CliError::failed(format!("cannot list archive with tar: {error}")))?;
    // The listing is drained on a thread and the BOUNDED WAIT drives the
    // lane, the same order `run_cli_bounded` uses. In the previous order
    // (blocking read, then wait) a tar that produced under the cap and
    // stalled without exiting or closing stdout would block the read
    // forever and the deadline would never be reached. Here a stall is
    // bounded by `wait_or_kill`: the kill closes the pipe and ends the
    // read. Past the cap the drainer keeps reading and discarding so tar
    // can never block on a full pipe and be misreported as timed out.
    let listing_thread = {
        use std::io::Read as _;
        let mut stdout = child.stdout.take().expect("tar stdout is piped");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = (&mut stdout).take(TAR_LIST_CAP).read_to_end(&mut bytes);
            let _ = std::io::copy(&mut stdout, &mut std::io::sink());
            let _ = tx.send(String::from_utf8_lossy(&bytes).into_owned());
        });
        rx
    };
    let listed = wait_or_kill(&mut child, "listing the connector archive");
    // Paired on every exit including the `?` arms of `listed` below: the
    // child is reaped or dead by now, so leaving the pgid registered would
    // only invite the recycled-pid kill.
    crate::support::supervise::forget_child_group(child.id());
    listed?;
    // Bounded collection instead of an unbounded `join`: a pipe-inheriting
    // descendant that escaped the group can hold stdout open past the reap,
    // and the sibling drains (`run_cli_bounded`) answer exactly that with a
    // grace + default rather than hanging the CLI.
    let listing = listing_thread
        .recv_timeout(TAR_DRAIN_GRACE)
        .unwrap_or_default();
    let entry = listing
        .lines()
        .map(str::trim)
        // A traversal-shaped member is skipped, not merely unselected: the
        // wanted file name could otherwise be reached through `../../..`.
        .filter(|entry| member_path_is_safe(entry))
        .find(|entry| {
            Path::new(entry)
                .file_name()
                .map(|name| name.to_string_lossy() == member)
                .unwrap_or(false)
        })
        .ok_or_else(|| CliError::failed(format!("connector archive does not contain {member}")))?;
    // `-O` streams the member to stdout instead of letting tar write it, so
    // the uncompressed bytes pass through a hard cap on the way to disk —
    // the CLI's equivalent of the GUI's `read_limited(&mut entry,
    // MAX_BINARY_BYTES)`. It also means tar never creates paths itself, so
    // the member name cannot steer where the file lands: the caller's
    // `target` dir and the requested `member` name do.
    let mut extract = Command::new("tar");
    extract
        .arg("-xOf")
        .arg(archive)
        .arg("--")
        .arg(entry)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::support::set_process_group(&mut extract);
    // Same bracket as the listing child above (the comment there applies
    // here too: short-lived and piped, but supervised until it is reaped,
    // with the spawn→register window closed).
    let mut child = crate::support::supervise::spawn_supervised(&mut extract)
        .map_err(|error| CliError::failed(format!("cannot extract archive with tar: {error}")))?;
    // Same wait-first discipline as the listing lane above: the bounded
    // wait drives the deadline, the drainer thread keeps the pipe empty
    // (one byte past the cap is KEPT so an over-size member is detectable
    // rather than silently truncated into a hash mismatch; everything after
    // it is drained and discarded), and a kill closes the pipe so the
    // drainer cannot hang past the kill.
    let bytes_thread = {
        use std::io::Read as _;
        let mut stdout = child.stdout.take().expect("tar stdout is piped");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = (&mut stdout)
                .take(MAX_MEMBER_BYTES + 1)
                .read_to_end(&mut bytes);
            let _ = std::io::copy(&mut stdout, &mut std::io::sink());
            let _ = tx.send(bytes);
        });
        rx
    };
    let extracted = wait_or_kill(&mut child, "extracting the connector archive");
    // Paired on every exit, as in the listing lane.
    crate::support::supervise::forget_child_group(child.id());
    extracted?;
    // Same bounded collection as the listing lane above.
    let bytes = bytes_thread
        .recv_timeout(TAR_DRAIN_GRACE)
        .unwrap_or_default();
    if bytes.len() as u64 > MAX_MEMBER_BYTES {
        // tar already exited (the wait above reaped it after it wrote
        // itself out past the kept cap), so only the verdict is left to
        // report — with the specific reason instead of tar's generic
        // EPIPE failure.
        return Err(CliError::failed(format!(
            "connector archive member {member} exceeds the {MAX_MEMBER_BYTES}-byte uncompressed \
             size cap"
        )));
    }
    std::fs::write(target.join(member), &bytes).map_err(|error| {
        CliError::failed(format!(
            "cannot write the extracted connector binary: {error}"
        ))
    })?;
    Ok(())
}

/// Bounded wait for a spawned child: on expiry the process group is killed
/// (the child is verifiably still alive, so no reaped-pid hazard) and the
/// caller gets an error naming the phase. A wait ERROR also kills: the
/// child's fate is unknown — it may still be running — and the caller's
/// paired forget would otherwise release a live group from the interrupt
/// registry (the same convention as `run_cli_bounded` and the npm-install
/// lane). The kill closes the child's pipe write ends, so the caller's
/// drainer threads reach EOF instead of lingering.
fn wait_or_kill(child: &mut std::process::Child, phase: &str) -> Result<(), CliError> {
    match child.wait_timeout(TAR_TIMEOUT) {
        Ok(Some(status)) if status.success() => Ok(()),
        Ok(Some(_)) => Err(CliError::failed(format!("cannot run tar: {phase} failed"))),
        Ok(None) => {
            crate::support::kill_process_tree(child);
            Err(CliError::failed(format!(
                "cannot run tar: timed out {phase}"
            )))
        }
        Err(error) => {
            crate::support::kill_process_tree(child);
            Err(CliError::failed(format!("cannot run tar: {error}")))
        }
    }
}

// ─────────────────────────────── connect ───────────────────────────────

/// Headless connect flows. The GUI spawns the same commands in background
/// threads and drives the UI through `<id>:qr` / `<id>:connected` events;
/// the CLI runs them in the foreground, prints the login URL(s) (no QR
/// image render — spec: GUI-bound), and polls the connected predicate until
/// success or `--timeout`.
/// Both halves the GUI performs the moment a connection lands, so the CLI
/// must not stop at the first:
/// - the bundle-store mirror write (`bundle_store_on_connected`) — both
///   surfaces' connect flows call it inline;
/// - the DenyAll code-scope sync (`sync_deny_all_scopes_after_install`).
///   The GUI runs it through the `*_apply_skills` command the frontend
///   invokes right after `connected` fires (`ToolStoreView.jsx` calls
///   `cfg.commands.applySkills` on the connect-completion handler), whose
///   `show` branch performs exactly this sync (`skill_gate.rs::
///   apply_skills_command`, fail-closed). Skipping it here meant a fresh
///   connect never re-applied the "code sessions default external
///   capabilities off" policy, so a newly connected connector stayed usable
///   from code sessions until the next GUI-side apply-skills happened to
///   run.
fn finish_connect_side_effects(spec: &VendorSpec) -> Result<(), CliError> {
    bundle_store_on_connected(spec.id);
    // GUI parity (round-18 finding 2): the desktop connect flow ends in the
    // `<id>_apply_skills` follow-up, whose `apply_skills_command`
    // (features/connectors/skill_gate.rs) runs
    // `sync_deny_all_scopes_after_install` — 「连接器转为可用等同『新装』：
    // 已初始化 code 开关时加入 code 禁用集」— so a freshly connected
    // connector lands in the initialized DenyAll scopes' disabled sets
    // (code sessions default external capabilities off). Without this, a
    // fresh CLI connection would run code sessions with the connector ON
    // where the GUI (which also runs the follow-up after its connect) would
    // have it OFF. The GUI fails visibly on a failed consent write
    // (skill_gate's marker contract); the CLI mirrors that instead of
    // reporting a connection whose consent never landed — the scope layer
    // logs nothing on write failure, so the error carries the reason.
    pinvou3_lib::features::marketplace::sync_deny_all_scopes_after_install(spec.id).map_err(
        |error| {
            CliError::failed(format!(
                "{} connected, but {}: new sessions will enable it by default — \
turn it off in the tools list: {error}",
                spec.id,
                pinvou3_lib::features::marketplace::scope::CONSENT_SYNC_FAILURE_MARKER
            ))
        },
    )?;
    Ok(())
}

fn connect(kind: ConnectorKind, timeout: u64, output: OutputMode) -> Result<CliOutcome, CliError> {
    let spec = kind.spec();
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let mut notes: Vec<String> = Vec::new();
    // Bounded, redacted tail of what the vendor CLI itself printed. Its
    // stdout/stderr are piped (the drainer needs them to find the login
    // link), so without this the user never sees the vendor's own failure
    // reason — the commonest dingtalk onboarding blocker ("CLI data access
    // is not enabled") arrives as one such line and is otherwise dropped on
    // the floor. Deliberately NOT streamed live: the link and the QR path
    // are the only things worth interrupting the terminal for, and a chatty
    // vendor CLI would bury them. It is attached to failure messages only.
    let mut tail: Vec<String> = Vec::new();
    match spec.id {
        "feishu" => {
            // Phase 1 (register app): `config init --new` until exit. The
            // register link is printed and noted live by
            // `spawn_and_capture_url` while the vendor CLI runs.
            let (url, _user_code, status_ok) = spawn_and_capture_url(
                spec,
                &["config", "init", "--new"],
                &mut notes,
                &mut tail,
                deadline,
                None,
            )?;
            if !status_ok {
                return Err(CliError::failed(format!(
                    "feishu app registration did not complete (cancelled or timed out){}{}",
                    captured_notes(&notes),
                    vendor_output_tail(spec, &tail)
                )));
            }
            let _ = url;
            // Phase 2 (authorize user): device-code login + polling.
            let (ok, stdout, stderr) = run_cli_bounded(
                spec,
                &["auth", "login", "--no-wait", "--json", "--recommend"],
                deadline,
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
            // Vendor-controlled URL into human output: same collapse as the
            // login link and version cells.
            let printable = crate::support::collapse_control_characters(&url);
            notes.push(format!("authorize-url: {printable}"));
            note!("lark-cli authorize-url: {printable}");
            loop {
                if Instant::now() >= deadline {
                    return Err(CliError::failed(format!(
                        "feishu authorization timed out before the scan completed{}{}",
                        captured_notes(&notes),
                        vendor_output_tail(spec, &tail)
                    )));
                }
                std::thread::sleep(Duration::from_secs(3));
                // This call may block until completion or return pending;
                // readiness is judged by the auth status probe either way.
                // Bounded so a hung vendor CLI cannot outlive the deadline.
                let _ = run_cli_bounded(
                    spec,
                    &["auth", "login", "--device-code", &device_code, "--json"],
                    deadline,
                );
                // A probe error counts as not-connected, exactly like the
                // GUI's poll folds `run_probe` errors to false (feishu.rs
                // `is_user_ready`): a spawn failure or probe timeout must
                // not abort the connect after a successful login — the next
                // tick retries and the loop deadline still bounds the wait.
                if cli_connected_bounded(spec, deadline) {
                    finish_connect_side_effects(spec)?;
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
            // The QR encodes a one-scan login grant (credential-equivalent),
            // so the directory is created owner-only from the start (no
            // world-readable window between create and tighten) and a failed
            // tightening is an error, not a silent downgrade: writing the
            // grant into a directory the chmod could not secure would defeat
            // the whole measure.
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt as _;
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&qr_dir)
                    .map_err(|error| {
                        CliError::failed(format!("cannot create the scan QR temp dir: {error}"))
                    })?;
            }
            #[cfg(not(unix))]
            std::fs::create_dir_all(&qr_dir).map_err(|error| {
                CliError::failed(format!("cannot create the scan QR temp dir: {error}"))
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&qr_dir, std::fs::Permissions::from_mode(0o700)).map_err(
                    |error| {
                        CliError::failed(format!(
                            "cannot tighten the scan QR temp dir to owner-only: {error}"
                        ))
                    },
                )?;
            }
            // The vendor CLI writes the relative `qr.png` next to its cwd; the
            // GUI redirects it into a temp dir the same way (`wecom.rs` sets
            // `current_dir`), so the user's working directory stays clean.
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
                &mut tail,
                deadline,
                Some(&qr_dir),
            );
            // The stdout URL is the landing page (which asks for another
            // scan); the live `scan-qr-file` note from
            // `spawn_and_capture_url` points at the PNG that authorizes in
            // one scan. On failure the QR dir is deliberately KEPT: the
            // error's captured notes point at the PNG, and deleting it
            // before returning would erase the one artifact the user can
            // still act on.
            let outcome = flow.and_then(|(_url, _user_code, _status_ok)| {
                // Judge by the auth probe alone like the GUI (`wecom.rs`
                // binds the child exit status to `_`): a wecom-cli that
                // authorizes successfully but exits non-zero connects in
                // the GUI and must not fail here. A probe error also folds
                // to not-connected (the GUI's `is_ready` fold), so a probe
                // hiccup reports the honest "did not complete" verdict
                // below instead of surfacing the probe's own error as the
                // connect failure.
                if !cli_connected_bounded(spec, deadline) {
                    return Err(CliError::failed(format!(
                        "wecom authorization did not complete (cancelled or timed out){}{}",
                        captured_notes(&notes),
                        vendor_output_tail(spec, &tail)
                    )));
                }
                Ok(())
            });
            if let Err(error) = outcome {
                // Kept on authorization failure: the notes point at the QR
                // PNG the user can still scan. A spawn/capture failure
                // produced no artifact — an empty dir would litter $TMPDIR
                // forever, so clean it up.
                if !qr_dir.join("qr.png").is_file() {
                    let _ = std::fs::remove_dir_all(&qr_dir);
                }
                return Err(error);
            }
            if let Err(error) = finish_connect_side_effects(spec) {
                // The login itself succeeded, so the one-scan grant already
                // took effect and the QR note promises removal once it does:
                // a failed post-login store sync must not leave the
                // credential-equivalent PNG behind.
                let _ = std::fs::remove_dir_all(&qr_dir);
                return Err(error);
            }
            let _ = std::fs::remove_dir_all(&qr_dir);
        }
        "dingtalk" | "tmeet" => {
            let args: &[&str] = if spec.id == "dingtalk" {
                &["auth", "login", "--device"]
            } else {
                &["auth", "login", "--no-browser"]
            };
            let (url, user_code, _status_ok) =
                spawn_and_capture_url(spec, args, &mut notes, &mut tail, deadline, None)?;
            if let Some(url) = compose_user_code(&url, user_code.as_deref()) {
                // Same collapse as the feishu lane: the URL is vendor output
                // and reaches the terminal through the notes.
                notes.push(format!(
                    "authorize-url: {}",
                    crate::support::collapse_control_characters(&url)
                ));
            }
            // Mirror the GUI's exit handling: judge by the auth probe alone —
            // a CLI that authorizes successfully but exits non-zero connects
            // there and must connect here too, while an exit-0 logout that
            // never authenticated must fail instead of passing. Vendor
            // output was piped (not shown), so the error carries the login
            // material captured so far instead of pointing at a terminal
            // that never saw it. This single probe after exit is the whole
            // wait — the vendor CLI itself blocks until authorization (or
            // its own timeout), and the spawn deadline above already bounds
            // the process.
            // The GUI polls `wait_logged_in` for up to 5 s after the child
            // exits because `tmeet auth status` can lag credential
            // persistence at process exit; mirror that so a successful login
            // is not misreported as a failure.
            let mut connected = cli_connected_bounded(spec, deadline);
            let grace_started = std::time::Instant::now();
            while !connected && grace_started.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(200));
                connected = cli_connected_bounded(spec, deadline);
            }
            if !connected {
                return Err(CliError::failed(format!(
                    "{} login exited before authorization completed{}{}",
                    spec.display_name,
                    captured_notes(&notes),
                    vendor_output_tail(spec, &tail)
                )));
            }
            finish_connect_side_effects(spec)?;
        }
        _ => return Err(CliError::usage("unknown connector")),
    }
    notes.push(format!("{} connected", spec.id));
    // The DenyAll scope sync ran inside `finish_connect_side_effects` on
    // each vendor arm — running it twice per connect only doubled the
    // last-writer-wins window against a concurrently persisting desktop
    // app, so it lives in exactly one place.
    let value = json!({
        "ok": true,
        "id": spec.id,
        "connected": true,
        "notes": notes,
    });
    Ok(success(render(output, notes.join("\n"), &value)))
}

/// Spawns a long-running login command, captures the first authorization URL
/// from its output (auth-domain filtered, mirror of `drain_for_url` /
/// `CliCtx::extract_url`), waits for exit within `deadline` and returns
/// `(url, exit_success)`. stdin is nulled like the GUI flows; `work_dir`
/// keeps relative scratch output (wecom's `qr.png`) out of the user's cwd.
fn spawn_and_capture_url(
    spec: &VendorSpec,
    args: &[&str],
    notes: &mut Vec<String>,
    tail: &mut Vec<String>,
    deadline: Instant,
    work_dir: Option<&Path>,
) -> Result<(Option<String>, Option<String>, bool), CliError> {
    let Some(executable) = resolve_vendor_cli(spec) else {
        return Err(CliError::failed(format!(
            "{} was not found (managed install or PATH); install it first: pinvou connectors \
             ensure-cli {}",
            spec.cli_bin, spec.id
        )));
    };
    // `build_command` already carries `args`; `Command::args` appends, so a
    // second call here would double the argv and break every vendor parser.
    let mut cmd = crate::support::build_command(&executable, args);
    for (key, value) in spec.envs {
        cmd.env(key, value);
    }
    if let Some(dir) = work_dir {
        cmd.current_dir(dir);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::support::set_process_group(&mut cmd);
    // RAII around the login child: see [`LoginChildGuard`]. `connect` blocks
    // for up to five minutes here waiting for a human to scan, and every
    // error path below (plus a panic unwind) would otherwise leave the
    // vendor CLI and its node/shell descendants running unreaped in their
    // own process group.
    // `spawn_supervised` registers the group with the interrupt signals
    // blocked across the spawn (the guard's own register below is then the
    // idempotent no-op), so an interrupt cannot land in the gap between a
    // bare spawn and the bracket taking effect.
    let mut guard = LoginChildGuard::new(
        crate::support::supervise::spawn_supervised(&mut cmd).map_err(|error| {
            CliError::failed(format!(
                "{} failed to start: {error} (install the connector CLI first: pinvou connectors \
                 ensure-cli {})",
                spec.cli_bin, spec.id
            ))
        })?,
    );
    // Two rendezvous slots: first auth-domain URL and first user code.
    // Vendor CLIs that never print a separate code line (feishu phase 1,
    // wecom, tmeet, dws) must not burn the whole `login_url_wait_secs`
    // window after the link is already in hand, so the wait ends at the
    // first URL for them — like the GUI loops (feishu.rs takes a single
    // `recv_timeout`, tmeet.rs `break`s on the first auth line). dingtalk
    // is the exception: its login page needs the `user_code` the vendor
    // prints on a separate line, and the GUI loop keeps draining after the
    // bare URL (`dingtalk.rs` stashes `plain_url` and returns only once
    // both halves are in hand). For dingtalk the CLI therefore keeps
    // draining for a short grace after a bare URL — long enough for the
    // code line that follows the link in practice, while a CLI that
    // already exited (no code is coming) falls through immediately via
    // the `Disconnected` branch instead of waiting the window out.
    // Whatever code has arrived is composed into the URL by the caller
    // (`compose_user_code`) because the login page asks for a code the
    // raw URL does not carry.
    let (tx, rx) = mpsc::channel::<LoginStreamEvent>();
    if let Some(stdout) = guard.child_mut().stdout.take() {
        drain_for_url(spec, stdout, tx.clone());
    }
    if let Some(stderr) = guard.child_mut().stderr.take() {
        drain_for_url(spec, stderr, tx.clone());
    }
    drop(tx);
    // Bounded tail of the vendor's own output, mirroring the GUI's 32-entry
    // ring buffer (`tmeet.rs::remember_auth_line`). Dropping the oldest line
    // keeps the newest ones — the failure reason is at the end.
    let mut remember = |line: String| {
        if tail.len() >= VENDOR_LOG_TAIL_LINES {
            tail.remove(0);
        }
        tail.push(line);
    };
    // The 1 s floor applies only while budget remains: a spent deadline must
    // not buy an extra second past it.
    let remaining = deadline.saturating_duration_since(Instant::now());
    let url_wait = if remaining.is_zero() {
        Duration::ZERO
    } else {
        // The 1 s floor applies only while the remaining budget can afford
        // it: a sub-second remainder must not be rounded UP past the
        // connect deadline.
        Duration::from_secs(spec.login_url_wait_secs)
            .min(remaining)
            .max(Duration::from_secs(1))
            .min(remaining)
    };
    let mut url: Option<String> = None;
    let mut user_code: Option<String> = None;
    let needs_code_line = spec.id == "dingtalk";
    // Once a bare URL is in hand, the code line — when the vendor emits one
    // — follows within moments of the same login initiation, so a short
    // grace (the qr.png precedent) suffices; the full window stays reserved
    // for capturing the URL itself.
    let code_line_grace = Duration::from_secs(10);
    let mut code_line_deadline: Option<Instant> = None;
    // wecom writes the one-scan `qr.png` around the time it prints the
    // landing-page URL. Poll it on the same tick as the login events: the
    // path is the actionable artifact and must be visible while the login
    // process runs, not 40s later when the URL window expires.
    let qr_png = work_dir.map(|dir| dir.join("qr.png"));
    let mut qr_announced = false;
    let deadline_wait = Instant::now() + url_wait;
    while url.is_none() || code_line_deadline.is_some() {
        let wait_until = code_line_deadline.unwrap_or(deadline_wait);
        let remaining = wait_until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        if let Some(qr) = &qr_png {
            if !qr_announced {
                qr_announced = announce_wecom_qr(qr, notes);
            }
        }
        match rx.recv_timeout(Duration::from_millis(200).min(remaining)) {
            Ok(LoginStreamEvent::Url(found)) => {
                // The vendor CLI stays alive until the user authorizes, so
                // this is the only moment the link is actionable — surface
                // it immediately (stderr keeps `--output json` stdout
                // single-line) and record it for the final summary and any
                // later error. The printed/recorded form is control-char
                // collapsed like every other human cell; the event keeps
                // the raw capture for the `user_code=` logic below.
                let printable = crate::support::collapse_control_characters(&found);
                note!("{} login link: {printable}", spec.cli_bin);
                notes.push(format!("login link: {printable}"));
                let carries_code = found.contains("user_code=");
                url = Some(found);
                if needs_code_line && !carries_code && user_code.is_none() {
                    code_line_deadline = Some(
                        Instant::now()
                            + code_line_grace
                                .min(deadline_wait.saturating_duration_since(Instant::now())),
                    );
                }
            }
            Ok(LoginStreamEvent::Code(code)) => {
                note!("{} user code: {code}", spec.cli_bin);
                notes.push(format!("user code: {code}"));
                user_code = Some(code);
            }
            Ok(LoginStreamEvent::Log(line)) => {
                remember(line);
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let url_complete = url
            .as_deref()
            .is_some_and(|found| found.contains("user_code="));
        if url.is_some() && (!needs_code_line || url_complete || user_code.is_some()) {
            break;
        }
    }
    if let Some(qr) = &qr_png {
        if !qr_announced {
            // Breaking on the first URL can win the race against the vendor
            // writing `qr.png` (the GUI polls the PNG for up to 6s after the
            // URL — `wecom.rs::poll_qr_png`); give the file the same short
            // grace window, capped by the connect budget, so the one-scan
            // note is not lost.
            let qr_deadline = (Instant::now() + Duration::from_secs(6)).min(deadline);
            while !qr.is_file() && Instant::now() < qr_deadline {
                std::thread::sleep(Duration::from_millis(150));
            }
            announce_wecom_qr(qr, notes);
        }
    }
    if url.is_none() {
        notes.push(format!(
            "no login link within {url_wait:?} (check network / proxy); still waiting for the CLI to exit"
        ));
    } else if needs_code_line
        && user_code.is_none()
        && !url
            .as_deref()
            .is_some_and(|found| found.contains("user_code="))
    {
        notes.push(
            "no separate user code line arrived; the dingtalk login page asks for a code the \
             CLI never printed, so the link alone cannot complete the login"
                .to_string(),
        );
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    let wait = guard.child_mut().wait_timeout(remaining);
    // Whatever the drain threads produced after the loop above stopped
    // reading (it breaks at the first URL) is still queued; absorb it
    // non-blockingly before the verdict is formatted, so the failure message
    // carries the vendor's last words and not just its first. The drains are
    // byte-capped (`LOGIN_DRAIN_CAP_BYTES`), so the queue is bounded too.
    for event in rx.try_iter() {
        match event {
            // The queue after the loop still carries events the loop never
            // consumed: a `Code` (or `Url`) queued in the final tick before
            // the break is the vendor's real pairing material — dropping it
            // composes a URL without its code, the one-way-code-drop class
            // the round-24/25 fixes closed on the forward path.
            LoginStreamEvent::Code(code) => {
                if user_code.is_none() {
                    note!("{} user code: {code}", spec.cli_bin);
                    notes.push(format!("user code: {code}"));
                    user_code = Some(code);
                }
            }
            LoginStreamEvent::Url(found) => {
                if url.is_none() {
                    url = Some(found);
                }
            }
            LoginStreamEvent::Log(line) => remember(line),
        }
    }
    let status = match wait {
        Ok(Some(status)) => {
            // `wait_timeout` reaped the child; killing its process group now
            // could hit a reused pid, so the guard is disarmed without a
            // kill (the same hazard `run_cli_bounded` documents).
            guard.disarm();
            status
        }
        Ok(None) => {
            // Still alive and verifiably unreaped: kill the group, then
            // disarm so Drop does not repeat it.
            guard.kill_and_disarm();
            return Err(CliError::failed(format!(
                "{} login timed out (connect budget exhausted; retry with a larger --timeout){}{}",
                spec.cli_bin,
                captured_notes(notes),
                vendor_output_tail(spec, tail.as_slice()),
            )));
        }
        Err(error) => {
            // The child's fate is unknown, so the guard stays armed and kills
            // the group on the way out.
            return Err(CliError::failed(format!(
                "waiting for {} failed: {error}{}",
                spec.cli_bin,
                vendor_output_tail(spec, tail.as_slice())
            )));
        }
    };
    Ok((url, user_code, status.success()))
}

/// Bounded tail of the vendor CLI's own output kept for failure messages,
/// mirroring the 32-entry ring buffer the GUI keeps per login
/// (`tmeet.rs::remember_auth_line`, `dingtalk.rs`).
const VENDOR_LOG_TAIL_LINES: usize = 32;

/// RAII around a spawned vendor login child. `connect` can sit in
/// `wait_timeout` for the full `--timeout` budget (5 minutes by default)
/// while a human scans a QR code, and the child was put in its own process
/// group by `set_process_group` — so nothing else reaps it. Before this
/// guard, `kill_process_tree` was reached only on the deadline branch, and
/// every other early return (a wait error, `?` on a helper, a panic unwind)
/// orphaned `lark-cli` / `dws` and their node descendants.
///
/// Scope, stated plainly: Drop covers the RETURN and UNWIND paths only. A
/// bare terminal SIGINT terminates the process without unwinding, so no
/// Drop runs there — which is why the constructor registers the child's
/// group with `crate::support::supervise` and the settled-fate paths forget
/// it: the supervisor's watcher is what takes the group down on a terminal
/// interrupt (the hole the old comment claimed needed a dependency the CLI
/// does not carry; it carries `libc` and installs the handler in `main`,
/// and `code login`/`voice` have bracketed their children this way since
/// round 18). The GUI's equivalent is
/// `ConnectorConn::kill_all_pids()` on `RunEvent::Exit`, a different
/// mechanism for a process that owns a runtime; a one-shot CLI takes the
/// signal-handler route.
struct LoginChildGuard {
    child: std::process::Child,
    /// Set once the child's fate is settled; Drop then does nothing.
    disarmed: bool,
}

impl LoginChildGuard {
    fn new(child: std::process::Child) -> Self {
        // The SIGINT guard every other long-running child in this crate is
        // bracketed with (`code login`, `voice`): this login child can sit
        // in its wait for the whole `--timeout` budget (default 300 s, up
        // to 604800 s) while a human scans, and a terminal SIGINT must take
        // it down with the CLI. Paired with the forget in `disarm` /
        // `kill_and_disarm` / Drop, the settled-fate paths, so the pgid
        // never stays registered for the OS to recycle onto an unrelated
        // process.
        crate::support::supervise::register_child_group(child.id());
        Self {
            child,
            disarmed: false,
        }
    }

    fn child_mut(&mut self) -> &mut std::process::Child {
        &mut self.child
    }

    /// The child was reaped: killing its group afterwards could race a reused
    /// pid, so stand down without killing. The supervise registration is
    /// released with it — the group is no longer this process's to signal.
    fn disarm(&mut self) {
        crate::support::supervise::forget_child_group(self.child.id());
        self.disarmed = true;
    }

    /// Kill the group now (the caller has verified the child is still alive)
    /// and stand down so Drop does not repeat it. The kill already takes the
    /// whole group down, so the supervise registration is released here.
    fn kill_and_disarm(&mut self) {
        crate::support::kill_process_tree(&mut self.child);
        crate::support::supervise::forget_child_group(self.child.id());
        self.disarmed = true;
    }
}

impl Drop for LoginChildGuard {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        crate::support::kill_process_tree(&mut self.child);
        // The group kill and the exit both ran: the registration has no
        // child left to cover, so it leaves with this guard.
        crate::support::supervise::forget_child_group(self.child.id());
    }
}

/// Announce the wecom one-scan `qr.png` once (stderr + the notes a failure
/// carries), mirroring the GUI's `wecom:qr` emit. Returns whether the file
/// was there to announce.
///
/// The announcement states the file's lifetime, because the CLI's differs
/// from the GUI's on purpose. The GUI renders the QR into the window and
/// always `remove_dir_all`s the temp dir; a headless user needs the PNG to
/// survive so they can open it, so the failure path deliberately keeps it
/// (`connect`'s wecom arm). Deleting it there is not an option — it is the
/// only artifact that can still finish the login — so the honest fix is to
/// tell the user what it is and that its removal is theirs: the image
/// encodes a one-scan login grant, i.e. credential-equivalent material, and
/// on non-unix hosts it is not even behind the `0o700` directory mode.
fn announce_wecom_qr(qr: &Path, notes: &mut Vec<String>) -> bool {
    if !qr.is_file() {
        return false;
    }
    note!(
        "wecom scan-qr-file: {} (scan this PNG to authorize in one step; it holds a one-scan \
         login grant — delete it once you are done, it is only removed automatically when the \
         login succeeds)",
        qr.display()
    );
    notes.push(format!(
        "scan-qr-file: {} (the login link is a landing page; scan this PNG to \
         authorize in one step; it holds a one-scan login grant — delete it once you are done, \
         it is only removed automatically when the login succeeds)",
        qr.display()
    ));
    true
}

/// Suffix for connect errors that carries the login material captured so far
/// — after a timeout the notes are often all the user has (the vendor CLI is
/// gone and its output was piped, not printed).
fn captured_notes(notes: &[String]) -> String {
    if notes.is_empty() {
        String::new()
    } else {
        format!("; captured so far: {}", notes.join(" | "))
    }
}

/// Suffix carrying the vendor CLI's own last words, already redacted by
/// [`safe_auth_log_line`]. The vendor's stdout/stderr are piped so the
/// drainer can find the login link, which means the terminal never shows
/// them: without this a headless user hitting the commonest dingtalk
/// onboarding blocker ("CLI data access is not enabled", which needs an
/// admin to flip a switch in the DingTalk developer console) gets a bare
/// "login exited before authorization completed" and no reason at all. The
/// GUI turns the same lines into actionable guidance
/// (`dingtalk.rs::dingtalk_auth_error_hint`, `tmeet.rs::auth_failure_message`).
/// Failure path only — see `connect`'s `tail`.
fn vendor_output_tail(spec: &VendorSpec, tail: &[String]) -> String {
    if tail.is_empty() {
        return String::new();
    }
    format!(
        "; last {} line(s) of {} output (redacted): {}",
        tail.len(),
        spec.cli_bin,
        tail.join(" | ")
    )
}

/// Mirror of `features/connectors/connector_cli.rs::safe_auth_log_line`
/// (crate-private to the app, so it cannot be imported — that function is
/// the source of truth and this copy must follow it): drop blank lines,
/// replace a line carrying credential material wholesale, and truncate the
/// rest to 320 characters.
///
/// `redact_bare_token` follows the GUI's per-connector choice: tmeet passes
/// `true` because its output says "token" without an underscore, dingtalk
/// passes `false` because its ordinary JSON lines contain camelCase token
/// FIELD NAMES and the fallback would swallow every non-sensitive line.
fn safe_auth_log_line(line: &str, redact_bare_token: bool) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let mut sensitive = lower.contains("access_token")
        || lower.contains("refresh_token")
        || lower.contains("authorization:")
        || lower.contains("bearer ");
    if redact_bare_token {
        sensitive = sensitive || lower.contains("token");
    }
    if sensitive {
        return Some("[redacted credential line]".to_string());
    }
    // Truncate by chars, not bytes: a byte slice could split a multi-byte
    // character and the vendor CLIs emit Chinese diagnostics. The tail is
    // human-facing diagnostic text, so control characters (ESC/CSI from a
    // chatty vendor's coloring, CR redraws) are collapsed like every other
    // human cell in this CLI.
    Some(crate::support::collapse_control_characters(
        &trimmed.chars().take(320).collect::<String>(),
    ))
}

/// Per-connector `redact_bare_token` setting for [`safe_auth_log_line`],
/// mirroring the GUI's per-drain choice exactly: the shared `drain_for_url`
/// hard-codes `safe_auth_log_line(&line, false)` for every connector (its
/// documented reason: feishu/wecom normal output lines carry `token`-shaped
/// FIELD names, so bare-token redaction wholesale-replaces non-sensitive
/// lines and the failure reason is lost); only tmeet's own drain passes
/// `true`. Over-redaction here would reintroduce exactly the swallowed
/// failure reasons the shared drain's comment warns about.
fn redact_bare_token_for(spec: &VendorSpec) -> bool {
    spec.id == "tmeet"
}

/// Stream event collected while a login command runs.
#[derive(Debug)]
enum LoginStreamEvent {
    Url(String),
    Code(String),
    /// One redacted output line, kept only for the bounded failure tail.
    Log(String),
}

/// Mirror of dingtalk `with_user_code_param`: attach the drained user code to
/// the login URL when the URL does not already carry it.
fn compose_user_code(url: &Option<String>, user_code: Option<&str>) -> Option<String> {
    let url = url.as_deref()?;
    let Some(code) = user_code else {
        return Some(url.to_owned());
    };
    if url.contains("user_code=") {
        return Some(url.to_owned());
    }
    let sep = if url.contains('?') { '&' } else { '?' };
    Some(format!("{url}{sep}user_code={code}"))
}

/// Whether the URL's HOST matches one of the connector's auth domains —
/// exact or dot-boundary subdomain. Deliberately stricter than the GUI's
/// `extract_url` substring filter it mirrors: a substring match lets
/// `https://login.dingtalk.com.evil.com/` or
/// `https://attacker.example/?r=dingtalk.com` through as an actionable
/// login link, which is this flow's phishing main channel. A URL that does
/// not parse is not a link worth surfacing.
fn url_host_matches_auth_domain(url: &str, domains: &[&str]) -> bool {
    url::Url::parse(url).ok().is_some_and(|parsed| {
        parsed.host_str().is_some_and(|host| {
            let host = host.to_ascii_lowercase();
            domains.iter().any(|domain| {
                let domain = domain.to_ascii_lowercase();
                host == domain
                    || host
                        .strip_suffix(domain.as_str())
                        .is_some_and(|rest| rest.ends_with('.'))
            })
        })
    })
}

/// Background pipe drainer capturing the first URL whose host matches the
/// connector's auth domains — mirror of `connector_cli::drain_for_url` and
/// `CliCtx::extract_url` (truncate at whitespace, keep query strings) — plus
/// the first `user_code`-style line (mirror of dingtalk `extract_user_code`).
/// Line processing keeps within [`LOGIN_DRAIN_CAP_BYTES`] like
/// `run_cli_bounded`'s drains (that cap is what bounds the event queue the
/// caller absorbs); past the cap the drainer keeps READING — and
/// discarding — until true EOF, so a chatty or hostile vendor CLI (or a
/// descendant that inherited the pipes) can still finish instead of
/// blocking on a full pipe and being misreported as a login timeout.
fn drain_for_url<R: std::io::Read + Send + 'static>(
    spec: &VendorSpec,
    reader: R,
    tx: mpsc::Sender<LoginStreamEvent>,
) -> std::thread::JoinHandle<()> {
    let domains = spec.auth_domains;
    let redact_bare_token = redact_bare_token_for(spec);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        // The caller stops consuming Log events once the URL is in hand and
        // keeps only a bounded tail (`VENDOR_LOG_TAIL_LINES`), so events
        // past this cap would otherwise pile up unbounded in the channel
        // while the login wait blocks (a hostile vendor emitting millions
        // of 1-byte lines would balloon memory before the drain cap ends
        // the stream).
        const LOGIN_LOG_EVENT_CAP: usize = 512;
        let mut log_events = 0usize;
        for line in (&mut reader).take(LOGIN_DRAIN_CAP_BYTES).lines() {
            let line = match line {
                Ok(line) => line,
                Err(_) => break,
            };
            // Every line also goes out as a redacted `Log` event so the
            // caller can keep a bounded tail for failure diagnostics; the
            // vendor's reason for failing is otherwise dropped on the floor
            // here. Receiver-gone ends the drain as with the other events.
            // The event budget bounds only the LOG channel: past it the log
            // send is skipped (with one truncation marker), but the code and
            // URL scans below still run — a chatty vendor whose authorize
            // link arrives after 512 diagnostic lines still gets its link
            // through, which is the whole point of this drain.
            if let Some(safe) = safe_auth_log_line(&line, redact_bare_token) {
                log_events += 1;
                let event = if log_events <= LOGIN_LOG_EVENT_CAP {
                    Some(LoginStreamEvent::Log(safe))
                } else if log_events == LOGIN_LOG_EVENT_CAP + 1 {
                    Some(LoginStreamEvent::Log(
                        "[truncated: vendor diagnostic output exceeded the event budget]"
                            .to_owned(),
                    ))
                } else {
                    None
                };
                if let Some(event) = event {
                    if tx.send(event).is_err() {
                        return;
                    }
                }
            }
            if let Some(code) = extract_user_code(&line) {
                if tx.send(LoginStreamEvent::Code(code)).is_err() {
                    return;
                }
            }
            let Some(index) = line.find("https://") else {
                continue;
            };
            let url: String = line[index..]
                .chars()
                .take_while(|c| !c.is_whitespace())
                .collect();
            if url_host_matches_auth_domain(&url, domains)
                && tx.send(LoginStreamEvent::Url(url)).is_err()
            {
                return;
            }
        }
        // Past the cap: same loop as `drain_vendor_output` — drain to EOF in
        // fixed chunks so the writer can never block on a full pipe. Events
        // stop here: the cap is what bounds the caller's event queue.
        let mut chunk = [0u8; 64 * 1024];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
}

/// Mirror of dingtalk `extract_user_code`: the last 6-32 char uppercase-ish
/// token on a code-labelled line.
fn extract_user_code(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let has_code_label = lower.contains("user_code")
        || lower.contains("user code")
        || lower.contains("device code")
        || lower.contains("code:")
        || line.contains("\u{7528}\u{6237}\u{7801}")
        || line.contains("\u{9a8c}\u{8bc1}\u{7801}")
        || line.contains("\u{6388}\u{6743}\u{7801}");
    if !has_code_label {
        return None;
    }
    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .rfind(|token| {
            (6..=32).contains(&token.len())
                && token
                    .chars()
                    .any(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                && token
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
        })
        .map(str::to_owned)
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
        ImaCommand::Logout { yes } => ima_logout(yes, output),
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
/// sync the DenyAll scopes. The live network + system keyring success path is
/// excluded from the contract tests by design; the hermetic tests cover the
/// failure paths that fire before any network or keyring access (missing
/// client id environment variable, missing api key).
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
        // Same entry point the app's `ima_connect` uses (the skill id is
        // normalized to its package id inside the scope layer), with the
        // app's fail-visible contract (review #455 R13-B3): a failed consent
        // write ERRORS the closure — which rolls the just-stored secrets
        // back below — so the reconnect never completes with credentials
        // committed and consent state lost. The pack itself is not
        // uninstalled (live-by-absence until the denied state applies on
        // reconnect); that residual is what the marker message names.
        pinvou3_lib::features::marketplace::sync_deny_all_scopes_after_install(IMA_SKILL_ID)
            .map_err(|error| {
                CliError::failed(format!(
                    "ima skills installed, but {}: new sessions will enable them by \
default — turn them off in the tools list: {error}",
                    pinvou3_lib::features::marketplace::scope::CONSENT_SYNC_FAILURE_MARKER
                ))
            })?;
        Ok(())
    })();
    if let Err(error) = result {
        // Mirror `rollback_secret`: restore the previous credential state
        // before surfacing the failure, and propagate a failed rollback (a
        // silently half-written credential state is worse than the original
        // error).
        let restore = |reference: &CredentialReference, previous: Option<String>| {
            let outcome = match previous {
                Some(value) => store.set(reference, &value).err(),
                None => store.delete(reference).err(),
            };
            if let Some(rollback_error) = outcome {
                return Err(CliError::failed(format!(
                    "ima connect failed ({error}) and the credential rollback also failed: \
                     {}",
                    rollback_error.user_message()
                )));
            }
            Ok(())
        };
        restore(&client_ref, previous_client_id)?;
        restore(&api_ref, previous_api_key)?;
        return Err(error);
    }
    let value = json!({ "ok": true, "id": "ima", "connected": true });
    Ok(success(render(output, "ima connected".to_owned(), &value)))
}

/// Mirror of `ima.rs::validate_credentials` + `request_ima`: one POST to the
/// allowlisted `openapi/check_skill_update` path; `code == 0` means the
/// credentials are accepted.
/// Response cap mirroring `ima.rs` (`MAX_RESPONSE_BYTES`).
const IMA_MAX_RESPONSE_BYTES: u64 = 1024 * 1024;

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
    // Mirror the GUI's 1 MiB response cap: a hostile or broken server must
    // not be able to balloon the CLI's memory.
    use std::io::Read as _;
    let mut bytes = Vec::new();
    response
        .take(IMA_MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| CliError::failed("cannot read the ima response"))?;
    if bytes.len() as u64 > IMA_MAX_RESPONSE_BYTES {
        return Err(CliError::failed("ima response exceeds the 1 MiB cap"));
    }
    let payload: Value = serde_json::from_slice(&bytes)
        .map_err(|_| CliError::failed("ima response is not valid JSON"))?;
    let code = payload
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(|| CliError::failed("ima validation response has no code field"))?;
    if code == 0 {
        return Ok(());
    }
    // The server message may echo request material (client id / api key) on
    // hostile or misbehaving responses; strip the exact known values like
    // `ima.rs::redact_known_credentials` — the heuristic `redact_secret`
    // alone can miss short or punctuation-heavy credentials — then run the
    // heuristic for anything secret-shaped left over.
    let message = payload
        .get("msg")
        .and_then(Value::as_str)
        .unwrap_or("ima OpenAPI authentication failed; check client id / api key");
    Err(CliError::failed(redact_secret(
        &redact_ima_known_credentials(message.to_owned(), client_id, api_key),
    )))
}

/// Mirror of `ima.rs::redact_known_credentials`: remove the exact client id /
/// api key values from a server-supplied message before it reaches the user.
fn redact_ima_known_credentials(mut text: String, client_id: &str, api_key: &str) -> String {
    for secret in [client_id, api_key] {
        if secret.is_empty() {
            continue;
        }
        // GUI parity: the JSON-quoted form (`ima.rs` redacts serialized
        // response payloads)…
        if let Ok(json_secret) = serde_json::to_string(secret) {
            text = text.replace(&json_secret, "\"[REDACTED]\"");
        }
        // …and the bare value: the CLI surfaces the plain `msg` field, where
        // an echoed credential carries no JSON quotes.
        text = text.replace(secret, "[REDACTED]");
    }
    text
}

/// Mirror of `ima_logout`: delete both secrets, uninstall ima-skills, then
/// remove its consent rows.
///
/// Deviations from the GUI, on purpose:
/// - Reporting (round-18 finding 4): the GUI degrades a cleanup persist
///   failure to `log::warn!`, but this logout reports itself complete, so
///   the CLI cannot swallow a failed cleanup — a stale disabled entry would
///   keep a reconnected ima hidden from the very scopes that just wrote it.
///   The same error the GUI logs is surfaced here instead.
/// - Ordering (GUI parity, `ima.rs`): a failed skill uninstall stops the run
///   BEFORE the consent cleanup. With `bundles/ima/` still on disk, stripping
///   the consent rows would set ima-skills live with zero consent in every
///   initialized scope — the fail-open direction; keeping the rows leaves
///   the logout retryable (rows and pack dir both intact).
///
/// The cleanup uses the GUI's exact-owner raw-file form
/// (`remove_bundle_from_disabled_scopes_exact`), not a normalized
/// load/modify/save: the normalized load re-owns dir-absent ids onto foreign
/// packs and persists the remap before the removal runs (the round-26 MAJOR
/// the exact form exists to prevent), and it would also miss the default-off
/// markers and the install-default sync ledger the exact form clears.
fn ima_logout(yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    let store = SystemCredentialStore::new();
    let client_result = store.delete(&ima_secret_ref("client_id"));
    let api_result = store.delete(&ima_secret_ref("api_key"));
    // The skill uninstall is part of this logout's promised state: a failed
    // one is collected here and named in the error below rather than being
    // swallowed — and it GATES the consent cleanup (see the module doc).
    let skill_uninstall_failure = match SkillMarketplaceManager::new().uninstall(IMA_SKILL_ID) {
        Ok(()) => None,
        Err(error) => Some(format!("skill uninstall: {error}")),
    };
    let cleanup_note = if skill_uninstall_failure.is_some() {
        "the skill uninstall FAILED (its consent rows were kept, GUI parity)"
    } else {
        "the skill uninstall succeeded and the consent cleanup already ran"
    };
    // Consent cleanup runs only when the uninstall landed, through the
    // GUI's exact-owner form (all scopes, hidden rows, default-off markers,
    // and the install-default ledger in one raw-file pass).
    let scope_failures: Vec<String> = match skill_uninstall_failure {
        Some(_) => Vec::new(),
        None => {
            use pinvou3_lib::features::marketplace::scope::{
                package_id_for, remove_bundle_from_disabled_scopes_exact,
            };
            let package_id = package_id_for(IMA_SKILL_ID);
            match remove_bundle_from_disabled_scopes_exact(&package_id) {
                Ok(()) => Vec::new(),
                Err(error) => vec![format!("consent cleanup: {error}")],
            }
        }
    };
    client_result.map_err(|error| {
        credential_error(format!(
            "{error}. Note: {cleanup_note} before this failure; retrying only \
             repeats the credential store step",
        ))
    })?;
    let api_error = api_result.err().map(|error| {
        credential_error(format!(
            "{error}. Note: {cleanup_note} before this failure; retrying only \
             repeats the credential store step",
        ))
    });
    // A skill-uninstall or consent-cleanup failure is partial state, not a
    // masked success: both join in one honest report naming what landed and
    // what did not.
    let mut partial: Vec<String> = Vec::new();
    if let Some(failure) = skill_uninstall_failure {
        partial.push(failure);
    }
    partial.extend(scope_failures);
    if let Some(api_failure) = api_error {
        return Err(api_failure);
    }
    if !partial.is_empty() {
        return Err(CliError::failed(format!(
            "connectors ima logout: partial state — the ima secrets were deleted, but \
             ({}); fix and retry the failed halves. What landed stays landed",
            partial.join("; "),
        )));
    }
    let value = json!({ "ok": true, "id": "ima", "connected": false });
    Ok(success(render(output, "ima logged out".to_owned(), &value)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ima_error_redaction_removes_the_exact_credential_values() {
        // A misbehaving validation endpoint can echo the request material in
        // `msg`; the exact client id / api key must be gone even when the
        // heuristic `redact_secret` would not flag them (short, unprefixed).
        let message =
            "authentication failed for client id ima-client-1 and api key ima-key-2".to_owned();
        let redacted = redact_ima_known_credentials(message, "ima-client-1", "ima-key-2");
        assert!(!redacted.contains("ima-client-1"), "{redacted}");
        assert!(!redacted.contains("ima-key-2"), "{redacted}");
        assert!(redacted.contains("[REDACTED]"), "{redacted}");
    }

    #[test]
    fn ima_error_redaction_keeps_the_gui_quoted_form() {
        // GUI parity (`ima.rs::redact_known_credentials`): the JSON-quoted
        // echo in a serialized payload is replaced without corrupting the
        // surrounding text.
        let message = r#"{"error":"client_id ima-client-1 rejected"}"#.to_owned();
        let redacted = redact_ima_known_credentials(message, "ima-client-1", "api-key-ignored");
        assert!(!redacted.contains("ima-client-1"), "{redacted}");
        assert!(redacted.contains("rejected"), "{redacted}");
    }

    #[test]
    fn drain_for_url_drains_past_the_cap_and_reaches_eof() {
        // A drainer that only kept the capped prefix would leave a writer
        // blocked on a full pipe — the exact "healthy vendor misreported as
        // timed out" failure `run_cli_bounded`'s drain-past-cap discipline
        // exists to prevent. The input is newline-free (the hostile single
        // giant line) and longer than the cap: the drainer must consume the
        // WHOLE stream and finish, not stop at the cap.
        struct Counting {
            inner: std::io::Cursor<Vec<u8>>,
            read: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        }
        impl std::io::Read for Counting {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.inner.read(buf)?;
                self.read.fetch_add(n, std::sync::atomic::Ordering::SeqCst);
                Ok(n)
            }
        }
        let read = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let total = LOGIN_DRAIN_CAP_BYTES as usize + 4096;
        let counter = Counting {
            inner: std::io::Cursor::new(vec![b'x'; total]),
            read: std::sync::Arc::clone(&read),
        };
        let (tx, _rx) = mpsc::channel::<LoginStreamEvent>();
        let drain = drain_for_url(ConnectorKind::Dingtalk.spec(), counter, tx);
        drain
            .join()
            .expect("drainer must reach EOF past the cap instead of blocking forever");
        assert_eq!(
            read.load(std::sync::atomic::Ordering::SeqCst),
            total,
            "the drainer must consume the whole stream, not stop at the cap"
        );
    }

    #[test]
    fn drain_for_url_forwards_code_and_url_and_mirrors_every_line_as_a_log() {
        let (tx, rx) = mpsc::channel::<LoginStreamEvent>();
        let input: &[u8] = b"noise\nUser Code: ZXCV1234\nvisit https://login.dingtalk.com/oauth/authorize?x=1 now\n";
        let drain = drain_for_url(ConnectorKind::Dingtalk.spec(), input, tx);
        let mut logs = Vec::new();
        let mut code = None;
        let mut url = None;
        while let Ok(event) = rx.recv_timeout(Duration::from_secs(5)) {
            match event {
                LoginStreamEvent::Log(line) => logs.push(line),
                LoginStreamEvent::Code(found) => code = Some(found),
                LoginStreamEvent::Url(found) => url = Some(found),
            }
        }
        assert_eq!(code.as_deref(), Some("ZXCV1234"));
        assert_eq!(
            url.as_deref(),
            Some("https://login.dingtalk.com/oauth/authorize?x=1")
        );
        // Every non-blank line is also mirrored as a `Log` so the caller can
        // keep a bounded tail for the failure message; a line that matches
        // nothing (`noise`) is exactly the kind the vendor puts its failure
        // reason on, so it may not be dropped.
        assert_eq!(logs.len(), 3, "{logs:?}");
        assert_eq!(logs[0], "noise");
        drain.join().expect("drainer thread must not panic");
    }

    /// `member_path_is_safe` is the tar lane's one gatekeeper: every
    /// non-normal component must be refused so a traversal- or root-shaped
    /// member name can never reach `tar` as an extraction target, while
    /// ordinary nested and `.`-relative names stay selectable (the connector
    /// archives lay the binary out in nested directories).
    #[test]
    fn member_path_is_safe_rejects_every_non_normal_component() {
        // Selectable: normal components, nesting, and `.`-relative names.
        assert!(member_path_is_safe("payload/spool/wecom-cli"));
        assert!(member_path_is_safe("wecom-cli"));
        assert!(member_path_is_safe("./payload/wecom-cli"));
        assert!(member_path_is_safe("payload/./wecom-cli"));
        // Refused: parent traversal in every position, absolute roots,
        // Windows drive prefixes (backslash check: unix `Path` parses
        // `C:\Windows` as one Normal component, so the components check
        // alone would pass Windows-shaped names through), and an empty
        // name. (`~` is NOT refused: it is a Normal component that tar
        // receives as a literal operand — no shell expansion happens on
        // this path, so it cannot steer extraction anywhere.)
        for entry in [
            "../../etc/passwd",
            "a/../../b",
            "payload/..",
            "../x",
            "/abs/wecom-cli",
            "C:\\Windows\\system32\\wecom-cli",
            "\\\\server\\share\\wecom-cli",
            "",
        ] {
            assert!(
                !member_path_is_safe(entry),
                "`{entry}` must never be handed to tar as an extraction target"
            );
        }
    }

    /// The listing feed skips (not merely unselects) traversal-shaped member
    /// lines — the wanted file name could otherwise be reached through a
    /// `..` chain on an archive that also carries a same-named nested file.
    /// This is the selection loop's own filter, pinned independently of the
    /// helper above so removing either refusal shows up as a failure here.
    #[test]
    fn listing_selection_prefers_a_safe_member_over_traversal_shaped_ones() {
        let listing = "a/../../wecom-cli\n../../wecom-cli\nsafe/dir/wecom-cli\n";
        // Same predicate chain as `extract_member`'s find: the safe line wins
        // even though the traversal lines also end in the wanted file name.
        let selected = listing
            .lines()
            .map(str::trim)
            .filter(|entry| member_path_is_safe(entry))
            .find(|entry| {
                Path::new(entry)
                    .file_name()
                    .map(|name| name.to_string_lossy() == "wecom-cli")
                    .unwrap_or(false)
            });
        assert_eq!(selected, Some("safe/dir/wecom-cli"));
    }

    /// Both tar child constructions must carry the `--` operator separator
    /// before any operand the archive controls, so a member whose name begins
    /// with a dash is treated as an operand, not parsed as options. Runs the
    /// REAL `extract_member` (both production children: the `-tf` listing and
    /// the `-xOf` extraction) against a hand-built archive whose wanted
    /// member is dash-prefixed — the exact shape that flips from extracted
    /// to option-parsed without the separator, verified live against bsdtar:
    /// `-xOf a.tar -- -dashmember.txt` extracts, `-xOf a.tar -dashmember.txt`
    /// prints a usage screen and exits 1.
    #[test]
    fn tar_children_carry_the_operand_separator_after_the_archive() {
        let workspace = std::env::temp_dir().join(format!(
            "pinvou-cli-connectors-tar-sep-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&workspace).expect("test workspace");
        let archive = workspace.join("a.tar");
        let member = "-dashmember.txt";
        let contents = b"separator-works";
        std::fs::write(workspace.join(member), contents).expect("seed the member file");
        // Create with a separator (same reason: tar -c would parse the
        // dash-prefixed name as options without one).
        let create = Command::new("tar")
            .arg("-cf")
            .arg(&archive)
            .arg("--")
            .arg(member)
            .current_dir(&workspace)
            .output()
            .expect("tar create must run");
        assert!(create.status.success(), "tar create: {create:?}");
        let target = workspace.join("out");
        std::fs::create_dir_all(&target).expect("target dir");
        extract_member(&archive, member, &target)
            .expect("the dash-prefixed member must extract through the production argv with `--`");
        assert_eq!(
            std::fs::read(target.join(member)).expect("extracted member lands under target"),
            contents
        );
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// The tail must never carry credential material: the redaction mirrors
    /// `connector_cli::safe_auth_log_line`, including its per-connector
    /// `redact_bare_token` split — the GUI's shared drain passes `false` for
    /// EVERY connector (feishu/wecom ordinary lines carry token-shaped FIELD
    /// names), and only tmeet's own drain passes `true`.
    #[test]
    fn safe_auth_log_line_mirrors_the_gui_redaction() {
        assert_eq!(
            safe_auth_log_line("access_token=secret", false).as_deref(),
            Some("[redacted credential line]")
        );
        assert_eq!(
            safe_auth_log_line("Authorization: Bearer secret", false).as_deref(),
            Some("[redacted credential line]")
        );
        assert_eq!(
            safe_auth_log_line("  hello  ", false).as_deref(),
            Some("hello")
        );
        assert_eq!(safe_auth_log_line("   ", false), None);
        // A bare `token` word: kept for dingtalk (its ordinary JSON lines
        // carry camelCase token FIELD names and the fallback would swallow
        // every one of them), redacted everywhere else.
        assert_eq!(
            safe_auth_log_line("refreshing the token now", false).as_deref(),
            Some("refreshing the token now")
        );
        assert_eq!(
            safe_auth_log_line("refreshing the token now", true).as_deref(),
            Some("[redacted credential line]")
        );
        assert!(!redact_bare_token_for(ConnectorKind::Dingtalk.spec()));
        // feishu/wecom stay at `false` with the GUI's shared drain; only
        // tmeet opts in.
        assert!(!redact_bare_token_for(ConnectorKind::Feishu.spec()));
        assert!(!redact_bare_token_for(ConnectorKind::Wecom.spec()));
        assert!(redact_bare_token_for(ConnectorKind::Tmeet.spec()));

        // The auth-domain matcher is host-suffix based, so the feishu spec
        // must carry REGISTRABLE domains: the GUI's bare labels ("feishu")
        // can never suffix-match open.feishu.cn, and the phase-1 register
        // URL was silently dropped (the BLOCKER this pins).
        let domains = ConnectorKind::Feishu.spec().auth_domains;
        assert!(url_host_matches_auth_domain(
            "https://open.feishu.cn/app?ticket=reg",
            domains
        ));
        assert!(url_host_matches_auth_domain(
            "https://accounts.feishu.cn/authorize?x=1",
            domains
        ));
        assert!(url_host_matches_auth_domain(
            "https://open.larksuite.com/app?ticket=reg",
            domains
        ));
        // A lookalike host is still rejected: suffix semantics, not substring.
        assert!(!url_host_matches_auth_domain(
            "https://evil.example.com/feishu-phish",
            domains
        ));

        // Truncation is by chars, not bytes: the vendor CLIs emit Chinese
        // diagnostics and a byte slice would split a code point.
        let long: String = "字".repeat(400);
        assert_eq!(
            safe_auth_log_line(&long, false).map(|line| line.chars().count()),
            Some(320)
        );
    }
}
