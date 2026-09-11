//! `code` family: the GUI's Code mode surface (ACP agents, providers,
//! code sessions, workspace browsing, checkpoints), mirroring
//! `pinvou3-app/src-tauri/src/app/commands/{codex,acp_providers,checkpoints}.rs`.
//!
//! Data access mapping (feature layer reached directly; no Tauri host):
//! - agents/catalog → `codex_acp::AcpPool::agent_catalog()` (the static
//!   registry the GUI list command renders) plus a local PATH/`--version`
//!   CLI probe mirroring `features::codex_acp::{runtime,login}` (those
//!   helpers are `pub(super)` and unreachable from the CLI crate).
//! - providers → `codex_acp::ProviderManager`, the exact orchestration the
//!   GUI provider commands call (`acp-providers.json` + system credential
//!   store + per-CLI config writers).
//! - sessions → `SessionStore` + `SessionAgentStore` (`session-agents.json`)
//!   with the same code-session filter as `list_codex_acp_sessions`, and the
//!   persisted `acp-timeline.jsonl` behind `AcpPool::timeline`.
//! - workspace → local mirror of `features::codex_acp::workspace`
//!   (`pub(crate)` and unreachable): list/search/preview/changes/diff/
//!   branches/checkout with the same limits, path validation, git semantics
//!   and process-local `CHECKOUT_LOCK`.
//! - checkpoints → `features::code_checkpoints` public functions plus the
//!   `SessionStore` rewind sidecar methods, mirroring the
//!   `rewind_to_turn` / `undo_last_rewind` orchestration. User-turn counting
//!   is a documented JSON approximation of `code_checkpoints::count_user_turns`
//!   (`pub(crate)`); the store re-validates with the exact predicate inside
//!   `truncate_to_user_turn` / `restore_rewound_turns`.
//!
//! Concurrency: `checkpoints rewind` / `undo` and `workspace checkout`
//! serialize through a cross-process advisory lock under
//! `~/.pinvou3/locks/` (CLI×CLI). The GUI's busy guards are process-local
//! and cannot be observed from the CLI, so a GUI turn running on the same
//! session is not detectable — see the `checkpoints rewind` doc comment.
//! - run/permissions/respond → stable honest errors: driving the ACP adapter
//!   (async protocol client, adapter process, pending-permission store) is
//!   bound to the product host; the pending-permission store is process-local
//!   to the GUI's `AcpPool` and cannot be reached from another process.
//!
//! Exit codes: 0 success, 1 host failure, 2 usage error. JSON output is a
//! single serde_json line mirroring the GUI DTOs (camelCase fields).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::support::{render, require_yes, resolve_secret, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::code_checkpoints as checkpoints;
use pinvou3_lib::features::codex_acp::{
    AcpPool, AcpProvidersView, AgentBackend, CodexWorkspaceKind, ProviderManager, ProviderWireApi,
    SessionAgentStore,
};
use pinvou3_lib::features::sessions::{SessionKind, SessionStore};
use pinvou3_lib::platform::credential_store::{CredentialEditAction, SystemCredentialStore};
use pinvou3_lib::platform::paths;
use wait_timeout::ChildExt;

const USAGE: &str = "usage: pinvou code <agents|login|logout|providers|sessions|workspace|checkpoints|run|permissions|respond> <subcommand>";

const AGENTS_USAGE: &str =
    "usage: pinvou code agents <list|status <agent>|install <agent>>  (agent: codex|claude|kimi)";
const LOGIN_USAGE: &str = "usage: pinvou code login <agent> [--code-env VAR|--code-stdin]  \
     (agent: codex|claude|kimi; the claude flow consumes an authorization code)";
const LOGOUT_USAGE: &str = "usage: pinvou code logout <agent>  (agent: codex|claude|kimi)";
const PROVIDERS_USAGE: &str = "usage: pinvou code providers <list [--agent A]|add --agent A --name N --base-url U \
     [--wire-api anthropic|openai|kimi] [--model M] [--model-slot SLOT=M]... [--context-window N] \
     (--api-key-env V|--api-key-stdin)|update <id> --agent A [...] |remove <id> --agent A --yes \
     |switch <agent> <provider-id>|switch-official <agent>|export --agent A [--output PATH] \
     |import --agent A <PATH>|probe <provider-id> --agent A>";
const SESSIONS_USAGE: &str = "usage: pinvou code sessions <list|info <id>|timeline <id>>";
const WORKSPACE_USAGE: &str = "usage: pinvou code workspace <list <session> [path]|search <session> Q|preview <session> FILE|changes <session>|diff <session> [FILE]|branches <session>|checkout <session> BRANCH --mode carry|stash|commit [--message M]>";
const CHECKPOINTS_USAGE: &str = "usage: pinvou code checkpoints <list <session>|diff <session> <checkpoint-id>|rewind <session> <turn> --yes|undo <session>>";
const RUN_USAGE: &str = "usage: pinvou code run <agent> --workspace DIR (--prompt-file F|--prompt S) [--timeout-secs N]";
const PERMISSIONS_USAGE: &str = "usage: pinvou code permissions <session>";
const RESPOND_USAGE: &str = "usage: pinvou code respond <session> <request-id> <allow|deny>";

/// Minimum agent CLI versions enforced by the GUI runtime probes
/// (`features::codex_acp`): codex via `MIN_CODEX_VERSION`, claude/kimi via
/// `MIN_CLAUDE_VERSION` / `MIN_KIMI_VERSION`.
const MIN_VERSIONS: [(&str, &str); 3] =
    [("codex", "0.144.6"), ("claude", "2.0.0"), ("kimi", "0.9.0")];

/// Hard deadline for the vendor logout subcommand (login uses 600/1800s
/// because it waits for the user; logout is a fast local call).
const LOGOUT_TIMEOUT_SECS: u64 = 120;

// ── command tree ────────────────────────────────────────────────────────────

/// Where the claude authorization code comes from; plaintext argv exists only
/// for callers that already hold the code in argv (env/stdin are the
/// policy-compliant forms, mirroring every other secret in this CLI).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginCodeSource {
    Arg(String),
    Env(String),
    Stdin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeCommand {
    AgentsList,
    AgentsStatus {
        agent: String,
    },
    AgentsInstall {
        agent: String,
    },
    Login {
        agent: String,
        code: Option<LoginCodeSource>,
    },
    Logout {
        agent: String,
    },
    ProvidersList {
        agent: Option<String>,
    },
    ProvidersAdd {
        agent: String,
        name: String,
        base_url: String,
        wire_api: Option<String>,
        model: Option<String>,
        model_slots: Vec<(String, String)>,
        context_window: Option<i64>,
        api_key_env: Option<String>,
        api_key_stdin: bool,
    },
    ProvidersUpdate {
        provider_id: String,
        agent: String,
        name: Option<String>,
        base_url: Option<String>,
        wire_api: Option<String>,
        model: Option<String>,
        model_slots: Vec<(String, String)>,
        context_window: Option<i64>,
        api_key_env: Option<String>,
        api_key_stdin: bool,
        delete_key: bool,
    },
    ProvidersRemove {
        provider_id: String,
        agent: String,
        yes: bool,
    },
    ProvidersSwitch {
        agent: String,
        provider_id: String,
    },
    ProvidersSwitchOfficial {
        agent: String,
    },
    ProvidersExport {
        agent: String,
        output: Option<PathBuf>,
    },
    ProvidersImport {
        agent: String,
        path: PathBuf,
    },
    ProvidersProbe {
        provider_id: String,
        agent: String,
    },
    SessionsList,
    SessionsInfo {
        id: String,
    },
    SessionsTimeline {
        id: String,
    },
    WorkspaceList {
        session: String,
        path: Option<String>,
    },
    WorkspaceSearch {
        session: String,
        query: String,
    },
    WorkspacePreview {
        session: String,
        file: String,
    },
    WorkspaceChanges {
        session: String,
    },
    WorkspaceDiff {
        session: String,
        file: Option<String>,
    },
    WorkspaceBranches {
        session: String,
    },
    WorkspaceCheckout {
        session: String,
        branch: String,
        mode: BranchSwitchMode,
        message: Option<String>,
    },
    CheckpointsList {
        session: String,
    },
    CheckpointsDiff {
        session: String,
        checkpoint_id: String,
    },
    CheckpointsRewind {
        session: String,
        turn: u32,
        yes: bool,
    },
    CheckpointsUndo {
        session: String,
    },
    Run {
        agent: String,
        workspace: PathBuf,
        prompt_file: Option<PathBuf>,
        prompt: Option<String>,
        timeout_secs: Option<u64>,
    },
    Permissions {
        session: String,
    },
    Respond {
        session: String,
        request_id: String,
        allow: bool,
    },
}

/// Dirty-worktree branch switch strategy, mirroring
/// `features::codex_acp::workspace::BranchSwitchMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchSwitchMode {
    Carry,
    Stash,
    Commit,
}

pub fn parse(values: &[String]) -> Result<CodeCommand, CliError> {
    let subcommand = values.get(1).ok_or_else(|| CliError::usage(USAGE))?;
    let rest = &values[2..];
    match subcommand.as_str() {
        "agents" => parse_agents(rest),
        "login" => {
            let agent = require_agent(rest.first().map(String::as_str), LOGIN_USAGE)?;
            let (options, booleans, _) = parse_flags(
                &rest[1..],
                &["--code", "--code-env"],
                &["--code-stdin"],
                "login",
            )?;
            let code = if let Some(raw) = option(&options, "--code") {
                Some(LoginCodeSource::Arg(raw.to_owned()))
            } else if let Some(var) = option(&options, "--code-env") {
                Some(LoginCodeSource::Env(var.to_owned()))
            } else if booleans.contains(&"--code-stdin") {
                Some(LoginCodeSource::Stdin)
            } else {
                None
            };
            Ok(CodeCommand::Login { agent, code })
        }
        "logout" => {
            let agent = require_agent(rest.first().map(String::as_str), LOGOUT_USAGE)?;
            if rest.len() > 1 {
                return Err(CliError::usage("code logout accepts no options"));
            }
            Ok(CodeCommand::Logout { agent })
        }
        "providers" => parse_providers(rest),
        "sessions" => parse_sessions(rest),
        "workspace" => parse_workspace(rest),
        "checkpoints" => parse_checkpoints(rest),
        "run" => parse_run(rest),
        "permissions" => {
            let session = require_session_id(rest.first())?;
            if rest.len() > 1 {
                return Err(CliError::usage(PERMISSIONS_USAGE));
            }
            Ok(CodeCommand::Permissions { session })
        }
        "respond" => {
            let session = require_session_id(rest.first())?;
            let request_id = rest
                .get(1)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| CliError::usage("code respond requires a request id"))?
                .clone();
            let allow = match rest.get(2).map(String::as_str) {
                Some("allow") => true,
                Some("deny") => false,
                _ => {
                    return Err(CliError::usage(RESPOND_USAGE));
                }
            };
            if rest.len() > 3 {
                return Err(CliError::usage(RESPOND_USAGE));
            }
            Ok(CodeCommand::Respond {
                session,
                request_id,
                allow,
            })
        }
        _ => Err(CliError::usage(USAGE)),
    }
}

fn parse_agents(rest: &[String]) -> Result<CodeCommand, CliError> {
    match rest.first().map(String::as_str) {
        Some("list") => {
            if rest.len() > 1 {
                return Err(CliError::usage("code agents list accepts no options"));
            }
            Ok(CodeCommand::AgentsList)
        }
        Some("status") => {
            let agent = require_agent(rest.get(1).map(String::as_str), AGENTS_USAGE)?;
            if rest.len() > 2 {
                return Err(CliError::usage("code agents status accepts no options"));
            }
            Ok(CodeCommand::AgentsStatus { agent })
        }
        Some("install") => {
            let agent = require_agent(rest.get(1).map(String::as_str), AGENTS_USAGE)?;
            if rest.len() > 2 {
                return Err(CliError::usage("code agents install accepts no options"));
            }
            Ok(CodeCommand::AgentsInstall { agent })
        }
        _ => Err(CliError::usage(AGENTS_USAGE)),
    }
}

fn parse_providers(rest: &[String]) -> Result<CodeCommand, CliError> {
    let action = rest
        .first()
        .ok_or_else(|| CliError::usage(PROVIDERS_USAGE))?;
    let rest = &rest[1..];
    const AGENT: &str = "--agent";
    match action.as_str() {
        "list" => {
            let (options, _, _) = parse_flags(rest, &[AGENT], &[], "providers list")?;
            Ok(CodeCommand::ProvidersList {
                agent: option(&options, AGENT).map(str::to_owned),
            })
        }
        "add" => {
            let (options, flags, repeated) = parse_flags(
                rest,
                &[
                    AGENT,
                    "--name",
                    "--base-url",
                    "--wire-api",
                    "--model",
                    "--model-slot",
                    "--context-window",
                    "--api-key-env",
                ],
                &["--api-key-stdin"],
                "providers add",
            )?;
            let agent = require_agent(options.get(AGENT).copied(), PROVIDERS_USAGE)?;
            let name = option(&options, "--name").unwrap_or_default().to_owned();
            if name.trim().is_empty() {
                return Err(CliError::usage("code providers add requires --name"));
            }
            let base_url = option(&options, "--base-url")
                .unwrap_or_default()
                .to_owned();
            if base_url.trim().is_empty() {
                return Err(CliError::usage("code providers add requires --base-url"));
            }
            let model_slots = parse_model_slot_pairs(&repeated, "providers add")?;
            if let Some(wire) = option(&options, "--wire-api") {
                parse_wire_api_value(Some(wire))?;
            }
            if flags.contains(&"--api-key-stdin") && options.contains_key("--api-key-env") {
                return Err(CliError::usage(
                    "use only one of --api-key-env or --api-key-stdin",
                ));
            }
            Ok(CodeCommand::ProvidersAdd {
                agent,
                name,
                base_url,
                wire_api: option(&options, "--wire-api").map(str::to_owned),
                model: option(&options, "--model").map(str::to_owned),
                model_slots,
                context_window: parse_context_window(&options, "providers add")?,
                api_key_env: option(&options, "--api-key-env").map(str::to_owned),
                api_key_stdin: flags.contains(&"--api-key-stdin"),
            })
        }
        "update" => {
            let (positionals, options, flags, repeated) = split_positional_flags(
                rest,
                &[
                    AGENT,
                    "--name",
                    "--base-url",
                    "--wire-api",
                    "--model",
                    "--model-slot",
                    "--context-window",
                    "--api-key-env",
                ],
                &["--api-key-stdin", "--delete-key"],
                "providers update",
            )?;
            let provider_id = positionals
                .first()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| CliError::usage("code providers update requires a provider id"))?;
            if positionals.len() > 1 {
                return Err(CliError::usage(
                    "code providers update accepts one provider id",
                ));
            }
            let agent = require_agent(options.get(AGENT).copied(), PROVIDERS_USAGE)?;
            if let Some(wire) = option(&options, "--wire-api") {
                parse_wire_api_value(Some(wire))?;
            }
            if flags.contains(&"--api-key-stdin") && options.contains_key("--api-key-env") {
                return Err(CliError::usage(
                    "use only one of --api-key-env or --api-key-stdin",
                ));
            }
            if flags.contains(&"--delete-key")
                && (flags.contains(&"--api-key-stdin") || options.contains_key("--api-key-env"))
            {
                return Err(CliError::usage(
                    "--delete-key cannot be combined with a new key",
                ));
            }
            Ok(CodeCommand::ProvidersUpdate {
                provider_id: provider_id.to_string(),
                agent,
                name: option(&options, "--name").map(str::to_owned),
                base_url: option(&options, "--base-url").map(str::to_owned),
                wire_api: option(&options, "--wire-api").map(str::to_owned),
                model: option(&options, "--model").map(str::to_owned),
                model_slots: parse_model_slot_pairs(&repeated, "providers update")?,
                context_window: parse_context_window(&options, "providers update")?,
                api_key_env: option(&options, "--api-key-env").map(str::to_owned),
                api_key_stdin: flags.contains(&"--api-key-stdin"),
                delete_key: flags.contains(&"--delete-key"),
            })
        }
        "remove" => {
            let (positionals, options, flags, _) =
                split_positional_flags(rest, &[AGENT], &["--yes"], "providers remove")?;
            let provider_id = positionals
                .first()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| CliError::usage("code providers remove requires a provider id"))?;
            if positionals.len() > 1 {
                return Err(CliError::usage(
                    "code providers remove accepts one provider id",
                ));
            }
            let agent = require_agent(options.get(AGENT).copied(), PROVIDERS_USAGE)?;
            Ok(CodeCommand::ProvidersRemove {
                provider_id: provider_id.to_string(),
                agent,
                yes: flags.contains(&"--yes"),
            })
        }
        "switch" => {
            let agent = require_agent(rest.first().map(String::as_str), PROVIDERS_USAGE)?;
            let provider_id = rest
                .get(1)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| CliError::usage("code providers switch requires a provider id"))?
                .clone();
            if rest.len() > 2 {
                return Err(CliError::usage("code providers switch accepts no options"));
            }
            Ok(CodeCommand::ProvidersSwitch { agent, provider_id })
        }
        "switch-official" => {
            let agent = require_agent(rest.first().map(String::as_str), PROVIDERS_USAGE)?;
            if rest.len() > 1 {
                return Err(CliError::usage(
                    "code providers switch-official accepts no options",
                ));
            }
            Ok(CodeCommand::ProvidersSwitchOfficial { agent })
        }
        "export" => {
            let (options, _, _) = parse_flags(rest, &[AGENT, "--output"], &[], "providers export")?;
            let agent = require_agent(options.get(AGENT).copied(), PROVIDERS_USAGE)?;
            Ok(CodeCommand::ProvidersExport {
                agent,
                output: option(&options, "--output").map(PathBuf::from),
            })
        }
        "import" => {
            let (positionals, options, _, _) =
                split_positional_flags(rest, &[AGENT], &[], "providers import")?;
            let path = positionals
                .first()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| CliError::usage("code providers import requires a file path"))?;
            if positionals.len() > 1 {
                return Err(CliError::usage(
                    "code providers import accepts one file path",
                ));
            }
            let agent = require_agent(options.get(AGENT).copied(), PROVIDERS_USAGE)?;
            Ok(CodeCommand::ProvidersImport {
                agent,
                path: PathBuf::from(path),
            })
        }
        "probe" => {
            let (positionals, options, _, _) =
                split_positional_flags(rest, &[AGENT], &[], "providers probe")?;
            let provider_id = positionals
                .first()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| CliError::usage("code providers probe requires a provider id"))?;
            if positionals.len() > 1 {
                return Err(CliError::usage(
                    "code providers probe accepts one provider id",
                ));
            }
            let agent = require_agent(options.get(AGENT).copied(), PROVIDERS_USAGE)?;
            Ok(CodeCommand::ProvidersProbe {
                provider_id: provider_id.to_string(),
                agent,
            })
        }
        _ => Err(CliError::usage(PROVIDERS_USAGE)),
    }
}

fn parse_sessions(rest: &[String]) -> Result<CodeCommand, CliError> {
    let action = rest
        .first()
        .ok_or_else(|| CliError::usage(SESSIONS_USAGE))?;
    match action.as_str() {
        "list" => {
            if rest.len() > 1 {
                return Err(CliError::usage("code sessions list accepts no options"));
            }
            Ok(CodeCommand::SessionsList)
        }
        "info" | "timeline" => {
            let id = require_session_id(rest.get(1))?;
            if rest.len() > 2 {
                return Err(CliError::usage(format!(
                    "code sessions {action} accepts no options"
                )));
            }
            if action == "info" {
                Ok(CodeCommand::SessionsInfo { id })
            } else {
                Ok(CodeCommand::SessionsTimeline { id })
            }
        }
        _ => Err(CliError::usage(SESSIONS_USAGE)),
    }
}

fn parse_workspace(rest: &[String]) -> Result<CodeCommand, CliError> {
    let action = rest
        .first()
        .ok_or_else(|| CliError::usage(WORKSPACE_USAGE))?;
    let action = action.as_str();
    let positional = |index: usize, what: &str| -> Result<String, CliError> {
        rest.get(index)
            .filter(|value| !value.is_empty() && !value.starts_with("--"))
            .cloned()
            .ok_or_else(|| CliError::usage(format!("code workspace {action} requires {what}")))
    };
    match action {
        "list" => {
            let session = positional(1, "a session id")?;
            if rest.get(2).is_some_and(|value| value.starts_with("--")) {
                return Err(CliError::usage("code workspace list accepts no options"));
            }
            if rest.len() > 3 {
                return Err(CliError::usage("code workspace list accepts no options"));
            }
            Ok(CodeCommand::WorkspaceList {
                session,
                path: rest.get(2).cloned(),
            })
        }
        "search" => {
            let session = positional(1, "a session id")?;
            let query = positional(2, "a search query")?;
            if rest.len() > 3 {
                return Err(CliError::usage("code workspace search accepts no options"));
            }
            Ok(CodeCommand::WorkspaceSearch { session, query })
        }
        "preview" => {
            let session = positional(1, "a session id")?;
            let file = positional(2, "a file path")?;
            if rest.len() > 3 {
                return Err(CliError::usage("code workspace preview accepts no options"));
            }
            Ok(CodeCommand::WorkspacePreview { session, file })
        }
        "changes" => {
            let session = positional(1, "a session id")?;
            if rest.len() > 2 {
                return Err(CliError::usage("code workspace changes accepts no options"));
            }
            Ok(CodeCommand::WorkspaceChanges { session })
        }
        "diff" => {
            let session = positional(1, "a session id")?;
            if rest.get(2).is_some_and(|value| value.starts_with("--")) {
                return Err(CliError::usage("code workspace diff accepts no options"));
            }
            if rest.len() > 3 {
                return Err(CliError::usage("code workspace diff accepts no options"));
            }
            Ok(CodeCommand::WorkspaceDiff {
                session,
                file: rest.get(2).cloned(),
            })
        }
        "branches" => {
            let session = positional(1, "a session id")?;
            if rest.len() > 2 {
                return Err(CliError::usage(
                    "code workspace branches accepts no options",
                ));
            }
            Ok(CodeCommand::WorkspaceBranches { session })
        }
        "checkout" => {
            let session = positional(1, "a session id")?;
            let branch = positional(2, "a branch name")?;
            let (options, _, _) = parse_flags(
                &rest[3..],
                &["--mode", "--message"],
                &[],
                "workspace checkout",
            )?;
            let mode = match option(&options, "--mode") {
                Some("carry") => BranchSwitchMode::Carry,
                Some("stash") => BranchSwitchMode::Stash,
                Some("commit") => BranchSwitchMode::Commit,
                _ => {
                    return Err(CliError::usage(
                        "code workspace checkout requires --mode carry|stash|commit",
                    ));
                }
            };
            let message = option(&options, "--message").map(str::to_owned);
            if mode == BranchSwitchMode::Commit
                && message
                    .as_deref()
                    .is_none_or(|message| message.trim().is_empty())
            {
                return Err(CliError::usage(
                    "code workspace checkout --mode commit requires --message",
                ));
            }
            Ok(CodeCommand::WorkspaceCheckout {
                session,
                branch,
                mode,
                message,
            })
        }
        _ => Err(CliError::usage(WORKSPACE_USAGE)),
    }
}

fn parse_checkpoints(rest: &[String]) -> Result<CodeCommand, CliError> {
    let action = rest
        .first()
        .ok_or_else(|| CliError::usage(CHECKPOINTS_USAGE))?;
    match action.as_str() {
        "list" => {
            let session = require_session_id(rest.get(1))?;
            if rest.len() > 2 {
                return Err(CliError::usage("code checkpoints list accepts no options"));
            }
            Ok(CodeCommand::CheckpointsList { session })
        }
        "diff" => {
            let session = require_session_id(rest.get(1))?;
            let checkpoint_id = rest
                .get(2)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| CliError::usage("code checkpoints diff requires a checkpoint id"))?
                .clone();
            if rest.len() > 3 {
                return Err(CliError::usage("code checkpoints diff accepts no options"));
            }
            Ok(CodeCommand::CheckpointsDiff {
                session,
                checkpoint_id,
            })
        }
        "rewind" => {
            let session = require_session_id(rest.get(1))?;
            let turn = rest
                .get(2)
                .and_then(|value| value.parse::<u32>().ok())
                .ok_or_else(|| {
                    CliError::usage("code checkpoints rewind requires a turn number (0..)")
                })?;
            let (_, flags, _) = parse_flags(&rest[3..], &[], &["--yes"], "checkpoints rewind")?;
            Ok(CodeCommand::CheckpointsRewind {
                session,
                turn,
                yes: flags.contains(&"--yes"),
            })
        }
        "undo" => {
            let session = require_session_id(rest.get(1))?;
            if rest.len() > 2 {
                return Err(CliError::usage("code checkpoints undo accepts no options"));
            }
            Ok(CodeCommand::CheckpointsUndo { session })
        }
        _ => Err(CliError::usage(CHECKPOINTS_USAGE)),
    }
}

fn parse_run(rest: &[String]) -> Result<CodeCommand, CliError> {
    let agent = require_agent(rest.first().map(String::as_str), RUN_USAGE)?;
    let (options, _, _) = parse_flags(
        &rest[1..],
        &["--workspace", "--prompt-file", "--prompt", "--timeout-secs"],
        &[],
        "run",
    )?;
    if options.contains_key("--prompt-file") && options.contains_key("--prompt") {
        return Err(CliError::usage("use only one of --prompt-file or --prompt"));
    }
    let workspace = option(&options, "--workspace")
        .map(PathBuf::from)
        .ok_or_else(|| CliError::usage("code run requires --workspace DIR"))?;
    let prompt_file = option(&options, "--prompt-file").map(PathBuf::from);
    let prompt = option(&options, "--prompt").map(str::to_owned);
    if prompt_file.is_none() && prompt.is_none() {
        return Err(CliError::usage(
            "code run requires --prompt-file F or --prompt S",
        ));
    }
    let timeout_secs =
        match option(&options, "--timeout-secs") {
            Some(value) => Some(value.parse::<u64>().map_err(|_| {
                CliError::usage("code run --timeout-secs must be a positive integer")
            })?),
            None => None,
        };
    Ok(CodeCommand::Run {
        agent,
        workspace,
        prompt_file,
        prompt,
        timeout_secs,
    })
}

// ── parse helpers (same conventions as the other families) ──────────────────

type Flags<'a> = HashMap<&'a str, &'a str>;

/// Mirrors the pair-based flag parsing of the other families: every token is a
/// known value flag (followed by a non-empty value), a known boolean flag, or
/// nothing. `--model-slot` and `--mode`-style repeats accumulate into a list.
fn parse_flags<'a>(
    values: &'a [String],
    value_flags: &[&str],
    boolean_flags: &[&str],
    label: &str,
) -> Result<(Flags<'a>, Vec<&'a str>, Vec<(&'a str, &'a str)>), CliError> {
    let mut options = Flags::new();
    let mut flags = Vec::new();
    let mut repeated = Vec::new();
    let mut index = 0;
    while index < values.len() {
        let token = values[index].as_str();
        if boolean_flags.contains(&token) {
            if flags.contains(&token) {
                return Err(CliError::usage(format!(
                    "duplicate code {label} option {token}"
                )));
            }
            flags.push(token);
            index += 1;
            continue;
        }
        if !value_flags.contains(&token) {
            return Err(CliError::usage(format!(
                "unsupported code {label} option: {token}"
            )));
        }
        let value = values.get(index + 1).ok_or_else(|| {
            CliError::usage(format!("code {label} option {token} requires a value"))
        })?;
        if value.is_empty() || (value.starts_with("--") && token != "--message") {
            return Err(CliError::usage(format!(
                "code {label} option {token} requires a value"
            )));
        }
        if token == "--model-slot" {
            repeated.push((token, value.as_str()));
        } else {
            if options.insert(token, value.as_str()).is_some() {
                return Err(CliError::usage(format!(
                    "duplicate code {label} option {token}"
                )));
            }
        }
        index += 2;
    }
    Ok((options, flags, repeated))
}

fn option<'a>(options: &'a Flags, name: &str) -> Option<&'a str> {
    options.get(name).copied()
}

/// Flag splitter for subcommands whose positional argument may appear before
/// or after the flags (`providers import <PATH> --agent A`). Positional tokens
/// are collected in order; anything starting with `--` that is not a known
/// flag is a usage error.
fn split_positional_flags<'a>(
    values: &'a [String],
    value_flags: &[&str],
    boolean_flags: &[&str],
    label: &str,
) -> Result<
    (
        Vec<&'a str>,
        Flags<'a>,
        Vec<&'a str>,
        Vec<(&'a str, &'a str)>,
    ),
    CliError,
> {
    let mut positionals = Vec::new();
    let mut options = Flags::new();
    let mut flags = Vec::new();
    let mut repeated = Vec::new();
    let mut index = 0;
    while index < values.len() {
        let token = values[index].as_str();
        if boolean_flags.contains(&token) {
            if flags.contains(&token) {
                return Err(CliError::usage(format!(
                    "duplicate code {label} option {token}"
                )));
            }
            flags.push(token);
            index += 1;
            continue;
        }
        if value_flags.contains(&token) {
            let value = values.get(index + 1).ok_or_else(|| {
                CliError::usage(format!("code {label} option {token} requires a value"))
            })?;
            if value.is_empty() || (value.starts_with("--") && token != "--message") {
                return Err(CliError::usage(format!(
                    "code {label} option {token} requires a value"
                )));
            }
            if token == "--model-slot" {
                repeated.push((token, value.as_str()));
            } else if options.insert(token, value.as_str()).is_some() {
                return Err(CliError::usage(format!(
                    "duplicate code {label} option {token}"
                )));
            }
            index += 2;
            continue;
        }
        if token.starts_with("--") {
            return Err(CliError::usage(format!(
                "unsupported code {label} option: {token}"
            )));
        }
        positionals.push(token);
        index += 1;
    }
    Ok((positionals, options, flags, repeated))
}

fn require_agent(value: Option<&str>, usage: &str) -> Result<String, CliError> {
    let agent = value
        .filter(|agent| !agent.is_empty())
        .ok_or_else(|| CliError::usage(usage))?;
    match agent {
        "codex" | "claude" | "kimi" => Ok(agent.to_owned()),
        other => Err(CliError::usage(format!(
            "unknown agent '{other}'; valid agents are codex|claude|kimi"
        ))),
    }
}

fn require_session_id(value: Option<&String>) -> Result<String, CliError> {
    let id = value
        .map(String::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| CliError::usage("code command requires a session id"))?;
    if !valid_session_id(id) {
        return Err(CliError::usage("invalid session id"));
    }
    Ok(id.to_owned())
}

/// Mirrors `features::sessions::validate_session_id`: only `[A-Za-z0-9_-]`, so
/// the id can never traverse out of the sessions root when joined onto a path.
fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Converts repeated `--model-slot SLOT=MODEL` pairs collected by
/// `parse_flags` into the map form `ProviderManager::save` expects.
fn parse_model_slot_pairs(
    repeated: &[(&str, &str)],
    label: &str,
) -> Result<Vec<(String, String)>, CliError> {
    repeated
        .iter()
        .map(|(_, value)| {
            let (slot, model) = value.split_once('=').ok_or_else(|| {
                CliError::usage(format!(
                    "code {label} --model-slot must be SLOT=MODEL (for example \
                     --model-slot sonnet=claude-sonnet-4)"
                ))
            })?;
            let slot = slot.trim();
            let model = model.trim();
            if slot.is_empty() || model.is_empty() {
                return Err(CliError::usage(format!(
                    "code {label} --model-slot must be SLOT=MODEL"
                )));
            }
            Ok((slot.to_owned(), model.to_owned()))
        })
        .collect()
}

fn parse_context_window(options: &Flags, label: &str) -> Result<Option<i64>, CliError> {
    match option(options, "--context-window") {
        None => Ok(None),
        Some(value) => value
            .parse::<i64>()
            .ok()
            .filter(|window| *window > 0)
            .map(Some)
            .ok_or_else(|| {
                CliError::usage(format!(
                    "code {label} --context-window must be a positive integer"
                ))
            }),
    }
}

// ── execute ─────────────────────────────────────────────────────────────────

pub fn execute(command: CodeCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    match command {
        CodeCommand::AgentsList => agents_list(output),
        CodeCommand::AgentsStatus { agent } => agents_status(&agent, output),
        CodeCommand::AgentsInstall { agent } => Err(CliError::failed(format!(
            "code_install_requires_product_host: installing '{agent}' downloads and runs the \
             vendor install script under GUI host supervision (progress events, cancel, PATH \
             repair); install the CLI manually or use the desktop app"
        ))),
        CodeCommand::Login { agent, code } => login(&agent, code, output),
        CodeCommand::Logout { agent } => logout(&agent, output),
        CodeCommand::ProvidersList { agent } => providers_list(agent.as_deref(), output),
        CodeCommand::ProvidersAdd {
            agent,
            name,
            base_url,
            wire_api,
            model,
            model_slots,
            context_window,
            api_key_env,
            api_key_stdin,
        } => providers_save(
            &agent,
            None,
            Some(name),
            Some(base_url),
            wire_api.as_deref(),
            model,
            &model_slots,
            context_window,
            api_key_env,
            api_key_stdin,
            false,
            output,
        ),
        CodeCommand::ProvidersUpdate {
            provider_id,
            agent,
            name,
            base_url,
            wire_api,
            model,
            model_slots,
            context_window,
            api_key_env,
            api_key_stdin,
            delete_key,
        } => providers_save(
            &agent,
            Some(&provider_id),
            name,
            base_url,
            wire_api.as_deref(),
            model,
            &model_slots,
            context_window,
            api_key_env,
            api_key_stdin,
            delete_key,
            output,
        ),
        CodeCommand::ProvidersRemove {
            provider_id,
            agent,
            yes,
        } => providers_remove(&agent, &provider_id, yes, output),
        CodeCommand::ProvidersSwitch { agent, provider_id } => {
            providers_switch(&agent, &provider_id, output)
        }
        CodeCommand::ProvidersSwitchOfficial { agent } => providers_switch_official(&agent, output),
        CodeCommand::ProvidersExport {
            agent,
            output: destination,
        } => providers_export(&agent, destination, output),
        CodeCommand::ProvidersImport { agent, path } => providers_import(&agent, &path, output),
        CodeCommand::ProvidersProbe { provider_id, agent } => {
            let _ = (provider_id, agent);
            Err(CliError::failed(
                "code_probe_requires_product_host: the model probe spawns a disposable ACP \
                 session through the product host (adapter process + async protocol client); \
                 not available headless",
            ))
        }
        CodeCommand::SessionsList => code_sessions_list(output),
        CodeCommand::SessionsInfo { id } => code_sessions_info(&id, output),
        CodeCommand::SessionsTimeline { id } => code_sessions_timeline(&id, output),
        CodeCommand::WorkspaceList { session, path } => with_workspace(&session, |root| {
            workspace_list(root, path.as_deref(), output)
        }),
        CodeCommand::WorkspaceSearch { session, query } => {
            with_workspace(&session, |root| workspace_search(root, &query, output))
        }
        CodeCommand::WorkspacePreview { session, file } => {
            with_workspace(&session, |root| workspace_preview(root, &file, output))
        }
        CodeCommand::WorkspaceChanges { session } => {
            with_workspace_session(&session, |session_id, root| {
                workspace_changes(&session_id, root, output)
            })
        }
        CodeCommand::WorkspaceDiff { session, file } => {
            with_workspace_session(&session, |session_id, root| {
                workspace_diff(&session_id, root, file.as_deref(), output)
            })
        }
        CodeCommand::WorkspaceBranches { session } => {
            with_workspace(&session, |root| workspace_branches(root, output))
        }
        CodeCommand::WorkspaceCheckout {
            session,
            branch,
            mode,
            message,
        } => {
            let mut mutation_lock = session_mutation_lock(&session)?;
            let _mutation_guard =
                lock_session_for_mutation(&mut mutation_lock, &session, "checkout")?;
            with_workspace(&session, |root| {
                workspace_checkout(&session, root, &branch, mode, message.as_deref(), output)
            })
        }
        CodeCommand::CheckpointsList { session } => checkpoints_list(&session, output),
        CodeCommand::CheckpointsDiff {
            session,
            checkpoint_id,
        } => checkpoints_diff(&session, &checkpoint_id, output),
        CodeCommand::CheckpointsRewind { session, turn, yes } => {
            checkpoints_rewind(&session, turn, yes, output)
        }
        CodeCommand::CheckpointsUndo { session } => checkpoints_undo(&session, output),
        CodeCommand::Run { .. } => Err(CliError::failed(
            "code_run_requires_product_host: a one-shot ACP turn needs the product host's \
             adapter process and async protocol client (AcpPool + agent-client-protocol), \
             which are bound to the GUI session; use the desktop app to run turns and \
             `pinvou code sessions timeline` to read the persisted events",
        )),
        CodeCommand::Permissions { session } => permissions(&session, output),
        CodeCommand::Respond {
            session,
            request_id,
            allow,
        } => respond(&session, &request_id, allow, output),
    }
}

fn open_store() -> Result<SessionStore, CliError> {
    let store = SessionStore::boot()
        .map_err(|error| CliError::failed(format!("sessions store unavailable: {error:#}")))?;
    // Same two-root wiring as the app startup (pinvou3-app/src-tauri/src/lib.rs):
    // project-bound native code sessions resolve their execution root through
    // the persisted SessionAgentStore; without this every root would fall back
    // to the session's private directory.
    let agents = open_agent_store()?;
    store.set_execution_root_resolver(std::sync::Arc::new(move |session_id: &str| {
        agents.code_project_workspace(session_id)
    }));
    Ok(store)
}

fn open_agent_store() -> Result<SessionAgentStore, CliError> {
    Ok(SessionAgentStore::load_or_empty())
}

fn store_error(action: &str, id: &str, error: impl std::fmt::Display) -> CliError {
    CliError::failed(format!("code {action}({id}): {error:#}"))
}

fn require_existing(store: &SessionStore, id: &str, action: &str) -> Result<(), CliError> {
    store
        .load(id)
        .map(|_| ())
        .map_err(|error| store_error(action, id, error))
}

fn open_providers() -> Result<ProviderManager, CliError> {
    ProviderManager::new(SystemCredentialStore::new())
        .map_err(|error| CliError::failed(format!("providers unavailable: {error:#}")))
}

// ── agents ──────────────────────────────────────────────────────────────────

/// One agent probed the same way the GUI status probes do: PATH lookup +
/// `--version` + version gate + login-state probe (`features::codex_acp`
/// mirrors; those helpers are pub(super), so this is a local reimplementation).
struct AgentProbe {
    agent_id: String,
    agent_name: String,
    cli_path: Option<PathBuf>,
    version: Option<String>,
    version_supported: bool,
    min_version: &'static str,
    authenticated: bool,
}

fn agent_cli_name(agent: &str) -> &'static str {
    match agent {
        "claude" => "claude",
        "kimi" => "kimi",
        _ => "codex",
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let candidates = crate::support::binary_candidates(name);
    std::env::split_paths(&path)
        .flat_map(|dir| candidates.iter().map(move |candidate| dir.join(candidate)))
        .find(|candidate| candidate.is_file())
}

/// Mirror of the app's per-agent resolution order (`install::resolve_*`):
/// explicit override env vars and official install locations win over PATH so
/// a stale binary earlier in PATH cannot shadow the real one (the app prefers
/// `~/.kimi-code/bin/kimi` over PATH for exactly that reason). The app's
/// adapter-beside claude runtime location is app-bundle-specific and not
/// mirrored here.
fn resolve_agent_cli(agent: &str, name: &str) -> Option<PathBuf> {
    let override_var = match agent {
        "codex" => Some("PINVOU3_CODEX_PATH"),
        "claude" => Some("PINVOU3_CLAUDE_CLI_PATH"),
        _ => None,
    };
    if let Some(var) = override_var {
        if let Some(path) = std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty() && path.is_file())
        {
            return Some(path);
        }
    }
    let home = pinvou3_lib::platform::paths::user_home_dir();
    let managed_dir = match agent {
        "kimi" => Some(home.join(".kimi-code").join("bin")),
        "codex" | "claude" => Some(home.join(".local").join("bin")),
        _ => None,
    };
    if let Some(dir) = managed_dir {
        let candidates = crate::support::binary_candidates(name);
        if let Some(path) = candidates
            .iter()
            .map(|candidate| dir.join(candidate))
            .find(|candidate| candidate.is_file())
        {
            return Some(path);
        }
    }
    find_in_path(name)
}

/// Runs `executable args...` with a hard timeout; returns (success, stdout)
/// when the process exits within the budget, `None` on timeout/spawn failure.
fn command_output_with_timeout(
    executable: &Path,
    args: &[&str],
    timeout: Duration,
) -> Option<(bool, String)> {
    let mut command = crate::support::build_command(executable, args);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
    ] {
        command.env_remove(variable);
    }
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || drain_stream(stdout, false));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(_) => return None,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let text = reader.join().unwrap_or_default();
    Some((status.success(), text.trim().to_string()))
}

/// Drains a byte stream into a string, optionally echoing it to our stdout
/// (used by the interactive login flow so the user sees the CLI's prompts).
fn drain_stream<R: Read>(mut pipe: Option<R>, echo: bool) -> String {
    let mut buffer = String::new();
    let Some(pipe) = pipe.as_mut() else {
        return buffer;
    };
    let mut chunk = [0u8; 2048];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let text = String::from_utf8_lossy(&chunk[..read]).into_owned();
                if echo {
                    print!("{text}");
                }
                buffer.push_str(&text);
                if buffer.len() > 65_536 {
                    let overflow = buffer.len() - 65_536;
                    buffer.drain(..overflow);
                }
            }
        }
    }
    buffer
}

fn cli_version(executable: &Path) -> Option<String> {
    command_output_with_timeout(executable, &["--version"], Duration::from_secs(15))
        .filter(|(ok, _)| *ok)
        .map(|(_, text)| text)
        // The app treats empty output as a failed probe, not as a version.
        .filter(|text| !text.trim().is_empty())
}

/// Mirror of `runtime::parse_codex_version_output`: codex prints a
/// package-prefixed line ("codex-cli 0.146.0"), so the version is the first
/// whitespace token whose dot/dash/plus-separated head is all digits.
fn codex_version_token(version: &str) -> &str {
    version
        .split_whitespace()
        .find(|token| {
            token
                .split(['.', '-', '+'])
                .next()
                .is_some_and(|head| !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()))
        })
        .unwrap_or(version)
}

/// Mirror of `runtime::parse_version`: only a leading digit-run of
/// dot/dash/plus-separated parts counts.
fn parse_version(version: &str) -> Vec<u64> {
    version
        .split(['.', '-', '+'])
        .take_while(|part| part.chars().all(|character| character.is_ascii_digit()))
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

fn version_at_least(version: &str, minimum: &str) -> bool {
    parse_version(version).cmp(&parse_version(minimum)) != std::cmp::Ordering::Less
}

/// Mirror of `install::is_bare_semver`: exactly three all-digit parts.
fn is_bare_semver(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// Mirror of `install::claude_version_supported`: the first whitespace token
/// of `--version` output ("2.1.163 (Claude Code)") must be a bare semver.
fn claude_version_supported(version: &str, minimum: &str) -> bool {
    version
        .split_whitespace()
        .next()
        .is_some_and(|token| is_bare_semver(token) && version_at_least(token, minimum))
}

/// Mirror of `install::kimi_version_supported`: the whole output must be a
/// bare semver (non-standard kimi-cli output is always unsupported).
fn kimi_version_supported(version: &str, minimum: &str) -> bool {
    is_bare_semver(version) && version_at_least(version, minimum)
}

fn cli_status_success(executable: &Path, args: &[&str]) -> bool {
    command_output_with_timeout(executable, args, Duration::from_secs(15))
        .map(|(ok, _)| ok)
        .unwrap_or(false)
}

fn nonempty_env(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

/// Mirrors `codex_authenticated`: env credentials, the active relay
/// model_provider with an env_key in `~/.codex/config.toml`, or
/// `codex login status` success.
fn codex_authenticated(executable: &Path) -> bool {
    if [
        "OPENAI_API_KEY",
        "OPENAI_CODEX_ACCESS_TOKEN",
        "CODEX_ACCESS_TOKEN",
    ]
    .into_iter()
    .any(nonempty_env)
    {
        return true;
    }
    let home = pinvou3_lib::platform::paths::user_home_dir();
    if let Ok(raw) = std::fs::read_to_string(home.join(".codex").join("config.toml")) {
        // Mirror of `providers::codex_config_relay_env_key_present`: the relay
        // provider counts only while it is the active `model_provider` and its
        // `env_key` is non-empty — a relay provider that was configured but
        // switched away from does not make codex authenticated.
        let active = toml::from_str::<toml::Value>(&raw)
            .ok()
            .and_then(|config| {
                let provider = config
                    .get("model_provider")
                    .and_then(toml::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())?
                    .to_owned();
                let env_key_present = config
                    .get("model_providers")
                    .and_then(|providers| providers.get(&provider))
                    .and_then(|provider| provider.get("env_key"))
                    .and_then(toml::Value::as_str)
                    .map(str::trim)
                    .is_some_and(|value| !value.is_empty());
                Some(env_key_present)
            })
            .unwrap_or(false);
        if active {
            return true;
        }
    }
    cli_status_success(executable, &["login", "status"])
}

/// Mirrors `claude_authenticated`: env credentials or `claude auth status`.
fn claude_authenticated(executable: &Path) -> bool {
    if [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ]
    .into_iter()
    .any(nonempty_env)
    {
        return true;
    }
    cli_status_success(executable, &["auth", "status"])
}

/// Mirrors `introspect::kimi_authenticated`: paired KIMI_MODEL_* overrides,
/// or a valid OAuth credential file plus a fully resolvable default model
/// (model → provider → type → api_key/env/oauth chain) in the kimi config.
fn kimi_authenticated() -> bool {
    if nonempty_env("KIMI_MODEL_NAME") && nonempty_env("KIMI_MODEL_API_KEY") {
        return true;
    }
    let root = kimi_data_root();
    let oauth_credentials_valid =
        std::fs::read_to_string(root.join("credentials").join("kimi-code.json"))
            .is_ok_and(|raw| kimi_credentials_valid(&raw));
    let Ok(config) = std::fs::read_to_string(root.join("config.toml")) else {
        return false;
    };
    kimi_runtime_config_ready(&config, oauth_credentials_valid)
}

/// Mirror of `introspect::kimi_credentials_valid`: both tokens must be
/// non-empty strings and `expires_at` a positive timestamp. Expiry itself is
/// not disqualifying — the kimi CLI refreshes access tokens automatically —
/// it only identifies a corrupted credential file.
fn kimi_credentials_valid(raw: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    let token_present = ["access_token", "refresh_token"].into_iter().all(|key| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|token| !token.trim().is_empty())
    });
    let expiry_valid = value
        .get("expires_at")
        .and_then(serde_json::Value::as_i64)
        .is_some_and(|expiry| expiry > 0);
    token_present && expiry_valid
}

/// Faithful mirror of `introspect::kimi_runtime_config_ready`: a default
/// model that resolves through an existing provider entry to a usable
/// credential (direct api_key, a `*_API_KEY` env entry, or OAuth state).
fn kimi_runtime_config_ready(raw: &str, oauth_credentials_valid: bool) -> bool {
    let Ok(config) = toml::from_str::<toml::Value>(raw) else {
        return false;
    };
    let Some(default_model) = config
        .get("default_model")
        .and_then(toml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return false;
    };
    let Some(model) = config
        .get("models")
        .and_then(|models| models.get(default_model))
        .and_then(toml::Value::as_table)
    else {
        return false;
    };
    let Some(provider) = model
        .get("provider")
        .and_then(toml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return false;
    };
    let model_ready = model
        .get("model")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        && model
            .get("max_context_size")
            .and_then(toml::Value::as_integer)
            .is_some_and(|value| value > 0);
    if !model_ready {
        return false;
    }
    let Some(provider) = config
        .get("providers")
        .and_then(|providers| providers.get(provider))
        .and_then(toml::Value::as_table)
    else {
        return false;
    };
    if provider
        .get("type")
        .and_then(toml::Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
    {
        return false;
    }
    let direct_api_key = provider
        .get("api_key")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let configured_env_api_key = provider
        .get("env")
        .and_then(toml::Value::as_table)
        .is_some_and(|env| {
            env.iter().any(|(name, value)| {
                name.ends_with("_API_KEY")
                    && value.as_str().is_some_and(|value| !value.trim().is_empty())
            })
        });
    let oauth_ready =
        provider.get("oauth").is_some_and(toml::Value::is_table) && oauth_credentials_valid;
    direct_api_key || configured_env_api_key || oauth_ready
}

/// Mirror of `introspect::kimi_data_root`: KIMI_CODE_HOME when set, else
/// `~/.kimi-code` (through the app's `user_home_dir` so Windows USERPROFILE
/// roots resolve identically).
fn kimi_data_root() -> PathBuf {
    if let Some(root) = std::env::var_os("KIMI_CODE_HOME").map(PathBuf::from) {
        return root;
    }
    pinvou3_lib::platform::paths::user_home_dir().join(".kimi-code")
}

fn probe_agent(agent: &str, agent_name: &str) -> AgentProbe {
    let cli_name = agent_cli_name(agent);
    let cli_path = resolve_agent_cli(agent, cli_name);
    let version = cli_path.as_deref().and_then(cli_version);
    let min_version = MIN_VERSIONS
        .iter()
        .find(|(id, _)| *id == agent)
        .map(|(_, min)| *min)
        .unwrap_or("0.0.0");
    let version_supported = version
        .as_deref()
        .map(|version| match agent {
            "codex" => version_at_least(codex_version_token(version), min_version),
            "claude" => claude_version_supported(version, min_version),
            "kimi" => kimi_version_supported(version, min_version),
            _ => false,
        })
        .unwrap_or(false);
    let authenticated = match cli_path.as_deref() {
        Some(path) => match agent {
            "codex" => codex_authenticated(path),
            "claude" => claude_authenticated(path),
            "kimi" => kimi_authenticated(),
            _ => false,
        },
        None => false,
    };
    AgentProbe {
        agent_id: agent.to_owned(),
        agent_name: agent_name.to_owned(),
        cli_path,
        version,
        version_supported,
        min_version,
        authenticated,
    }
}

fn probe_all_agents() -> Vec<AgentProbe> {
    AcpPool::agent_catalog()
        .into_iter()
        .map(|descriptor| probe_agent(descriptor.agent_id, descriptor.agent_name))
        .collect()
}

fn probe_json(probe: &AgentProbe) -> serde_json::Value {
    serde_json::json!({
        "agent_id": probe.agent_id,
        "agent_name": probe.agent_name,
        "installed": probe.cli_path.is_some() && probe.version_supported,
        "cli_found": probe.cli_path.is_some(),
        "cli_path": probe.cli_path.as_ref().map(|path| path.display().to_string()),
        "version": probe.version,
        "version_supported": probe.version_supported,
        "min_version": probe.min_version,
        "authenticated": probe.authenticated,
    })
}

fn agents_list(output: OutputMode) -> Result<CliOutcome, CliError> {
    let probes = probe_all_agents();
    let human = probes
        .iter()
        .map(|probe| {
            format!(
                "{}\t{}\t{}\t{}\t{}",
                probe.agent_id,
                probe.agent_name,
                if probe.cli_path.is_some() && probe.version_supported {
                    "installed"
                } else if probe.cli_path.is_some() {
                    "version-too-old"
                } else {
                    "not-installed"
                },
                probe.version.as_deref().unwrap_or("-"),
                if probe.authenticated {
                    "authenticated"
                } else {
                    "-"
                },
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let value = serde_json::json!({ "agents": probes.iter().map(probe_json).collect::<Vec<_>>() });
    Ok(success(render(output, human, &value)))
}

fn agents_status(agent: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let descriptor = AcpPool::agent_catalog()
        .into_iter()
        .find(|descriptor| descriptor.agent_id == agent);
    let agent_name = descriptor
        .map(|descriptor| descriptor.agent_name)
        .unwrap_or(agent);
    let probe = probe_agent(agent, agent_name);
    let value = probe_json(&probe);
    let human = format!(
        "agent: {}\nname: {}\ninstalled: {}\nversion: {}\nmin_version: {}\nauthenticated: {}\npath: {}",
        probe.agent_id,
        probe.agent_name,
        if probe.cli_path.is_some() && probe.version_supported {
            "yes"
        } else {
            "no"
        },
        probe.version.as_deref().unwrap_or("-"),
        probe.min_version,
        if probe.authenticated { "yes" } else { "no" },
        probe
            .cli_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_owned()),
    );
    Ok(success(render(output, human, &value)))
}

// ── login / logout ──────────────────────────────────────────────────────────

fn login_url_allowed(agent: &str, url: &str) -> bool {
    match agent {
        "codex" => {
            url.starts_with("https://auth.openai.com/")
                || url.starts_with("https://platform.openai.com/")
        }
        "claude" => {
            url.starts_with("https://claude.com/")
                || url.starts_with("https://claude.ai/")
                || url.starts_with("https://platform.claude.com/")
        }
        "kimi" => url.starts_with("https://www.kimi.com/") || url.starts_with("https://kimi.com/"),
        _ => false,
    }
}

/// Mirrors `login::extract_agent_login_url`: the last `https://` URL whose
/// host is on the agent's allow-list.
fn extract_login_url(agent: &str, output: &str) -> Option<String> {
    output
        .match_indices("https://")
        .filter_map(|(start, _)| {
            let tail = &output[start..];
            let end = tail
                .char_indices()
                .find_map(|(index, character)| {
                    (character.is_whitespace()
                        || character.is_control()
                        || matches!(character, '"' | '\'' | '<' | '>'))
                    .then_some(index)
                })
                .unwrap_or(tail.len());
            let candidate = tail[..end].trim_end_matches(['.', ',', ')', ']']);
            login_url_allowed(agent, candidate).then(|| candidate.to_string())
        })
        .last()
}

/// Mirrors `login::extract_device_code`: `user_code=` in the URL, or the
/// "enter code:"/"user code:" prompt, validated like `valid_device_code`.
fn extract_device_code(output: &str, login_url: Option<&str>) -> Option<String> {
    let valid = |code: &str| {
        (4..=32).contains(&code.len())
            && code
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
    };
    if let Some(url) = login_url {
        if let Some(value) = url.split("user_code=").nth(1) {
            let code = value
                .split(|character: char| character == '&' || character.is_whitespace())
                .next()
                .unwrap_or_default();
            if valid(code) {
                return Some(code.to_string());
            }
        }
    }
    ["enter code:", "user code:"]
        .into_iter()
        .find_map(|marker| {
            let start = output.to_ascii_lowercase().rfind(marker)? + marker.len();
            let code = output[start..].split_whitespace().next()?;
            valid(code).then(|| code.to_string())
        })
}

fn login_executable(agent: &str) -> Result<PathBuf, CliError> {
    resolve_agent_cli(agent, agent_cli_name(agent)).ok_or_else(|| {
        CliError::failed(format!(
            "code_login_cli_missing: {agent} CLI not found; install it first \
             (`pinvou code agents status {agent}` shows the probe result)"
        ))
    })
}

fn login_args(agent: &str) -> &'static [&'static str] {
    match agent {
        "claude" => &["auth", "login"],
        "kimi" => &["login"],
        _ => &["login"],
    }
}

/// `code login <agent>`: spawns the same login command the GUI's
/// `login_acp_agent` runs, streams its output through, extracts the
/// allow-listed authorization URL / device code, and waits for the flow to
/// finish (bounded like the GUI: 600s, kimi 1800s). The claude authorization
/// code is accepted via `--code-env VAR` / `--code-stdin` (plaintext argv is
/// deliberately not offered — argv leaks through shell history and process
/// listings; `--code C` remains for callers that already hold it in argv) and
/// is written to the child's stdin; the child reads it when it prompts.
fn login(
    agent: &str,
    code: Option<LoginCodeSource>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let code = match code {
        Some(LoginCodeSource::Arg(raw)) => Some(raw),
        Some(LoginCodeSource::Env(var)) => Some(std::env::var(&var).map_err(|_| {
            CliError::failed(format!(
                "code login: authorization code environment variable {var} is not set"
            ))
        })?),
        Some(LoginCodeSource::Stdin) => {
            let mut raw = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw).map_err(|error| {
                CliError::failed(format!("code login: cannot read code from stdin: {error}"))
            })?;
            Some(raw)
        }
        None => None,
    };
    if let Some(code) = code.as_deref() {
        if agent != "claude" {
            return Err(CliError::usage(format!(
                "code login {agent} does not accept an authorization code; only the claude \
                 login flow consumes one"
            )));
        }
        let trimmed = code.trim();
        if trimmed.is_empty() || trimmed.len() > 4096 || trimmed.chars().any(char::is_control) {
            return Err(CliError::usage("invalid claude authorization code"));
        }
    }
    let executable = login_executable(agent)?;
    let mut command = crate::support::build_command(&executable, login_args(agent));
    crate::support::set_process_group(&mut command);
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        CliError::failed(format!("code login({agent}): cannot spawn CLI: {error}"))
    })?;
    {
        use std::io::Write;
        let mut stdin = child.stdin.take();
        if let (Some(code), Some(stdin)) = (code.as_deref(), stdin.as_mut()) {
            let _ = writeln!(stdin, "{}", code.trim());
            let _ = stdin.flush();
        }
        // Close stdin for non-code flows so CLI login prompts on the terminal
        // fail fast instead of blocking on a pipe that never fills.
    }
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_reader = std::thread::spawn(move || drain_stream(stdout, false));
    let err_reader = std::thread::spawn(move || drain_stream(stderr, false));
    let deadline = Duration::from_secs(if agent == "kimi" { 1800 } else { 600 });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(error) => {
                return Err(CliError::failed(format!("code login({agent}): {error}")));
            }
        }
        if started.elapsed() > deadline {
            crate::support::kill_process_tree(&mut child);
            return Err(CliError::failed(
                "code_login_timeout: authorization wait timed out; rerun `pinvou code login`",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let out_text = out_reader.join().unwrap_or_default();
    let err_text = err_reader.join().unwrap_or_default();
    let status = status.expect("loop only breaks with a status or returns");
    let combined = format!("{out_text}\n{err_text}");
    // The vendor login output is echoed once, redacted: live streaming would
    // bypass redaction, and login transcripts are exactly what users paste
    // into issues.
    let echoed = pinvou3_lib::platform::credential_store::redact_secret(&combined);
    if !echoed.trim().is_empty() {
        println!("{echoed}");
    }
    let login_url = extract_login_url(agent, &combined);
    let device_code = extract_device_code(&combined, login_url.as_deref());
    let state = if status.success() {
        "completed"
    } else {
        "failed"
    };
    let value = serde_json::json!({
        "agent": agent,
        "status": state,
        "exit_code": status.code(),
        "login_url": login_url,
        "device_code": device_code,
    });
    let human = format!(
        "login {agent}: {state}\nlogin_url: {}\ndevice_code: {}",
        login_url.as_deref().unwrap_or("-"),
        device_code.as_deref().unwrap_or("-"),
    );
    if status.success() {
        // Mirror the GUI's post-login re-check: a successful process exit does
        // not guarantee a usable credential (kimi can complete OAuth yet fail
        // to write the model config; codex/claude probes re-run here).
        let authenticated = match agent {
            "codex" => codex_authenticated(&executable),
            "claude" => claude_authenticated(&executable),
            "kimi" => kimi_authenticated(),
            _ => false,
        };
        if !authenticated {
            return Err(CliError::failed(format!(
                "code_login_not_authenticated: {agent} login process completed but the \
                 authentication probe does not report a usable credential yet; check \
                 `pinvou code agents status {agent}`"
            )));
        }
        Ok(success(render(output, human, &value)))
    } else {
        Err(CliError::failed(format!(
            "code_login_failed: {agent} login process exited with {}",
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_owned())
        )))
    }
}

/// `code logout <agent>`: runs the same non-interactive logout subcommand the
/// GUI's `logout_acp_agent` uses (`codex logout` / `claude auth logout` /
/// `kimi provider remove managed:kimi-code`).
fn logout(agent: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let args: &[&str] = match agent {
        "codex" => &["logout"],
        "claude" => &["auth", "logout"],
        "kimi" => &["provider", "remove", "managed:kimi-code"],
        _ => {
            return Err(CliError::usage(format!(
                "unknown agent '{agent}'; valid agents are codex|claude|kimi"
            )));
        }
    };
    let executable = login_executable(agent)?;
    // Bounded like the login flow (logout is a fast subcommand; a hung
    // vendor CLI must not block the terminal forever).
    let mut command = crate::support::build_command(&executable, args);
    crate::support::set_process_group(&mut command);
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| CliError::failed(format!("code logout({agent}): {error}")))?;
    let status = match child
        .wait_timeout(Duration::from_secs(LOGOUT_TIMEOUT_SECS))
        .map_err(|error| CliError::failed(format!("code logout({agent}): {error}")))?
    {
        Some(status) => status,
        None => {
            crate::support::kill_process_tree(&mut child);
            return Err(CliError::failed(format!(
                "code_logout_failed: {agent} logout did not finish within \
                 {LOGOUT_TIMEOUT_SECS}s (killed)"
            )));
        }
    };
    if !status.success() {
        return Err(CliError::failed(format!(
            "code_logout_failed: {agent} logout command exited with {}",
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_owned())
        )));
    }
    let value = serde_json::json!({ "agent": agent, "action": "logged_out" });
    Ok(success(render(
        output,
        format!("logged out {agent}"),
        &value,
    )))
}

// ── providers ───────────────────────────────────────────────────────────────

fn require_provider_agent(agent: &str) -> Result<(), CliError> {
    match agent {
        "codex" | "claude" | "kimi" => Ok(()),
        other => Err(CliError::usage(format!(
            "unknown agent '{other}'; valid agents are codex|claude|kimi"
        ))),
    }
}

fn providers_list(agent: Option<&str>, output: OutputMode) -> Result<CliOutcome, CliError> {
    let manager = open_providers()?;
    let agents: Vec<String> = match agent {
        Some(agent) => {
            require_provider_agent(agent)?;
            vec![agent.to_owned()]
        }
        None => vec!["codex".to_owned(), "claude".to_owned(), "kimi".to_owned()],
    };
    let mut views = Vec::new();
    for agent in &agents {
        let view = manager
            .list(agent)
            .map_err(|error| store_error("providers list", agent, error))?;
        views.push((agent.clone(), view));
    }
    if views.len() == 1 {
        let (agent, view) = &views[0];
        let value = serde_json::json!({ "agent": agent, "providers": view });
        let human = render_provider_view(agent, &view);
        Ok(success(render(output, human, &value)))
    } else {
        let value = serde_json::json!({
            "agents": views
                .iter()
                .map(|(agent, view)| serde_json::json!({ "agent": agent, "providers": view }))
                .collect::<Vec<_>>(),
        });
        let human = views
            .iter()
            .map(|(agent, view)| render_provider_view(agent, view))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(success(render(output, human, &value)))
    }
}

fn render_provider_view(agent: &str, view: &AcpProvidersView) -> String {
    let mut lines = vec![format!(
        "agent: {}\tofficial: {}\tcurrent: {}\texternal: {}",
        agent,
        view.official_active,
        view.current_provider_id.as_deref().unwrap_or("-"),
        view.external_active,
    )];
    for provider in &view.providers {
        lines.push(format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            provider.id,
            provider.name,
            provider.base_url,
            serde_json::to_value(&provider.wire_api)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default(),
            provider.model.as_deref().unwrap_or("-"),
            if provider.has_credential {
                "key-set"
            } else {
                "no-key"
            },
            provider
                .context_window
                .map(|window| window.to_string())
                .unwrap_or_else(|| "-".to_owned()),
        ));
    }
    lines.join("\n")
}

fn parse_wire_api_value(value: Option<&str>) -> Result<ProviderWireApi, CliError> {
    match value {
        None => ProviderWireApi::parse(None)
            .map_err(|error| CliError::failed(format!("wire api: {error:#}"))),
        Some("anthropic")
        | Some("openai")
        | Some("openai_compatible")
        | Some("chat")
        | Some("kimi") => ProviderWireApi::parse(value)
            .map_err(|error| CliError::failed(format!("wire api: {error:#}"))),
        Some(other) => Err(CliError::usage(format!(
            "--wire-api must be anthropic|openai|kimi (got {other})"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn providers_save(
    agent: &str,
    provider_id: Option<&str>,
    name: Option<String>,
    base_url: Option<String>,
    wire_api: Option<&str>,
    model: Option<String>,
    model_slots: &[(String, String)],
    context_window: Option<i64>,
    api_key_env: Option<String>,
    api_key_stdin: bool,
    delete_key: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    require_provider_agent(agent)?;
    let manager = open_providers()?;
    let secret = resolve_secret(&api_key_env, api_key_stdin)?;
    // Update merges with the existing record so unspecified fields keep their
    // stored values (the GUI edit form prefills the same way).
    let existing = provider_id.and_then(|id| manager.store().get(agent, id));
    if let (Some(id), None) = (provider_id, existing.as_ref()) {
        return Err(CliError::failed(format!(
            "provider_not_found: no provider '{id}' for agent {agent}"
        )));
    }
    let existing = existing.clone();
    let name = name.or_else(|| existing.as_ref().map(|record| record.name.clone()));
    let base_url = base_url.or_else(|| existing.as_ref().map(|record| record.base_url.clone()));
    let model = match model {
        Some(model) => Some(model),
        None => existing.as_ref().and_then(|record| record.model.clone()),
    };
    let existing_slots = existing
        .as_ref()
        .and_then(|record| record.model_slots.clone())
        .map(|slots| slots.into_iter().collect::<HashMap<String, String>>());
    let mut slots_map = existing_slots.clone().unwrap_or_default();
    for (slot, value) in model_slots {
        slots_map.insert(slot.clone(), value.clone());
    }
    let model_slots = if !slots_map.is_empty() || existing_slots.is_some() {
        Some(slots_map)
    } else {
        None
    };
    let context_window =
        context_window.or_else(|| existing.as_ref().and_then(|record| record.context_window));
    let wire_api = parse_wire_api_value(wire_api.or_else(|| {
        existing.as_ref().map(|record| match record.wire_api {
            ProviderWireApi::Anthropic => "anthropic",
            ProviderWireApi::Openai => "openai",
            ProviderWireApi::Kimi => "kimi",
        })
    }))?;
    let api_key_action = if delete_key {
        CredentialEditAction::Delete
    } else if secret.is_some() {
        CredentialEditAction::Replace
    } else {
        CredentialEditAction::KeepExisting
    };
    let name = name.ok_or_else(|| CliError::usage("providers add requires --name"))?;
    let base_url = base_url.ok_or_else(|| CliError::usage("providers add requires --base-url"))?;
    let record = manager
        .save(
            agent,
            provider_id,
            name,
            base_url,
            model,
            model_slots,
            context_window,
            wire_api,
            secret,
            api_key_action,
        )
        .map_err(|error| store_error("providers save", agent, error))?;
    let value = serde_json::json!({
        "agent": agent,
        "action": if provider_id.is_some() { "updated" } else { "added" },
        "provider": record,
    });
    let human = format!(
        "{} provider {}: {} ({})",
        if provider_id.is_some() {
            "updated"
        } else {
            "added"
        },
        record.id,
        record.name,
        record.base_url,
    );
    Ok(success(render(output, human, &value)))
}

fn providers_remove(
    agent: &str,
    provider_id: &str,
    yes: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    require_provider_agent(agent)?;
    require_yes(yes)?;
    let manager = open_providers()?;
    let removed = manager
        .delete(agent, provider_id)
        .map_err(|error| store_error("providers remove", provider_id, error))?;
    let value = serde_json::json!({
        "agent": agent,
        "action": "removed",
        "provider_id": provider_id,
        "removed": removed.is_some(),
    });
    Ok(success(render(
        output,
        format!("removed {provider_id} from {agent}"),
        &value,
    )))
}

fn providers_switch(
    agent: &str,
    provider_id: &str,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    require_provider_agent(agent)?;
    let manager = open_providers()?;
    manager
        .switch(agent, provider_id)
        .map_err(|error| store_error("providers switch", provider_id, error))?;
    let value = serde_json::json!({
        "agent": agent,
        "action": "switched",
        "provider_id": provider_id,
    });
    Ok(success(render(
        output,
        format!("switched {agent} to {provider_id}"),
        &value,
    )))
}

fn providers_switch_official(agent: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_provider_agent(agent)?;
    let manager = open_providers()?;
    manager
        .switch_official(agent)
        .map_err(|error| store_error("providers switch-official", agent, error))?;
    let value = serde_json::json!({
        "agent": agent,
        "action": "switched_official",
    });
    Ok(success(render(
        output,
        format!("restored official login for {agent}"),
        &value,
    )))
}

fn providers_export(
    agent: &str,
    destination: Option<PathBuf>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    require_provider_agent(agent)?;
    let manager = open_providers()?;
    let content = manager
        .export(agent)
        .map_err(|error| store_error("providers export", agent, error))?;
    const PLAINTEXT_WARNING: &str =
        "warning: the export contains plaintext API keys; store the file in a safe place";
    match destination {
        Some(path) => {
            std::fs::write(&path, &content).map_err(|error| {
                CliError::failed(format!(
                    "code providers export({agent}): cannot write {}: {error}",
                    path.display()
                ))
            })?;
            eprintln!("{PLAINTEXT_WARNING}");
            let value = serde_json::json!({
                "agent": agent,
                "output": path.display().to_string(),
                "bytes": content.len(),
                "containsPlaintextKeys": true,
            });
            Ok(success(render(
                output,
                format!(
                    "exported {} providers to {} (contains plaintext keys)",
                    agent,
                    path.display()
                ),
                &value,
            )))
        }
        None => {
            // Same warning the GUI shows on export; stdout stays pipeable.
            eprintln!("{PLAINTEXT_WARNING}");
            let value = serde_json::json!({
                "agent": agent,
                "content": content,
                "containsPlaintextKeys": true,
            });
            Ok(success(render(output, content, &value)))
        }
    }
}

fn providers_import(agent: &str, path: &Path, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_provider_agent(agent)?;
    let manager = open_providers()?;
    let json = std::fs::read_to_string(path).map_err(|error| {
        CliError::failed(format!(
            "code providers import({agent}): cannot read {}: {error}",
            path.display()
        ))
    })?;
    let result = manager
        .import(agent, &json)
        .map_err(|error| store_error("providers import", agent, error))?;
    let value = serde_json::json!({ "agent": agent, "result": result });
    let human = format!(
        "imported {}\nid conflicts: {}\nskipped: {}",
        result.imported, result.id_conflicts, result.skipped,
    );
    Ok(success(render(output, human, &value)))
}

// ── code sessions ───────────────────────────────────────────────────────────

/// The three ACP session model names (`features::codex_acp`), used to detect
/// ACP sessions whose sidecar record was lost (same fallback as
/// `acp_session_backend`).
fn acp_session_model(model: &str) -> bool {
    matches!(model, "Codex (ACP)" | "Claude Code (ACP)" | "Kimi (ACP)")
}

fn is_code_chat_session(agents: &SessionAgentStore, id: &str, model: &str) -> bool {
    let record = agents.get(id);
    record.backend.is_acp() || record.mode.is_code() || acp_session_model(model)
}

fn agent_label(agents: &SessionAgentStore, id: &str, model: &str) -> (&'static str, &'static str) {
    let record = agents.get(id);
    if record.mode.is_code() {
        return ("pinvou", "Pinvou");
    }
    match record.backend {
        AgentBackend::CodexAcp => ("codex", "Codex"),
        AgentBackend::ClaudeAcp => ("claude", "Claude Code"),
        AgentBackend::KimiAcp => ("kimi", "Kimi"),
        AgentBackend::Deepseek => match model {
            "Codex (ACP)" => ("codex", "Codex"),
            "Claude Code (ACP)" => ("claude", "Claude Code"),
            "Kimi (ACP)" => ("kimi", "Kimi"),
            _ => ("pinvou", "Pinvou"),
        },
    }
}

/// Mirrors `AcpPool::workspace_info` for persisted state: native code sessions
/// resolve their two roots through `SessionStore::session_roots`; ACP sessions
/// use the bound project directory or the temporary execution root.
fn code_workspace_info(
    store: &SessionStore,
    agents: &SessionAgentStore,
    id: &str,
) -> Result<(CodexWorkspaceKind, PathBuf, bool), CliError> {
    let record = agents.get(id);
    if record.mode.is_code() {
        let roots = store
            .session_roots(id)
            .map_err(|error| store_error("workspace", id, error))?;
        let available = roots.execution.is_dir();
        return Ok((record.workspace_kind, roots.execution, available));
    }
    if !record.backend.is_acp() {
        return Err(CliError::failed(format!(
            "code_session_not_found: session {id} is not a code session"
        )));
    }
    let (kind, path) = match record.workspace_kind {
        CodexWorkspaceKind::Project => (
            CodexWorkspaceKind::Project,
            record.workspace_path.clone().ok_or_else(|| {
                CliError::failed(format!(
                    "code_workspace_missing: project session {id} has no workspace record"
                ))
            })?,
        ),
        CodexWorkspaceKind::Temporary => {
            let roots = store
                .session_roots(id)
                .map_err(|error| store_error("workspace", id, error))?;
            (CodexWorkspaceKind::Temporary, roots.execution)
        }
    };
    let available = match kind {
        CodexWorkspaceKind::Project => path.is_dir(),
        CodexWorkspaceKind::Temporary => true,
    };
    Ok((kind, path, available))
}

fn code_sessions_list(output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    let agents = open_agent_store()?;
    let mut rows = store
        .list()
        .map_err(|error| store_error("sessions list", "-", error))?;
    rows.retain(|metadata| {
        matches!(store.session_kind(&metadata.id), Ok(SessionKind::Chat))
            && !store.is_hidden(&metadata.id)
            && is_code_chat_session(&agents, &metadata.id, &metadata.model)
    });
    rows.sort_by_key(|metadata| std::cmp::Reverse(metadata.updated_at));
    let mut items = Vec::new();
    for metadata in &rows {
        let info = match code_workspace_info(&store, &agents, &metadata.id) {
            Ok(info) => info,
            // Sessions whose workspace cannot be resolved are still listed;
            // the workspace fields degrade to unavailable, matching the GUI's
            // per-item error isolation only loosely (the GUI aborts instead).
            Err(_) => (CodexWorkspaceKind::Temporary, PathBuf::new(), false),
        };
        let (agent_id, agent_name) = agent_label(&agents, &metadata.id, &metadata.model);
        let mut value = serde_json::to_value(metadata)
            .map_err(|error| CliError::failed(format!("code sessions list: {error}")))?;
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "pinned".into(),
                serde_json::json!(store.is_pinned(&metadata.id)),
            );
            object.insert(
                "workspace_kind".into(),
                serde_json::json!(match info.0 {
                    CodexWorkspaceKind::Project => "project",
                    CodexWorkspaceKind::Temporary => "temporary",
                }),
            );
            object.insert(
                "workspace_path".into(),
                serde_json::json!(info.1.display().to_string()),
            );
            object.insert("workspace_available".into(), serde_json::json!(info.2));
            object.insert("agent_id".into(), serde_json::json!(agent_id));
            object.insert("agent_name".into(), serde_json::json!(agent_name));
        }
        items.push(value);
    }
    let human = rows
        .iter()
        .zip(items.iter())
        .map(|(metadata, item)| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                metadata.id,
                item["agent_id"].as_str().unwrap_or("-"),
                item["workspace_kind"].as_str().unwrap_or("-"),
                metadata.updated_at.to_rfc3339(),
                metadata.title,
                item["workspace_path"].as_str().unwrap_or("-"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let value = serde_json::json!({ "sessions": items });
    Ok(success(render(output, human, &value)))
}

fn code_sessions_info(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    let agents = open_agent_store()?;
    require_existing(&store, id, "sessions info")?;
    let record = agents.get(id);
    if !record.backend.is_acp() && !record.mode.is_code() {
        return Err(CliError::failed(format!(
            "code_session_not_found: session {id} is not a code session"
        )));
    }
    let metadata = store
        .list()
        .map_err(|error| store_error("sessions info", id, error))?
        .into_iter()
        .find(|metadata| metadata.id == id);
    let info = code_workspace_info(&store, &agents, id)?;
    let (agent_id, agent_name) = agent_label(&agents, id, "");
    let value = serde_json::json!({
        "id": id,
        "title": metadata.as_ref().map(|metadata| metadata.title.clone()),
        "updated_at": metadata.as_ref().map(|metadata| metadata.updated_at.to_rfc3339()),
        "pinned": store.is_pinned(id),
        "agent_id": agent_id,
        "agent_name": agent_name,
        "backend": serde_json::to_value(record.backend).unwrap_or(serde_json::json!(null)),
        "acp_session_id": record.acp_session_id,
        "acp_model_id": record.acp_model_id,
        "acp_mode_id": record.acp_mode_id,
        "workspace_kind": match info.0 {
            CodexWorkspaceKind::Project => "project",
            CodexWorkspaceKind::Temporary => "temporary",
        },
        "workspace_path": info.1.display().to_string(),
        "workspace_available": info.2,
    });
    let human = format!(
        "id: {}\nagent: {}\nworkspace: {}\navailable: {}\nacp_session: {}\nmodel: {}",
        id,
        agent_id,
        info.1.display(),
        info.2,
        record.acp_session_id.as_deref().unwrap_or("-"),
        record.acp_model_id.as_deref().unwrap_or("-"),
    );
    Ok(success(render(output, human, &value)))
}

/// Per-event ACP timeline from `sessions_root/<id>/acp-timeline.jsonl`, the
/// same append-only file `AcpPool::timeline` reads. Malformed lines are
/// skipped (the GUI logs and skips identically).
fn code_sessions_timeline(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    require_existing(&store, id, "sessions timeline")?;
    let path = paths::sessions_root().join(id).join("acp-timeline.jsonl");
    let mut events = Vec::new();
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            for line in content.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                    if value.is_object() {
                        events.push(value);
                    }
                }
            }
            events.sort_by_key(|event| {
                event
                    .get("seq")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0)
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CliError::failed(format!(
                "code sessions timeline({id}): cannot read {}: {error}",
                path.display()
            )));
        }
    }
    let human = events
        .iter()
        .map(|event| {
            format!(
                "{}\t{}\t{}\t{}",
                event
                    .get("seq")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0),
                event
                    .get("timestamp")
                    .and_then(|value| value.as_str())
                    .unwrap_or(""),
                event
                    .pointer("/event/event_type")
                    .and_then(|value| value.as_str())
                    .unwrap_or(""),
                event
                    .get("turn_id")
                    .and_then(|value| value.as_str())
                    .unwrap_or("-"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let value = serde_json::json!({ "id": id, "events": events });
    Ok(success(render(output, human, &value)))
}

// ── workspace (local mirror of features::codex_acp::workspace) ──────────────

const LIST_LIMIT: usize = 500;
const SEARCH_LIMIT: usize = 300;
const WALK_LIMIT: usize = 20_000;
const PREVIEW_LIMIT: usize = 512 * 1024;
const IMAGE_PREVIEW_LIMIT: u64 = 10 * 1024 * 1024;
const DIFF_LIMIT: usize = 1024 * 1024;
/// Upper bound on per-file diffs composed into one whole-workspace diff; each
/// file costs two git spawns, so this bounds the subprocess fan-out.
const WORKSPACE_DIFF_FILE_CAP: usize = 500;

/// `String::truncate` panics on a non-char-boundary index; a multi-byte diff
/// cut near the 1 MiB boundary is the common case, so cut back to the nearest
/// boundary instead.
fn truncate_utf8(text: &mut String, limit: usize) {
    if text.len() <= limit {
        return;
    }
    let mut boundary = limit;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text.truncate(boundary);
}

const IGNORED_DIRECTORIES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".cache",
    "__pycache__",
    ".venv",
    "venv",
];

/// Same process-local serialization as `workspace::CHECKOUT_LOCK`: concurrent
/// checkouts must not interleave stash push/checkout/pop sequences.
static CHECKOUT_LOCK: Mutex<()> = Mutex::new(());

/// Cross-process advisory lock (fd-lock, the same OS primitive the app's
/// remote-control owner lock uses) serializing mutations of one code session
/// — `checkpoints rewind` / `undo` and `workspace checkout`. The GUI's
/// guards (`is_turn_active`, `begin_execution_root_rewind`,
/// `busy_peer_on_same_execution_root`) are process-local and therefore
/// invisible to the CLI; this lock closes the CLI×CLI race, while a
/// concurrent GUI turn on the same session remains undetectable here
/// (documented in the command docs and `docs/pinvou-cli.md`).
/// Callers must keep the returned lock alive alongside its write guard.
fn session_mutation_lock(session: &str) -> Result<fd_lock::RwLock<std::fs::File>, CliError> {
    if !valid_session_id(session) {
        return Err(CliError::usage("invalid session id"));
    }
    let dir = paths::pinvou3_home().join("locks");
    std::fs::create_dir_all(&dir).map_err(|error| {
        CliError::failed(format!(
            "code session lock: cannot create {}: {error}",
            dir.display()
        ))
    })?;
    let path = dir.join(format!("code-session-{session}.lock"));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| {
            CliError::failed(format!(
                "code session lock: cannot open {}: {error}",
                path.display()
            ))
        })?;
    Ok(fd_lock::RwLock::new(file))
}

/// Opens the execution-root lock: the GUI's rewind exclusion unit is the
/// execution root (`begin_execution_root_rewind`), because two native code
/// sessions bound to the same project directory share one working tree — the
/// per-session lock above cannot serialize those. This second lock (keyed by
/// a hash of the canonical execution root) closes the preventable CLI×CLI
/// half. The GUI-vs-CLI residual (a GUI turn or rewind in the desktop process
/// takes no CLI lock) remains undetectable and stays documented.
fn execution_root_lock(root: &Path) -> Result<fd_lock::RwLock<std::fs::File>, CliError> {
    fn stable(root: &Path) -> String {
        // FNV-1a over the canonical path: deterministic across processes is
        // the only requirement (this is a lock key, not a security digest).
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in root.to_string_lossy().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{hash:016x}")
    }
    let dir = paths::pinvou3_home().join("locks");
    std::fs::create_dir_all(&dir).map_err(|error| {
        CliError::failed(format!(
            "code root lock: cannot create {}: {error}",
            dir.display()
        ))
    })?;
    let path = dir.join(format!("code-root-{}.lock", stable(root)));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| {
            CliError::failed(format!(
                "code root lock: cannot open {}: {error}",
                path.display()
            ))
        })?;
    Ok(fd_lock::RwLock::new(file))
}

/// Acquires the execution-root lock or fails fast with a stable busy error.
fn lock_root_for_mutation<'a>(
    lock: &'a mut fd_lock::RwLock<std::fs::File>,
    root: &Path,
    action: &str,
    session: &str,
) -> Result<fd_lock::RwLockWriteGuard<'a, std::fs::File>, CliError> {
    lock.try_write().map_err(|error| {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            CliError::failed(format!(
                "{action}_busy: another pinvou process is mutating the project directory of \
                 session {session} ({}); retry after it finishes",
                root.display()
            ))
        } else {
            CliError::failed(format!(
                "code {action}: cannot lock the project directory of session {session}: {error}"
            ))
        }
    })
}

/// Canonicalized execution root for lock keying; falls back to the unresolved
/// path when the directory vanished (the mutation itself will fail anyway).
fn canonical_execution_root(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// Acquires the session mutation lock or fails fast with a stable busy error
/// (a CLI that silently queued behind another process would race the user's
/// intent just the same).
fn lock_session_for_mutation<'a>(
    lock: &'a mut fd_lock::RwLock<std::fs::File>,
    session: &str,
    action: &str,
) -> Result<fd_lock::RwLockWriteGuard<'a, std::fs::File>, CliError> {
    lock.try_write().map_err(|error| {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            CliError::failed(format!(
                "{action}_busy: another pinvou process is mutating session {session}; retry \
                 after it finishes"
            ))
        } else {
            CliError::failed(format!(
                "{action}_lock: cannot lock session {session}: {error}"
            ))
        }
    })
}

fn canonical_workspace(root: &Path) -> Result<PathBuf, CliError> {
    let canonical = std::fs::canonicalize(root).map_err(|error| {
        CliError::failed(format!(
            "code workspace: workspace unavailable {}: {error}",
            root.display()
        ))
    })?;
    if !canonical.is_dir() {
        return Err(CliError::failed(format!(
            "code workspace: workspace unavailable {}",
            canonical.display()
        )));
    }
    Ok(canonical)
}

fn normalize_relative_path(raw: &str) -> Result<String, CliError> {
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(CliError::usage("workspace path must be relative"));
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(CliError::usage("workspace path cannot escape the root"));
            }
        }
    }
    Ok(parts.join("/"))
}

fn relative_text(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default()
}

fn modified_seconds(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default()
}

fn is_ignored_directory(name: &str) -> bool {
    IGNORED_DIRECTORIES.contains(&name)
}

fn should_walk(root: &Path, path: &Path) -> bool {
    if path == root {
        return true;
    }
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    !path.is_dir() || !is_ignored_directory(&name)
}

struct WalkFile {
    path: PathBuf,
    relative: String,
}

/// Depth-first walk mirroring the GUI's `WalkDir` limits: ignored directories
/// are pruned, symlinks are never followed, and the walk stops at WALK_LIMIT.
fn walk_files(root: &Path) -> Vec<WalkFile> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut visited = 0usize;
    while let Some(directory) = stack.pop() {
        visited += 1;
        if visited > WALK_LIMIT {
            break;
        }
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if path.is_symlink() {
                continue;
            }
            if !should_walk(root, &path) {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                files.push(WalkFile {
                    relative: relative_text(root, &path),
                    path,
                });
            }
            if files.len() >= WALK_LIMIT {
                return files;
            }
        }
    }
    files
}

fn workspace_list(
    root: &Path,
    relative_path: Option<&str>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let root = canonical_workspace(root)?;
    let relative = normalize_relative_path(relative_path.unwrap_or_default())?;
    let directory = if relative.is_empty() {
        root.clone()
    } else {
        let candidate = root.join(&relative);
        let canonical = std::fs::canonicalize(&candidate).map_err(|_| {
            CliError::failed(format!("code workspace list: path not found {relative}"))
        })?;
        if !canonical.starts_with(&root) {
            return Err(CliError::failed(
                "code workspace list: path escapes the workspace",
            ));
        }
        if !canonical.is_dir() {
            return Err(CliError::failed(format!(
                "code workspace list: not a directory {relative}"
            )));
        }
        canonical
    };
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&directory)
        .map_err(|error| CliError::failed(format!("code workspace list: {error}")))?
        .filter_map(|entry| entry.ok())
    {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.is_empty() || (metadata.is_dir() && is_ignored_directory(&name)) {
            continue;
        }
        let kind = if metadata.is_dir() {
            "directory"
        } else if metadata.is_file() {
            "file"
        } else {
            continue;
        };
        let has_children = metadata.is_dir()
            && std::fs::read_dir(&path).is_ok_and(|mut entries| entries.next().is_some());
        entries.push(serde_json::json!({
            "name": name,
            "relativePath": relative_text(&root, &path),
            "kind": kind,
            "size": if metadata.is_file() { metadata.len() } else { 0 },
            "modified": modified_seconds(&metadata),
            "hasChildren": has_children,
        }));
    }
    entries.sort_by(|left, right| {
        let left_dir = left["kind"] == "directory";
        let right_dir = right["kind"] == "directory";
        right_dir.cmp(&left_dir).then_with(|| {
            left["name"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase()
                .cmp(&right["name"].as_str().unwrap_or_default().to_lowercase())
        })
    });
    let truncated = entries.len() > LIST_LIMIT;
    entries.truncate(LIST_LIMIT);
    let human = entries
        .iter()
        .map(|entry| {
            format!(
                "{}\t{}\t{}\t{}",
                entry["kind"].as_str().unwrap_or("-"),
                entry["name"].as_str().unwrap_or("-"),
                entry["size"].as_u64().unwrap_or(0),
                entry["modified"].as_i64().unwrap_or(0),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let value = serde_json::json!({
        "relativePath": relative,
        "entries": entries,
        "truncated": truncated,
    });
    Ok(success(render(output, human, &value)))
}

fn workspace_search(root: &Path, query: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Ok(success(render(
            output,
            String::new(),
            &serde_json::json!({ "results": [] }),
        )));
    }
    let root = canonical_workspace(root)?;
    let mut results = Vec::new();
    for file in walk_files(&root) {
        if file.relative.to_lowercase().contains(&query) {
            if let Ok(metadata) = std::fs::symlink_metadata(&file.path) {
                results.push(serde_json::json!({
                    "name": file.path.file_name().map(|value| value.to_string_lossy().into_owned()).unwrap_or_default(),
                    "relativePath": file.relative,
                    "kind": "file",
                    "size": metadata.len(),
                    "modified": modified_seconds(&metadata),
                    "hasChildren": false,
                }));
            }
            if results.len() >= SEARCH_LIMIT {
                break;
            }
        }
    }
    results.sort_by(|left, right| {
        left["relativePath"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["relativePath"].as_str().unwrap_or_default())
    });
    let human = results
        .iter()
        .map(|entry| {
            entry["relativePath"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n");
    let value = serde_json::json!({ "results": results });
    Ok(success(render(output, human, &value)))
}

fn file_kind(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg"
    ) {
        return "image";
    }
    if matches!(
        extension.as_str(),
        "md" | "markdown"
            | "txt"
            | "log"
            | "csv"
            | "json"
            | "yaml"
            | "yml"
            | "toml"
            | "xml"
            | "html"
            | "htm"
            | "rs"
            | "py"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "go"
            | "c"
            | "cpp"
            | "h"
            | "hpp"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "bat"
            | "cmd"
            | "ps1"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "vue"
            | "svelte"
            | "sql"
            | "ini"
            | "conf"
            | "cfg"
            | "env"
            | "properties"
            | "diff"
            | "patch"
            | "lock"
            | "proto"
            | "graphql"
            | "gql"
            | "prisma"
            | "java"
            | "kt"
            | "swift"
            | "rb"
            | "php"
            | "cs"
            | "lua"
            | "scala"
            | "gradle"
            | "tf"
            | "tex"
            | "rst"
            | "pl"
            | "pm"
            | "r"
            | "m"
            | "mm"
    ) {
        return "text";
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        file_name.as_str(),
        "dockerfile"
            | "makefile"
            | "gnumakefile"
            | "jenkinsfile"
            | "vagrantfile"
            | "gemfile"
            | "rakefile"
            | "brewfile"
            | "cmakelists.txt"
            | "license"
            | "licence"
            | "copying"
            | "notice"
            | "authors"
            | "contributors"
            | "changelog"
            | "readme"
            | ".gitignore"
            | ".gitattributes"
            | ".gitmodules"
            | ".editorconfig"
            | ".npmrc"
            | ".yarnrc"
            | ".env"
            | ".envrc"
    ) {
        return "text";
    }
    if looks_like_text(path) {
        return "text";
    }
    "binary"
}

fn looks_like_text(path: &Path) -> bool {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut buffer = [0u8; 8192];
    let read = file.read(&mut buffer).unwrap_or(0);
    let sample = &buffer[..read];
    if sample.contains(&0) {
        return false;
    }
    match std::str::from_utf8(sample) {
        Ok(_) => true,
        Err(error) => error.valid_up_to() > 0 && sample.len() - error.valid_up_to() <= 4,
    }
}

fn image_mime_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        _ => "image/png",
    }
}

/// Minimal standard base64 encoder for image preview data URLs (no base64
/// dependency in the CLI crate).
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let value = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        out.push(ALPHABET[(value >> 18) as usize & 63] as char);
        out.push(ALPHABET[(value >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(value >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[value as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn workspace_preview(
    root: &Path,
    relative_path: &str,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let root = canonical_workspace(root)?;
    let relative = normalize_relative_path(relative_path)?;
    let path = if relative.is_empty() {
        return Err(CliError::usage("workspace preview requires a file path"));
    } else {
        let candidate = root.join(&relative);
        let canonical = std::fs::canonicalize(&candidate).map_err(|_| {
            CliError::failed(format!("code workspace preview: path not found {relative}"))
        })?;
        if !canonical.starts_with(&root) {
            return Err(CliError::failed(
                "code workspace preview: path escapes the workspace",
            ));
        }
        canonical
    };
    let metadata = path
        .metadata()
        .map_err(|error| CliError::failed(format!("code workspace preview: {error}")))?;
    if !metadata.is_file() {
        return Err(CliError::failed(format!(
            "code workspace preview: not a file {relative}"
        )));
    }
    let kind = file_kind(&path);
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(&relative)
        .to_string();
    let (text, data_url, truncated) = if kind == "image" {
        if metadata.len() > IMAGE_PREVIEW_LIMIT {
            (None, None, true)
        } else {
            let bytes = std::fs::read(&path)
                .map_err(|error| CliError::failed(format!("code workspace preview: {error}")))?;
            (
                None,
                Some(format!(
                    "data:{};base64,{}",
                    image_mime_type(&path),
                    base64_encode(&bytes)
                )),
                false,
            )
        }
    } else if kind == "text" {
        let mut file = std::fs::File::open(&path)
            .map_err(|error| CliError::failed(format!("code workspace preview: {error}")))?;
        let mut bytes = Vec::with_capacity(PREVIEW_LIMIT.min(metadata.len() as usize));
        file.by_ref()
            .take(PREVIEW_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| CliError::failed(format!("code workspace preview: {error}")))?;
        let truncated = bytes.len() > PREVIEW_LIMIT;
        bytes.truncate(PREVIEW_LIMIT);
        (
            Some(String::from_utf8_lossy(&bytes).into_owned()),
            None,
            truncated,
        )
    } else {
        (None, None, false)
    };
    let value = serde_json::json!({
        "name": name,
        "relativePath": relative,
        "kind": kind,
        "size": metadata.len(),
        "modified": modified_seconds(&metadata),
        "text": text,
        "dataUrl": data_url,
        "truncated": truncated,
    });
    let human = match (&text, &data_url) {
        (Some(text), _) => text.clone(),
        (None, Some(url)) => url.clone(),
        _ => format!("{} ({kind}, {} bytes)", relative, metadata.len()),
    };
    Ok(success(render(output, human, &value)))
}

fn git_command(root: &Path, arguments: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command
        .current_dir(root)
        .args(arguments)
        .stdin(std::process::Stdio::null());
    // Checkpoint-grade git isolation — stronger than the GUI's workspace lane
    // (which inherits ambient GIT_*) and misattributed before: raw-byte prefix
    // matching (like the app's checkpoint lane) strips every ambient GIT_*
    // variable without tripping on non-UTF-8 names, and user/system gitconfig
    // is pinned away so hooks, aliases, and credential helpers cannot inject
    // themselves into these calls.
    for (name, _) in std::env::vars_os() {
        if name.as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(name);
        }
    }
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    #[cfg(unix)]
    command.env("GIT_CONFIG_GLOBAL", "/dev/null");
    #[cfg(target_os = "windows")]
    command.env("GIT_CONFIG_GLOBAL", "NUL");
    command
}

fn git_output(root: &Path, arguments: &[&str]) -> Result<String, CliError> {
    let output = git_command(root, arguments).output().map_err(|error| {
        CliError::failed(format!(
            "code workspace: git {}: {error}",
            arguments.join(" ")
        ))
    })?;
    if !output.status.success() {
        return Err(CliError::failed(format!(
            "code workspace: git {} failed: {}",
            arguments.join(" "),
            pinvou3_lib::platform::credential_store::redact_secret(
                String::from_utf8_lossy(&output.stderr).trim(),
            )
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_root(root: &Path) -> Option<PathBuf> {
    let output = git_command(root, &["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    std::fs::canonicalize(String::from_utf8_lossy(&output.stdout).trim()).ok()
}

fn git_branch(root: &Path) -> Option<String> {
    let branch = git_output(root, &["branch", "--show-current"]).ok()?;
    let branch = branch.trim();
    (!branch.is_empty()).then(|| branch.to_string())
}

fn git_status_label(x: char, y: char) -> &'static str {
    if x == '?' && y == '?' {
        "untracked"
    } else if x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D') {
        "conflict"
    } else if x == 'A' || y == 'A' {
        "added"
    } else if x == 'D' || y == 'D' {
        "deleted"
    } else if x == 'R' || y == 'R' {
        "renamed"
    } else if x == 'C' || y == 'C' {
        "copied"
    } else {
        "modified"
    }
}

fn git_status_entries(root: &Path) -> Result<Vec<(String, String, bool)>, CliError> {
    let output = git_command(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            ".",
        ],
    )
    .output()
    .map_err(|error| CliError::failed(format!("code workspace: git status: {error}")))?;
    if !output.status.success() {
        return Err(CliError::failed(format!(
            "code workspace: git status failed: {}",
            pinvou3_lib::platform::credential_store::redact_secret(
                String::from_utf8_lossy(&output.stderr).trim(),
            )
        )));
    }
    let records = output.stdout.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut changes = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.len() < 4 {
            continue;
        }
        let x = record[0] as char;
        let y = record[1] as char;
        let path = String::from_utf8_lossy(&record[3..]).replace('\\', "/");
        if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            index += 1;
        }
        changes.push((
            path,
            git_status_label(x, y).to_string(),
            x != ' ' && x != '?',
        ));
    }
    Ok(changes)
}

fn baseline_path(session_id: &str) -> PathBuf {
    paths::sessions_root()
        .join(session_id)
        .join("codex-workspace-baseline.json")
}

/// Mirror of the GUI baseline loader: a missing baseline is `Ok(None)`, but
/// read or parse failures surface instead of silently degrading origin
/// reporting to "unknown".
fn load_baseline(session_id: &str, root: &Path) -> Result<Option<serde_json::Value>, CliError> {
    let bytes = match std::fs::read(baseline_path(session_id)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CliError::failed(format!(
                "code workspace: cannot read the workspace baseline: {error}"
            )));
        }
    };
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| {
        CliError::failed(format!(
            "code workspace: cannot parse the workspace baseline: {error}"
        ))
    })?;
    if value.get("workspace_path").and_then(|value| value.as_str())
        != Some(root.to_string_lossy().as_ref())
    {
        return Ok(None);
    }
    Ok(Some(value))
}

fn classify_origin(
    root: &Path,
    baseline: Option<&serde_json::Value>,
    relative_path: &str,
) -> String {
    let Some(baseline) = baseline else {
        return "unknown".to_owned();
    };
    let dirty = baseline
        .get("dirtyPaths")
        .and_then(|value| value.as_array())
        .map(|paths| {
            paths
                .iter()
                .filter_map(|value| value.as_str())
                .any(|path| path == relative_path)
        })
        .unwrap_or(false);
    if !dirty {
        return "session".to_owned();
    }
    // The GUI compares sha256 fingerprints for pre-existing dirty files; the
    // CLI mirror reports the file as preexisting_modified without hashing
    // (the size+mtime fields come from the same baseline capture).
    match (
        baseline
            .get("entries")
            .and_then(|entries| entries.get(relative_path)),
        std::fs::metadata(root.join(relative_path)).ok(),
    ) {
        (Some(before), Some(current)) => {
            let same_size =
                before.get("size").and_then(|value| value.as_u64()) == Some(current.len());
            let same_mtime = before.get("modified").and_then(|value| value.as_i64())
                == Some(modified_seconds(&current));
            if same_size && same_mtime {
                "preexisting".to_owned()
            } else {
                "preexisting_modified".to_owned()
            }
        }
        // A dirty-at-baseline file deleted during the session compares
        // unequal in the GUI, so it is "preexisting_modified", not a clean
        // "preexisting".
        (Some(_), None) => "preexisting_modified".to_owned(),
        _ => "preexisting_modified".to_owned(),
    }
}

fn workspace_changes(
    session_id: &str,
    root: &Path,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let root = canonical_workspace(root)?;
    let git = git_root(&root).is_some_and(|git_root| git_root == root);
    let baseline = load_baseline(session_id, &root)?;
    let baseline_available = baseline.is_some();
    let mut changes: Vec<(String, String, bool)> = if git {
        git_status_entries(&root)?
    } else {
        filesystem_changes(&root, baseline.as_ref())?
    };
    let origin = |path: &str| classify_origin(&root, baseline.as_ref(), path);
    changes.sort_by(|left, right| left.0.cmp(&right.0));
    let rows = changes
        .iter()
        .map(|(path, status, staged)| {
            serde_json::json!({
                "relativePath": path,
                "status": status,
                "staged": staged,
                "origin": origin(path),
            })
        })
        .collect::<Vec<_>>();
    let human = rows
        .iter()
        .map(|row| {
            format!(
                "{}\t{}\t{}\t{}",
                row["status"].as_str().unwrap_or("-"),
                if row["staged"].as_bool().unwrap_or(false) {
                    "staged"
                } else {
                    "-"
                },
                row["origin"].as_str().unwrap_or("-"),
                row["relativePath"].as_str().unwrap_or("-"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let value = serde_json::json!({
        "git": git,
        "branch": git.then(|| git_branch(&root)).flatten(),
        "baselineAvailable": baseline_available,
        "changes": rows,
    });
    Ok(success(render(output, human, &value)))
}

/// Non-git change detection against the workspace baseline fingerprints.
fn filesystem_changes(
    root: &Path,
    baseline: Option<&serde_json::Value>,
) -> Result<Vec<(String, String, bool)>, CliError> {
    let current: BTreeMap<String, (u64, i64)> = walk_files(root)
        .into_iter()
        .filter_map(|file| {
            let metadata = std::fs::metadata(&file.path).ok()?;
            Some((file.relative, (metadata.len(), modified_seconds(&metadata))))
        })
        .collect();
    let Some(baseline) = baseline else {
        return Ok(current
            .keys()
            .map(|path| (path.clone(), "unknown".to_owned(), false))
            .collect());
    };
    let before: BTreeMap<String, (u64, i64)> = baseline
        .get("entries")
        .and_then(|entries| entries.as_object())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|(path, value)| {
                    Some((
                        path.clone(),
                        (
                            value.get("size").and_then(|value| value.as_u64())?,
                            value.get("modified").and_then(|value| value.as_i64())?,
                        ),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut paths: BTreeSet<String> = BTreeSet::new();
    paths.extend(current.keys().cloned());
    paths.extend(before.keys().cloned());
    Ok(paths
        .into_iter()
        .filter_map(|path| match (before.get(&path), current.get(&path)) {
            (None, Some(_)) => Some((path, "added".to_owned(), false)),
            (Some(_), None) => Some((path, "deleted".to_owned(), false)),
            (Some(before), Some(current)) if before != current => {
                Some((path, "modified".to_owned(), false))
            }
            _ => None,
        })
        .collect())
}

fn workspace_branches(root: &Path, output: OutputMode) -> Result<CliOutcome, CliError> {
    let root = canonical_workspace(root)?;
    let branches = workspace_branches_inner(&root)?;
    let human = format!(
        "git: {}\ncurrent: {}\nbranches: {}\ndirty: {}",
        branches["git"],
        branches["current"].as_str().unwrap_or("-"),
        branches["branches"]
            .as_array()
            .map(|branches| branches
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(", "))
            .unwrap_or_default(),
        branches["dirtyCount"].as_u64().unwrap_or(0),
    );
    Ok(success(render(output, human, &branches)))
}

fn workspace_branches_inner(root: &Path) -> Result<serde_json::Value, CliError> {
    let root = canonical_workspace(root)?;
    let is_git = git_root(&root).is_some_and(|git_root| git_root == root);
    if !is_git {
        return Ok(serde_json::json!({
            "git": false,
            "current": null,
            "branches": [],
            "dirtyCount": 0,
        }));
    }
    let listing = git_output(
        &root,
        &[
            "branch",
            "--sort=-committerdate",
            "--format=%(refname:short)",
        ],
    )?;
    let branches = listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let dirty_count = git_status_entries(&root)?.len();
    Ok(serde_json::json!({
        "git": true,
        "current": git_branch(&root),
        "branches": branches,
        "dirtyCount": dirty_count,
    }))
}

fn stash_head(root: &Path) -> Result<Option<String>, CliError> {
    Ok(
        git_output(root, &["rev-parse", "-q", "--verify", "refs/stash"])
            .ok()
            .map(|head| head.trim().to_string()),
    )
}

fn workspace_checkout(
    session: &str,
    root: &Path,
    branch: &str,
    mode: BranchSwitchMode,
    commit_message: Option<&str>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let _checkout_guard = CHECKOUT_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let root = canonical_workspace(root)?;
    // Cross-process root lock (same key as rewind/undo): two CLI processes
    // checking out different sessions bound to one project directory must not
    // interleave stash/checkout/pop on one working tree.
    let canonical_root = canonical_execution_root(&root);
    let mut root_lock = execution_root_lock(&canonical_root)?;
    let _root_guard = lock_root_for_mutation(&mut root_lock, &canonical_root, "checkout", session)?;
    let is_git = git_root(&root).is_some_and(|git_root| git_root == root);
    if !is_git {
        return Err(CliError::failed(
            "code workspace checkout: the workspace is not a git repository",
        ));
    }
    let branch = branch.trim();
    // Same guards as `workspace::checkout_workspace_branch`: never let a name
    // starting with '-' be parsed as a git option, and only allow existing
    // local branches (a file path would be restored as a pathspec, dropping
    // uncommitted changes).
    if branch.is_empty() || branch.starts_with('-') {
        return Err(CliError::failed(format!(
            "code workspace checkout: invalid branch name {branch}"
        )));
    }
    let known = git_output(&root, &["branch", "--format=%(refname:short)"])?;
    if !known.lines().map(str::trim).any(|name| name == branch) {
        return Err(CliError::failed(format!(
            "code workspace checkout: branch not found {branch}"
        )));
    }
    if git_status_entries(&root)?.is_empty() {
        git_output(&root, &["checkout", branch])?;
        return finish_checkout(&root, branch, output);
    }
    match mode {
        BranchSwitchMode::Carry => {
            git_output(&root, &["checkout", branch])?;
        }
        BranchSwitchMode::Stash => {
            // Stash (with untracked files) → checkout → pop. Pop only when the
            // push actually created an entry; on checkout failure restore the
            // stash first, and report honestly when the restore also fails.
            let prior_stash = stash_head(&root)?;
            git_output(
                &root,
                &[
                    "stash",
                    "push",
                    "--include-untracked",
                    "-m",
                    "pinvou: branch switch",
                ],
            )?;
            let stash_created = stash_head(&root)? != prior_stash;
            if let Err(error) = git_output(&root, &["checkout", branch]) {
                if stash_created {
                    if let Err(pop_error) = git_output(&root, &["stash", "pop"]) {
                        return Err(CliError::failed(format!(
                            "code workspace checkout: switching to {branch} failed and restoring the \
                             stashed changes failed too; your changes are safe in the stash (git \
                             stash list). checkout error: {error}; pop error: {pop_error}"
                        )));
                    }
                }
                return Err(error);
            }
            if stash_created {
                if let Err(error) = git_output(&root, &["stash", "pop"]) {
                    return Err(CliError::failed(format!(
                        "code workspace checkout: switched to {branch}, but restoring the stashed \
                         changes failed; the conflicting content was applied and the stash entry \
                         was kept. Inspect it with `git stash show -u stash@{{0}}` before dropping: \
                         {error}"
                    )));
                }
            }
        }
        BranchSwitchMode::Commit => {
            let message = commit_message.map(str::trim).unwrap_or_default();
            if message.is_empty() {
                return Err(CliError::usage(
                    "code workspace checkout --mode commit requires --message",
                ));
            }
            git_output(&root, &["add", "-A"])?;
            git_output(&root, &["commit", "-m", message])?;
            git_output(&root, &["checkout", branch])?;
        }
    }
    finish_checkout(&root, branch, output)
}

fn finish_checkout(root: &Path, branch: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let branches = workspace_branches_inner(root)?;
    let value = serde_json::json!({
        "checkedOut": branch,
        "branches": branches,
    });
    Ok(success(render(
        output,
        format!("checked out {branch}"),
        &value,
    )))
}

fn workspace_diff(
    session_id: &str,
    root: &Path,
    file: Option<&str>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let root = canonical_workspace(root)?;
    match file {
        Some(file) => {
            let diff = workspace_diff_one(&root, file)?;
            let value = serde_json::json!({
                "relativePath": diff.0,
                "text": diff.1,
                "truncated": diff.2,
            });
            Ok(success(render(output, diff.1, &value)))
        }
        None => {
            // Whole-workspace diff: concatenate per-file diffs of every change
            // (the GUI shows the per-file list in the changes panel instead).
            // Capped so a 2,000-file refactor does not turn into thousands of
            // git spawns; the changes list itself stays uncapped.
            let changes = workspace_changes_value(session_id, &root)?;
            let mut combined = String::new();
            let mut diffed_files = 0usize;
            for row in changes["changes"].as_array().cloned().unwrap_or_default() {
                if diffed_files >= WORKSPACE_DIFF_FILE_CAP {
                    break;
                }
                diffed_files += 1;
                let relative = row["relativePath"].as_str().unwrap_or_default().to_owned();
                if let Ok((_, text, _)) = workspace_diff_one(&root, &relative) {
                    if !combined.is_empty() {
                        combined.push('\n');
                    }
                    combined.push_str(&text);
                }
            }
            // Actually cut the payload when reporting truncation — the
            // per-file path below does the same.
            let truncated = combined.len() > DIFF_LIMIT;
            if truncated {
                truncate_utf8(&mut combined, DIFF_LIMIT);
            }
            let value = serde_json::json!({
                "relativePath": null,
                "text": combined,
                "truncated": truncated,
                "changes": changes["changes"],
            });
            Ok(success(render(output, combined, &value)))
        }
    }
}

fn workspace_changes_value(session_id: &str, root: &Path) -> Result<serde_json::Value, CliError> {
    let outcome = workspace_changes(session_id, root, OutputMode::Json)?;
    serde_json::from_str(&outcome.stdout)
        .map_err(|error| CliError::failed(format!("code workspace diff: {error}")))
}

/// One-file diff with the same composition rules as
/// `workspace::workspace_diff`: staged + unstaged sections, synthetic diff for
/// untracked text files, preview fallback outside git.
fn workspace_diff_one(
    root: &Path,
    relative_path: &str,
) -> Result<(String, String, bool), CliError> {
    let root = canonical_workspace(root)?;
    let relative = normalize_relative_path(relative_path)?;
    let path = root.join(&relative);
    if !path.starts_with(&root) {
        return Err(CliError::failed(
            "code workspace diff: path escapes the workspace",
        ));
    }
    let mut text = if git_root(&root).is_some_and(|git_root| git_root == root) {
        let unstaged = git_output(
            &root,
            &["diff", "--no-ext-diff", "--no-color", "--", &relative],
        )?;
        let staged = git_output(
            &root,
            &[
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-color",
                "--",
                &relative,
            ],
        )?;
        let mut combined = String::new();
        if !staged.trim().is_empty() {
            combined.push_str("# staged\n");
            combined.push_str(&staged);
        }
        if !unstaged.trim().is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str("# unstaged\n");
            combined.push_str(&unstaged);
        }
        if combined.is_empty() && path.is_file() {
            untracked_diff(&path, &relative)?
        } else {
            combined
        }
    } else if path.is_file() {
        match file_kind(&path) {
            // Mirror the GUI preview lane: read at most PREVIEW_LIMIT+1 bytes
            // and convert lossily instead of loading arbitrary multi-gigabyte
            // files or failing whole-file on non-UTF-8 content.
            "text" => {
                let mut bytes = Vec::new();
                std::fs::File::open(&path)
                    .and_then(|mut file| {
                        use std::io::Read as _;
                        file.by_ref()
                            .take(PREVIEW_LIMIT as u64 + 1)
                            .read_to_end(&mut bytes)
                    })
                    .map_err(|error| CliError::failed(format!("code workspace diff: {error}")))?;
                String::from_utf8_lossy(&bytes).into_owned()
            }
            "image" => "image file: use `pinvou code workspace preview` for a data URL".to_owned(),
            _ => "binary file: text diff preview is not supported".to_owned(),
        }
    } else {
        "file was deleted; a non-git workspace cannot recover the pre-delete content".to_owned()
    };
    let truncated = text.len() > DIFF_LIMIT;
    if truncated {
        truncate_utf8(&mut text, DIFF_LIMIT);
        text.push_str("\n\n...diff truncated");
    }
    Ok((relative, text, truncated))
}

fn untracked_diff(path: &Path, relative: &str) -> Result<String, CliError> {
    if file_kind(path) != "text" {
        return Ok("untracked binary files do not support diff preview".to_owned());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|error| CliError::failed(format!("code workspace diff: {error}")))?;
    let mut output_text = format!(
        "diff --git a/{0} b/{0}\nnew file mode 100644\n--- /dev/null\n+++ b/{0}\n",
        relative
    );
    for line in content.lines() {
        output_text.push('+');
        output_text.push_str(line);
        output_text.push('\n');
        if output_text.len() > DIFF_LIMIT {
            break;
        }
    }
    Ok(output_text)
}

/// Session → workspace root with the same gate as the GUI's
/// `codex_workspace_root`: the session must be an ACP or native code session
/// and the resolved workspace must exist.
fn resolve_session_workspace(
    session: &str,
) -> Result<(SessionStore, SessionAgentStore, PathBuf), CliError> {
    let store = open_store()?;
    let agents = open_agent_store()?;
    require_existing(&store, session, "workspace")?;
    let (_, root, available) = code_workspace_info(&store, &agents, session)?;
    if !available {
        return Err(CliError::failed(format!(
            "code_workspace_unavailable: workspace {} is not available",
            root.display()
        )));
    }
    Ok((store, agents, root))
}

fn with_workspace(
    session: &str,
    handler: impl FnOnce(&Path) -> Result<CliOutcome, CliError>,
) -> Result<CliOutcome, CliError> {
    let (_, _, root) = resolve_session_workspace(session)?;
    handler(&root)
}

fn with_workspace_session(
    session: &str,
    handler: impl FnOnce(String, &Path) -> Result<CliOutcome, CliError>,
) -> Result<CliOutcome, CliError> {
    let (_, _, root) = resolve_session_workspace(session)?;
    handler(session.to_owned(), &root)
}

// ── checkpoints ─────────────────────────────────────────────────────────────

/// Native-code-session gate mirroring `resolve_code_session_roots` (the GUI's
/// `SessionStore::is_code_session` is pub(crate); the persisted
/// `session-agents.json` record carries the same mode flag).
fn require_native_code_session(
    store: &SessionStore,
    agents: &SessionAgentStore,
    session: &str,
) -> Result<(PathBuf, PathBuf), CliError> {
    if !agents.is_code_session(session) {
        return Err(CliError::failed(format!(
            "code_checkpoints_requires_native_code_session: only native code sessions support \
             checkpoints (session {session})"
        )));
    }
    let roots = store
        .session_roots(session)
        .map_err(|error| store_error("checkpoints", session, error))?;
    Ok((roots.ledger, roots.execution))
}

fn checkpoints_list(session: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    let agents = open_agent_store()?;
    let (ledger, _) = require_native_code_session(&store, &agents, session)?;
    let entries = checkpoints::list_checkpoints(&ledger)
        .map_err(|error| store_error("checkpoints list", session, error))?;
    let value = serde_json::to_value(&entries)
        .map(|checkpoints| serde_json::json!({ "session": session, "checkpoints": checkpoints }))
        .unwrap_or_else(|_| serde_json::json!({ "session": session, "checkpoints": [] }));
    let human = entries
        .iter()
        .map(|entry| {
            format!(
                "{}\t{}\t{}\t{}",
                entry.id,
                match entry.kind {
                    checkpoints::CheckpointKind::Turn => "turn",
                    checkpoints::CheckpointKind::PreRestore => "pre-restore",
                },
                entry
                    .turn
                    .map(|turn| turn.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                entry.label,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(success(render(output, human, &value)))
}

fn checkpoints_diff(
    session: &str,
    checkpoint_id: &str,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    if !valid_checkpoint_id(checkpoint_id) {
        return Err(CliError::usage("invalid checkpoint id"));
    }
    // `diff_checkpoint` writes the shadow index (`git add -A`), so it takes
    // the same session mutation lock as rewind/undo.
    let mut mutation_lock = session_mutation_lock(session)?;
    let _mutation_guard = lock_session_for_mutation(&mut mutation_lock, session, "diff")?;
    let store = open_store()?;
    let agents = open_agent_store()?;
    let (ledger, execution) = require_native_code_session(&store, &agents, session)?;
    let diff = checkpoints::diff_checkpoint(&ledger, &execution, checkpoint_id)
        .map_err(|error| store_error("checkpoints diff", checkpoint_id, error))?;
    let value = serde_json::to_value(&diff)
        .map(|value| serde_json::json!({ "session": session, "diff": value }))
        .unwrap_or_else(|_| serde_json::json!({ "session": session }));
    let human = format!(
        "checkpoint: {}\nchanges: {}\npatch:\n{}",
        diff.checkpoint.id,
        diff.changes.len(),
        diff.patch,
    );
    Ok(success(render(output, human, &value)))
}

/// Mirrors `validate_checkpoint_id`.
fn valid_checkpoint_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

/// JSON-level approximation of `code_checkpoints::count_user_turns` (the exact
/// predicate lives behind `deepseek_tui::is_user_turn_prompt`, which the CLI
/// crate cannot reach): user messages that carry no tool-result blocks and are
/// not runtime `<turn_meta>` envelopes. The authoritative count is re-checked
/// inside `truncate_to_user_turn` / `restore_rewound_turns`.
fn approx_user_turns(messages: &serde_json::Value) -> u32 {
    messages
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .filter(|message| {
                    message.get("role").and_then(|value| value.as_str()) == Some("user")
                        && !message
                            .get("content")
                            .and_then(|value| value.as_array())
                            .map(|blocks| {
                                blocks.iter().any(|block| {
                                    matches!(
                                        block.get("type").and_then(|value| value.as_str()),
                                        Some("tool_result")
                                            | Some("tool_search_tool_result")
                                            | Some("code_execution_tool_result")
                                    )
                                })
                            })
                            .unwrap_or(false)
                        && !is_runtime_owned_user_message(message)
                })
                .count() as u32
        })
        .unwrap_or(0)
}

/// Mirror of the engine's runtime-owned predicate (`runtime_handoff`):
/// authority comes only from an engine-shaped `<turn_meta>` block (trailing,
/// or the legacy leading shape with ordinary trailing text) that carries a
/// non-authoritative provenance line. A bare composer envelope (date or
/// workspace metadata without a provenance line) and an authoritative
/// provenance envelope are real user turns; restored subagent checkpoint
/// messages carry a non-authoritative provenance line and are covered by the
/// same check.
fn is_runtime_owned_user_message(message: &serde_json::Value) -> bool {
    fn text_blocks(message: &serde_json::Value) -> Vec<&str> {
        message
            .get("content")
            .and_then(|value| value.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|block| {
                        block.get("type").and_then(|value| value.as_str()) == Some("text")
                    })
                    .filter_map(|block| block.get("text").and_then(|value| value.as_str()))
                    .collect()
            })
            .unwrap_or_default()
    }
    let complete = |text: &str| text.starts_with("<turn_meta>") && text.ends_with("</turn_meta>");
    let blocks = text_blocks(message);
    if blocks.len() < 2 {
        return false;
    }
    let last = blocks[blocks.len() - 1].trim();
    let metadata = if complete(last) {
        last
    } else {
        // Legacy `[metadata, prompt, ...]` shape: complete envelope first and
        // ordinary text last (a single user-authored metadata block is never
        // hidden).
        let first = blocks[0].trim();
        if complete(first) && !complete(last) {
            first
        } else {
            return false;
        }
    };
    // Mirror of `has_non_authoritative_turn_provenance`.
    let mut has_provenance = false;
    let mut condensed_non_authoritative = false;
    let mut legacy_non_authoritative = false;
    for line in metadata.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("Input provenance: ") {
            has_provenance = true;
            condensed_non_authoritative |= value.ends_with(" (non-authoritative)");
        }
        legacy_non_authoritative |= line == "Input authority: non_authoritative";
    }
    condensed_non_authoritative || (has_provenance && legacy_non_authoritative)
}

fn parse_rfc3339_epoch_secs(value: &str) -> Option<i64> {
    // Minimal RFC3339 parser for the rewind sidecar timestamps
    // (`chrono::Utc::now().to_rfc3339()` — the only writer, always `+00:00`),
    // days-from-civil based; the UTC offset is therefore ignored by design.
    // Field ranges are validated so a hand-corrupted sidecar degrades to
    // `None` instead of overflowing the intermediate multiplies.
    let (date, rest) = value.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    let time = rest.split(['+', '-', 'Z']).next()?;
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts
        .next()
        .and_then(|value| value.split('.').next().map(str::to_owned))
        .and_then(|value| value.parse().ok())?;
    if !(1..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // days_from_civil (Howard Hinnant's algorithm).
    let adjusted_year = if month <= 2 { year - 1 } else { year };
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Mirrors `resolve_rewind_plan`: "rewind to turn N" restores the first-created
/// Turn snapshot of turn N+1. Degraded (conversation-only) rewinds are a GUI
/// confirmation variant; the CLI always attempts the restore and errors when
/// the snapshot is missing.
fn resolve_rewind_plan(
    entries: &[checkpoints::CheckpointMeta],
    keep_turns: u32,
) -> Result<checkpoints::CheckpointMeta, CliError> {
    let target_turn = keep_turns + 1;
    entries
        .iter()
        .find(|entry| {
            entry.kind == checkpoints::CheckpointKind::Turn && entry.turn == Some(target_turn)
        })
        .cloned()
        .ok_or_else(|| {
            CliError::failed(format!(
                "checkpoint_missing: no checkpoint for turn {target_turn} (evicted or capture \
                 failed at the time); use a turn that still has a snapshot"
            ))
        })
}

/// `code checkpoints rewind <session> <turn> --yes`: the headless mirror of
/// the GUI `rewind_to_turn` orchestration — stale-snapshot reconciliation,
/// checkpoint restore (auto PreRestore), transcript truncation with the
/// sidecar backup, and invalidation of the abandoned branch snapshots.
/// The GUI's busy gates (EnginePool turn reservation, execution-root mutex)
/// only exist inside the running app; the cross-process advisory lock
/// serializes CLI×CLI mutations, but a GUI turn running on the same session
/// cannot be detected here — do not rewind a session whose GUI Code session
/// may be mid-turn.
fn checkpoints_rewind(
    session: &str,
    keep_turns: u32,
    yes: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    let mut mutation_lock = session_mutation_lock(session)?;
    let _mutation_guard = lock_session_for_mutation(&mut mutation_lock, session, "rewind")?;
    let store = open_store()?;
    let agents = open_agent_store()?;
    let (ledger, execution) = require_native_code_session(&store, &agents, session)?;
    // Second lock keyed by the (canonical) execution root: two CLI processes
    // rewinding *different* sessions bound to the same project directory must
    // not interleave file restores on one working tree.
    let root = canonical_execution_root(&execution);
    let mut root_lock = execution_root_lock(&root)?;
    let _root_guard = lock_root_for_mutation(&mut root_lock, &root, "rewind", session)?;

    // Stale Turn snapshot reconciliation (same source of truth as the GUI).
    match store.rewound_turns_records(session) {
        Ok(records) => {
            for record in records {
                let cutoff = match parse_rfc3339_epoch_secs(&record.rewound_at) {
                    Some(value) => value,
                    None => continue,
                };
                if let Err(error) = checkpoints::invalidate_stale_turn_checkpoints(
                    &ledger,
                    record.kept_turns,
                    cutoff,
                ) {
                    eprintln!(
                        "[pinvou-cli] stale checkpoint reconciliation failed (cleanup only): {error:#}"
                    );
                }
            }
        }
        Err(error) => {
            eprintln!(
                "[pinvou-cli] reading rewind backups failed (reconciliation skipped): {error:#}"
            );
        }
    }

    let entries = checkpoints::list_checkpoints(&ledger)
        .map_err(|error| store_error("checkpoints rewind", session, error))?;
    let loaded = store
        .load(session)
        .map_err(|error| store_error("checkpoints rewind", session, error))?;
    let messages = serde_json::to_value(&loaded.messages)
        .map_err(|error| CliError::failed(format!("code checkpoints rewind: {error}")))?;
    let total_turns = approx_user_turns(&messages);
    if keep_turns >= total_turns {
        return Err(CliError::failed(format!(
            "cannot_rewind: the session currently has {total_turns} turns; cannot rewind past \
             turn {keep_turns}"
        )));
    }
    let checkpoint = resolve_rewind_plan(&entries, keep_turns)?;

    // 1) Restore the code snapshot (restore_checkpoint forces a PreRestore
    //    rollback point and returns it for the undo bookkeeping).
    let undo = checkpoints::restore_checkpoint(&ledger, &execution, &checkpoint.id)
        .map_err(|error| store_error("checkpoints rewind", &checkpoint.id, error))?;
    // 2) Truncate the transcript to the end of turn N (backs up the removed
    //    messages into the rewind sidecar, bound to the PreRestore id).
    let outcome = store
        .truncate_to_user_turn(session, keep_turns, Some(undo.id.clone()))
        .map_err(|error| {
            CliError::failed(format!(
                "code checkpoints rewind: code was restored (original code kept in rollback point \
                 {}), but truncating the transcript failed: {error:#}. Retry the rewind to \
                 complete the truncation",
                undo.id
            ))
        })?;
    // 3) Invalidate the abandoned branch snapshots (best-effort, same as GUI).
    if let Err(error) = checkpoints::invalidate_turn_checkpoints_after(&ledger, keep_turns) {
        eprintln!(
            "[pinvou-cli] invalidating abandoned checkpoints failed (rewind already applied): {error:#}"
        );
    }
    let value = serde_json::json!({
        "session": session,
        "restoredCheckpoint": undo,
        "rewoundTurns": outcome.rewound_turns,
        "degraded": false,
        "hadCompaction": outcome.had_compaction,
    });
    let human = format!(
        "rewound {} to turn {keep_turns}\nrestored checkpoint: {}\nremoved turns: {}\ncompaction residue: {}",
        session,
        undo.id,
        outcome.rewound_turns,
        if outcome.had_compaction { "yes" } else { "no" },
    );
    Ok(success(render(output, human, &value)))
}

/// Undoability probe mirroring `resolve_rewind_undo_state`: latest sidecar
/// record, current turn count equals the kept count, transcript revision
/// matches, and the bound PreRestore checkpoint still exists.
fn resolve_undo_state(
    store: &SessionStore,
    ledger: &Path,
    session: &str,
) -> Result<Option<serde_json::Value>, CliError> {
    let Some(record) = store
        .latest_rewound_turns_record(session)
        .map_err(|error| store_error("checkpoints undo", session, error))?
    else {
        return Ok(None);
    };
    let loaded = store
        .load(session)
        .map_err(|error| store_error("checkpoints undo", session, error))?;
    let messages = serde_json::to_value(&loaded.messages)
        .map_err(|error| CliError::failed(format!("code checkpoints undo: {error}")))?;
    if approx_user_turns(&messages) != record.kept_turns {
        return Ok(None);
    }
    if !record.truncated_revision.is_empty() {
        let revision = pinvou3_lib::features::sessions::transcript_revision(&loaded.messages)
            .map_err(|error| store_error("checkpoints undo", session, error))?;
        if revision != record.truncated_revision {
            return Ok(None);
        }
    }
    let checkpoint_id = match &record.pre_restore_checkpoint_id {
        Some(bound) => {
            let entries = checkpoints::list_checkpoints(ledger)
                .map_err(|error| store_error("checkpoints undo", session, error))?;
            let still_there = entries.iter().any(|entry| {
                entry.kind == checkpoints::CheckpointKind::PreRestore && entry.id == *bound
            });
            if !still_there {
                return Ok(None);
            }
            Some(bound.clone())
        }
        None => None,
    };
    let removed = serde_json::to_value(&record.removed_messages)
        .map_err(|error| CliError::failed(format!("code checkpoints undo: {error}")))?;
    Ok(Some(serde_json::json!({
        "checkpointId": checkpoint_id,
        "keptTurns": record.kept_turns,
        "rewoundTurns": approx_user_turns(&removed),
        "rewoundAt": record.rewound_at,
    })))
}

/// `code checkpoints undo <session>`: headless mirror of `undo_last_rewind`.
fn checkpoints_undo(session: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let mut mutation_lock = session_mutation_lock(session)?;
    let _mutation_guard = lock_session_for_mutation(&mut mutation_lock, session, "undo")?;
    let store = open_store()?;
    let agents = open_agent_store()?;
    let (ledger, execution) = require_native_code_session(&store, &agents, session)?;
    let root = canonical_execution_root(&execution);
    let mut root_lock = execution_root_lock(&root)?;
    let _root_guard = lock_root_for_mutation(&mut root_lock, &root, "undo", session)?;
    let info = resolve_undo_state(&store, &ledger, session)?.ok_or_else(|| {
        CliError::failed(
            "no_undoable_rewind: nothing to undo (no rewind happened, or new turns were created \
             after it)",
        )
    })?;
    let checkpoint_id = info["checkpointId"].as_str().map(str::to_owned);
    if let Some(checkpoint_id) = checkpoint_id.as_deref() {
        checkpoints::restore_checkpoint(&ledger, &execution, checkpoint_id)
            .map_err(|error| store_error("checkpoints undo", checkpoint_id, error))?;
    }
    let restored_messages = store.restore_rewound_turns(session).map_err(|error| {
        CliError::failed(format!(
            "code checkpoints undo({session}): the working tree was already restored to rollback \
             point {}, but restoring the transcript failed: {error:#}. The rewind record was not \
             consumed, so `checkpoints undo` can be retried",
            checkpoint_id.as_deref().unwrap_or("-")
        ))
    })?;
    let value = serde_json::json!({
        "session": session,
        "restoredMessages": restored_messages,
        "restoredCheckpoint": checkpoint_id,
        "info": info,
    });
    let human = format!(
        "undid rewind of {session}: restored {restored_messages} messages{}",
        checkpoint_id
            .as_deref()
            .map(|id| format!(" and code to rollback point {id}"))
            .unwrap_or_default(),
    );
    Ok(success(render(output, human, &value)))
}

// ── permissions / respond (process-local in the product host) ───────────────

fn permissions(session: &str, _output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    require_existing(&store, session, "permissions")?;
    Err(CliError::failed(format!(
        "code_permissions_requires_product_host: pending permissions live in the running app's \
         AcpPool memory for session {session}; no CLI process can observe them. Start the \
         desktop app (or a product-host run) and answer the permission there"
    )))
}

fn respond(
    session: &str,
    request_id: &str,
    allow: bool,
    _output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    require_existing(&store, session, "respond")?;
    let _ = (request_id, allow);
    Err(CliError::failed(format!(
        "code_respond_requires_product_host: permission requests are answered inside the process \
         that owns the ACP connection (the GUI host); the pending store is process-local, so \
         request {request_id} cannot be answered from the CLI"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_version_gate_extracts_the_digit_token_like_the_gui() {
        // Mirror of `runtime::parse_codex_version_output` + `parse_version`:
        // package-prefixed output ("codex-cli 0.146.0", the format the app
        // documents) is accepted; only the digit-headed token counts.
        assert!(version_at_least(
            codex_version_token("codex-cli 0.146.0"),
            "0.144.6"
        ));
        assert!(!version_at_least(
            codex_version_token("codex-cli 0.140.0"),
            "0.144.6"
        ));
        assert!(version_at_least(codex_version_token("0.200.0"), "0.144.6"));
        assert!(version_at_least(codex_version_token("1.0"), "0.144.6"));
        assert!(!version_at_least(codex_version_token("0.14.9"), "0.144.6"));
    }

    #[test]
    fn runtime_owned_predicate_keys_on_provenance_not_envelope_shape() {
        let message =
            |blocks: serde_json::Value| serde_json::json!({ "role": "user", "content": blocks });
        let text = |body: &str| serde_json::json!({ "type": "text", "text": body });
        // Bare composer envelope (date metadata, no provenance line): a real
        // turn, not runtime-owned.
        assert!(!is_runtime_owned_user_message(&message(serde_json::json!(
            [
                text("please fix the bug"),
                text("<turn_meta>\nCurrent local date: 2026-08-12\n</turn_meta>")
            ]
        ))));
        // Non-authoritative provenance (condensed + legacy + restored
        // subagent shapes): runtime-owned.
        assert!(is_runtime_owned_user_message(&message(serde_json::json!(
            [
                text("plan"),
                text(
                    "<turn_meta>\nInput provenance: tool_output (non-authoritative)\n</turn_meta>"
                )
            ]
        ))));
        // Legacy pair shape: a provenance line plus the legacy authority line
        // (the authority line alone is not authority, matching the engine).
        assert!(is_runtime_owned_user_message(&message(serde_json::json!(
            [
                text("plan"),
                text(
                    "<turn_meta>\nInput provenance: restored_context\nInput authority: non_authoritative\n</turn_meta>"
                )
            ]
        ))));
        assert!(!is_runtime_owned_user_message(&message(serde_json::json!(
            [
                text("plan"),
                text("<turn_meta>\nInput authority: non_authoritative\n</turn_meta>")
            ]
        ))));
        assert!(is_runtime_owned_user_message(&message(serde_json::json!(
            [
                text("[Codewhale restored sub-agent checkpoint] ..."),
                text(
                    "<turn_meta>\nInput provenance: subagent_handoff (non-authoritative)\nRestore projection: subagent_checkpoint_v1\n</turn_meta>"
                )
            ]
        ))));
        // Authoritative provenance (external current turn): a real turn.
        assert!(!is_runtime_owned_user_message(&message(serde_json::json!(
            [
                text("go on"),
                text("<turn_meta>\nInput provenance: external_current_turn\n</turn_meta>")
            ]
        ))));
        // Legacy leading envelope with ordinary trailing text and
        // non-authoritative provenance: runtime-owned.
        assert!(is_runtime_owned_user_message(&message(serde_json::json!(
            [
                text(
                    "<turn_meta>\nInput provenance: restored_context\nInput authority: non_authoritative\n</turn_meta>"
                ),
                text("continue")
            ]
        ))));
        // A single user-authored metadata lookalike block is never hidden.
        assert!(!is_runtime_owned_user_message(&message(serde_json::json!(
            [text(
                "<turn_meta>\nInput authority: non_authoritative\n</turn_meta>"
            )]
        ))));
    }

    #[test]
    fn claude_version_gate_takes_first_bare_semver_token() {
        assert!(claude_version_supported("2.1.163 (Claude Code)", "2.0.0"));
        assert!(!claude_version_supported("1.9.0 (Claude Code)", "2.0.0"));
        assert!(!claude_version_supported(
            "2.1.163-rc (Claude Code)",
            "2.0.0"
        ));
        assert!(!claude_version_supported("", "2.0.0"));
    }

    #[test]
    fn kimi_version_gate_requires_bare_semver_output() {
        assert!(kimi_version_supported("0.31.1", "0.9.0"));
        assert!(!kimi_version_supported("kimi 0.31.1", "0.9.0"));
        assert!(!kimi_version_supported("0.31", "0.9.0"));
    }

    #[test]
    fn kimi_credentials_gate_requires_both_tokens_and_positive_expiry() {
        let valid = r#"{"access_token":"a","refresh_token":"r","expires_at":123}"#;
        assert!(kimi_credentials_valid(valid));
        // Either token missing, corrupted JSON, or a negative timestamp.
        assert!(!kimi_credentials_valid(r#"{"access_token":"a"}"#));
        assert!(!kimi_credentials_valid(
            r#"{"access_token":"a","refresh_token":"r","expires_at":-1}"#
        ));
        assert!(!kimi_credentials_valid("access_token refresh_token"));
    }

    #[test]
    fn kimi_config_gate_mirrors_the_gui_resolution_chain() {
        let config = "\
default_model = \"m2\"

[providers.kimi]
type = \"kimi\"
api_key = \"sk\"

[models.m2]
provider = \"kimi\"
model = \"kimi-k2\"
max_context_size = 1000
";
        // Direct api_key / env api_key / valid OAuth each satisfy the gate.
        assert!(kimi_runtime_config_ready(config, false));
        let with_env = config.replace(
            "type = \"kimi\"",
            "type = \"kimi\"\nenv = { KIMI_API_KEY = \"sk\" }",
        );
        assert!(kimi_runtime_config_ready(&with_env, false));
        let oauth = config.replace(
            "type = \"kimi\"",
            "type = \"kimi\"\n[providers.kimi.oauth]\naccount = \"a\"",
        );
        assert!(kimi_runtime_config_ready(&oauth, true));
        assert!(!kimi_runtime_config_ready(&oauth, false));
        // No provider credential at all: not ready even with valid credentials.
        let no_key = config.replace("api_key = \"sk\"", "api_key = \"\"");
        assert!(!kimi_runtime_config_ready(&no_key, true));
        // Broken chains: unknown default model, unknown provider, bad context.
        assert!(!kimi_runtime_config_ready(
            &config.replace("default_model = \"m2\"", "default_model = \"missing\""),
            false
        ));
        assert!(!kimi_runtime_config_ready(
            &config.replace("provider = \"kimi\"", "provider = \"ghost\""),
            false
        ));
        assert!(!kimi_runtime_config_ready(
            &config.replace("max_context_size = 1000", "max_context_size = 0"),
            false
        ));
    }
}
