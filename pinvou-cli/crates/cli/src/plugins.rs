//! `plugins` family: marketplace tools (MCP packages), skills, package
//! import/export, recycle bin, bundle readiness, and per-scope enable/disable,
//! mirroring `pinvou3-app/src-tauri/src/app/commands/marketplace.rs` and
//! `app/commands/connectors.rs` on the same `pinvou3_lib::features::marketplace`
//! building blocks:
//! - tools list/install/uninstall → `MarketplaceManager::list_tools` /
//!   `install` / `uninstall` plus the companion-skill and DenyAll-scope
//!   follow-ups the GUI command layer performs (`companion_skills`,
//!   `skill_marketplace::SkillMarketplaceManager::install`,
//!   `skill_scope::sync_deny_all_scopes_after_skill_install`,
//!   `sync_deny_all_scopes_after_install`, `remove_connector_from_disabled_scopes`).
//!   The GUI's post-install `validate_remote_connection` handshake and the
//!   OAuth token deletion on uninstall run inside the Tauri host on the
//!   foundation's async MCP stack, which the CLI does not link; the CLI prints
//!   an explicit warning (validation) instead of silently skipping. Two
//!   consequences are behavioural, not cosmetic, and are printed at the point
//!   of action: `install` skips not only the handshake but the ROLLBACK it
//!   guards (the GUI uninstalls a tool whose handshake fails, so the CLI can
//!   leave installed a tool the GUI would have removed), and `uninstall`
//!   leaves any stored remote OAuth tokens behind, so a reinstall of that tool
//!   is still authorized.
//! - tools auth → `get_marketplace_tool_auth_status` fields computed from
//!   `installed_ids` / `oauth_remote_server_name` / `mcp.json`. The OAuth
//!   token presence probe lives in the foundation's token store (not reachable
//!   headless), so `oauth_token_present` is reported conservatively as `false`
//!   with `token_check: "unavailable_in_cli"`. A damaged `mcp.json` degrades
//!   rather than failing the status read (the GUI logs and continues with
//!   `mcp_configured = false`), and the `mcp.json` shape is validated against
//!   the same field types the GUI's typed `McpConfig` deserialization enforces
//!   — an untyped presence check reported `mcp_configured: true` for files the
//!   GUI rejects.
//! - tools oauth-login / oauth-cancel → same guards as
//!   `start_marketplace_tool_oauth_login` (tool must declare a remote OAuth
//!   server present in `mcp.json`). The login flow itself
//!   (`deepseek_tui::mcp::oauth::perform_oauth_login_for_server_with_cancel`)
//!   is foundation-internal and not reachable from this crate, so the command
//!   fails with `oauth_login_unavailable_in_cli` instead of half-reimplementing
//!   PKCE + callback + token persistence that the desktop app could not read.
//! - skills list/install/update/uninstall → `SkillMarketplaceManager` +
//!   `skill_scope` exactly like `install_marketplace_skill_sync` /
//!   `update_marketplace_skill` / `uninstall_marketplace_skill_sync`.
//! - import → `plugin_import::import_plugin_package` (the unified plugin
//!   upload pipeline behind `import_plugin_package_cmd`), replacing the GUI
//!   native file dialog with an explicit path: `.zip` packages, `.md` /
//!   `.markdown` skill files (wrapped into a root-SKILL.md zip like the GUI's
//!   `import_skill_md_content`), and SKILL.md directories (zipped the same
//!   way; the GUI has no directory input). Single-file fallback id derivation
//!   mirrors the GUI's `sanitize_skill_name` + FNV-1a `stable_stem_hash`.
//!   The pre-pipeline wrap reads are bounded per file and cumulatively by the
//!   pipeline's own package limit (including the frontmatter the wrapper
//!   prepends, which is charged in place of the raw SKILL.md it replaces), and
//!   the `.zip` channel pre-flights the same limit before handoff (the
//!   importer bounds only what it extracts, after the fact); non-regular
//!   inputs (FIFOs, devices) are rejected before any open (opening a FIFO
//!   would block until an unrelated writer appears). The stored display name
//!   goes through the GUI's `sanitize_display_name` rules (drop `/` and `\`,
//!   cap at 128 chars) plus this crate's stricter zero-width/bidi hygiene, so
//!   both surfaces store the same name for the same file.
//! - export → `package_export::export_installed_plugin`; recycle →
//!   `recycle_bin::{RecycleBin, restore_plugin}`; meta →
//!   `SkillMarketplaceManager::update_display_meta`.
//! - readiness → `bundle::BundleRegistry` + `readiness_for` (the registry
//!   branch of `bundle_readiness`). CLI-connector live status (feishu/wecom/
//!   dingtalk/tmeet `*_status`) and ima credential status need the connector
//!   runtime; the CLI reports the registry's conservative readiness and
//!   defers live probes to the `connectors` family. Because a CLI connector's
//!   readiness IS its connection state, that deferral means the verdict for a
//!   `cli`-kind bundle is not decidable here at all: those rows are reported
//!   as `ready: false` with `probe: "unavailable_in_cli"` (and reason
//!   `connection_unknown_in_cli` when nothing else is wrong) rather than
//!   claiming a readiness no probe established — the same disclosure shape as
//!   `oauth_token_present` / `token_check` above. Every row carries `probe`,
//!   so `registry` rows are equally self-describing and the JSON shape does
//!   not vary by kind. In short: `readiness` NEVER reports a `cli`-kind bundle
//!   as ready, whatever its real state — use `pinvou connectors ... status`
//!   for that verdict. Credential presence is consulted in the system
//!   credential store for every installed bundle, so a read-only CLI run CAN
//!   touch the OS keyring (macOS may prompt) — only a run with nothing
//!   installed never does. The `assets_missing` demotion of a `degraded`
//!   non-CLI package is derived in this module rather than in `readiness_for`,
//!   so the desktop readiness card keeps its existing verdict.
//! - enable/disable/project-skills → `scope::update_disabled_bundles_for`
//!   (single-critical-section RMW, with the requested state verified inside
//!   that same critical section) / `set_project_skills_enabled` (the storage
//!   behind `set_disabled_skills` / `set_project_skills_enabled`), with
//!   `package_id_for` normalizing ids for the verification. Two caveats are
//!   printed at the point of action: the toggle performs no hot-refresh
//!   broadcast (the GUI's `hot_refresh` needs the engine pool this crate does
//!   not host), so a running desktop app's live engines keep the stale
//!   whitelist until they respawn; and `--scope both` is two single-scope
//!   writes with no two-scope transaction available, so a failure on the
//!   second scope reports exactly which scopes already landed instead of
//!   implying an all-or-nothing apply.
//!
//! Pure storage only: no Tauri host, no engine, no async runtime.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::support::{render, require_yes, sandbox_home, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::marketplace::{
    ConnectorScope, MarketplaceManager,
    bundle::{BundleKind, BundleRegistry, Readiness, keyring_target, readiness_for},
    package_export, plugin_import, recycle_bin,
    skill_marketplace::SkillMarketplaceManager,
    skill_scope,
    store::{BundleSource, BundleStore},
    sync_deny_all_scopes_after_install,
};
use pinvou3_lib::platform::credential_store::{
    CredentialReference, CredentialStore, SystemCredentialStore,
};

const USAGE: &str = "usage: pinvou plugins <tools|skills|import|export|meta|recycle|readiness|enable|disable|project-skills> (see subcommand help: pinvou plugins <tools|skills|recycle>)";

const TOOLS_USAGE: &str = "usage: pinvou plugins tools <list [--installed-only] | install <id> [--secret KEY=ENV_VAR_NAME...] | uninstall <id> --yes | auth <id> | oauth-login <id> [--timeout SECS] | oauth-cancel <id>>";

const SKILLS_USAGE: &str = "usage: pinvou plugins skills <list [--installed-only] | install <id> | update <id> | uninstall <id> --yes>";

const RECYCLE_USAGE: &str = "usage: pinvou plugins recycle <list | restore <id> | purge <id> --yes | export <id> [--output PATH]>";

/// `--scope` values for enable/disable: `plain`, `code`, or `both` (default).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeArg {
    Plain,
    Code,
    Both,
}

impl ScopeArg {
    fn parse(value: &str) -> Result<Self, CliError> {
        match value {
            "plain" => Ok(Self::Plain),
            "code" => Ok(Self::Code),
            "both" => Ok(Self::Both),
            other => Err(CliError::usage(format!(
                "plugins --scope must be plain, code, or both (got {other})"
            ))),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Code => "code",
            Self::Both => "both",
        }
    }

    fn scopes(self) -> Vec<ConnectorScope> {
        match self {
            Self::Plain => vec![ConnectorScope::Plain],
            Self::Code => vec![ConnectorScope::Code],
            Self::Both => vec![ConnectorScope::Plain, ConnectorScope::Code],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginsCommand {
    ToolsList {
        installed_only: bool,
    },
    ToolsInstall {
        id: String,
        secrets: Vec<(String, String)>,
    },
    ToolsUninstall {
        id: String,
        yes: bool,
    },
    ToolsAuth {
        id: String,
    },
    ToolsOauthLogin {
        id: String,
        timeout: Option<u64>,
    },
    ToolsOauthCancel {
        id: String,
    },
    SkillsList {
        installed_only: bool,
    },
    SkillsInstall {
        id: String,
    },
    SkillsUpdate {
        id: String,
    },
    SkillsUninstall {
        id: String,
        yes: bool,
    },
    Import {
        path: PathBuf,
    },
    Export {
        id: String,
        output: Option<PathBuf>,
    },
    Meta {
        id: String,
        name: Option<String>,
        description: Option<String>,
    },
    RecycleList,
    RecycleRestore {
        id: String,
    },
    RecyclePurge {
        id: String,
        yes: bool,
    },
    RecycleExport {
        id: String,
        output: Option<PathBuf>,
    },
    Readiness,
    Enable {
        id: String,
        scope: ScopeArg,
    },
    Disable {
        id: String,
        scope: ScopeArg,
    },
    ProjectSkills {
        enabled: bool,
    },
}

/// Flags that carry a value, per subcommand.
const TOOLS_LIST_OPTIONS: &[&str] = &[];
const TOOLS_OAUTH_LOGIN_OPTIONS: &[&str] = &["--timeout"];
const EXPORT_OPTIONS: &[&str] = &["--output"];
const META_OPTIONS: &[&str] = &["--name", "--description"];
const SCOPE_OPTIONS: &[&str] = &["--scope"];

/// Boolean (valueless) flags, per subcommand.
const LIST_FLAGS: &[&str] = &["--installed-only"];
const UNINSTALL_FLAGS: &[&str] = &["--yes"];

pub fn parse(values: &[String]) -> Result<PluginsCommand, CliError> {
    let subcommand = values
        .get(1)
        .ok_or_else(|| CliError::usage(USAGE))?
        .as_str();
    let rest = &values[2..];
    match subcommand {
        "tools" => parse_tools(rest),
        "skills" => parse_skills(rest),
        "import" => {
            let path = rest
                .first()
                .ok_or_else(|| CliError::usage("plugins import requires a PATH (zip, .md/.markdown skill file, or SKILL.md directory)"))?;
            if rest.len() > 1 {
                return Err(CliError::usage("plugins import accepts no options"));
            }
            Ok(PluginsCommand::Import {
                path: PathBuf::from(path),
            })
        }
        "export" => {
            let id = require_id(rest.first())?;
            let (options, _) = parse_flags(&rest[1..], EXPORT_OPTIONS, &[])?;
            Ok(PluginsCommand::Export {
                id,
                output: option(&options, "--output").map(PathBuf::from),
            })
        }
        "meta" => {
            let id = require_id(rest.first())?;
            let (options, _) = parse_flags(&rest[1..], META_OPTIONS, &[])?;
            let name = option(&options, "--name").map(str::to_owned);
            let description = option(&options, "--description").map(str::to_owned);
            if name.is_none() && description.is_none() {
                return Err(CliError::usage(
                    "plugins meta requires --name N and/or --description D",
                ));
            }
            Ok(PluginsCommand::Meta {
                id,
                name,
                description,
            })
        }
        "recycle" => parse_recycle(rest),
        "readiness" => {
            if !rest.is_empty() {
                return Err(CliError::usage("plugins readiness accepts no arguments"));
            }
            Ok(PluginsCommand::Readiness)
        }
        "enable" | "disable" => {
            let id = require_id(rest.first())?;
            let (options, _) = parse_flags(&rest[1..], SCOPE_OPTIONS, &[])?;
            let scope = match option(&options, "--scope") {
                None => ScopeArg::Both,
                Some(value) => ScopeArg::parse(value)?,
            };
            if subcommand == "enable" {
                Ok(PluginsCommand::Enable { id, scope })
            } else {
                Ok(PluginsCommand::Disable { id, scope })
            }
        }
        "project-skills" => match rest {
            [value] if value == "on" => Ok(PluginsCommand::ProjectSkills { enabled: true }),
            [value] if value == "off" => Ok(PluginsCommand::ProjectSkills { enabled: false }),
            [value] => Err(CliError::usage(format!(
                "plugins project-skills must be on or off (got {value})"
            ))),
            [_, extra, ..] => Err(CliError::usage(format!(
                "plugins project-skills accepts one argument (unexpected `{extra}`)"
            ))),
            [] => Err(CliError::usage("plugins project-skills requires on or off")),
        },
        _ => Err(CliError::usage(USAGE)),
    }
}

fn parse_tools(rest: &[String]) -> Result<PluginsCommand, CliError> {
    let action = rest
        .first()
        .ok_or_else(|| CliError::usage(TOOLS_USAGE))?
        .as_str();
    let tail = &rest[1..];
    match action {
        "list" => {
            let (_, flags) = parse_flags(tail, TOOLS_LIST_OPTIONS, LIST_FLAGS)?;
            Ok(PluginsCommand::ToolsList {
                installed_only: flags.contains(&"--installed-only"),
            })
        }
        "install" => {
            let id = require_id(tail.first())?;
            // --secret is repeatable (one per manifest config key), so the
            // shared duplicate-rejecting flag parser does not apply here.
            let mut secrets: Vec<(String, String)> = Vec::new();
            let mut index = 1;
            while index < tail.len() {
                let token = tail[index].as_str();
                if token != "--secret" {
                    return Err(CliError::usage(format!(
                        "unsupported plugins tools install option: {token}"
                    )));
                }
                let value = tail.get(index + 1).ok_or_else(|| {
                    CliError::usage("plugins tools install --secret requires KEY=ENV_VAR_NAME")
                })?;
                secrets.push(parse_secret_pair(value)?);
                index += 2;
            }
            Ok(PluginsCommand::ToolsInstall { id, secrets })
        }
        "uninstall" => {
            let id = require_id(tail.first())?;
            let (_, flags) = parse_flags(&tail[1..], &[], UNINSTALL_FLAGS)?;
            Ok(PluginsCommand::ToolsUninstall {
                id,
                yes: flags.contains(&"--yes"),
            })
        }
        "auth" => {
            let id = require_id(tail.first())?;
            if tail.len() > 1 {
                return Err(CliError::usage("plugins tools auth accepts no options"));
            }
            Ok(PluginsCommand::ToolsAuth { id })
        }
        "oauth-login" => {
            let id = require_id(tail.first())?;
            let (options, _) = parse_flags(&tail[1..], TOOLS_OAUTH_LOGIN_OPTIONS, &[])?;
            let timeout = parse_positive(&options, "--timeout")?;
            Ok(PluginsCommand::ToolsOauthLogin { id, timeout })
        }
        "oauth-cancel" => {
            let id = require_id(tail.first())?;
            if tail.len() > 1 {
                return Err(CliError::usage(
                    "plugins tools oauth-cancel accepts no options",
                ));
            }
            Ok(PluginsCommand::ToolsOauthCancel { id })
        }
        _ => Err(CliError::usage(TOOLS_USAGE)),
    }
}

fn parse_skills(rest: &[String]) -> Result<PluginsCommand, CliError> {
    let action = rest
        .first()
        .ok_or_else(|| CliError::usage(SKILLS_USAGE))?
        .as_str();
    let tail = &rest[1..];
    match action {
        "list" => {
            let (_, flags) = parse_flags(tail, &[], LIST_FLAGS)?;
            Ok(PluginsCommand::SkillsList {
                installed_only: flags.contains(&"--installed-only"),
            })
        }
        "install" | "update" => {
            let id = require_id(tail.first())?;
            if tail.len() > 1 {
                return Err(CliError::usage(format!(
                    "plugins skills {action} accepts no options"
                )));
            }
            if action == "install" {
                Ok(PluginsCommand::SkillsInstall { id })
            } else {
                Ok(PluginsCommand::SkillsUpdate { id })
            }
        }
        "uninstall" => {
            let id = require_id(tail.first())?;
            let (_, flags) = parse_flags(&tail[1..], &[], UNINSTALL_FLAGS)?;
            Ok(PluginsCommand::SkillsUninstall {
                id,
                yes: flags.contains(&"--yes"),
            })
        }
        _ => Err(CliError::usage(SKILLS_USAGE)),
    }
}

fn parse_recycle(rest: &[String]) -> Result<PluginsCommand, CliError> {
    let action = rest
        .first()
        .ok_or_else(|| CliError::usage(RECYCLE_USAGE))?
        .as_str();
    let tail = &rest[1..];
    match action {
        "list" => {
            if !tail.is_empty() {
                return Err(CliError::usage("plugins recycle list accepts no arguments"));
            }
            Ok(PluginsCommand::RecycleList)
        }
        "restore" => {
            let id = require_id(tail.first())?;
            if tail.len() > 1 {
                return Err(CliError::usage(
                    "plugins recycle restore accepts no options",
                ));
            }
            Ok(PluginsCommand::RecycleRestore { id })
        }
        "purge" => {
            let id = require_id(tail.first())?;
            let (_, flags) = parse_flags(&tail[1..], &[], UNINSTALL_FLAGS)?;
            Ok(PluginsCommand::RecyclePurge {
                id,
                yes: flags.contains(&"--yes"),
            })
        }
        "export" => {
            let id = require_id(tail.first())?;
            let (options, _) = parse_flags(&tail[1..], EXPORT_OPTIONS, &[])?;
            Ok(PluginsCommand::RecycleExport {
                id,
                output: option(&options, "--output").map(PathBuf::from),
            })
        }
        _ => Err(CliError::usage(RECYCLE_USAGE)),
    }
}

/// `KEY=ENV_VAR_NAME` for `--secret`: the key names the manifest config field,
/// the value is the NAME of the environment variable holding the secret (the
/// plaintext never appears on argv).
fn parse_secret_pair(value: &str) -> Result<(String, String), CliError> {
    let (key, env_var) = value.split_once('=').ok_or_else(|| {
        CliError::usage(format!(
            "plugins tools install --secret must be KEY=ENV_VAR_NAME (got {value})"
        ))
    })?;
    if key.is_empty() || env_var.is_empty() {
        return Err(CliError::usage(
            "plugins tools install --secret requires non-empty KEY and ENV_VAR_NAME",
        ));
    }
    Ok((key.to_owned(), env_var.to_owned()))
}

fn require_id(value: Option<&String>) -> Result<String, CliError> {
    let id = value
        .ok_or_else(|| CliError::usage("plugins command requires an id"))?
        .clone();
    // A `--`-prefixed id is a mistyped flag (e.g. `plugins disable --scope`);
    // recording it into disabled_bundles.json would disable nothing and
    // confuse the next list read.
    if id.is_empty() || id.starts_with("--") {
        return Err(CliError::usage("plugins command requires an id"));
    }
    Ok(id)
}

/// Shared implementation in `support::parse_family_flags`; `family`
/// only names this family in error messages.
fn parse_flags<'a>(
    values: &'a [String],
    value_flags: &[&str],
    boolean_flags: &[&str],
) -> Result<(Vec<(&'a str, &'a str)>, Vec<&'a str>), CliError> {
    crate::support::parse_family_flags(values, value_flags, boolean_flags, "plugins")
}

fn option<'a>(options: &'a [(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    crate::support::family_option(options, name)
}

fn parse_positive(options: &[(&str, &str)], name: &str) -> Result<Option<u64>, CliError> {
    crate::support::parse_family_positive::<u64>(options, name, "plugins")
}

fn feature_error(action: &str, id: &str, error: impl std::fmt::Display) -> CliError {
    // Passed through verbatim like the personas/deps translation boundaries
    // document for their unmapped tails: the importer's error strings are
    // product copy owned by the lib (partly Chinese), not CLI developer copy.
    CliError::failed(format!("plugins {action}({id}): {error:#}"))
}

/// English copy for the shared importer's security rejections, which are
/// Chinese lib-owned strings (`features/marketplace/plugin_import.rs`). The
/// importer keeps working unchanged; the CLI just refuses to surface
/// untranslated copy on exactly the rejections a user most needs to
/// understand (zip-slip, symlinks, decompression bombs — the same
/// translation-boundary pattern `personas::translate_persona_error` uses).
/// Unmapped tails pass through untouched.
fn translate_plugin_import_error(error: &str) -> Option<String> {
    if error.contains("不安全路径") {
        return Some("the package contains an unsafe path (zip-slip traversal); rejected".into());
    }
    if error.contains("symlink") {
        return Some("the package contains a symlink entry; rejected".into());
    }
    if error.contains("伪造头部") {
        return Some(
            "the package decompresses beyond its zip header's declared size (forged header / \
             zip bomb); rejected"
                .into(),
        );
    }
    if error.contains("解压") && error.contains("上限") {
        return Some(format!(
            "the package decompresses beyond the {} MiB cap (declared and actual size must \
             both fit); rejected",
            plugin_import::MAX_PLUGIN_SIZE_BYTES / 1024 / 1024
        ));
    }
    None
}

/// Mirror of the GUI's display-value hygiene
/// (`features/marketplace/store.rs::is_display_unsafe_char`, crate-private):
/// control characters, zero-width/bidi controls, and line/paragraph
/// separators never belong in a stored display name.
fn is_display_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{00AD}' // SOFT HYPHEN
            | '\u{200B}'..='\u{200D}' // ZERO WIDTH SPACE..JOINER
            | '\u{2028}'..='\u{2029}' // LINE/PARAGRAPH SEPARATOR
            | '\u{202A}'..='\u{202E}' // bidi embedding/override controls
            | '\u{2066}'..='\u{2069}' // bidi isolate controls
            | '\u{FEFF}' // BOM / ZERO WIDTH NO-BREAK SPACE
        )
}

/// Longest display name the GUI stores for an imported package
/// (`app/commands/marketplace.rs::sanitize_display_name` takes 128 chars).
const MAX_IMPORT_DISPLAY_NAME_CHARS: usize = 128;

/// Display name for an imported package, kept identical to the GUI's stored
/// value for the same file. The value lands verbatim in `bundles.json` and
/// nothing downstream bounds it, so the same file imported from the two
/// surfaces must not produce two different stored names.
///
/// Composition of the two rules:
/// - the GUI's `sanitize_display_name`: drop path separators (`/`, `\`) and
///   control characters, then cap at 128 CHARS (not bytes — a char cap cannot
///   split a multi-byte scalar);
/// - this crate's stricter invisible-character hygiene
///   ([`is_display_unsafe_char`]): zero-width, bidi-override and BOM code
///   points are dropped too. That part is an improvement the GUI lacks and is
///   kept: it only ever removes characters the GUI would have stored, so the
///   surfaces stay convergent on every name that does not contain them.
///
/// Trimming and the truncation run in that order so the cap is applied to what
/// is actually stored; a name that sanitizes away entirely falls back to the
/// same generic label the caller uses for a missing file name.
fn sanitize_import_display_name(raw_name: &str) -> String {
    let cleaned: String = raw_name
        .chars()
        .filter(|c| !is_display_unsafe_char(*c) && *c != '/' && *c != '\\')
        .collect();
    let trimmed: String = cleaned
        .trim()
        .chars()
        .take(MAX_IMPORT_DISPLAY_NAME_CHARS)
        .collect();
    // The cap can expose trailing whitespace that was interior before the cut.
    let trimmed = trimmed.trim_end();
    if trimmed.is_empty() {
        "plugin.zip".to_owned()
    } else {
        trimmed.to_owned()
    }
}

pub fn execute(command: PluginsCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    // Every store path this family touches resolves through the lib's
    // `pinvou3_home`, which accepts a relative `PINVOU3_HOME` verbatim;
    // enforce the same absolute-home contract as the sibling families so one
    // binary cannot half-apply state against a cwd-relative store.
    sandbox_home()?;
    match command {
        PluginsCommand::ToolsList { installed_only } => tools_list(installed_only, output),
        PluginsCommand::ToolsInstall { id, secrets } => tools_install(&id, &secrets, output),
        PluginsCommand::ToolsUninstall { id, yes } => tools_uninstall(&id, yes, output),
        PluginsCommand::ToolsAuth { id } => tools_auth(&id, output),
        PluginsCommand::ToolsOauthLogin { id, timeout } => tools_oauth_login(&id, timeout, output),
        PluginsCommand::ToolsOauthCancel { id } => tools_oauth_cancel(&id, output),
        PluginsCommand::SkillsList { installed_only } => skills_list(installed_only, output),
        PluginsCommand::SkillsInstall { id } => skills_install(&id, output),
        PluginsCommand::SkillsUpdate { id } => skills_update(&id, output),
        PluginsCommand::SkillsUninstall { id, yes } => skills_uninstall(&id, yes, output),
        PluginsCommand::Import { path } => import(&path, output),
        PluginsCommand::Export { id, output: dest } => export(&id, dest, output),
        PluginsCommand::Meta {
            id,
            name,
            description,
        } => meta(&id, name, description, output),
        PluginsCommand::RecycleList => recycle_list(output),
        PluginsCommand::RecycleRestore { id } => recycle_restore(&id, output),
        PluginsCommand::RecyclePurge { id, yes } => recycle_purge(&id, yes, output),
        PluginsCommand::RecycleExport { id, output: dest } => recycle_export(&id, dest, output),
        PluginsCommand::Readiness => readiness(output),
        PluginsCommand::Enable { id, scope } => set_enabled(&id, scope, true, output),
        PluginsCommand::Disable { id, scope } => set_enabled(&id, scope, false, output),
        PluginsCommand::ProjectSkills { enabled } => project_skills(enabled, output),
    }
}

// ---------------------------------------------------------------------------
// tools
// ---------------------------------------------------------------------------

fn tools_list(installed_only: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    let mut tools = MarketplaceManager::new().list_tools();
    if installed_only {
        tools.retain(|tool| tool.installed);
    }
    let value = serde_json::to_value(&tools)
        .map(|tools| serde_json::json!({ "tools": tools }))
        .unwrap_or_else(|_| serde_json::json!({ "tools": [] }));
    let human = tools
        .iter()
        .map(|tool| {
            format!(
                "{}\t{}\t{}\t{}\t{}",
                tool.id,
                if tool.installed { "installed" } else { "-" },
                tool.source,
                tool.version,
                format!("{}: {}", tool.name, tool.description),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(success(render(output, human, &value)))
}

/// Resolves `--secret KEY=ENV_VAR_NAME` pairs into the user-config map the
/// feature layer expects (manifest key → plaintext), reading each plaintext
/// from the named process environment variable. Missing variables are a hard
/// error so an install never proceeds with half the declared secrets.
fn resolve_secrets(secrets: &[(String, String)]) -> Result<HashMap<String, String>, CliError> {
    let mut config = HashMap::new();
    for (key, env_var) in secrets {
        let value = std::env::var(env_var).map_err(|_| {
            CliError::failed(format!(
                "secret environment variable {env_var} is not set (for config key {key})"
            ))
        })?;
        if value.trim().is_empty() {
            return Err(CliError::failed(format!(
                "secret environment variable {env_var} is empty (for config key {key})"
            )));
        }
        config.insert(key.clone(), value);
    }
    Ok(config)
}

fn tools_install(
    id: &str,
    secrets: &[(String, String)],
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let config = resolve_secrets(secrets)?;
    let mgr = MarketplaceManager::new();
    mgr.install(id, &config)
        .map_err(|error| feature_error("tools install", id, error))?;
    // Companion skills follow the package (GUI `install_marketplace_tool`):
    // a companion INSTALL failure is logged and does not roll back the MCP
    // install (a skill is an enhancement), but a companion SCOPE SYNC failure
    // is a consent gap and is collected, not swallowed. The GUI makes exactly
    // that split: it pushes sync failures into `sync_errors` and returns them
    // from the command once the loop is done. Collect-then-raise rather than
    // `?` per iteration for the same reason it gives: an early return would
    // skip the remaining companions and the package-level sync below, leaving
    // "package installed but its consent set did not follow" — a fail-open
    // half state.
    let mut companion_note = Vec::new();
    let mut sync_errors: Vec<String> = Vec::new();
    for sid in mgr.companion_skills(id) {
        match SkillMarketplaceManager::new().install(&sid) {
            Ok(()) => {
                if let Err(error) = skill_scope::sync_deny_all_scopes_after_skill_install(&sid) {
                    sync_errors.push(format!(
                        "companion skill '{sid}' scope sync failed: {error}"
                    ));
                }
                companion_note.push(sid);
            }
            Err(error) => {
                note!("[plugins] companion skill '{sid}' install failed: {error}");
            }
        }
    }
    // DenyAll scopes (e.g. code) keep newly installed packages off by default.
    if let Err(error) = sync_deny_all_scopes_after_install(id) {
        sync_errors.push(format!("package '{id}' scope sync failed: {error}"));
    }
    if !sync_errors.is_empty() {
        return Err(CliError::failed(format!(
            "plugins tools install({id}): {}",
            sync_errors.join("; ")
        )));
    }
    // The GUI validates remote MCP connections right after install
    // (validate_on_install manifests) and, on a failed handshake, UNINSTALLS
    // the tool again. The handshake runs on the foundation's async MCP stack,
    // unavailable headless, so neither the check nor its rollback happens
    // here: surface an explicit warning naming both halves instead of
    // pretending the connection was verified.
    let validation_skipped = mgr.requires_remote_connection_validation(id);
    let mut value = serde_json::json!({
        "id": id,
        "action": "installed",
        "companion_skills": companion_note,
        "validation": if validation_skipped { "skipped" } else { "not_required" },
    });
    let mut human = format!("installed {id}");
    if !companion_note.is_empty() {
        human.push_str(&format!(
            " (companion skills: {})",
            companion_note.join(", ")
        ));
        value["companion_skills_installed"] = serde_json::json!(true);
    }
    if validation_skipped {
        human.push_str(&format!(
            "\nwarning: remote connection validation skipped (headless CLI), and so was the \
             rollback it guards: the desktop uninstalls a tool whose handshake fails, this \
             install is left in place either way; check 'pinvou plugins tools auth {id}'"
        ));
        value["validation_rollback"] = serde_json::json!("skipped");
    }
    Ok(success(render(output, human, &value)))
}

fn tools_uninstall(id: &str, yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    // Mirrors the GUI `uninstall_marketplace_tool_sync`: companion teardown
    // before the package record goes away, scope cleanup only after the
    // skill is actually gone. The GUI additionally deletes stored remote
    // OAuth tokens via the foundation's token store before uninstalling
    // OAuth tools; that store is not reachable headless (disclosed in the
    // module docs), so a reinstalled OAuth tool may still be authorized.
    let mgr = MarketplaceManager::new();
    // Captured before the uninstall: the foundation token store is not
    // reachable headless, so an OAuth tool's stored remote tokens survive
    // the uninstall — say so at the point of action, not only in the module
    // docs.
    let keeps_oauth_tokens = mgr.oauth_remote_server_name(id).is_some();
    let companions = mgr.companion_skills(id);
    let recycles_with_package = BundleStore::new()
        .get(id)
        .map_err(|error| feature_error("tools uninstall", id, error))?
        .is_some_and(|record| matches!(record.source, BundleSource::Upload(_)));
    for sid in &companions {
        if recycles_with_package {
            continue; // companion is recycled with the whole package
        }
        SkillMarketplaceManager::new()
            .uninstall(sid)
            .map_err(|error| feature_error("tools uninstall", id, error))?;
        skill_scope::remove_skill_from_disabled_scopes(sid)
            .map_err(|error| feature_error("tools uninstall", id, error))?;
    }
    mgr.uninstall(id)
        .map_err(|error| feature_error("tools uninstall", id, error))?;
    if recycles_with_package {
        for sid in &companions {
            skill_scope::remove_skill_from_disabled_scopes(sid)
                .map_err(|error| feature_error("tools uninstall", id, error))?;
        }
    }
    // Keep the disabled sets free of stale connector ids (GUI parity).
    pinvou3_lib::features::marketplace::remove_connector_from_disabled_scopes(id)
        .map_err(|error| feature_error("tools uninstall", id, error))?;
    let action = if recycles_with_package {
        "uninstalled (moved to recycle bin)"
    } else {
        "uninstalled"
    };
    let oauth_note = if keeps_oauth_tokens {
        "\nnote: if this tool was authorized, its stored OAuth tokens were kept; a \
         reinstall stays authorized"
    } else {
        ""
    };
    let value =
        serde_json::json!({ "id": id, "action": "uninstalled", "recycled": recycles_with_package });
    Ok(success(render(
        output,
        format!("{action} {id}{oauth_note}"),
        &value,
    )))
}

/// Reads the `servers` map out of `mcp.json` the way the desktop does.
///
/// The desktop deserializes the whole file into `deepseek_tui::mcp::McpConfig`
/// (`marketplace_oauth_server_from_mcp_config`), so a file that merely *looks*
/// right is not enough there: any entry whose known field carries the wrong
/// JSON type fails the parse and the server counts as absent. This crate does
/// not depend on the foundation crate (and has no `serde` derive in its
/// dependency graph), so the same decision is re-applied by hand below —
/// an untyped `get("servers").get(name)` reported `mcp_configured: true` for
/// configurations the GUI rejects.
///
/// Returns `Err` only for a read/parse/shape failure; `Ok(None)` means the
/// file (or the map) is simply absent.
fn read_mcp_json_servers(context: &str) -> Result<Option<serde_json::Value>, CliError> {
    let path = pinvou3_lib::platform::paths::mcp_config_path();
    if !path.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path).map_err(|error| {
        CliError::failed(format!(
            "plugins tools {context}: cannot read mcp.json: {error}"
        ))
    })?;
    let value: serde_json::Value = serde_json::from_str(&content).map_err(|error| {
        CliError::failed(format!(
            "plugins tools {context}: cannot parse mcp.json: {error}"
        ))
    })?;
    let Some(root) = value.as_object() else {
        return Err(CliError::failed(format!(
            "plugins tools {context}: mcp.json is not a JSON object"
        )));
    };
    // `McpConfig::servers` carries `#[serde(alias = "mcpServers")]`; serde
    // rejects a file that spells both (duplicate field), so this does too.
    let servers = match (root.get("servers"), root.get("mcpServers")) {
        (Some(_), Some(_)) => {
            return Err(CliError::failed(format!(
                "plugins tools {context}: mcp.json declares both 'servers' and its \
                 'mcpServers' alias"
            )));
        }
        (Some(servers), None) | (None, Some(servers)) => servers,
        (None, None) => return Ok(None),
    };
    let Some(entries) = servers.as_object() else {
        return Err(CliError::failed(format!(
            "plugins tools {context}: mcp.json 'servers' is not a JSON object"
        )));
    };
    // Whole-map validation, not just the queried entry: the desktop parses the
    // file in one shot, so ONE malformed neighbour makes every server read as
    // unconfigured there. Checking only the queried name would re-open the
    // divergence from the other side.
    for (name, entry) in entries {
        if !mcp_server_entry_matches_typed_shape(entry) {
            return Err(CliError::failed(format!(
                "plugins tools {context}: mcp.json server '{name}' does not match the \
                 MCP server config shape"
            )));
        }
    }
    Ok(Some(servers.clone()))
}

/// Whether one `mcp.json` server entry would deserialize into the foundation's
/// `McpServerConfig`. Mirrors that struct's field declarations:
/// - `Option<T>` fields accept an explicit `null`;
/// - `#[serde(default)]` fields that are NOT `Option` do not — serde's default
///   only fills a *missing* key, so `"args": null` fails the desktop's parse;
/// - unknown keys pass, because `McpServerConfig` is not
///   `deny_unknown_fields` and rejecting them would be stricter than the
///   surface being mirrored.
///
/// Residual, deliberate: the nested `oauth` object's own fields are checked
/// only as "object or null", and the root `timeouts` block is not inspected at
/// all. Both are configuration the marketplace flow never writes, so a
/// divergence there is bounded to hand-edited files.
fn mcp_server_entry_matches_typed_shape(entry: &serde_json::Value) -> bool {
    let Some(fields) = entry.as_object() else {
        return false;
    };
    const OPTIONAL_STRING: &[&str] = &[
        "command",
        "cwd",
        "url",
        "transport",
        "bearer_token_env_var",
        "oauth_resource",
    ];
    const OPTIONAL_U64: &[&str] = &["connect_timeout", "execute_timeout", "read_timeout"];
    const PLAIN_BOOL: &[&str] = &["disabled", "enabled", "required"];
    const STRING_LIST: &[&str] = &["args", "enabled_tools", "disabled_tools", "scopes"];
    // `env_http_headers` is the declared alias of `env_headers`.
    const STRING_MAP: &[&str] = &["env", "headers", "env_headers", "env_http_headers"];
    fields.iter().all(|(key, value)| {
        let key = key.as_str();
        if OPTIONAL_STRING.contains(&key) {
            value.is_null() || value.is_string()
        } else if OPTIONAL_U64.contains(&key) {
            value.is_null() || value.as_u64().is_some()
        } else if PLAIN_BOOL.contains(&key) {
            value.is_boolean()
        } else if STRING_LIST.contains(&key) {
            value
                .as_array()
                .is_some_and(|items| items.iter().all(serde_json::Value::is_string))
        } else if STRING_MAP.contains(&key) {
            value
                .as_object()
                .is_some_and(|items| items.values().all(serde_json::Value::is_string))
        } else if key == "oauth" {
            value.is_null() || value.is_object()
        } else {
            true
        }
    })
}

/// Degrading variant for `tools auth`: a status READ must not turn a damaged
/// `mcp.json` into exit 1. The GUI's `get_marketplace_tool_auth_status` logs
/// the failure and continues with `mcp_configured = false` (which yields
/// `auth_pending`), so the CLI does the same and writes the reason to stderr
/// instead of dropping it. `oauth-login` deliberately keeps the strict reader:
/// its GUI counterpart (`start_marketplace_tool_oauth_login`) propagates the
/// parse error too, because it is about to ACT on that config.
fn mcp_json_servers_lenient(context: &str) -> Option<serde_json::Value> {
    match read_mcp_json_servers(context) {
        Ok(servers) => servers,
        Err(error) => {
            note!("[plugins] {error}; reporting the MCP config as not installed");
            None
        }
    }
}

fn tools_auth(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let mgr = MarketplaceManager::new();
    let installed = mgr.installed_ids().iter().any(|tool| tool == id);
    let server_name = mgr.oauth_remote_server_name(id);
    let oauth_required = server_name.is_some();
    let mut mcp_configured = false;
    if let Some(name) = server_name.as_deref() {
        if let Some(servers) = mcp_json_servers_lenient("auth") {
            mcp_configured = servers.get(name).is_some();
        }
    }
    // The foundation keeps OAuth tokens in its own credential-store namespace;
    // the CLI cannot read them, so token presence is conservative.
    let (status, message, oauth_token_present) = if oauth_required && mcp_configured {
        (
            "config_installed_auth_pending",
            "MCP config is installed, but the CLI cannot verify OAuth token presence; use the desktop app for the interactive grant.",
            false,
        )
    } else if oauth_required && installed {
        (
            "auth_pending",
            "Tool is installed, but the MCP config is incomplete; reconnect from the desktop app.",
            false,
        )
    } else if oauth_required {
        ("not_installed", "Tool is not connected yet.", false)
    } else if installed {
        ("connected", "Tool is installed.", false)
    } else {
        ("not_installed", "Tool is not installed.", false)
    };
    let value = serde_json::json!({
        "installed": installed,
        "mcp_configured": mcp_configured,
        "oauth_required": oauth_required,
        "oauth_token_present": oauth_token_present,
        "status": status,
        "server_name": server_name,
        "message": message,
        "token_check": if oauth_required { "unavailable_in_cli" } else { "not_applicable" },
    });
    let human = format!(
        "tool: {id}\ninstalled: {installed}\noauth_required: {oauth_required}\nmcp_configured: {mcp_configured}\noauth_token_present: {oauth_token_present}\nstatus: {status}\nserver_name: {}\nmessage: {message}",
        server_name.as_deref().unwrap_or("-"),
    );
    Ok(success(render(output, human, &value)))
}

fn tools_oauth_login(
    id: &str,
    timeout: Option<u64>,
    _output: OutputMode,
) -> Result<CliOutcome, CliError> {
    // Same guards as the GUI `start_marketplace_tool_oauth_login`.
    let mgr = MarketplaceManager::new();
    let server_name = mgr.oauth_remote_server_name(id).ok_or_else(|| {
        CliError::failed(format!(
            "plugins tools oauth-login({id}): tool does not declare a remote MCP OAuth login"
        ))
    })?;
    let configured = read_mcp_json_servers("oauth-login")?
        .and_then(|servers| servers.get(&server_name).cloned())
        .ok_or_else(|| {
            CliError::failed(format!(
                "plugins tools oauth-login({id}): mcp.json has no server '{server_name}'"
            ))
        })?;
    let _ = configured;
    let _ = timeout;
    Err(CliError::failed(
        "oauth_login_unavailable_in_cli: the MCP OAuth login flow (PKCE + local callback + token persistence) lives in the CodeWhale foundation crate, which the pinvou CLI does not link; complete the interactive grant in the desktop app, or configure the credential with 'pinvou plugins tools install --secret KEY=ENV_VAR_NAME'",
    ))
}

fn tools_oauth_cancel(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    // The GUI cancels an in-process login through its coordinator; each CLI
    // invocation is a separate process, so there is never an active login to
    // cancel here.
    let value = serde_json::json!({
        "id": id,
        "cancelled": false,
        "status": "not_running",
    });
    Ok(success(render(
        output,
        format!("no active oauth login for {id}"),
        &value,
    )))
}

// ---------------------------------------------------------------------------
// skills
// ---------------------------------------------------------------------------

fn skills_list(installed_only: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    let mut skills = SkillMarketplaceManager::new().list_skills();
    if installed_only {
        skills.retain(|skill| skill.installed);
    }
    let value = serde_json::to_value(&skills)
        .map(|skills| serde_json::json!({ "skills": skills }))
        .unwrap_or_else(|_| serde_json::json!({ "skills": [] }));
    let human = skills
        .iter()
        .map(|skill| {
            format!(
                "{}\t{}\t{}\t{}\t{}",
                skill.id,
                if skill.installed { "installed" } else { "-" },
                if skill.user_uploaded {
                    "uploaded"
                } else {
                    "preset"
                },
                if skill.update_available {
                    "update-available"
                } else {
                    "-"
                },
                format!("{}: {}", skill.title, skill.description),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(success(render(output, human, &value)))
}

fn skills_install(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    // `install_marketplace_skill_sync`: install, then default the skill into
    // every initialized DenyAll scope's disabled set (explicit opt-in).
    SkillMarketplaceManager::new()
        .install(id)
        .map_err(|error| feature_error("skills install", id, error))?;
    skill_scope::sync_deny_all_scopes_after_skill_install(id)
        .map_err(|error| feature_error("skills install", id, error))?;
    let value = serde_json::json!({ "id": id, "action": "installed" });
    Ok(success(render(output, format!("installed {id}"), &value)))
}

fn skills_update(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    // `update_marketplace_skill` guard: only installed preset skills have an
    // embedded new version; updates keep the user's enable/disable state.
    let mgr = SkillMarketplaceManager::new();
    let installed = mgr
        .list_skills()
        .into_iter()
        .any(|skill| skill.id == id && skill.installed && !skill.user_uploaded);
    if !installed {
        return Err(CliError::failed(format!(
            "plugins skills update({id}): skill is not an installed preset skill, nothing to update"
        )));
    }
    mgr.install(id)
        .map_err(|error| feature_error("skills update", id, error))?;
    let value = serde_json::json!({ "id": id, "action": "updated" });
    Ok(success(render(output, format!("updated {id}"), &value)))
}

fn skills_uninstall(id: &str, yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    // `uninstall_marketplace_skill_sync`: uninstall, then drop stale scope
    // entries so a still-listed id cannot silently re-enable anything.
    SkillMarketplaceManager::new()
        .uninstall(id)
        .map_err(|error| feature_error("skills uninstall", id, error))?;
    skill_scope::remove_skill_from_disabled_scopes(id)
        .map_err(|error| feature_error("skills uninstall", id, error))?;
    let value = serde_json::json!({ "id": id, "action": "uninstalled" });
    Ok(success(render(output, format!("uninstalled {id}"), &value)))
}

// ---------------------------------------------------------------------------
// import / export / meta
// ---------------------------------------------------------------------------

fn import(path: &Path, output: OutputMode) -> Result<CliOutcome, CliError> {
    if !path.exists() {
        return Err(CliError::failed(format!(
            "plugins import({}): path does not exist",
            path.display()
        )));
    }
    // The raw file name becomes the package display name in bundles.json, so
    // it gets the same invisible-character hygiene as the GUI's display
    // values (mirror of `features/marketplace/store.rs
    // is_display_unsafe_char`, which is crate-private): strip instead of
    // reject — the name came from the user's own file path, and a rename
    // requirement would be a worse outcome than a cleaned label. The RAW
    // lossy name is kept alongside: the fallback-id branch inside
    // `wrap_markdown_skill` hashes it (the GUI hashes the raw filename
    // stem), so names that sanitize to the same label keep distinct
    // `skill-<hash>` ids and GUI re-imports stay id-stable.
    let raw_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "plugin.zip".to_owned());
    let display = sanitize_import_display_name(&raw_name);
    // The unified pipeline accepts zip packages; .md files and SKILL.md
    // directories are wrapped into a root-SKILL.md zip first (replacing the
    // GUI's native file dialog).
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());
    let wrapper: Option<(PathBuf, Vec<u8>)> = if path.is_dir() {
        if !path.join("SKILL.md").is_file() {
            return Err(CliError::failed(format!(
                "plugins import({}): directory has no SKILL.md at its root",
                path.display()
            )));
        }
        let mut cumulative = 0u64;
        let mut entries = collect_directory_entries(path, &display, &mut cumulative)?;
        // A SKILL.md without a frontmatter `name` is rejected downstream as
        // an "empty package"; the .md channel derives a name from the file
        // name, so the directory channel injects the same fallback here. A
        // READ failure (permissions, non-UTF-8 body) must surface like the
        // .md channel's error instead of degrading to an empty string — that
        // would silently replace the user's skill body with a stub.
        //
        // The walk above already read and charged the root SKILL.md against
        // `cumulative`, so the bytes are taken from `entries` instead of
        // re-read against the same budget: a second charge rejected legal
        // directories near the limit (true content ≤ limit but
        // content + SKILL.md > limit). A missing entry means SKILL.md
        // vanished mid-walk or is a symlink (which the walk skips, matching
        // the pipeline's no-symlink policy) — rejected here like the
        // missing-root check above.
        let skill_md_bytes = entries
            .iter()
            .find(|(name, _)| name == "SKILL.md")
            .map(|(_, bytes)| bytes.clone())
            .ok_or_else(|| {
                CliError::failed(format!(
                    "plugins import({}): directory has no SKILL.md at its root",
                    path.display()
                ))
            })?;
        let skill_md = String::from_utf8(skill_md_bytes).map_err(|_| {
            CliError::failed(format!(
                "plugins import({}): SKILL.md is not valid UTF-8",
                path.display()
            ))
        })?;
        if frontmatter_name(&skill_md).is_none() {
            // The RAW name, not the sanitized form: the anti-collision branch
            // inside wrap_markdown_skill hashes it, so directories whose
            // names sanitize to the same generic "skill" (any pure non-ASCII
            // name) still get distinct `skill-<hash>` ids (GUI FNV collision
            // defense) instead of collapsing onto one constant id.
            let wrapped = wrap_markdown_skill(&skill_md, &raw_name).into_bytes();
            charge_wrapped_skill_md(&mut cumulative, skill_md.len(), &wrapped, &display, path)?;
            entries.retain(|(name, _)| name != "SKILL.md");
            entries.push(("SKILL.md".to_owned(), wrapped));
        }
        Some((
            temp_zip_path("pinvou-cli-import-dir"),
            build_stored_zip(&entries)?,
        ))
    } else if extension.as_deref() == Some("md") || extension.as_deref() == Some("markdown") {
        let mut cumulative = 0u64;
        let bytes = read_import_file_capped(path, "skill file", &display, &mut cumulative)?;
        let content = String::from_utf8(bytes).map_err(|_| {
            CliError::failed(format!(
                "plugins import({}): skill file is not valid UTF-8",
                path.display()
            ))
        })?;
        let wrapped = wrap_markdown_skill(&content, &raw_name).into_bytes();
        charge_wrapped_skill_md(&mut cumulative, content.len(), &wrapped, &display, path)?;
        Some((
            temp_zip_path("pinvou-cli-import-md"),
            build_stored_zip(&[("SKILL.md".to_owned(), wrapped)])?,
        ))
    } else if extension.as_deref() == Some("zip") {
        // The importer streams zip entries and bounds decompressed content
        // only — its File::open would still block on a FIFO, and the GUI
        // upload path rejects oversized package files up front. Pre-flight
        // the package file so a handoff can only succeed or fail, not hang.
        let declared = import_file_meta(path, "plugin package", &display)?;
        if declared > plugin_import::MAX_PLUGIN_SIZE_BYTES {
            return Err(import_over_limit(&display, path));
        }
        None
    } else {
        return Err(CliError::usage(
            "plugins import supports .zip plugin packages, .md/.markdown skill files, or directories containing SKILL.md",
        ));
    };
    let import_path;
    let mut temp_zip: Option<TempWrapperZip> = None;
    if let Some((tmp, bytes)) = &wrapper {
        // Exclusive create + write through the same handle: the path lives
        // in the shared temp directory, so a pre-planted symlink or file
        // must not be followed (the wrapper zip holds the user's skill body
        // in transit).
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(tmp)
            .map_err(|error| {
                CliError::failed(format!(
                    "plugins import: cannot create temporary zip {}: {error}",
                    tmp.display()
                ))
            })?;
        // Armed before the first write, so a failed write_all (or any other
        // early return below) removes the just-created wrapper via Drop.
        let guard = TempWrapperZip { path: tmp.clone() };
        std::io::Write::write_all(&mut file, bytes).map_err(|error| {
            CliError::failed(format!(
                "plugins import: cannot write temporary zip: {error}"
            ))
        })?;
        temp_zip = Some(guard);
        import_path = tmp.clone();
    } else {
        import_path = path.to_path_buf();
    }
    let result = plugin_import::import_plugin_package(&import_path.to_string_lossy(), &display);
    // Dropping the armed guard removes the wrapper; every earlier failure
    // path already removed it through Drop.
    drop(temp_zip);
    let report = result.map_err(|error| {
        let rendered = translate_plugin_import_error(&error.to_string())
            .unwrap_or_else(|| format!("{error:#}"));
        feature_error("import", &display, rendered)
    })?;
    // Upload safety default (GUI parity): imported packages start disabled in
    // initialized DenyAll scopes until explicitly enabled.
    sync_deny_all_scopes_after_install(&report.id)
        .map_err(|error| feature_error("import", &report.id, error))?;
    let kind = serde_json::to_value(&report.kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned());
    let value = serde_json::json!({
        "id": report.id,
        "kind": kind,
        "icon": report.icon,
    });
    Ok(success(render(
        output,
        format!("imported {} (kind={kind} icon={})", report.id, report.icon),
        &value,
    )))
}

/// Reserves an export destination atomically: the exclusive create closes
/// the check-then-use window between the overwrite refusal and the library's
/// tmp+persist write (a concurrent creator loses the create_new race instead
/// of clobbering us). The caller removes the empty reservation when its
/// export fails.
fn reserve_export_destination(id: &str, dest: &Path, action: &str) -> Result<(), CliError> {
    let refuse = || {
        CliError::failed(format!(
            "{action}({id}): refusing to overwrite {}; choose a destination that does \
             not exist yet",
            dest.display()
        ))
    };
    match std::fs::File::create_new(dest) {
        Ok(marker) => {
            drop(marker);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = dest.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).map_err(|error| {
                        CliError::failed(format!(
                            "{action}({id}): cannot create output directory {}: {error}",
                            parent.display()
                        ))
                    })?;
                }
            }
            match std::fs::File::create_new(dest) {
                Ok(marker) => {
                    drop(marker);
                    Ok(())
                }
                Err(_) => Err(refuse()),
            }
        }
        Err(_) => Err(refuse()),
    }
}

fn export(
    id: &str,
    destination: Option<PathBuf>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let dest = destination.unwrap_or_else(|| default_export_name(id));
    // Existence first: creating the destination directory for an unknown id
    // would leave an empty tree behind on the failure path.
    let installed = MarketplaceManager::new()
        .installed_ids()
        .iter()
        .any(|pkg| pkg == id)
        || SkillMarketplaceManager::new()
            .installed_skill_ids()
            .iter()
            .any(|pkg| pkg == id);
    if !installed {
        return Err(feature_error(
            "export",
            id,
            format!("package '{id}' is not installed"),
        ));
    }
    // An existing destination is refused, not overwritten: the default name
    // is `<id>.zip` in the caller's cwd, so a silent overwrite could destroy
    // an unrelated file with exit 0 (the same policy as `sessions export`).

    reserve_export_destination(id, &dest, "plugins export")?;
    let export_result = package_export::export_installed_plugin(id, &dest)
        .map_err(|error| feature_error("export", id, error));
    if export_result.is_err() {
        // The lib writes through a temp file and persists at the end, so a
        // failure leaves our empty reservation behind — remove it.
        let _ = std::fs::remove_file(&dest);
    }
    export_result?;
    let value = serde_json::json!({
        "id": id,
        "output": dest.display().to_string(),
    });
    Ok(success(render(
        output,
        format!("exported {id} -> {}", dest.display()),
        &value,
    )))
}

fn meta(
    id: &str,
    name: Option<String>,
    description: Option<String>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    SkillMarketplaceManager::new()
        .update_display_meta(id, name.as_deref(), description.as_deref())
        .map_err(|error| feature_error("meta", id, error))?;
    let value = serde_json::json!({
        "id": id,
        "action": "meta_updated",
        "display_name": name,
        "display_description": description,
    });
    Ok(success(render(
        output,
        format!("updated display meta for {id}"),
        &value,
    )))
}

// ---------------------------------------------------------------------------
// recycle
// ---------------------------------------------------------------------------

fn recycle_list(output: OutputMode) -> Result<CliOutcome, CliError> {
    let entries = recycle_bin::RecycleBin::new()
        .list()
        .map_err(|error| CliError::failed(format!("plugins recycle list: {error:#}")))?;
    let value = serde_json::to_value(&entries)
        .map(|entries| serde_json::json!({ "recycled": entries }))
        .unwrap_or_else(|_| serde_json::json!({ "recycled": [] }));
    let human = entries
        .iter()
        .map(|entry| {
            format!(
                "{}\t{}\t{}\t{}\t{}",
                entry.id,
                entry.kind,
                entry.display_name,
                entry.recycled_at,
                if entry.package_missing {
                    "package-missing"
                } else {
                    "-"
                },
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(success(render(output, human, &value)))
}

fn recycle_restore(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let result = recycle_bin::restore_plugin(id)
        .map_err(|error| feature_error("recycle restore", id, error))?;
    let value = serde_json::to_value(&result)
        .unwrap_or_else(|_| serde_json::json!({ "credentials_required": false }));
    let mut human = format!("restored {id}");
    if result.credentials_required {
        human
            .push_str("\nwarning: credentials were removed at uninstall; re-enter them before use");
    }
    Ok(success(render(output, human, &value)))
}

fn recycle_purge(id: &str, yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    recycle_bin::RecycleBin::new()
        .purge(id)
        .map_err(|error| feature_error("recycle purge", id, error))?;
    let value = serde_json::json!({ "id": id, "action": "purged" });
    Ok(success(render(output, format!("purged {id}"), &value)))
}

fn recycle_export(
    id: &str,
    destination: Option<PathBuf>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let dest = destination.unwrap_or_else(|| default_export_name(id));
    // Existence first (same ordering as `export`): creating the destination
    // directory for an unknown id would leave an empty tree behind on the
    // failure path.
    let known = recycle_bin::RecycleBin::new()
        .list()
        .map_err(|error| feature_error("recycle export", id, error))?
        .iter()
        .any(|entry| entry.id == id);
    if !known {
        return Err(feature_error(
            "recycle export",
            id,
            format!("recycled package '{id}' does not exist"),
        ));
    }
    // Same no-overwrite policy as `export` (the default is `<id>.zip` in the
    // caller's cwd), with the same atomic reservation.
    reserve_export_destination(id, &dest, "plugins recycle export")?;
    let export_result = recycle_bin::RecycleBin::new()
        .export_package(id, &dest)
        .map_err(|error| feature_error("recycle export", id, error));
    if export_result.is_err() {
        let _ = std::fs::remove_file(&dest);
    }
    export_result?;
    let value = serde_json::json!({
        "id": id,
        "output": dest.display().to_string(),
    });
    Ok(success(render(
        output,
        format!("exported recycled {id} -> {}", dest.display()),
        &value,
    )))
}

/// Same default the GUI save dialog proposes: `<package id>.zip` in the
/// working directory.
fn default_export_name(id: &str) -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(format!("{id}.zip"))
}

// ---------------------------------------------------------------------------
// readiness / enable / disable / project-skills
// ---------------------------------------------------------------------------

/// `probe` field on a `plugins readiness` row: how complete the row's
/// `ready` / `reason` pair is. `registry` means the registry verdict IS the
/// whole answer (the GUI computes it from the same `readiness_for` call).
/// `unavailable_in_cli` means the verdict is a headless under-approximation:
/// the desktop reaches the real answer through a live probe this crate cannot
/// run, so `ready: false` on such a row means "not determined here", not
/// "known broken" — `pinvoy connectors ... status` is the authority. Every row
/// carries the field so the JSON shape does not depend on the kind.
const PROBE_REGISTRY: &str = "registry";
const PROBE_UNAVAILABLE_IN_CLI: &str = "unavailable_in_cli";

/// Reason paired with `probe: "unavailable_in_cli"` when the registry has
/// nothing negative to report about a CLI connector: everything headless can
/// check passes, but "ready" for a CLI connector means "logged in", and the
/// login state lives in the connector runtime. Named after the limitation
/// (same spirit as `token_check: "unavailable_in_cli"` in `tools auth`) so a
/// reader is not sent chasing a fault that was never observed.
const REASON_CONNECTION_UNKNOWN: &str = "connection_unknown_in_cli";

/// Reason for a non-CLI package the store flags `degraded` — "registered, but
/// its assets are missing" (`store::BundleRecord::installed`). The verdict is
/// derived HERE rather than inside `readiness_for` on purpose: the desktop
/// readiness card consumes `readiness_for` verbatim through `bundle_readiness`'s
/// `_` arm, so demoting a degraded package there would flip a GUI card that
/// nobody asked to change. The CLI needs the signal (a package whose resources
/// are gone cannot serve a request, and `actions_for` already offers `repair`
/// for it regardless of kind), and `degraded` is on `BundleInfo`, so the row
/// builder derives it itself. Credentials keep precedence: a missing required
/// credential is the cause the operator can fix unaided, and `readiness_for`
/// already reports it.
const REASON_ASSETS_MISSING: &str = "assets_missing";

fn readiness(output: OutputMode) -> Result<CliOutcome, CliError> {
    let registry = BundleRegistry::new();
    let rows = registry
        .list_bundles()
        .into_iter()
        .map(|bundle| {
            // Credential presence is only consulted for installed bundles:
            // a fresh read-only CLI run (nothing installed) must never touch
            // the OS keyring. Uninstalled bundles report the same
            // `missing_credentials` reason the GUI derives for absent
            // credentials.
            let credential_store = SystemCredentialStore::new();
            let bundle_id = bundle.id.clone();
            let has = |key: &str| -> bool {
                if !bundle.installed {
                    return false;
                }
                bundle
                    .credentials
                    .iter()
                    .find(|spec| spec.key == key)
                    .is_some_and(|spec| {
                        let reference = CredentialReference::for_mcp_secret(
                            &bundle_id,
                            keyring_target(spec.target),
                            key,
                        );
                        credential_store.get(&reference).ok().flatten().is_some()
                    })
            };
            let (registry_ready, registry_reason) = match readiness_for(&bundle, has) {
                Readiness::Ready => (true, None),
                Readiness::NotReady(reason) => (false, Some(reason.to_owned())),
            };
            // A CLI connector's readiness IS its connection state: the desktop
            // answers it from a live `*_status` probe (`bundle_readiness`'s Cli
            // arm, `connected = connected_of(&status)`), which needs the
            // connector runtime this crate does not link. `readiness_for`'s
            // headless fallback only sees registry facts — installed, and its
            // assets verify — and a logged-out feishu satisfies both. Passing
            // that through as `ready: true` would be a false positive against
            // the GUI's `not_connected`, so the headless surface never claims a
            // CLI bundle is ready: it reports the conservative `false` plus a
            // reason and a probe marker naming the limitation, the same
            // disclosure shape `tools auth` uses for `oauth_token_present` /
            // `token_check`. Registry-visible negatives (`cli_not_installed`,
            // `cli_disconnected`, `cli_assets_mismatch`) are facts the CLI
            // really did observe and pass through unchanged — only their probe
            // marker records that a connection check was still not performed.
            let (ready, reason, probe) = if bundle.kind == BundleKind::Cli {
                let reason = registry_reason.or_else(|| Some(REASON_CONNECTION_UNKNOWN.to_owned()));
                (false, reason, PROBE_UNAVAILABLE_IN_CLI)
            } else if registry_ready && bundle.degraded.is_some() {
                // Headless-only demotion (see REASON_ASSETS_MISSING): the
                // registry has nothing else against the package, but the store
                // says its assets are gone. Still `probe: "registry"` — the
                // verdict is fully decided from registry state, no live probe
                // was skipped.
                (
                    false,
                    Some(REASON_ASSETS_MISSING.to_owned()),
                    PROBE_REGISTRY,
                )
            } else {
                (registry_ready, registry_reason, PROBE_REGISTRY)
            };
            let kind = serde_json::to_value(&bundle.kind)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".to_owned());
            serde_json::json!({
                "bundle_id": bundle.id,
                "kind": kind,
                "installed": bundle.installed,
                "ready": ready,
                "reason": reason,
                "probe": probe,
            })
        })
        .collect::<Vec<_>>();
    let value = serde_json::json!({ "bundles": rows });
    let human = rows
        .iter()
        .map(|row| {
            // The probe column is part of the human line too: without it a
            // reader cannot tell a verdict that was computed from a verdict
            // that could not be computed here.
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                row["bundle_id"].as_str().unwrap_or(""),
                row["kind"].as_str().unwrap_or(""),
                row["installed"].as_bool().unwrap_or(false),
                row["ready"].as_bool().unwrap_or(false),
                row["reason"].as_str().unwrap_or("-"),
                row["probe"].as_str().unwrap_or("-"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(success(render(output, human, &value)))
}

fn set_enabled(
    id: &str,
    scope: ScopeArg,
    enabled: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let action = if enabled { "enabled" } else { "disabled" };
    // The id is matched against what the CLI can see installed (tool
    // packages + installed skills). `skill:`-prefixed or companion-owned
    // skills remap to a package id inside the storage layer, so this is a
    // warning, not a gate — but `enable some-unknown-id` no longer reports
    // unverified success.
    let installed = known_installed_ids();
    let stripped = id.strip_prefix("skill:").unwrap_or(id);
    let known = installed.iter().any(|existing| existing == id)
        || installed.iter().any(|existing| existing == stripped);
    // Storage keys on the package id the raw id remaps to (a `skill:`- or
    // companion-owned skill id maps to its owner package), so the mutation
    // and the read-back verification must both use that id: comparing the
    // raw id reported false `persistence_verified` for enables (nothing was
    // removed) and could never verify disables of remapped ids.
    let packages = pinvou3_lib::features::marketplace::package_id_for(id);
    // `--scope both` is two independent single-scope writes: the storage layer
    // exposes one scope per critical section and no two-scope transaction, so
    // a failure on the second scope cannot roll the first one back. Rather
    // than let the caller believe an all-or-nothing apply, every scope that
    // landed is recorded and reported — in the error when a later scope fails,
    // and in the success payload otherwise.
    let mut applied: Vec<&'static str> = Vec::new();
    for connector_scope in scope.scopes() {
        // Single-critical-section read-modify-write: loading and saving in
        // two separate lock acquisitions let a concurrent GUI toggle between
        // them be silently dropped (the same lost-update window the app
        // layer documented and fixed for its own RMW).
        // The RMW is fail-closed: an unavailable bundle lock, a corrupt
        // consent file, or a failing write all surface through this `?`.
        //
        // The requested state is verified INSIDE the closure, i.e. under the
        // same flock the write holds and against the exact list the writer is
        // about to persist (it maps every entry through the same
        // `package_id_for` normalization, which is idempotent on an already
        // normalized id). The previous shape re-read the scope with
        // `load_disabled_bundles_for` AFTER the lock was released: a GUI
        // toggle of the same package in that window was reported as "the
        // storage write failed or was dropped" for a write that had in fact
        // succeeded, i.e. a hard failure invented by an unrelated concurrent
        // writer.
        let recorded = std::cell::Cell::new(false);
        pinvou3_lib::features::marketplace::update_disabled_bundles_for(
            connector_scope,
            |ids: &mut Vec<String>| {
                if enabled {
                    ids.retain(|existing| existing != &packages);
                } else if !ids.iter().any(|existing| existing == &packages) {
                    ids.push(packages.clone());
                }
                // Verified on the NORMALIZED projection, which is what the
                // writer persists and what every later read resolves: an
                // entry whose ownership flipped (a companion skill id still
                // spelled raw) normalizes onto `packages` too, and an enable
                // that only dropped the exact spelling would otherwise report
                // success while the package stayed disabled. The per-entry
                // `package_id_for` is the same walk the writer already runs
                // inside this critical section, so it adds no new scan class.
                recorded.set(ids.iter().any(|existing| {
                    pinvou3_lib::features::marketplace::package_id_for(existing) == packages
                }));
            },
        )
        .map_err(|error| {
            CliError::failed(format!(
                "plugins {action}: could not update disabled bundles for {id} in \
                 scope {} : {error}{}",
                connector_scope.as_str(),
                applied_scopes_suffix(&applied)
            ))
        })?;
        // Only reachable if the mutation above did not leave the list in the
        // requested state — storage errors already surfaced through the `?`.
        // Fail closed in both directions: an enable that did not stick
        // re-activates the package, and a lost disable leaves it ACTIVE while
        // the caller sees success.
        if recorded.get() == enabled {
            return Err(CliError::failed(format!(
                "plugins {action}: could not persist {id} for scope {} (the resolved \
                 disabled set did not take the requested state){}",
                connector_scope.as_str(),
                applied_scopes_suffix(&applied)
            )));
        }
        applied.push(connector_scope.as_str());
    }
    // Reaching this point means every scope's under-lock verification matched
    // the requested state; a mismatch hard-failed above.
    let value = serde_json::json!({
        "id": id,
        "action": action,
        "scope": scope.label(),
        "scopes_applied": applied,
        "known_id": known,
        "persistence_verified": true,
    });
    let mut human = format!("{action} {id} (scope={})", scope.label());
    if !known {
        human.push_str(
            "\nwarning: id not found in the installed catalog; the toggle was recorded anyway",
        );
    }
    // Caveat at the point of action: the GUI runs `hot_refresh` after a scope
    // change (`refresh_live_sessions_skills` + `refresh_permission_rulesets`),
    // which needs the engine pool the CLI does not host. A desktop app running
    // alongside keeps its live engines on the whitelist they started with.
    human.push_str(
        "\nnote: no hot-refresh broadcast was sent; a running desktop app's live \
         engines keep the previous whitelist until they are restarted",
    );
    Ok(success(render(output, human, &value)))
}

/// Suffix naming the scopes a `--scope both` run already persisted when a
/// later scope fails. The scopes are written one critical section at a time
/// and the storage layer offers no two-scope transaction, so the earlier write
/// stands; saying which ones landed is the difference between a recoverable
/// message and a caller who assumes nothing changed.
fn applied_scopes_suffix(applied: &[&str]) -> String {
    if applied.is_empty() {
        String::new()
    } else {
        format!(
            " (partially applied: scope {} already persisted and was NOT rolled back)",
            applied.join(", ")
        )
    }
}

/// The installed ids the CLI can see: tool packages plus installed skills.
fn known_installed_ids() -> Vec<String> {
    let mut ids = MarketplaceManager::new().installed_ids();
    ids.extend(SkillMarketplaceManager::new().installed_skill_ids());
    ids
}

fn project_skills(enabled: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    // `skill_scope::set_project_skills_enabled` (alias of the scope.rs
    // storage the GUI set_project_skills_enabled command writes). The write
    // is fail-closed; the getter still verifies the persisted value.
    skill_scope::set_project_skills_enabled(enabled)
        .map_err(|error| CliError::failed(format!("plugins project-skills: {error}")))?;
    if skill_scope::project_skills_enabled() != enabled {
        return Err(CliError::failed(
            "plugins project-skills: could not persist the new value (the storage write \
             failed or was dropped)",
        ));
    }
    let value = serde_json::json!({ "project_skills_enabled": enabled });
    let human = if enabled {
        "project skills enabled".to_owned()
    } else {
        "project skills disabled".to_owned()
    };
    Ok(success(render(output, human, &value)))
}

// ---------------------------------------------------------------------------
// zip building for import (std-only stored zip)
// ---------------------------------------------------------------------------
//
// The GUI wraps single .md files and (for the CLI) SKILL.md directories into
// a temporary zip before the unified import. The CLI crate links no zip
// writer, so a minimal STORED (uncompressed) zip is emitted here; the zip
// reader on the import path only needs the standard local header / central
// directory / EOCD layout.

fn temp_zip_path(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("{label}-{}-{nanos}.zip", std::process::id()))
}

/// Removes a temporary wrapper zip when dropped. Armed right after the
/// exclusive create and dropped only once the import no longer needs the
/// file, so a failed write (or any early return in between) cannot leave
/// `pinvou-cli-import-*.zip` debris in the shared temp directory.
struct TempWrapperZip {
    path: PathBuf,
}

impl Drop for TempWrapperZip {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The import-limit rejection every import input shares.
fn import_over_limit(display: &str, file: &Path) -> CliError {
    CliError::failed(format!(
        "plugins import({}): import input exceeds the {} MiB import limit: {}",
        display,
        plugin_import::MAX_PLUGIN_SIZE_BYTES / 1024 / 1024,
        file.display()
    ))
}

/// Stats one import input and rejects non-regular files from metadata:
/// opening a FIFO (or device) would block until an unrelated writer appears,
/// so it must fail before any open. Returns the declared file length.
fn import_file_meta(path: &Path, what: &str, display: &str) -> Result<u64, CliError> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        CliError::failed(format!(
            "plugins import({}): cannot read {what}: {error}",
            display
        ))
    })?;
    if !metadata.is_file() {
        return Err(CliError::failed(format!(
            "plugins import({}): {what} is not a regular file: {}",
            display,
            path.display()
        )));
    }
    Ok(metadata.len())
}

/// Reads one input file for the pre-import wrap under the unified import
/// pipeline's own package limit (`plugin_import::MAX_PLUGIN_SIZE_BYTES`,
/// imported directly so a change breaks this crate's build), tracked against
/// `cumulative` so a directory cannot exceed the limit file by file. The
/// size check runs before any allocation and again while reading, so a file
/// that grows (or lies about its size) between stat and read is still
/// bounded.
fn read_import_file_capped(
    path: &Path,
    what: &str,
    display: &str,
    cumulative: &mut u64,
) -> Result<Vec<u8>, CliError> {
    let limit = plugin_import::MAX_PLUGIN_SIZE_BYTES;
    let declared = import_file_meta(path, what, display)?;
    if declared > limit || *cumulative + declared > limit {
        return Err(import_over_limit(display, path));
    }
    let file = std::fs::File::open(path).map_err(|error| {
        CliError::failed(format!(
            "plugins import({}): cannot read {what}: {error}",
            display
        ))
    })?;
    let remaining = limit - *cumulative;
    let mut reader = file.take(remaining + 1);
    let mut bytes = Vec::with_capacity(declared.min(remaining) as usize);
    reader.read_to_end(&mut bytes).map_err(|error| {
        CliError::failed(format!(
            "plugins import({}): cannot read {what}: {error}",
            display
        ))
    })?;
    if bytes.len() as u64 > remaining {
        return Err(import_over_limit(display, path));
    }
    *cumulative += bytes.len() as u64;
    Ok(bytes)
}

/// Re-charges the import budget when a SKILL.md is replaced by its wrapped
/// form: `wrap_markdown_skill` PREPENDS a frontmatter block, so the bytes that
/// actually go into the package are larger than the ones
/// [`read_import_file_capped`] charged. The raw length is given back and the
/// wrapped one taken, then the budget is re-checked — pushing the wrapper in
/// without any charge let an import sitting just under
/// `MAX_PLUGIN_SIZE_BYTES` ship a package over it by the frontmatter's size.
fn charge_wrapped_skill_md(
    cumulative: &mut u64,
    raw_len: usize,
    wrapped: &[u8],
    display: &str,
    path: &Path,
) -> Result<(), CliError> {
    // saturating: the raw read is always part of `cumulative`, but a future
    // caller that charges differently must not wrap around into a huge budget.
    *cumulative = cumulative.saturating_sub(raw_len as u64) + wrapped.len() as u64;
    if *cumulative > plugin_import::MAX_PLUGIN_SIZE_BYTES {
        return Err(import_over_limit(display, path));
    }
    Ok(())
}

/// Recursively collects regular, non-hidden files under `root` as
/// (zip-relative, bytes) entries with `/` separators, in sorted order so the
/// produced archive is deterministic. Total bytes read are bounded through
/// `cumulative` by the import pipeline's package limit.
fn collect_directory_entries(
    root: &Path,
    display: &str,
    cumulative: &mut u64,
) -> Result<Vec<(String, Vec<u8>)>, CliError> {
    let mut entries = Vec::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let read = std::fs::read_dir(&dir).map_err(|error| {
            CliError::failed(format!(
                "plugins import({}): cannot read directory: {error}",
                dir.display()
            ))
        })?;
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // Same skip rules as the import pipeline's extraction pass.
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            // Symlinks are rejected by the import pipeline anyway; skip them
            // here so the wrapper zip cannot smuggle one in.
            if entry
                .file_type()
                .map(|file_type| file_type.is_symlink())
                .unwrap_or(true)
            {
                continue;
            }
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if path.is_dir() {
                stack.push((path, rel));
            } else if path.is_file() {
                let bytes = read_import_file_capped(&path, "file", display, cumulative)?;
                entries.push((rel, bytes));
            }
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

/// Mirrors the GUI `import_skill_md_content` fallback: a `.md` file with no
/// frontmatter `name` gets a deterministic `skill-<fnv1a(stem)>` name
/// injected as minimal frontmatter; files that already declare `name` are
/// imported unchanged.
fn wrap_markdown_skill(content: &str, filename: &str) -> String {
    if frontmatter_name(content).is_some() {
        return content.to_owned();
    }
    let stem = filename
        .rfind('.')
        .map(|index| &filename[..index])
        .unwrap_or(filename);
    let sanitized = sanitize_skill_name(stem);
    let fallback = if sanitized == "skill" && !stem.is_empty() {
        format!("skill-{}", stable_stem_hash(stem))
    } else {
        sanitized
    };
    format!("---\nname: {fallback}\n---\n\n{content}")
}

/// Mirrors `skill_marketplace::read_skill_name_from_str` (pub(crate) in the
/// app): frontmatter `name:` from a leading `---` block.
fn frontmatter_name(content: &str) -> Option<String> {
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("name:") {
            let value = rest.trim().trim_matches('"').trim_matches('\'').trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

/// Mirrors `skill_marketplace::sanitize_skill_name`: `[A-Za-z0-9_-]`, other
/// characters collapse to `-`, trimmed, max 64 chars, empty → "skill".
fn sanitize_skill_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "skill".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Mirrors the GUI's FNV-1a 64-bit `stable_stem_hash`: deterministic,
/// cross-platform stable id derivation for non-sanitizable file names.
fn stable_stem_hash(stem: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in stem.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// CRC-32 (IEEE 802.3, reflected), bitwise — enough for a handful of small
/// entries and avoids a lookup table.
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

fn push_u16(buffer: &mut Vec<u8>, value: u16) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(buffer: &mut Vec<u8>, value: u32) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

/// Builds a stored (method 0) zip archive from name→bytes entries. Names use
/// `/` separators. No unix external attributes are emitted (version made by
/// is DOS), so readers see plain files, never symlinks.
fn build_stored_zip(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, CliError> {
    if entries.is_empty() {
        return Err(CliError::failed(
            "plugins import: package directory is empty after filtering",
        ));
    }
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, data) in entries {
        let name_bytes = name.as_bytes();
        if name_bytes.len() > u16::MAX as usize {
            return Err(CliError::failed(format!(
                "plugins import: entry name too long: {name}"
            )));
        }
        let offset = u32::try_from(out.len())
            .map_err(|_| CliError::failed("plugin package exceeds the 4 GiB archive limit"))?;
        let checksum = crc32(data);
        // Local file header
        push_u32(&mut out, 0x0403_4b50);
        push_u16(&mut out, 20); // version needed
        push_u16(&mut out, 0x0800); // UTF-8 names
        push_u16(&mut out, 0); // method: stored
        push_u16(&mut out, 0); // mod time
        push_u16(&mut out, 0x21); // mod date (1980-01-01)
        push_u32(&mut out, checksum);
        let size = u32::try_from(data.len()).map_err(|_| {
            CliError::failed("a single plugin file exceeds the 4 GiB archive limit")
        })?;
        push_u32(&mut out, size); // compressed
        push_u32(&mut out, size); // uncompressed
        push_u16(&mut out, name_bytes.len() as u16);
        push_u16(&mut out, 0); // extra length
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(data);
        // Central directory entry
        push_u32(&mut central, 0x0201_4b50);
        push_u16(&mut central, 20); // version made by (DOS)
        push_u16(&mut central, 20); // version needed
        push_u16(&mut central, 0x0800);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0x21);
        push_u32(&mut central, checksum);
        push_u32(&mut central, size);
        push_u32(&mut central, size);
        push_u16(&mut central, name_bytes.len() as u16);
        push_u16(&mut central, 0); // extra
        push_u16(&mut central, 0); // comment
        push_u16(&mut central, 0); // disk number start
        push_u16(&mut central, 0); // internal attrs
        push_u32(&mut central, 0); // external attrs
        push_u32(&mut central, offset);
        central.extend_from_slice(name_bytes);
    }
    let central_offset = u32::try_from(out.len())
        .map_err(|_| CliError::failed("plugin package exceeds the 4 GiB archive limit"))?;
    let central_size = u32::try_from(central.len())
        .map_err(|_| CliError::failed("plugin package exceeds the 4 GiB archive limit"))?;
    out.extend_from_slice(&central);
    // End of central directory
    push_u32(&mut out, 0x0605_4b50);
    push_u16(&mut out, 0); // disk number
    push_u16(&mut out, 0); // disk with central directory
    let entry_count = u16::try_from(entries.len())
        .map_err(|_| CliError::failed("plugin package exceeds the 65535-entry archive limit"))?;
    push_u16(&mut out, entry_count);
    push_u16(&mut out, entry_count);
    push_u32(&mut out, central_size);
    push_u32(&mut out, central_offset);
    push_u16(&mut out, 0); // comment length
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_rejections_translate_to_english() {
        // The shared importer's Chinese security copy (samples from
        // `plugin_import.rs`) must map to English; unknown tails pass
        // through verbatim.
        let traversal = translate_plugin_import_error("zip 含不安全路径(穿越),拒绝");
        assert_eq!(
            traversal.as_deref(),
            Some("the package contains an unsafe path (zip-slip traversal); rejected")
        );
        let bomb = translate_plugin_import_error(
            "插件包实际解压超过 200 MiB 上限（zip 头声明与真实大小不符，可能为 zip bomb）",
        );
        assert_eq!(
            bomb.as_deref(),
            Some(format!(
                "the package decompresses beyond the {} MiB cap (declared and actual size \
                 must both fit); rejected",
                plugin_import::MAX_PLUGIN_SIZE_BYTES / 1024 / 1024
            ))
            .as_deref()
        );
        assert_eq!(
            translate_plugin_import_error("some unrelated english failure"),
            None
        );
    }

    #[test]
    fn scope_arg_covers_both_scopes() {
        assert_eq!(ScopeArg::Plain.scopes(), vec![ConnectorScope::Plain]);
        assert_eq!(
            ScopeArg::Both.scopes(),
            vec![ConnectorScope::Plain, ConnectorScope::Code]
        );
    }

    #[test]
    fn secret_pairs_require_key_and_env_name() {
        assert_eq!(
            parse_secret_pair("AMAP_KEY=MY_ENV").unwrap(),
            ("AMAP_KEY".to_owned(), "MY_ENV".to_owned())
        );
        assert!(parse_secret_pair("AMAP_KEY").is_err());
        assert!(parse_secret_pair("=ENV").is_err());
        assert!(parse_secret_pair("KEY=").is_err());
    }

    #[test]
    fn markdown_wrapper_injects_deterministic_fallback_name() {
        let named = wrap_markdown_skill("---\nname: mine\n---\nbody", "x.md");
        assert_eq!(named, "---\nname: mine\n---\nbody");
        let plain = wrap_markdown_skill("body", "我的技能.md");
        assert!(plain.starts_with("---\nname: skill-"));
    }

    #[test]
    fn stored_zip_roundtrip_is_well_formed() {
        let entries = vec![
            ("SKILL.md".to_owned(), b"---\nname: t\n---\n".to_vec()),
            ("docs/note.md".to_owned(), b"hello".to_vec()),
        ];
        let bytes = build_stored_zip(&entries).unwrap();
        // End of central directory signature present and entry count matches.
        let eocd = bytes.len() - 22;
        assert_eq!(&bytes[eocd..eocd + 4], &[0x50, 0x4b, 0x05, 0x06]);
        let count = u16::from_le_bytes([bytes[eocd + 10], bytes[eocd + 11]]);
        assert_eq!(count, 2);
    }
}
