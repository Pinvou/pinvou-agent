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
//!   an explicit warning (validation) instead of silently skipping.
//! - tools auth → `get_marketplace_tool_auth_status` fields computed from
//!   `installed_ids` / `oauth_remote_server_name` / a generic parse of
//!   `mcp.json`. The OAuth token presence probe lives in the foundation's
//!   token store (not reachable headless), so `oauth_token_present` is
//!   reported conservatively as `false` with `token_check:
//!   "unavailable_in_cli"`.
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
//! - export → `package_export::export_installed_plugin`; recycle →
//!   `recycle_bin::{RecycleBin, restore_plugin}`; meta →
//!   `SkillMarketplaceManager::update_display_meta`.
//! - readiness → `bundle::BundleRegistry` + `readiness_for` (the registry
//!   branch of `bundle_readiness`). CLI-connector live status (feishu/wecom/
//!   dingtalk/tmeet `*_status`) and ima credential status need the connector
//!   runtime; the CLI reports the registry's conservative readiness and
//!   defers live probes to the `connectors` family. Credential presence is
//!   consulted in the system credential store only for installed bundles so
//!   read-only CLI usage never touches the OS keyring.
//! - enable/disable/project-skills → `scope::load_disabled_bundles_for` /
//!   `save_disabled_bundles_for` / `set_project_skills_enabled` (the storage
//!   behind `set_disabled_skills` / `set_project_skills_enabled`).
//!
//! Pure storage only: no Tauri host, no engine, no async runtime.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::support::{render, require_yes, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::marketplace::{
    ConnectorScope, MarketplaceManager,
    bundle::{BundleRegistry, Readiness, keyring_target, readiness_for},
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
    if id.is_empty() {
        return Err(CliError::usage("plugins command requires an id"));
    }
    Ok(id)
}

/// Mirrors the sessions-family flag parser: every token must be a known value
/// flag (followed by a non-empty value) or a known boolean flag.
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
                return Err(CliError::usage(format!("duplicate plugins option {token}")));
            }
            flags.push(token);
            index += 1;
            continue;
        }
        if !value_flags.contains(&token) {
            return Err(CliError::usage(format!(
                "unsupported plugins option: {token}"
            )));
        }
        if options.iter().any(|(name, _)| *name == token) {
            return Err(CliError::usage(format!("duplicate plugins option {token}")));
        }
        let value = values
            .get(index + 1)
            .ok_or_else(|| CliError::usage(format!("plugins option {token} requires a value")))?;
        if value.is_empty() || value.starts_with("--") {
            return Err(CliError::usage(format!(
                "plugins option {token} requires a value"
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

fn parse_positive(options: &[(&str, &str)], name: &str) -> Result<Option<u64>, CliError> {
    match option(options, name) {
        None => Ok(None),
        Some(value) => value
            .parse::<u64>()
            .ok()
            .filter(|count| *count > 0)
            .map(Some)
            .ok_or_else(|| CliError::usage(format!("plugins {name} must be a positive integer"))),
    }
}

fn feature_error(action: &str, id: &str, error: impl std::fmt::Display) -> CliError {
    CliError::failed(format!("plugins {action}({id}): {error:#}"))
}

pub fn execute(command: PluginsCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
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
    // a companion failure is logged and does not roll back the MCP install.
    let mut companion_note = Vec::new();
    for sid in mgr.companion_skills(id) {
        match SkillMarketplaceManager::new().install(&sid) {
            Ok(()) => {
                skill_scope::sync_deny_all_scopes_after_skill_install(&sid);
                companion_note.push(sid);
            }
            Err(error) => {
                eprintln!("[plugins] companion skill '{sid}' install failed: {error}");
            }
        }
    }
    // DenyAll scopes (e.g. code) keep newly installed packages off by default.
    sync_deny_all_scopes_after_install(id);
    // The GUI validates remote MCP connections right after install
    // (validate_on_install manifests). The handshake runs on the foundation's
    // async MCP stack, unavailable headless: surface an explicit warning
    // instead of pretending the connection was verified.
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
            "\nwarning: remote connection validation skipped (headless CLI); check 'pinvou plugins tools auth {id}'"
        ));
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
        skill_scope::remove_skill_from_disabled_scopes(sid);
    }
    mgr.uninstall(id)
        .map_err(|error| feature_error("tools uninstall", id, error))?;
    if recycles_with_package {
        for sid in &companions {
            skill_scope::remove_skill_from_disabled_scopes(sid);
        }
    }
    // Keep the disabled sets free of stale connector ids (GUI parity).
    pinvou3_lib::features::marketplace::remove_connector_from_disabled_scopes(id);
    let action = if recycles_with_package {
        "uninstalled (moved to recycle bin)"
    } else {
        "uninstalled"
    };
    let value =
        serde_json::json!({ "id": id, "action": "uninstalled", "recycled": recycles_with_package });
    Ok(success(render(output, format!("{action} {id}"), &value)))
}

fn mcp_json_servers(context: &str) -> Result<Option<serde_json::Value>, CliError> {
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
    Ok(value.get("servers").cloned())
}

fn tools_auth(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let mgr = MarketplaceManager::new();
    let installed = mgr.installed_ids().iter().any(|tool| tool == id);
    let server_name = mgr.oauth_remote_server_name(id);
    let oauth_required = server_name.is_some();
    let mut mcp_configured = false;
    if let Some(name) = server_name.as_deref() {
        if let Some(servers) = mcp_json_servers("auth")? {
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
    let configured = mcp_json_servers("oauth-login")?
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
    skill_scope::sync_deny_all_scopes_after_skill_install(id);
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
    skill_scope::remove_skill_from_disabled_scopes(id);
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
    let display = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "plugin.zip".to_owned());
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
        let mut entries = collect_directory_entries(path)?;
        // A SKILL.md without a frontmatter `name` is rejected downstream as
        // an "empty package"; the .md channel derives a name from the file
        // name, so the directory channel injects the same fallback here.
        let skill_md = std::fs::read_to_string(path.join("SKILL.md")).unwrap_or_default();
        if frontmatter_name(&skill_md).is_none() {
            let wrapped = wrap_markdown_skill(&skill_md, &sanitize_skill_name(&display));
            entries.retain(|(name, _)| name != "SKILL.md");
            entries.push(("SKILL.md".to_owned(), wrapped.into_bytes()));
        }
        Some((
            temp_zip_path("pinvou-cli-import-dir"),
            build_stored_zip(&entries)?,
        ))
    } else if extension.as_deref() == Some("md") || extension.as_deref() == Some("markdown") {
        let content = std::fs::read_to_string(path).map_err(|error| {
            CliError::failed(format!(
                "plugins import({}): cannot read skill file: {error}",
                path.display()
            ))
        })?;
        let wrapped = wrap_markdown_skill(&content, &display);
        Some((
            temp_zip_path("pinvou-cli-import-md"),
            build_stored_zip(&[("SKILL.md".to_owned(), wrapped.into_bytes())])?,
        ))
    } else if extension.as_deref() == Some("zip") {
        None
    } else {
        return Err(CliError::usage(
            "plugins import supports .zip plugin packages, .md/.markdown skill files, or directories containing SKILL.md",
        ));
    };
    let import_path;
    if let Some((tmp, bytes)) = &wrapper {
        std::fs::write(tmp, bytes).map_err(|error| {
            CliError::failed(format!(
                "plugins import: cannot write temporary zip: {error}"
            ))
        })?;
        import_path = tmp.clone();
    } else {
        import_path = path.to_path_buf();
    }
    let result = plugin_import::import_plugin_package(&import_path.to_string_lossy(), &display);
    if wrapper.is_some() {
        let _ = std::fs::remove_file(&import_path); // temp wrapper: clean up on all paths
    }
    let report = result.map_err(|error| feature_error("import", &display, error))?;
    // Upload safety default (GUI parity): imported packages start disabled in
    // initialized DenyAll scopes until explicitly enabled.
    sync_deny_all_scopes_after_install(&report.id);
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
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|error| {
                CliError::failed(format!(
                    "plugins export({id}): cannot create output directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
    }
    package_export::export_installed_plugin(id, &dest)
        .map_err(|error| feature_error("export", id, error))?;
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
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|error| {
                CliError::failed(format!(
                    "plugins recycle export({id}): cannot create output directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
    }
    recycle_bin::RecycleBin::new()
        .export_package(id, &dest)
        .map_err(|error| feature_error("recycle export", id, error))?;
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
            let (ready, reason) = match readiness_for(&bundle, has) {
                Readiness::Ready => (true, None),
                Readiness::NotReady(reason) => (false, Some(reason.to_owned())),
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
            })
        })
        .collect::<Vec<_>>();
    let value = serde_json::json!({ "bundles": rows });
    let human = rows
        .iter()
        .map(|row| {
            format!(
                "{}\t{}\t{}\t{}\t{}",
                row["bundle_id"].as_str().unwrap_or(""),
                row["kind"].as_str().unwrap_or(""),
                row["installed"].as_bool().unwrap_or(false),
                row["ready"].as_bool().unwrap_or(false),
                row["reason"].as_str().unwrap_or("-"),
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
    for connector_scope in scope.scopes() {
        let mut ids =
            pinvou3_lib::features::marketplace::load_disabled_bundles_for(connector_scope);
        if enabled {
            ids.retain(|existing| existing != id);
        } else if !ids.iter().any(|existing| existing == id) {
            ids.push(id.to_owned());
        }
        // save_disabled_bundles_for marks the scope initialized — the same
        // storage effect the GUI's set_disabled_skills toggle produces.
        pinvou3_lib::features::marketplace::save_disabled_bundles_for(connector_scope, &ids);
    }
    let value = serde_json::json!({
        "id": id,
        "action": action,
        "scope": scope.label(),
    });
    Ok(success(render(
        output,
        format!("{action} {id} (scope={})", scope.label()),
        &value,
    )))
}

fn project_skills(enabled: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    // `skill_scope::set_project_skills_enabled` (alias of the scope.rs
    // storage the GUI set_project_skills_enabled command writes).
    skill_scope::set_project_skills_enabled(enabled);
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

/// Recursively collects regular, non-hidden files under `root` as
/// (zip-relative, bytes) entries with `/` separators, in sorted order so the
/// produced archive is deterministic.
fn collect_directory_entries(root: &Path) -> Result<Vec<(String, Vec<u8>)>, CliError> {
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
                let bytes = std::fs::read(&path).map_err(|error| {
                    CliError::failed(format!(
                        "plugins import({}): cannot read file: {error}",
                        path.display()
                    ))
                })?;
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
    let central_size = central.len() as u32;
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
