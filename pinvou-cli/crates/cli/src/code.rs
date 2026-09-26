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
//! - workspace → read-only ops (list/search/preview/changes/branches) call
//!   `features::codex_acp::workspace` directly and serialize its types, so
//!   CLI output is byte-identical to the GUI's. Two pieces stay local: the
//!   per-file/whole-workspace diff mirror (bounded untracked reads, English
//!   copy, whole-workspace composition — differentially pinned against the
//!   app module by a contract test) and `workspace checkout` (cross-process
//!   locks, an explicit `--yes` gate, and commit-identity hardening are
//!   CLI-specific value). Every git call of these lanes runs with the GUI's
//!   environment handling — ambient redirection variables stripped, the
//!   user's `~/.gitconfig`/`/etc/gitconfig` honoured — because they act on
//!   the user's real working tree. Path validation stays a CLI pre-check so
//!   escapes remain usage errors (exit 2).
//! - checkpoints → `features::code_checkpoints` public functions plus the
//!   `SessionStore` rewind sidecar methods, mirroring the
//!   `rewind_to_turn` / `undo_last_rewind` orchestration. User-turn counting
//!   goes through `code_checkpoints::count_user_turns_in_json`, which runs
//!   the engine's exact `is_user_turn_prompt` predicate over the transcript
//!   JSON (the CLI cannot depend on the foundation crate directly, so the
//!   shared entry point lives in the app crate — no local approximation to
//!   drift).
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
//! single serde_json line mirroring the GUI DTOs' own serialization (their
//! field casing varies — do not assume camelCase).

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::support::{read_text_file_capped, render, require_yes, resolve_secret, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::code_checkpoints as checkpoints;
use pinvou3_lib::features::codex_acp::workspace;
use pinvou3_lib::features::codex_acp::{
    AcpPool, AcpProvidersView, AgentBackend, CodexWorkspaceKind, MIN_CLAUDE_VERSION,
    MIN_CODEX_VERSION, MIN_KIMI_VERSION, ProviderManager, ProviderWireApi, SessionAgentStore,
};
use pinvou3_lib::features::sessions::{SessionKind, SessionStore};
use pinvou3_lib::platform::credential_store::{CredentialEditAction, SystemCredentialStore};
use pinvou3_lib::platform::paths;
use wait_timeout::ChildExt;

const USAGE: &str = "usage: pinvou code <agents|login|logout|providers|sessions|workspace|checkpoints|run|permissions|respond> <subcommand>";

const AGENTS_USAGE: &str =
    "usage: pinvou code agents <list|status <agent>|install <agent>>  (agent: codex|claude|kimi)";
const LOGIN_USAGE: &str = "usage: pinvou code login <agent> [--code C|--code-env VAR|--code-stdin]  \
     (agent: codex|claude|kimi; the claude flow consumes an authorization code)";
const LOGOUT_USAGE: &str = "usage: pinvou code logout <agent> --yes  (agent: codex|claude|kimi)";
const PROVIDERS_USAGE: &str = "usage: pinvou code providers <list [--agent A]|add --agent A --name N --base-url U \
     [--wire-api anthropic|openai|kimi (aliases: openai_compatible|chat)] [--model M] [--model-slot SLOT=M]... [--context-window N] \
     (--api-key-env V|--api-key-stdin)|update <id> --agent A [--model M] [--model-slot SLOT=M]... [--context-window N] \
     (--api-key-env V|--api-key-stdin|--delete-key --yes)|remove <id> --agent A --yes \
     |switch <agent> <provider-id>|switch-official <agent>|export --agent A [--output PATH] \
     |import --agent A <PATH>|probe <provider-id> --agent A>  \
     (claude --model-slot SLOT: opus|sonnet|haiku|fable|subagent; add requires all five, \
     a missing slot falls back to official traffic)";
const SESSIONS_USAGE: &str = "usage: pinvou code sessions <list|info <id>|timeline <id>>";
const WORKSPACE_USAGE: &str = "usage: pinvou code workspace <list <session> [path]|search <session> Q|preview <session> FILE|changes <session>|diff <session> [FILE]|branches <session>|checkout <session> BRANCH --mode carry|stash|commit [--message M] --yes>";
const CHECKPOINTS_USAGE: &str = "usage: pinvou code checkpoints <list <session>|diff <session> <checkpoint-id>|rewind <session> <turn> --yes|undo <session> --yes>";
const RUN_USAGE: &str = "usage: pinvou code run <agent> --workspace DIR (--prompt-file F|--prompt S) [--timeout-secs N]";
const PERMISSIONS_USAGE: &str = "usage: pinvou code permissions <session>";
const RESPOND_USAGE: &str = "usage: pinvou code respond <session> <request-id> <allow|deny>";

/// Minimum agent CLI versions enforced by the GUI runtime probes
/// (`features::codex_acp`): codex via `runtime::MIN_CODEX_VERSION`, claude/kimi
/// via `codex_acp::{MIN_CLAUDE_VERSION, MIN_KIMI_VERSION}`.
///
/// Direct references, not a mirror — the CLI consumes the same `pub` constants
/// the app's runtime probes and install gates use, so an app-side bump breaks
/// this build instead of silently diverging (the same discipline as
/// `workspace::{SEARCH_LIMIT, PREVIEW_LIMIT, DIFF_LIMIT}`).
const MIN_VERSIONS: [(&str, &str); 3] = [
    ("codex", MIN_CODEX_VERSION),
    ("claude", MIN_CLAUDE_VERSION),
    ("kimi", MIN_KIMI_VERSION),
];

/// Mirror of `providers::CLAUDE_MODEL_SLOTS`
/// (`pinvou3-app/src-tauri/src/features/codex_acp/providers/mod.rs`), slot ids
/// only. `ProviderManager::save` requires a non-empty model for every one of
/// them when it stores a claude provider; a missing slot makes Claude Code's
/// sub-agent and helper calls fall back to official models (official
/// traffic), which is why the store treats them as mandatory rather than
/// optional. The app constant is `pub(crate)` and unreachable from this
/// crate, so the ids are mirrored here and must be kept in step with it: a
/// slot added there without a matching entry here would slip past this
/// family's English pre-check and surface the store's untranslated
/// "<slot> is a required field" message through `store_error` — the same
/// translation-boundary rule the marketplace importer follows.
const CLAUDE_MODEL_SLOTS: [&str; 5] = ["opus", "sonnet", "haiku", "fable", "subagent"];

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
        yes: bool,
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
        yes: bool,
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
        yes: bool,
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
        yes: bool,
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
            let sources = [
                option(&options, "--code").is_some(),
                option(&options, "--code-env").is_some(),
                booleans.iter().any(|flag| *flag == "--code-stdin"),
            ];
            if sources.iter().filter(|present| **present).count() > 1 {
                return Err(CliError::usage(
                    "use only one of --code, --code-env, or --code-stdin",
                ));
            }
            let code = if sources[0] {
                Some(LoginCodeSource::Arg(
                    option(&options, "--code").unwrap_or_default().to_owned(),
                ))
            } else if sources[1] {
                Some(LoginCodeSource::Env(
                    option(&options, "--code-env")
                        .unwrap_or_default()
                        .to_owned(),
                ))
            } else if sources[2] {
                Some(LoginCodeSource::Stdin)
            } else {
                None
            };
            Ok(CodeCommand::Login { agent, code })
        }
        "logout" => {
            let agent = require_agent(rest.first().map(String::as_str), LOGOUT_USAGE)?;
            let (_, flags, _) = parse_flags(&rest[1..], &[], &["--yes"], "logout")?;
            Ok(CodeCommand::Logout {
                agent,
                yes: flags.contains(&"--yes"),
            })
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
                &["--api-key-stdin", "--delete-key", "--yes"],
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
                yes: flags.contains(&"--yes"),
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
    // Every workspace subcommand addresses one session first; enforce the
    // shared id shape here so an invalid id is a usage error (exit 2), the
    // same contract `require_session_id` enforces everywhere else. Deep
    // existence validation stays with the store.
    if let Some(session) = rest.get(1) {
        if !crate::support::valid_session_id(session) {
            return Err(CliError::usage("invalid session id"));
        }
    }
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
            let (options, flags, _) = parse_flags(
                &rest[3..],
                &["--mode", "--message"],
                &["--yes"],
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
            // `--yes` parses here but is enforced at execute level (exit 2 via
            // `require_yes`), like `checkpoints rewind`/`undo`: a caller that
            // forgot it gets the same "pass --yes to confirm" message for every
            // destructive command instead of a per-command usage string.
            Ok(CodeCommand::WorkspaceCheckout {
                session,
                branch,
                mode,
                message,
                yes: flags.contains(&"--yes"),
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
            let (_, flags, _) = parse_flags(&rest[2..], &[], &["--yes"], "checkpoints undo")?;
            Ok(CodeCommand::CheckpointsUndo {
                session,
                yes: flags.contains(&"--yes"),
            })
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
        // `--message` is the one flag allowed a `--`-prefixed value, because a
        // commit message may legitimately start with `--`. That exemption also
        // makes `--message --yes` swallow the confirmation flag, and the
        // failure then surfaces much later as the generic "pass --yes to
        // confirm this destructive action" — which names neither `--message`
        // nor the flag it ate. Rejecting only the exact case where the value
        // IS one of this command's own boolean flags is the smaller fix than
        // dropping the exemption: every legitimate `--`-prefixed message keeps
        // working, and the one ambiguous spelling gets told what happened.
        reject_swallowed_boolean_flag(token, value, boolean_flags, label)?;
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

/// Guard for the `--message` value exemption shared by the two splitters: a
/// `--`-prefixed message is legal, but when the value is verbatim one of this
/// command's own boolean flags the user almost certainly meant to pass the
/// flag, not to name their commit after it. Say so instead of consuming it.
fn reject_swallowed_boolean_flag(
    token: &str,
    value: &str,
    boolean_flags: &[&str],
    label: &str,
) -> Result<(), CliError> {
    if token == "--message" && boolean_flags.contains(&value) {
        return Err(CliError::usage(format!(
            "code {label} option --message consumed {value} as its message text, so {value} \
             was never applied; move {value} before --message (a message that must read \
             exactly \"{value}\" is not expressible here)"
        )));
    }
    Ok(())
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
            // Same `--message` exemption, same swallowed-flag hazard as
            // `parse_flags` above; kept in step so a future subcommand that
            // routes `--message` through the positional splitter inherits the
            // guard instead of the confusing late failure.
            reject_swallowed_boolean_flag(token, value, boolean_flags, label)?;
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
    if !crate::support::valid_session_id(id) {
        return Err(CliError::usage("invalid session id"));
    }
    Ok(id.to_owned())
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
        CodeCommand::Logout { agent, yes } => logout(&agent, yes, output),
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
            // `add` never deletes a key (delete_key=false above), so the
            // update-lane --yes gate would never fire; pass false and let
            // require_yes see a non-destructive call.
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
            yes,
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
            yes,
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
            yes,
        } => {
            // The confirmation gate runs before the advisory locks, exactly
            // like `checkpoints_rewind`/`checkpoints_undo` run it before
            // theirs: an unconfirmed invocation must not create lock files,
            // and must not be able to answer `checkout_busy` (exit 1) when
            // another process holds the lock instead of the usage error.
            require_yes(yes)?;
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
        CodeCommand::CheckpointsUndo { session, yes } => checkpoints_undo(&session, yes, output),
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
    // Same absolute-path contract as the other families: a relative
    // PINVOU3_HOME would silently resolve against the cwd.
    crate::support::sandbox_home()?;
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
    // Same absolute-path contract as `open_store`: the provider store
    // resolves through `pinvou3_home()` and would silently land in a
    // cwd-relative directory.
    crate::support::sandbox_home()?;
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
    /// Why `version` is None even though a binary was found — a genuinely
    /// too-old version is a different state from a probe that never
    /// returned.
    version_probe_failed: bool,
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

/// Mirror of `install::agent_cli_names`: the per-agent Windows candidate
/// order — codex's npm `.cmd` shim wins there, the native runtimes prefer
/// their `.exe`; Unix installs a single unadorned name.
fn agent_cli_names<'a>(agent: &str, name: &'a str) -> Vec<std::borrow::Cow<'a, str>> {
    if cfg!(windows) {
        match agent {
            "codex" => vec!["codex.cmd".into(), "codex.exe".into()],
            "claude" => vec!["claude.exe".into(), "claude.cmd".into()],
            "kimi" => vec!["kimi.exe".into(), "kimi.cmd".into()],
            _ => vec![name.into()],
        }
    } else {
        vec![name.into()]
    }
}

/// Candidate-major PATH scan (each candidate name across every PATH dir
/// before the next), matching `install::find_agent_cli_in_path` — a
/// dir-major scan would pick dir1/codex.exe where the GUI picks dir2's
/// codex.cmd.
fn find_in_path(agent: &str, name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for candidate in agent_cli_names(agent, name) {
        let candidate = PathBuf::from(candidate.as_ref());
        if let Some(found) = std::env::split_paths(&path)
            .map(|dir| dir.join(&candidate))
            .find(|candidate| nonempty_file(candidate))
        {
            return Some(found);
        }
    }
    None
}

/// Mirror of the app's per-agent resolution order (`install::resolve_*` /
/// `runtime::probe_codex_runtime`): explicit override env vars and official
/// install locations win over PATH so a stale binary earlier in PATH cannot
/// shadow the real one (the app prefers `~/.kimi-code/bin/kimi` over PATH for
/// exactly that reason). Only codex gates its override through the
/// compatibility gate; claude/kimi overrides win unconditionally, like the
/// GUI. The app's adapter-beside claude runtime location is app-bundle-specific
/// and not mirrored here.
fn resolve_agent_cli(agent: &str, name: &str) -> Option<PathBuf> {
    let override_var = match agent {
        "codex" => Some("PINVOU3_CODEX_PATH"),
        "claude" => Some("PINVOU3_CLAUDE_CLI_PATH"),
        // Mirror of `install::resolve_kimi_path`, which checks the override
        // first.
        "kimi" => Some("PINVOU3_KIMI_ACP_BIN"),
        _ => None,
    };
    if let Some(var) = override_var {
        if let Some(path) = std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|path| nonempty_file(&path))
        {
            // Codex is the only agent whose GUI resolution gates the
            // override through the compatibility check
            // (`runtime::probe_codex_runtime`); claude
            // (`PINVOU3_CLAUDE_CLI_PATH`) and kimi (`PINVOU3_KIMI_ACP_BIN`)
            // honor a nonempty override file unconditionally
            // (`install::resolve_claude_cli`/`resolve_kimi_path`). Gating
            // them here made the CLI silently fall back to a different
            // binary than the GUI operates.
            if agent != "codex" || override_passes_version_gate(agent, &path) {
                return Some(path);
            }
        }
    }
    let home = pinvou3_lib::platform::paths::user_home_dir();
    // Script-installed managed dirs before PATH, per agent, like the GUI's
    // resolve_* functions. `~/.local/bin` is the Unix codex/claude installer
    // default and `~\.local\bin` the Windows claude installer default
    // (install.rs checks it there on Windows too); codex on Windows resolves
    // through the official install below.
    let managed_dir: Option<PathBuf> = if agent == "kimi" {
        Some(home.join(".kimi-code").join("bin"))
    } else if agent == "codex" || agent == "claude" {
        {
            #[cfg(target_os = "windows")]
            {
                if agent == "claude" {
                    Some(home.join(".local").join("bin"))
                } else {
                    None
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                Some(home.join(".local").join("bin"))
            }
        }
    } else {
        None
    };
    if let Some(dir) = managed_dir {
        if let Some(path) = agent_cli_names(agent, name)
            .iter()
            .map(|candidate| dir.join(candidate.as_ref()))
            .find(|candidate| nonempty_file(candidate))
        {
            return Some(path);
        }
    }
    #[cfg(target_os = "windows")]
    if agent == "codex" {
        // Mirror of `codex_official_install_path` (windows.rs): the official
        // Windows install lives under %LOCALAPPDATA% (falling back to
        // ~\AppData\Local) and is deliberately not PATH-dependent, so a
        // script-installed codex is found without a shell restart. The
        // installer writes a fixed codex.exe there.
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Local"));
        let path = local
            .join("Programs")
            .join("OpenAI")
            .join("Codex")
            .join("bin")
            .join("codex.exe");
        if nonempty_file(&path) {
            return Some(path);
        }
    }
    find_in_path(agent, name)
}

/// Runs `executable args...` with a hard timeout. `Err` = the process could
/// not be spawned at all; `Ok(None)` = no exit within the budget;
/// `Ok(Some(..))` = exited, with its trimmed stdout.
fn command_output_with_timeout(
    executable: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<Option<(bool, String)>, std::io::Error> {
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
    crate::support::set_process_group(&mut command);
    let mut child = command.spawn()?;
    let stdout = child.stdout.take();
    // The reader hands its buffer back through a channel so the wait stays
    // bounded: a vendor CLI's grandchild can inherit the pipe and outlive
    // the reaped child, and an unbounded `join()` there would hang this
    // probe well past its deadline.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(drain_stream(stdout));
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // The child is reaped; whatever still holds the pipe gets a
                // short grace before the probe reports whatever arrived.
                // Straggler descendants are deliberately left alone: killing
                // a reaped child's group would race pid reuse, and this
                // one-shot process exits right after the probe anyway.
                let text = rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
                return Ok(Some((status.success(), text.trim().to_string())));
            }
            Ok(None) => {}
            Err(_) => {
                crate::support::kill_process_tree(&mut child);
                return Ok(None);
            }
        }
        if Instant::now() >= deadline {
            // The bare `kill()` this site used before orphans process-group
            // descendants; every other spawn site goes through the tree kill.
            crate::support::kill_process_tree(&mut child);
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Drains a byte stream into a string (bounded tail: the buffer keeps the
/// last 64 KiB so a chatty vendor CLI cannot grow it without limit). The
/// bytes are decoded once at the end: converting per chunk mangles a
/// multi-byte character split across a 2 KiB read boundary into U+FFFD.
fn drain_stream<R: Read>(mut pipe: Option<R>) -> String {
    let mut bytes: Vec<u8> = Vec::new();
    let Some(pipe) = pipe.as_mut() else {
        return String::new();
    };
    let mut chunk = [0u8; 2048];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                bytes.extend_from_slice(&chunk[..read]);
                if bytes.len() > 65_536 {
                    // Byte-count eviction can land inside a multi-byte
                    // character; advance to the next UTF-8 boundary (any
                    // non-continuation byte starts a character) so the
                    // buffer never starts mid-character.
                    let mut cut = bytes.len() - 65_536;
                    while cut < bytes.len() && (bytes[cut] & 0xC0) == 0x80 {
                        cut += 1;
                    }
                    bytes.drain(..cut);
                }
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Outcome of a `--version` probe: a parsed version, or the reason there is
/// none. The GUI distinguishes probe failure/timeout from a genuinely
/// too-old version (and retries timeouts); the CLI at least must not label
/// a cold or hanging binary "version-too-old".
enum VersionProbe {
    Version(String),
    /// The binary exited non-zero, could not be spawned, or produced no
    /// usable output.
    Failed,
    /// The binary did not finish within the probe budget.
    TimedOut,
}

fn probe_cli_version(executable: &Path) -> VersionProbe {
    match command_output_with_timeout(executable, &["--version"], Duration::from_secs(15)) {
        // The app treats empty output as a failed probe, not as a version.
        Ok(Some((true, text))) if !text.trim().is_empty() => VersionProbe::Version(text),
        Ok(Some(_)) | Err(_) => VersionProbe::Failed,
        Ok(None) => VersionProbe::TimedOut,
    }
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

/// Per-agent compatibility gate over probed version output — the single
/// comparison used by `agents status` and by the override resolution.
fn version_supported_for(agent: &str, version: &str, minimum: &str) -> bool {
    match agent {
        "codex" => version_at_least(codex_version_token(version), minimum),
        "claude" => claude_version_supported(version, minimum),
        "kimi" => kimi_version_supported(version, minimum),
        _ => false,
    }
}

/// An override binary must clear the same compatibility gate as any other
/// candidate (mirror of the GUI's `runtime_version_is_compatible` filter on
/// overrides).
/// Mirror of `install::nonempty_file`: the GUI's candidate checks require a
/// readable file with content, not mere existence (a 0-byte stub is not a
/// usable binary).
fn nonempty_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.len() > 0)
        .unwrap_or(false)
}

fn override_passes_version_gate(agent: &str, path: &Path) -> bool {
    let version = match probe_cli_version(path) {
        VersionProbe::Version(version) => version,
        VersionProbe::Failed | VersionProbe::TimedOut => return false,
    };
    let minimum = MIN_VERSIONS
        .iter()
        .find(|(id, _)| *id == agent)
        .map(|(_, min)| *min);
    match minimum {
        Some(minimum) => version_supported_for(agent, &version, minimum),
        None => true,
    }
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
    matches!(
        command_output_with_timeout(executable, args, Duration::from_secs(15)),
        Ok(Some((true, _)))
    )
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
    let probe = cli_path.as_deref().map(probe_cli_version);
    let version = match &probe {
        Some(VersionProbe::Version(text)) => Some(text.clone()),
        _ => None,
    };
    let min_version = MIN_VERSIONS
        .iter()
        .find(|(id, _)| *id == agent)
        .map(|(_, min)| *min)
        .unwrap_or("0.0.0");
    let version_supported = version
        .as_deref()
        .map(|version| version_supported_for(agent, version, min_version))
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
        version_probe_failed: matches!(
            probe,
            Some(VersionProbe::Failed) | Some(VersionProbe::TimedOut)
        ),
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
        // Distinguishes "probe failed/hung" from a genuinely too-old version
        // for JSON consumers (`version: null` alone is ambiguous).
        "version_probe_failed": probe.version_probe_failed,
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
                } else if probe.cli_path.is_some() && probe.version_probe_failed {
                    // A cold binary, an unprobing one, or a hang is not the
                    // same fact as a too-old version (the GUI distinguishes
                    // TimedOut/Failed from Found and retries timeouts).
                    "probe-failed"
                } else if probe.cli_path.is_some() {
                    "version-too-old"
                } else {
                    "not-installed"
                },
                crate::support::collapse_control_characters(
                    probe.version.as_deref().unwrap_or("-"),
                ),
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
        crate::support::collapse_control_characters(probe.version.as_deref().unwrap_or("-")),
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

/// Exact-value strip of the authorization code this process wrote to the
/// child's stdin, applied to the captured transcript before the heuristic
/// redaction pass: a short, non-secret-shaped code the vendor CLI echoed
/// back would survive `redact_secret` alone. The value is trimmed the same
/// way the stdin write trims it, so the echoed copy matches byte for byte.
fn strip_login_code<'a>(combined: &'a str, code: Option<&str>) -> std::borrow::Cow<'a, str> {
    match code {
        Some(value) if !value.trim().is_empty() => {
            combined.replace(value.trim(), "[REDACTED]").into()
        }
        _ => combined.into(),
    }
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

/// Which login pipe a drain event came from; the two transcripts are
/// concatenated in a fixed order, so they cannot be pooled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoginStream {
    Stdout,
    Stderr,
}

/// The two things a user must act on while the vendor CLI is still running.
#[derive(Debug)]
enum LoginArtifact {
    Url(String),
    Code(String),
}

/// What a login drain thread reports: artefacts the moment they appear in the
/// pipe, then the full captured transcript once that pipe reaches EOF.
#[derive(Debug)]
enum LoginDrainEvent {
    Artifact(LoginArtifact),
    Finished(LoginStream, String),
}

/// Incremental drain for the login pipes. It keeps everything `drain_stream`
/// guarantees for the transcript — a bounded 64 KiB tail with UTF-8-safe
/// eviction, decoded once at the end so no multi-byte character split across
/// a read boundary becomes U+FFFD — and adds a scan of each newly completed
/// line for the agent's login URL and device code, pushed onto `tx` at once.
/// Without this the whole point of `code login` was unreachable: the vendor
/// CLI holds the flow open until the user opens the link, but the link sat
/// unread in this buffer for the entire 600 s (kimi 1800 s) wait.
///
/// Structural mirror of `connectors::drain_for_url` (detached reader thread,
/// artefacts on an mpsc channel, each emitted at most once, the consumer
/// announcing them on stderr so `--output json` keeps stdout a single line).
/// The two cannot share one helper: that family matches a per-connector auth
/// domain allow-list line by line, while this family's extractors are
/// agent-aware whole-text scans (`extract_login_url` / `extract_device_code`)
/// that must run over a text slice, not a line.
///
/// Nothing else from the stream is streamed live — only the URL and the code
/// — and the authorization code this process wrote to the child's stdin is
/// stripped from every scanned slice before matching, so a vendor CLI that
/// echoes it back cannot get it re-emitted as a "device code".
fn spawn_login_drain<R: Read + Send + 'static>(
    agent: String,
    stream: LoginStream,
    pipe: Option<R>,
    code: Option<String>,
    tx: std::sync::mpsc::Sender<LoginDrainEvent>,
) {
    std::thread::spawn(move || {
        let Some(mut pipe) = pipe else {
            let _ = tx.send(LoginDrainEvent::Finished(stream, String::new()));
            return;
        };
        let mut bytes: Vec<u8> = Vec::new();
        // Line-assembly buffer for the live scan, separate from the capped
        // transcript tail above: a URL or a code prompt can straddle a 2 KiB
        // read boundary, so only whole lines are ever scanned — emitting a
        // truncated authorize link would be worse than emitting none. A
        // newline can never fall inside a multi-byte character, so the lossy
        // decode of a completed-lines slice is exact.
        let mut scan: Vec<u8> = Vec::new();
        let mut url_sent = false;
        let mut code_sent = false;
        let mut chunk = [0u8; 2048];
        loop {
            let read = match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            bytes.extend_from_slice(&chunk[..read]);
            if bytes.len() > 65_536 {
                // Byte-count eviction can land inside a multi-byte character;
                // advance to the next UTF-8 boundary (any non-continuation
                // byte starts a character) so the buffer never starts
                // mid-character.
                let mut cut = bytes.len() - 65_536;
                while cut < bytes.len() && (bytes[cut] & 0xC0) == 0x80 {
                    cut += 1;
                }
                bytes.drain(..cut);
            }
            if url_sent && code_sent {
                // Both artefacts are out; the rest of this pipe is transcript
                // only, so skip the scan entirely.
                continue;
            }
            scan.extend_from_slice(&chunk[..read]);
            let Some(end) = scan.iter().rposition(|byte| *byte == b'\n') else {
                if scan.len() > 65_536 {
                    // A newline-free flood must not grow this buffer without
                    // limit. The transcript tail above still carries the
                    // bytes, so the post-exit extraction can still find an
                    // artefact this live scan gave up on.
                    scan.clear();
                }
                continue;
            };
            let completed: Vec<u8> = scan.drain(..=end).collect();
            let text = String::from_utf8_lossy(&completed);
            let text = strip_login_code(&text, code.as_deref());
            let mut artifacts: Vec<LoginArtifact> = Vec::new();
            if !url_sent && let Some(found) = extract_login_url(&agent, &text) {
                url_sent = true;
                // The device code frequently rides in the link itself
                // (`...authorize_device?user_code=...`); take it from
                // there rather than waiting for a separate prompt line
                // the vendor may never print.
                let from_url = extract_device_code("", Some(found.as_str()));
                artifacts.push(LoginArtifact::Url(found));
                if let Some(device) = from_url {
                    code_sent = true;
                    artifacts.push(LoginArtifact::Code(device));
                }
            }
            if !code_sent && let Some(device) = extract_device_code(&text, None) {
                code_sent = true;
                artifacts.push(LoginArtifact::Code(device));
            }
            // A closed channel means the wait loop is already gone, so there
            // is nobody left to report to — stop reading rather than keep
            // filling a buffer nothing will read.
            let mut consumer_gone = false;
            for artifact in artifacts {
                if tx.send(LoginDrainEvent::Artifact(artifact)).is_err() {
                    consumer_gone = true;
                    break;
                }
            }
            if consumer_gone {
                break;
            }
        }
        let _ = tx.send(LoginDrainEvent::Finished(
            stream,
            String::from_utf8_lossy(&bytes).into_owned(),
        ));
    });
}

/// Folds one drain event into the login wait state. Artefacts are announced
/// on stderr the first time they are seen — stdout and stderr can both carry
/// the link, and vendor CLIs repeat it, so the `is_none()` guards are what
/// make "emit each artefact once" hold across both pipes and both channels.
fn apply_login_event(
    event: LoginDrainEvent,
    streamed_url: &mut Option<String>,
    streamed_code: &mut Option<String>,
    transcripts: (&mut String, &mut String),
    finished: &mut usize,
) {
    let (out_text, err_text) = transcripts;
    match event {
        LoginDrainEvent::Artifact(LoginArtifact::Url(found)) => {
            if streamed_url.is_none() {
                note!("login link: {found}");
                *streamed_url = Some(found);
            }
        }
        LoginDrainEvent::Artifact(LoginArtifact::Code(found)) => {
            if streamed_code.is_none() {
                note!("device code: {found}");
                *streamed_code = Some(found);
            }
        }
        LoginDrainEvent::Finished(LoginStream::Stdout, text) => {
            *out_text = text;
            *finished += 1;
        }
        LoginDrainEvent::Finished(LoginStream::Stderr, text) => {
            *err_text = text;
            *finished += 1;
        }
    }
}

/// Reads the `--code-stdin` authorization code, once the wait loop has
/// signalled that the login URL is on this terminal (the interactive lane's
/// deferred read). Identical cap and validation to the pre-spawn read the
/// `--code`/`--code-env` lanes perform up front: bounded like
/// `resolve_secret`, an unbounded stdin read lets `yes | pinvou code login
/// claude --code-stdin` exhaust memory before the 4096-char validity check
/// ever runs. Runs inside the stdin writer thread, so its errors are
/// reported as a note the way the drains report theirs — there is no
/// outcome left to fail by then; the login either completes on the
/// already-known code or times out like any other stuck flow.
fn read_deferred_login_code() -> Option<String> {
    let mut raw = String::new();
    {
        use std::io::Read;
        let mut bounded = std::io::stdin().take(64 * 1024 + 1);
        if let Err(error) = bounded.read_to_string(&mut raw) {
            note!("code login: cannot read code from stdin: {error}");
            return None;
        }
    }
    if raw.len() > 64 * 1024 {
        note!("code login: stdin authorization code exceeds the 64 KiB read cap");
        return None;
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 4096 || trimmed.chars().any(char::is_control) {
        note!("code login: invalid claude authorization code");
        return None;
    }
    Some(raw)
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
/// `login_acp_agent` runs, buffers its output, extracts the allow-listed
/// authorization URL / device code, and waits for the flow to finish
/// (bounded like the GUI: 600s, kimi 1800s).
///
/// The URL and the device code are announced on stderr the moment the vendor
/// CLI prints them, not when it exits: the child stays alive precisely until
/// the user opens the link (kimi's device-code flow cannot be completed
/// otherwise), so a link published only at exit is published too late. Only
/// those two artefacts stream live; the rest of the transcript is still
/// echoed once, redacted, at the end. A timeout still surfaces the login link
/// captured so far — the URL is the only actionable part of the transcript.
///
/// The claude authorization code is accepted via `--code-env VAR` /
/// `--code-stdin` (plaintext argv is deliberately not offered — argv leaks
/// through shell history and process listings; `--code C` remains for callers
/// that already hold it in argv) and is written to the child's stdin; the
/// child reads it when it prompts.
///
/// The two `--code`/`--code-env` lanes are for callers that already hold the
/// code, so their value is validated before anything is spawned. The
/// `--code-stdin` lane is the interactive one, and it waits: in a real claude
/// flow the user can only hold the code after this CLI has printed the
/// authorize URL (round-18 finding: the code used to be read before the child
/// even existed — before any URL could be visible — and stdin was closed
/// right after). The wait loop therefore signals the stdin writer thread the
/// moment it announces the URL on stderr; only then is the invoker's stdin
/// read (same 64 KiB cap, same shape validation) and forwarded to the child.
/// The child's stdin stays open for its whole lifetime after that one write —
/// the GUI keeps the login child's stdin open through the entire login
/// (`login_inputs` is only cleared when the login future finishes), instead
/// of delivering an EOF mid-flow.
///
/// The login child runs in its own process group (see
/// `support::set_process_group`), and its group is registered with
/// `support::supervise` for the child's whole lifetime so an interrupt that
/// kills this CLI takes the login child down with it instead of orphaning a
/// kimi login that can legitimately run to 1800 s; the group is forgotten
/// once this flow has waited for (or killed) the child, the same bracket
/// every supervised spawn site uses.
fn login(
    agent: &str,
    code: Option<LoginCodeSource>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    // Resolve the code lane as far as it can be before the child exists.
    // `--code`/`--code-env` are for callers that already hold the code, so
    // their value resolves (and is validated) up front — a missing or empty
    // env var must fail before spawning anything. `--code-stdin` is the
    // interactive lane: the user cannot hold the code before the child has
    // printed the authorize URL (round-18 finding: the pre-spawn read
    // consumed the invoker's stdin before any URL existed), so its value
    // stays deferred — the wait loop below hands it to the stdin writer
    // thread only after the URL has been announced on stderr.
    let (code, stdin_code_deferred): (Option<String>, bool) = match code {
        Some(LoginCodeSource::Arg(raw)) => (Some(raw), false),
        Some(LoginCodeSource::Env(var)) => {
            let value = std::env::var(&var).map_err(|_| {
                CliError::failed(format!(
                    "code login: authorization code environment variable {var} is not set"
                ))
            })?;
            if value.trim().is_empty() {
                return Err(CliError::failed(format!(
                    "code login: authorization code environment variable {var} is empty"
                )));
            }
            (Some(value), false)
        }
        Some(LoginCodeSource::Stdin) => {
            // Deferred: no byte of the invoker's stdin may be consumed before
            // the URL artifact has been announced on stderr. The read (same
            // 64 KiB cap, same validation) then runs inside the stdin writer
            // thread, so the deadline loop below is never parked by it — the
            // same unbounded-read hazard the pre-spawn version posed to the
            // `yes | …` case, just relocated.
            (None, true)
        }
        None => (None, false),
    };
    if (code.is_some() || stdin_code_deferred) && agent != "claude" {
        return Err(CliError::usage(format!(
            "code login {agent} does not accept an authorization code; only the claude \
             login flow consumes one"
        )));
    }
    if let Some(code) = code.as_deref() {
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
    // The kill guard: `set_process_group` above put the child in its own
    // group, and that is exactly what orphans it if this CLI dies without
    // this flow's own exit paths running (kimi can legitimately run to
    // 1800 s; round-18 finding) — `supervise` forwards a terminal SIGINT to
    // the group while this login flow is alive. Every exit below pairs the
    // registration with a forget, so the pgid is never left registered for
    // the OS to recycle onto an unrelated process (the same bracket
    // `support/supervise.rs` documents for spawn sites).
    crate::support::supervise::register_child_group(child.id());
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // The drains report through a channel instead of join handles: a
    // grandchild (a browser or helper the vendor CLI spawned) can inherit
    // the pipes and outlive the reaped child, and an unbounded `join()`
    // here would hang the CLI after login finished — the same hazard the
    // auth probe bounds one room over. The threads stay unjoined for exactly
    // that reason; the channel is what bounds the wait.
    //
    // The channel carries artefacts as well as the two final transcripts, so
    // the deadline loop below can surface the authorize link and the device
    // code while the child is still waiting on the user (see
    // `spawn_login_drain`).
    let (events_tx, events_rx) = std::sync::mpsc::channel::<LoginDrainEvent>();
    spawn_login_drain(
        agent.to_owned(),
        LoginStream::Stdout,
        stdout,
        code.clone(),
        events_tx.clone(),
    );
    spawn_login_drain(
        agent.to_owned(),
        LoginStream::Stderr,
        stderr,
        code.clone(),
        events_tx,
    );
    // The stdin writer runs on its own thread so it can never park the
    // deadline loop below: the code is up to the GUI's 4096-char max, which
    // exceeds the 4 KiB Windows pipe buffer, so a child that never reads
    // stdin would block the write indefinitely. The thread is deliberately
    // not joined — if the write is still parked when the deadline (or the
    // child's own exit) closes the pipe, the write fails with EPIPE and the
    // thread exits on its own.
    //
    // With a `--code`/`--code-env` value the write happens immediately (the
    // caller already held the code when it invoked the CLI). The
    // `--code-stdin` lane instead parks on `code_gate` until the wait loop
    // has announced the URL on stderr (two-phase semantics, the same order
    // the GUI's `submit_agent_login_code` enforces): the user cannot hold
    // the code before seeing the URL, so the child's stdin sees bytes only
    // after the URL is on this terminal. The gate's channel — not the
    // invoker's stdin — is what the writer waits on, so an invoker that
    // never sends the code stalls in the deadline like every other stuck
    // flow instead of blocking a pipe read the deadline cannot bound.
    //
    // stdin EOF timing is agent-scoped like the GUI's, whose `run_agent_login`
    // drops the child's stdin immediately for every backend except claude
    // and holds claude's open through the whole login (round-18 finding's
    // "stdin is closed afterwards" half: a writer thread that returns after
    // the write delivers an EOF mid-flow). So claude's writer parks on
    // `stdin_park` after its write and the wait loop below releases it only
    // once the waiting is over — deadline, failure or completion — at the
    // same point the child's group is forgotten. Every other agent keeps the
    // old immediate-EOF behavior, vendor flows that fail fast on EOF
    // included.
    let (code_gate_tx, code_gate_rx) = std::sync::mpsc::channel::<()>();
    let (stdin_park_tx, stdin_park_rx) = std::sync::mpsc::channel::<()>();
    // Where the deferred lane parks its value for the transcript redaction
    // below: the immediate lanes carry theirs in `code`, and either value
    // must be stripped from the echoed transcript for the same reason — a
    // short, non-secret-shaped code the vendor CLI echoed back would survive
    // `redact_secret` alone.
    let deferred_code_slot: std::sync::Arc<std::sync::Mutex<Option<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    {
        use std::io::Write as _;
        let stdin = child.stdin.take();
        let code = code.as_deref().map(str::to_owned);
        let hold_stdin_open = agent == "claude";
        let deferred_code_slot = deferred_code_slot.clone();
        std::thread::spawn(move || {
            let Some(mut stdin) = stdin else {
                return;
            };
            if !stdin_code_deferred {
                if let Some(code) = code {
                    let _ = writeln!(stdin, "{}", code.trim());
                    let _ = stdin.flush();
                }
            } else {
                // --code-stdin: wait for the URL announcement before reading.
                // The child stays alive through the whole wait; a gate closed
                // without the URL (deadline, drains gone, CLI exit paths)
                // unwinds this thread without reading or writing anything.
                if code_gate_rx.recv().is_err() {
                    return;
                }
                let code = read_deferred_login_code();
                if let Some(code) = code {
                    *deferred_code_slot
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(code.clone());
                    let _ = writeln!(stdin, "{}", code.trim());
                    let _ = stdin.flush();
                }
            }
            if !hold_stdin_open {
                return;
            }
            // claude: hold the pipe open until the wait loop is done with the
            // child; `stdin_park` closes (drop or send) only at that point,
            // and the drop of `stdin` below is the EOF the vendor sees.
            let _ = stdin_park_rx.recv();
        });
    }
    let deadline = Duration::from_secs(if agent == "kimi" { 1800 } else { 600 });
    let started = Instant::now();
    let mut timed_out = false;
    // Artefacts already announced on stderr, so the exit paths below do not
    // print the same link twice.
    let mut streamed_url: Option<String> = None;
    let mut streamed_code: Option<String> = None;
    let mut out_text = String::new();
    let mut err_text = String::new();
    let mut finished_streams = 0usize;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(error) => {
                // The wait itself failed; the child may still be running, so
                // it goes down with the group like every other exit path.
                crate::support::kill_process_tree(&mut child);
                crate::support::supervise::forget_child_group(child.id());
                return Err(CliError::failed(format!("code login({agent}): {error}")));
            }
        }
        if started.elapsed() > deadline {
            // `kill_process_tree` signals the group and then reaps the child
            // itself, so no zombie survives this path.
            crate::support::kill_process_tree(&mut child);
            timed_out = true;
            break None;
        }
        // The 100 ms poll tick is spent waiting on the drains instead of
        // sleeping, so an artefact reaches the terminal within a tick of the
        // vendor CLI printing it. That is the whole point of this flow: the
        // child deliberately stays alive until the user opens the link, so
        // anything published only after it exits is published too late.
        match events_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => {
                let url_arrived = matches!(event, LoginDrainEvent::Artifact(LoginArtifact::Url(_)))
                    && streamed_url.is_none();
                apply_login_event(
                    event,
                    &mut streamed_url,
                    &mut streamed_code,
                    (&mut out_text, &mut err_text),
                    &mut finished_streams,
                );
                if url_arrived {
                    // First URL announcement: the user can go get the code
                    // now, so the deferred stdin read may start.
                    let _ = code_gate_tx.send(());
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            // Both drains are gone (pipes closed early, or a reader died).
            // Keep the poll cadence so the deadline and `try_wait` above
            // still run; a `recv` on a dead channel returns instantly and
            // would otherwise spin this loop.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };
    // One short grace for the drains once the child is gone; in the timeout
    // case the killed child's pipes close so the readers finish promptly.
    // When a straggler still holds a pipe, proceed with whatever was
    // captured — the readers die with this process and cannot reach the
    // transcript. Both streams share the one budget (it used to be 5 s each,
    // sequentially) because they now report on one channel.
    // The waiting is over (status, timeout kill, or failure kill). Release
    // the held-open claude stdin BEFORE the drain grace below: the vendor's
    // own descendants can inherit the pipe's read end (a helper the CLI
    // spawned and left behind), and until this end closes they also hold the
    // write ends of the transcript pipes the drains are waiting on — the
    // grace would expire with empty transcripts otherwise. The park drop is
    // the end-of-login EOF, the same release the GUI's login future performs.
    drop(stdin_park_tx);
    // A deferred stdin read that never got its URL is likewise over: the
    // gate close unwinds the writer thread without touching the invoker's
    // stdin.
    drop(code_gate_tx);
    let grace = Instant::now() + Duration::from_secs(5);
    while finished_streams < 2 {
        let remaining = grace.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match events_rx.recv_timeout(remaining) {
            Ok(event) => apply_login_event(
                event,
                &mut streamed_url,
                &mut streamed_code,
                (&mut out_text, &mut err_text),
                &mut finished_streams,
            ),
            Err(_) => break,
        }
    }
    // The child's exit paths have taken it down, so an interrupt from here on
    // must not signal a group the OS may have already recycled.
    crate::support::supervise::forget_child_group(child.id());
    let combined = format!("{out_text}\n{err_text}");
    // Exact-value strip of the authorization code this process wrote to the
    // child's stdin before the heuristic pass: a short, non-secret-shaped
    // code the vendor CLI echoed back would survive `redact_secret` alone.
    // Whichever lane carried it — the immediate lanes in `code`, the
    // deferred lane's post-URL read in its slot — redacts the same way.
    let deferred_code = deferred_code_slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    let combined = strip_login_code(&combined, code.as_deref().or(deferred_code.as_deref()));
    if timed_out {
        // The buffered transcript would die with this error otherwise, and
        // its login link is exactly what the user needs to finish the flow.
        // The full-transcript scan stays the source of truth for the error
        // hint below; the note only fires for a link the live drain never
        // published — one that arrived without a trailing newline, or after
        // the transcript tail evicted the line the drain would have scanned.
        let login_url = extract_login_url(agent, &combined);
        if let Some(url) = &login_url
            && streamed_url.as_deref() != Some(url.as_str())
        {
            note!("login link: {url}");
        }
        // Unlike the unconditionally-printed single-line link above, the
        // multi-line transcript dump below is human-gated: `--output json`
        // keeps stderr free of it.
        let echoed = pinvou3_lib::platform::credential_store::redact_secret(&combined);
        if output == OutputMode::Human && !echoed.trim().is_empty() {
            note!("{echoed}");
        }
        let link_hint = match &login_url {
            Some(url) => format!("; last login link: {url}"),
            None => String::new(),
        };
        return Err(CliError::failed(format!(
            "code_login_timeout: authorization wait timed out; rerun `pinvou code login`{link_hint}"
        )));
    }
    let status = status.expect("loop only breaks with a status or returns");
    // The vendor login output is echoed once, redacted: login transcripts are
    // exactly what users paste into issues, so the bulk of the stream must
    // never reach the terminal unredacted. (The login URL and the device code
    // are the two exceptions, already streamed live by the drains above —
    // they are what the user must act on, and they are extracted through the
    // agent allow-list rather than passed through verbatim.) It goes to
    // stderr and only in human mode, so `--output json` stdout stays a single
    // serde_json line.
    let echoed = pinvou3_lib::platform::credential_store::redact_secret(&combined);
    if output == OutputMode::Human && !echoed.trim().is_empty() {
        note!("{echoed}");
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
        // Same rationale as the timeout path above: the captured login link
        // is the only actionable part of a failed flow, so it survives into
        // the error.
        let link_hint = match &login_url {
            Some(url) => format!("; last login link: {url}"),
            None => String::new(),
        };
        Err(CliError::failed(format!(
            "code_login_failed: {agent} login process exited with {}{link_hint}",
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_owned())
        )))
    }
}

/// `code logout <agent>`: runs the same non-interactive logout subcommand the
/// GUI's `logout_acp_agent` uses (`codex logout` / `claude auth logout` /
/// `kimi provider remove managed:kimi-code`). Erases the vendor CLI's stored
/// credentials, so it requires `--yes` like every other destructive action.
fn logout(agent: &str, yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
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
    // Logout resolves the same binary as login but reports its own error: a
    // missing CLI on a logout action is "nothing to log out", not a
    // login-flow failure.
    let executable = resolve_agent_cli(agent, agent_cli_name(agent)).ok_or_else(|| {
        CliError::failed(format!(
            "code_logout_cli_missing: {agent} CLI not found; there is nothing to log \
                 out (`pinvou code agents status {agent}` shows the probe result)"
        ))
    })?;
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
            "--wire-api must be anthropic|openai|kimi (aliases: openai_compatible|chat, mirroring \
             ProviderWireApi::parse) (got {other})"
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
    yes: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    require_provider_agent(agent)?;
    // A key deletion is a destructive one-way credential action: mirror the
    // family's remove/require_yes convention so an unattended run cannot
    // drop stored keys (round-18 --yes gap).
    if delete_key {
        require_yes(yes)?;
    }
    // Mirror the lib store's statically reachable validation in English
    // before any secret resolution (--api-key-stdin blocks on stdin): the
    // store's own messages for these two rules are Chinese, the same
    // translation-boundary rule as the marketplace importer.
    if wire_api == Some("kimi") && agent != "kimi" {
        return Err(CliError::failed(
            "the kimi wire protocol only applies to the kimi agent",
        ));
    }
    // Lane label for the English mirrors below and the store pre-checks.
    let lane = if provider_id.is_none() {
        "add"
    } else {
        "update"
    };
    if provider_id.is_none() && agent == "claude" {
        // The store does not merely reject an EMPTY slot set: it requires a
        // non-empty model for every id in `CLAUDE_MODEL_SLOTS` and fails the
        // first missing one with a Chinese message. Checking only
        // `is_empty()` here let `--model-slot sonnet=x` through the English
        // gate and surfaced that Chinese text via `store_error`, so the
        // pre-check validates the full required set and names what is
        // missing. Empty models cannot reach here (`parse_model_slot_pairs`
        // rejects `SLOT=`), but the trim mirrors the store's own filter so a
        // future caller of `providers_save` cannot slip one past.
        let missing: Vec<&str> = CLAUDE_MODEL_SLOTS
            .into_iter()
            .filter(|slot| {
                !model_slots
                    .iter()
                    .any(|(name, model)| name.as_str() == *slot && !model.trim().is_empty())
            })
            .collect();
        if !missing.is_empty() {
            return Err(CliError::failed(format!(
                "code providers {lane}: claude requires --model-slot SLOT=MODEL for every Claude \
                 model slot (a missing slot falls back to official traffic); missing: {} \
                 (valid slots: {})",
                missing.join(", "),
                CLAUDE_MODEL_SLOTS.join(", "),
            )));
        }
    }
    // The store's other user-reachable validation rules fail with Chinese
    // text that `store_error` would surface, so they are mirrored in English
    // here as well (the lib re-checks authoritatively): refined model slots
    // are claude-only, a user-supplied blank --name would be trimmed into an
    // invalid empty store name, and the base URL must be a full http(s)
    // address. Only the user-supplied value is gated; a merged update keeps
    // the existing record's already-valid values.
    if agent != "claude" && !model_slots.is_empty() {
        return Err(CliError::failed(format!(
            "code providers {lane}: --model-slot is only supported for the claude agent \
             (refined model slots); these agents take --model only"
        )));
    }
    if let Some(name) = name.as_deref() {
        if name.trim().is_empty() {
            return Err(CliError::failed(format!(
                "code providers {lane}: --name must not be blank"
            )));
        }
    }
    if let Some(url) = base_url.as_deref() {
        let trimmed = url.trim();
        if !trimmed.starts_with("https://") && !trimmed.starts_with("http://") {
            return Err(CliError::failed(format!(
                "code providers {lane}: --base-url must be a full http(s):// address"
            )));
        }
    }
    let manager = open_providers()?;
    // Update must refuse an unknown provider before any secret resolution:
    // `--api-key-stdin` blocks on stdin, and piping a key into a typo'd
    // provider id must not consume it (or echo a prompt) before failing.
    let existing = provider_id.and_then(|id| manager.store().get(agent, id));
    if let (Some(id), None) = (provider_id, existing.as_ref()) {
        return Err(CliError::failed(format!(
            "provider_not_found: no provider '{id}' for agent {agent}"
        )));
    }
    let secret = resolve_secret(&api_key_env, api_key_stdin)?;
    // Update merges with the existing record so unspecified fields keep their
    // stored values (the GUI edit form prefills the same way).
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
    // Pre-check in English, mirroring the update and switch lanes: the
    // store's delete path fails an unknown id with a Chinese "not found"
    // message that `store_error` would otherwise surface.
    if manager.store().get(agent, provider_id).is_none() {
        return Err(CliError::failed(format!(
            "provider_not_found: no provider '{provider_id}' for agent {agent}"
        )));
    }
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
    // Pre-check the id so the common typo path reports in English instead of
    // surfacing the lib's untranslated store message (the update lane does
    // the same before secret resolution); the lib re-checks authoritatively.
    if manager.store().get(agent, provider_id).is_none() {
        return Err(CliError::failed(format!(
            "provider_not_found: no provider '{provider_id}' for agent {agent}"
        )));
    }
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
            // An existing destination is refused, not overwritten: the export
            // carries plaintext API keys, and a silent truncate could destroy
            // an unrelated file the user pointed at (an earlier export, a
            // shell redirect) with exit 0 — the same policy as `sessions
            // export` / `plugins export`, which reserve the destination with
            // an exclusive create. `create_new` makes the check and the write
            // one atomic step, so a destination swapped onto the path after
            // a plain exists() probe can no longer be truncated, and it also
            // refuses a pre-planted symlink to a file this caller may not
            // own. `mode(0o600)` applies at create time, and `create_new`
            // guarantees this call is the create — a pre-existing destination
            // is refused below instead of being reused — so no follow-up
            // `set_permissions` is needed (the old tighten-existing-file
            // dance existed only to make the then-permitted overwrite of a
            // world-readable file safe; with the overwrite refused, a fresh
            // 0600 create is the only write that can happen).
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                // Plaintext keys land in a 0600 file (the GUI hands the same
                // content to a save dialog; a default-permission file would
                // be readable by every local user).
                options.mode(0o600);
            }
            // Non-unix: no POSIX mode bits — ACL tightening is a follow-up
            // (docs/pinvou-cli.md scopes the 0600 claim to unix). The
            // exclusive create and the overwrite refusal apply on every
            // platform; only the 0600 mode is unix-gated.
            let mut file = match options.open(&path) {
                Ok(file) => file,
                Err(error) => {
                    // `AlreadyExists` is the unix refusal; Windows `CREATE_NEW`
                    // against an existing DIRECTORY reports ERROR_ACCESS_DENIED
                    // (`PermissionDenied`), so "the path is already there"
                    // decides the refusal, with the raw error only in the
                    // cannot-create branch (same classification as `sessions
                    // export`).
                    let already_there =
                        error.kind() == std::io::ErrorKind::AlreadyExists || path.exists();
                    return Err(CliError::failed(if already_there {
                        format!(
                            "code providers export({agent}): refusing to overwrite {}; choose a \
                             destination that does not exist yet",
                            path.display()
                        )
                    } else {
                        format!(
                            "code providers export({agent}): cannot create {}: {error}",
                            path.display()
                        )
                    }));
                }
            };
            // The body write is kept a separate failure from the create:
            // only a create this call performed may be cleaned up below.
            if let Err(error) = std::io::Write::write_all(&mut file, content.as_bytes()) {
                // The exclusive create DID succeed, so this call owns the
                // destination: a partial file (ENOSPC, quota) must not be
                // left behind posing as an export.
                drop(file);
                let _ = std::fs::remove_file(&path);
                return Err(CliError::failed(format!(
                    "code providers export({agent}): cannot write {}: {error}",
                    path.display()
                )));
            }
            note!("{PLAINTEXT_WARNING}");
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
            note!("{PLAINTEXT_WARNING}");
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
    let json = read_text_file_capped(
        path,
        4 * 1024 * 1024,
        &format!("code providers import({agent})"),
    )?;
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
/// use the bound project directory or the temporary execution root. `model` is
/// the session's persisted model name: an ACP model name with a lost
/// session-agents sidecar record still counts as a code session (same fallback
/// as `is_code_chat_session`) and degrades to an unavailable temporary
/// workspace exactly like `code_sessions_list` renders it.
fn code_workspace_info(
    store: &SessionStore,
    agents: &SessionAgentStore,
    id: &str,
    model: &str,
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
        if acp_session_model(model) {
            // Sidecar record lost: keep the session usable (list/info) with
            // the same degraded workspace shape the list uses.
            return Ok((CodexWorkspaceKind::Temporary, PathBuf::new(), false));
        }
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
        let info = match code_workspace_info(&store, &agents, &metadata.id, &metadata.model) {
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
                crate::support::collapse_control_characters(&metadata.title),
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
    // Fetch the session (and its model name) first: the gate below must
    // accept exactly the sessions `code sessions list` accepts, including
    // ACP sessions whose session-agents sidecar record was lost (detected
    // through the persisted model name).
    let metadata = store
        .load(id)
        .map_err(|error| store_error("sessions info", id, error))?
        .metadata;
    if !is_code_chat_session(&agents, id, &metadata.model) {
        return Err(CliError::failed(format!(
            "code_session_not_found: session {id} is not a code session"
        )));
    }
    let record = agents.get(id);
    let info = code_workspace_info(&store, &agents, id, &metadata.model)?;
    let (agent_id, agent_name) = agent_label(&agents, id, &metadata.model);
    let value = serde_json::json!({
        "id": id,
        "title": metadata.title,
        "updated_at": metadata.updated_at.to_rfc3339(),
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
/// skipped (the GUI logs and skips identically). Like `info`, the command is
/// a code-session view: plain chat sessions are refused with the same stable
/// error instead of reporting an empty journal.
fn code_sessions_timeline(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    let agents = open_agent_store()?;
    // Same gate (and same error) as `code sessions info`, including the
    // model-name fallback for sessions whose sidecar record was lost.
    let metadata = store
        .load(id)
        .map_err(|error| store_error("sessions timeline", id, error))?
        .metadata;
    if !is_code_chat_session(&agents, id, &metadata.model) {
        return Err(CliError::failed(format!(
            "code_session_not_found: session {id} is not a code session"
        )));
    }
    let path = paths::sessions_root().join(id).join("acp-timeline.jsonl");
    // Same 32 MiB cap as the sessions timeline reader: a runaway journal
    // must not be slurped whole into memory (the GUI streams this file).
    // The read itself is bounded (`File::take`), so a journal growing between
    // a size check and the read cannot bypass the cap.
    const MAX_TIMELINE_BYTES: u64 = 32 * 1024 * 1024;
    let mut events = Vec::new();
    match std::fs::File::open(&path) {
        Ok(file) => {
            let mut capped = file.take(MAX_TIMELINE_BYTES + 1);
            let mut bytes = Vec::new();
            capped.read_to_end(&mut bytes).map_err(|error| {
                CliError::failed(format!(
                    "code sessions timeline({id}): cannot read {}: {error}",
                    path.display()
                ))
            })?;
            if bytes.len() as u64 > MAX_TIMELINE_BYTES {
                return Err(CliError::failed(format!(
                    "code sessions timeline({id}): journal too large: over {} bytes (limit \
                     {MAX_TIMELINE_BYTES})",
                    MAX_TIMELINE_BYTES + 1
                )));
            }
            let content = String::from_utf8_lossy(&bytes);
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
            // AcpEventEnvelope serializes camelCase with `AcpEvent::event_type`
            // renamed to "type" (features/codex_acp/events.rs); the older
            // snake_case spellings are accepted so journals written by
            // intermediate builds still render.
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
                    .pointer("/event/type")
                    .or_else(|| event.pointer("/event/event_type"))
                    .and_then(|value| value.as_str())
                    .unwrap_or(""),
                event
                    .get("turnId")
                    .or_else(|| event.get("turn_id"))
                    .and_then(|value| value.as_str())
                    .unwrap_or("-"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let value = serde_json::json!({ "id": id, "events": events });
    Ok(success(render(output, human, &value)))
}

// ── workspace ───────────────────────────────────────────────────────────────
//
// Read-only ops (list/search/preview/changes/branches) call
// `features::codex_acp::workspace` directly and serialize its types, so the
// CLI JSON matches the GUI's exactly. Two pieces stay local on purpose: the
// diff lane (the app's per-file diff reads untracked files unbounded, uses
// Chinese section copy, and has no whole-workspace composition — the mirror
// is differentially pinned against the app module by a contract test) and
// `workspace checkout` (cross-process locks, the `--yes` gate, and
// commit-identity hardening are CLI-specific value). Path validation stays a
// CLI pre-check so escapes remain usage errors (exit 2). `git_command` below
// owns the shared environment contract for both.

// Direct references to the app's workspace limits (not local copies): the
// CLI's diff/preview paths share the GUI's caps, so a change on either side
// breaks this build instead of silently diverging. (`SEARCH_LIMIT` is
// referenced through the module path where it is compared.)
use pinvou3_lib::features::codex_acp::workspace::{DIFF_LIMIT, PREVIEW_LIMIT};
/// Upper bound on per-file diffs composed into one whole-workspace diff. Each
/// file costs exactly two `git diff` spawns (unstaged + staged); the
/// canonicalization and the `git rev-parse --show-toplevel` root resolution
/// are hoisted out of the loop and paid once per command, not per file. So
/// this bounds the subprocess fan-out at 2N + 1 git spawns.
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
    if !crate::support::valid_session_id(session) {
        return Err(CliError::usage("invalid session id"));
    }
    // Same absolute-path contract as `open_store`: the lock file is created
    // before `open_store` runs on some paths, and a relative PINVOU3_HOME
    // would put it in a cwd-relative directory.
    crate::support::sandbox_home()?;
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
    // Same absolute-path contract as `open_store`: this lock is acquired
    // before `open_store` runs on some paths.
    crate::support::sandbox_home()?;
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

fn workspace_list(
    root: &Path,
    relative_path: Option<&str>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let root = canonical_workspace(root)?;
    // CLI pre-check so an escape attempt stays a usage error (exit 2); the
    // app module re-validates and resolves the directory itself.
    let relative = normalize_relative_path(relative_path.unwrap_or_default())?;
    let listing = workspace::list_workspace(&root, Some(&relative))
        .map_err(|error| CliError::failed(format!("code workspace list: {error:#}")))?;
    let value = serde_json::to_value(&listing)
        .map_err(|error| CliError::failed(format!("code workspace list: {error}")))?;
    let human = listing
        .entries
        .iter()
        .map(|entry| {
            // RAW by intent (mirror claim): `entry.name` goes to the terminal
            // exactly as the GUI panel receives it. Neither side sanitizes
            // control characters, and only a real filename can reach here —
            // a name with a tab/newline in it breaks this row's column
            // contract on both surfaces equally, so collapsing it here would
            // diverge from the GUI for the same filename.
            format!(
                "{}\t{}\t{}\t{}",
                entry.kind, entry.name, entry.size, entry.modified
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(success(render(output, human, &value)))
}

fn workspace_search(root: &Path, query: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Ok(success(render(
            output,
            String::new(),
            &serde_json::json!({ "results": [], "truncated": false }),
        )));
    }
    let root = canonical_workspace(root)?;
    let results = workspace::search_workspace(&root, &query)
        .map_err(|error| CliError::failed(format!("code workspace search: {error:#}")))?;
    // The app walk stops silently at SEARCH_LIMIT, so from the outside the
    // cap being reached is all we can observe; `truncated` is therefore the
    // honest "at least SEARCH_LIMIT matches exist" signal (a CLI-specific
    // envelope computed on top of the app result, mirroring `workspace list`).
    let truncated = results.len() >= workspace::SEARCH_LIMIT;
    let value = serde_json::json!({ "results": results, "truncated": truncated });
    let human = results
        .iter()
        .map(|entry| entry.relative_path.clone())
        .collect::<Vec<_>>()
        .join("\n");
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

fn workspace_preview(
    root: &Path,
    relative_path: &str,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let root = canonical_workspace(root)?;
    let relative = normalize_relative_path(relative_path)?;
    if relative.is_empty() {
        return Err(CliError::usage("workspace preview requires a file path"));
    }
    let preview = workspace::preview_workspace_file(&root, &relative)
        .map_err(|error| CliError::failed(format!("code workspace preview: {error:#}")))?;
    let value = serde_json::to_value(&preview)
        .map_err(|error| CliError::failed(format!("code workspace preview: {error}")))?;
    let human = match (&preview.text, &preview.data_url) {
        (Some(text), _) => text.clone(),
        (None, Some(url)) => url.clone(),
        _ => format!(
            "{} ({}, {} bytes)",
            preview.relative_path, preview.kind, preview.size
        ),
    };
    Ok(success(render(output, human, &value)))
}

/// `git commit` arguments for the user-visible `workspace checkout --mode
/// commit`. The commit lane strips the ambient `GIT_AUTHOR_*`/`GIT_COMMITTER_*`
/// variables (see [`git_commit_output`]) so the identity cannot be decided by
/// whatever the invoking shell happened to export; the identity the user's own
/// git would use is read back by [`ambient_git_identity`] and passed explicitly
/// with `-c`. With none configured anywhere, `user.useConfigOnly=true` makes the
/// commit fail honestly instead of recording a fabricated `user@hostname`.
fn commit_command_args(root: &Path, message: &str) -> Result<Vec<String>, CliError> {
    let mut args = vec!["-c".to_owned(), "commit.gpgsign=false".to_owned()];
    match ambient_git_identity(root) {
        Some((name, email)) => {
            args.push("-c".to_owned());
            args.push(format!("user.name={name}"));
            args.push("-c".to_owned());
            args.push(format!("user.email={email}"));
        }
        None => {
            args.push("-c".to_owned());
            args.push("user.useConfigOnly=true".to_owned());
        }
    }
    args.push("commit".to_owned());
    args.push("-m".to_owned());
    args.push(message.to_owned());
    Ok(args)
}

/// The ambient `user.name`/`user.email` the GUI's workspace lane would commit
/// with (repo-local first, then global/system). A read-only `git config`
/// probe: it runs no hooks, so it needs no isolation beyond the redirection
/// strip every lane shares.
fn ambient_git_identity(root: &Path) -> Option<(String, String)> {
    // Uses the same redirection strip as the commit it feeds, so both see the
    // same effective repository and the same configuration files: an ambient
    // GIT_DIR/GIT_WORK_TREE/GIT_CONFIG_* would otherwise read a different
    // repo's (or no) identity and fail an otherwise-valid commit with
    // user.useConfigOnly. The user's global config is deliberately visible
    // here — reading the identity they actually commit with is the point.
    let mut command = std::process::Command::new("git");
    command
        .current_dir(root)
        .args(["config", "--null", "--get-regexp", r"^user\.(name|email)$"]);
    strip_git_redirection_env(&mut command);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut name = None;
    let mut email = None;
    for record in text.split('\0') {
        let Some((key, value)) = record.split_once('\n') else {
            continue;
        };
        match key.trim() {
            "user.name" => name = Some(value.trim().to_owned()),
            "user.email" => email = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    Some((name?, email?))
}

/// Ambient `GIT_*` variables that *redirect* git at another repository or at
/// another set of configuration files. Local mirror of the app's
/// `platform::process::GIT_OVERRIDE_KEYS` (that module is `pub(crate)` inside
/// the app crate and cannot be reached from this crate); keep the two in step.
///
/// Removing `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM`/`GIT_CONFIG_NOSYSTEM` is
/// what makes the *user's own* `~/.gitconfig` and `/etc/gitconfig` apply, and
/// removing `GIT_CONFIG_COUNT` disables the whole `GIT_CONFIG_KEY_n`/
/// `GIT_CONFIG_VALUE_n` group, so the numbered pairs need no enumeration (key
/// and value 0 are still removed to guard against injections that bypass
/// `COUNT`).
///
/// A fixed list — deliberately not a `GIT_` prefix scan over `env::vars_os()`:
/// iterating `environ` while another thread runs `setenv`/`remove_var` can
/// silently miss entries (glibc environ mutation is not thread-safe), which is
/// exactly how the app lost test isolation before it moved to a fixed list.
const GIT_REDIRECTION_KEYS: [&str; 22] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_GRAFT_FILE",
    "GIT_SHALLOW_FILE",
    "GIT_REPLACE_REF_BASE",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_TEMPLATE_DIR",
    // Without GIT_LITERAL_PATHSPECS stripped, an ambient `1` would make the
    // `--` pathspecs below match literally, so a path containing git's magic
    // `:(...)` prefix would silently select nothing.
    "GIT_LITERAL_PATHSPECS",
    "GIT_CONFIG",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_CONFIG_NOSYSTEM",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_KEY_0",
    "GIT_CONFIG_VALUE_0",
];

/// Identity/date variables. They outrank `-c user.name=` / `-c user.email=` on
/// the command line, so only the commit lane removes them — see
/// [`git_commit_output`].
const GIT_IDENTITY_KEYS: [&str; 6] = [
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
];

/// Mirror of the app's `strip_git_override_env`.
fn strip_git_redirection_env(command: &mut std::process::Command) {
    for key in GIT_REDIRECTION_KEYS {
        command.env_remove(key);
    }
}

/// Every git call of the workspace read and mutate lanes (`rev-parse`,
/// `status`, `diff`, `branch`, `checkout`, `stash`, `add`).
///
/// These commands operate on the user's *real* working tree, so they must
/// behave exactly like the git the user would run there — which is why this
/// mirrors the GUI workspace lane (`codex_acp::workspace::git_output` →
/// `platform::process::strip_git_override_env`) rather than the checkpoint
/// lane's shadow-repository hardening. Only the ambient redirection variables
/// are removed; `~/.gitconfig` and `/etc/gitconfig` are left to apply.
///
/// Pinning the global config away here (an earlier attempt at "stronger than
/// the GUI" isolation) is not a safe strengthening of a working-tree lane, it
/// is a different repository state: `core.excludesFile` stops being honoured,
/// so globally ignored files show up as `??` and enter the stash/commit path
/// (`--mode commit` would then `git add -A` them into a commit), and
/// `filter.<driver>.clean`/`.smudge` drivers named by `.gitattributes` (what
/// `git lfs install` writes) stop resolving, so `diff` reports unmodified
/// files as changed and `checkout` writes the unsmudged clean-side content
/// back into the user's tree.
///
/// Hooks, aliases, and credential helpers are consequently the user's own.
/// That is the intended contract for a lane that mutates their checkout;
/// commands are always spelled out in full (never through an alias) and
/// stdin is `/dev/null` so nothing here can turn interactive.
fn git_command(root: &Path, arguments: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command
        .current_dir(root)
        .args(arguments)
        .stdin(std::process::Stdio::null());
    strip_git_redirection_env(&mut command);
    command
}

fn git_output(root: &Path, arguments: &[&str]) -> Result<String, CliError> {
    run_git_output(git_command(root, arguments), arguments)
}

/// `workspace checkout --mode commit`'s own `git commit`: identical to
/// [`git_command`] except that the ambient identity variables are dropped, so
/// the `-c user.name=` / `-c user.email=` pair [`commit_command_args`] derives
/// from the user's configuration actually decides the commit instead of being
/// overridden by whatever the invoking shell exported.
fn git_commit_output(root: &Path, arguments: &[&str]) -> Result<String, CliError> {
    let mut command = git_command(root, arguments);
    for key in GIT_IDENTITY_KEYS {
        command.env_remove(key);
    }
    run_git_output(command, arguments)
}

fn run_git_output(
    mut command: std::process::Command,
    arguments: &[&str],
) -> Result<String, CliError> {
    let output = command.output().map_err(|error| {
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

/// Bounded capture for the tracked-diff lane: `Command::output()` would
/// buffer a modified multi-gigabyte file whole just so the caller can
/// truncate it right after. Both streams keep the first `cap + 1` bytes and
/// then keep draining to EOF — stderr too, because a hostile repo hook could
/// write arbitrarily much while the stdout side drains — and the second
/// reader runs on a thread so the two pipes cannot deadlock. Draining past
/// the cap is load-bearing: a reader that stopped at the cap would leave git
/// blocked on a full pipe forever whenever the payload exceeds the cap by
/// more than one pipe buffer, parking the `join()`/`wait()` below (this lane
/// has no deadline). The boolean reports that the cut actually happened, so
/// the caller's truncation marker stays exact even when the lossy decode
/// lands just under the caller's own length check.
fn git_output_capped(
    root: &Path,
    arguments: &[&str],
    cap: u64,
) -> Result<(String, bool), CliError> {
    let mut child = git_command(root, arguments)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            CliError::failed(format!(
                "code workspace: git {}: {error}",
                arguments.join(" ")
            ))
        })?;
    let stdout_pipe = child.stdout.take().expect("git stdout is piped");
    let stderr_pipe = child.stderr.take().expect("git stderr is piped");
    let stderr_thread = std::thread::spawn(move || read_capped_to_eof(stderr_pipe, cap));
    let (stdout_bytes, stdout_total) = read_capped_to_eof(stdout_pipe, cap);
    let (stderr_bytes, _) = stderr_thread.join().unwrap_or_default();
    let status = child.wait().map_err(|error| {
        CliError::failed(format!(
            "code workspace: git {}: {error}",
            arguments.join(" ")
        ))
    })?;
    if !status.success() {
        return Err(CliError::failed(format!(
            "code workspace: git {} failed: {}",
            arguments.join(" "),
            pinvou3_lib::platform::credential_store::redact_secret(
                String::from_utf8_lossy(&stderr_bytes).trim(),
            )
        )));
    }
    let truncated = stdout_total > cap;
    let mut stdout_bytes = stdout_bytes;
    if truncated {
        stdout_bytes.truncate(cap as usize);
    }
    Ok((
        String::from_utf8_lossy(&stdout_bytes).into_owned(),
        truncated,
    ))
}

/// Keeps the first `cap + 1` bytes of a stream and discards the rest while
/// still reading to EOF, reporting the total bytes seen. The discard loop is
/// what lets a writer that outproduces the cap finish instead of blocking on
/// a full pipe.
fn read_capped_to_eof(mut pipe: impl std::io::Read, cap: u64) -> (Vec<u8>, u64) {
    let mut kept = Vec::new();
    let mut total: u64 = 0;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                total += n as u64;
                if (kept.len() as u64) <= cap {
                    let remaining = (cap + 1 - kept.len() as u64) as usize;
                    kept.extend_from_slice(&chunk[..n.min(remaining)]);
                }
            }
            Err(_) => break,
        }
    }
    (kept, total)
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

fn workspace_changes(
    session_id: &str,
    root: &Path,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let root = canonical_workspace(root)?;
    // The app module owns git status parsing, baseline loading, and origin
    // classification (with the GUI's sha256 fingerprint comparison); the CLI
    // serializes the same `WorkspaceChanges` type the GUI command returns.
    let changes = workspace::workspace_changes(session_id, &root)
        .map_err(|error| CliError::failed(format!("code workspace changes: {error:#}")))?;
    let value = serde_json::to_value(&changes)
        .map_err(|error| CliError::failed(format!("code workspace changes: {error}")))?;
    let human = changes
        .changes
        .iter()
        .map(|change| {
            format!(
                "{}\t{}\t{}\t{}",
                change.status,
                if change.staged { "staged" } else { "-" },
                change.origin,
                change.relative_path,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(success(render(output, human, &value)))
}

fn workspace_branches(root: &Path, output: OutputMode) -> Result<CliOutcome, CliError> {
    let root = canonical_workspace(root)?;
    let value = workspace_branches_value(&root)?;
    let human = format!(
        "git: {}\ncurrent: {}\nbranches: {}\ndirty: {}",
        value["git"],
        value["current"].as_str().unwrap_or("-"),
        value["branches"]
            .as_array()
            .map(|branches| branches
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(", "))
            .unwrap_or_default(),
        value["dirtyCount"].as_u64().unwrap_or(0),
    );
    Ok(success(render(output, human, &value)))
}

/// `features::codex_acp::workspace::workspace_branches` serialized for the
/// JSON envelope; shared by `workspace branches` and the checkout result.
fn workspace_branches_value(root: &Path) -> Result<serde_json::Value, CliError> {
    let branches = workspace::workspace_branches(root)
        .map_err(|error| CliError::failed(format!("code workspace branches: {error:#}")))?;
    serde_json::to_value(&branches)
        .map_err(|error| CliError::failed(format!("code workspace branches: {error}")))
}

fn stash_head(root: &Path) -> Result<Option<String>, CliError> {
    Ok(
        git_output(root, &["rev-parse", "-q", "--verify", "refs/stash"])
            .ok()
            .map(|head| head.trim().to_string()),
    )
}

/// `code workspace checkout <session> <branch> --mode carry|stash|commit --yes`:
/// the headless mirror of the GUI's branch switcher. Destructive in all three
/// modes — it moves the user's working tree — so `execute` runs `require_yes`
/// before it is ever reached, mirroring the GUI's confirmation dialog.
///
/// The gate is deliberately flat rather than dirty-tree-conditional. The GUI
/// can afford a conditional prompt because it already renders `dirty_count`
/// next to the branch list and the user is looking at it; a CLI caller sees
/// nothing before the tree moves, and the clean-tree path still rewrites every
/// file that differs between the two branches. Making the gate depend on
/// dirtiness would also make the same command line succeed or fail depending on
/// state a script cannot see, which is worse for automation than one constant
/// rule. The dirty count remains observable beforehand via
/// `code workspace branches` and afterwards in this command's own result.
///
/// Guard the CLI cannot reproduce: the GUI additionally refuses to switch while
/// a code session is running in that workspace (`code_sessions_in_workspace`
/// lives in the desktop process's `AcpPool` and is unreachable headlessly). The
/// cross-process locks below close the CLI×CLI race only — the same disclosure
/// the module header makes for `checkpoints rewind`/`undo`.
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
            let commit_args = commit_command_args(&root, message)?;
            let commit_refs: Vec<&str> = commit_args.iter().map(String::as_str).collect();
            git_commit_output(&root, &commit_refs)?;
            git_output(&root, &["checkout", branch])?;
        }
    }
    finish_checkout(&root, branch, output)
}

fn finish_checkout(root: &Path, branch: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let branches = workspace_branches_value(root)?;
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
    // Resolved once per command rather than per file: `git rev-parse
    // --show-toplevel` is a process spawn, and the whole-workspace lane below
    // calls the per-file diff up to WORKSPACE_DIFF_FILE_CAP times. The root is
    // already canonical here, so the per-file helper no longer re-canonicalizes
    // it either.
    let at_git_root = git_root(&root).is_some_and(|git_root| git_root == root);
    match file {
        Some(file) => {
            let diff = workspace_diff_one(&root, at_git_root, file)?;
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
            let mut hit_diff_limit = false;
            let mut failures: Vec<serde_json::Value> = Vec::new();
            for row in changes["changes"].as_array().cloned().unwrap_or_default() {
                if diffed_files >= WORKSPACE_DIFF_FILE_CAP {
                    break;
                }
                diffed_files += 1;
                // Stop diffing once the payload is over the cap instead of
                // accumulating every per-file diff (500 × 1 MiB) before the
                // final truncation runs.
                if combined.len() >= DIFF_LIMIT {
                    hit_diff_limit = true;
                    break;
                }
                // A file listed by `changes` that cannot be diffed is reported,
                // never dropped: silently omitting it would render exactly like
                // "this file has no changes", and the cases that get here are
                // the ones a caller most needs to know about — a file removed
                // between the status scan and its diff, an unreadable file, a
                // git invocation that failed, or a change row without a usable
                // `relativePath` (which reaches git as an empty pathspec and
                // fails there). The command still exits 0: this lane is a
                // best-effort aggregate over up to 500 files, and failing the
                // whole diff because one of them vanished mid-scan would lose
                // the other 499 diffs the caller asked for. The failure is
                // visible in three places instead — a marker inside `text`
                // (the only thing a human-mode caller sees), a `failures` array
                // in the JSON envelope (machine-readable, keyed by path), and a
                // stderr note.
                let relative = row["relativePath"].as_str().unwrap_or_default().to_owned();
                match workspace_diff_one(&root, at_git_root, &relative) {
                    Ok((_, text, _)) => {
                        if !combined.is_empty() {
                            combined.push('\n');
                        }
                        combined.push_str(&text);
                    }
                    Err(error) => {
                        let message = error.to_string();
                        note!("code workspace diff: {relative}: {message}");
                        if !combined.is_empty() {
                            combined.push('\n');
                        }
                        combined.push_str(&format!("# diff failed: {relative}: {message}\n"));
                        failures.push(serde_json::json!({
                            "relativePath": relative,
                            "error": message,
                        }));
                    }
                }
            }
            // Actually cut the payload when reporting truncation — the
            // per-file path below does the same.
            let truncated = hit_diff_limit || combined.len() > DIFF_LIMIT;
            if truncated {
                truncate_utf8(&mut combined, DIFF_LIMIT);
            }
            let value = serde_json::json!({
                "relativePath": null,
                "text": combined,
                "truncated": truncated,
                "changes": changes["changes"],
                // Always present (empty on the happy path) so a consumer can
                // test `failures.length` without distinguishing "no failures"
                // from "an older build that never reported them".
                "failures": failures,
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
///
/// `root` must already be canonical and `at_git_root` must already say whether
/// it is the top level of a git work tree: both are resolved once by
/// `workspace_diff` (the latter costs a `git rev-parse` spawn) because the
/// whole-workspace lane calls this up to `WORKSPACE_DIFF_FILE_CAP` times, and
/// repeating them per file tripled the subprocess fan-out of the command.
fn workspace_diff_one(
    root: &Path,
    at_git_root: bool,
    relative_path: &str,
) -> Result<(String, String, bool), CliError> {
    let relative = normalize_relative_path(relative_path)?;
    let path = root.join(&relative);
    if !path.starts_with(root) {
        return Err(CliError::failed(
            "code workspace diff: path escapes the workspace",
        ));
    }
    let mut captured_over_cap = false;
    let mut text = if at_git_root {
        // Capped like the untracked lane below: a modified multi-gigabyte
        // file must not buffer whole just to be truncated at DIFF_LIMIT.
        const GIT_READ_CAP: u64 = DIFF_LIMIT as u64 + 1024;
        let (unstaged, unstaged_cut) = git_output_capped(
            root,
            &["diff", "--no-ext-diff", "--no-color", "--", &relative],
            GIT_READ_CAP,
        )?;
        let (staged, staged_cut) = git_output_capped(
            root,
            &[
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-color",
                "--",
                &relative,
            ],
            GIT_READ_CAP,
        )?;
        captured_over_cap = unstaged_cut || staged_cut;
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
    // A captured_over_cap cut means the take() hit the read cap even if the
    // lossy decode shrank the payload under DIFF_LIMIT — the marker must
    // still fire, so the flag joins the length check.
    let truncated = captured_over_cap || text.len() > DIFF_LIMIT;
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
    // The output is truncated at DIFF_LIMIT, but the read itself must also be
    // bounded: a multi-GB text file would be loaded whole just to be cut.
    // Reading DIFF_LIMIT + 1 bytes keeps the truncation marker exact (the
    // loop's final iteration overshoots by one line at most, which the loop
    // already tolerates); a torn multi-byte tail is lossy-decoded away.
    const READ_CAP: u64 = DIFF_LIMIT as u64 + 1024;
    let file = std::fs::File::open(path)
        .map_err(|error| CliError::failed(format!("code workspace diff: {error}")))?;
    let mut capped = std::io::Read::take(file, READ_CAP);
    let mut raw = Vec::new();
    capped
        .read_to_end(&mut raw)
        .map_err(|error| CliError::failed(format!("code workspace diff: {error}")))?;
    let content = String::from_utf8_lossy(&raw);
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
    // Workspace *operations* stay record-gated (empty model): a sidecar-lost
    // ACP session has no reliable workspace record, so resolving one would
    // guess at a directory.
    let (_, root, available) = code_workspace_info(&store, &agents, session, "")?;
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
        .map_err(|error| {
            CliError::failed(format!(
                "checkpoints list({session}): checkpoint entries failed to serialize: {error}"
            ))
        })?;
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
    // the same session mutation lock as rewind/undo, plus the execution-root
    // lock: a concurrent CLI rewind of a *different* session bound to the
    // same project directory would otherwise be diffed against a
    // half-restored tree (the GUI gates diff on busy same-root peers the
    // same way).
    let mut mutation_lock = session_mutation_lock(session)?;
    let _mutation_guard = lock_session_for_mutation(&mut mutation_lock, session, "diff")?;
    let store = open_store()?;
    let agents = open_agent_store()?;
    let (ledger, execution) = require_native_code_session(&store, &agents, session)?;
    let root = canonical_execution_root(&execution);
    let mut root_lock = execution_root_lock(&root)?;
    let _root_guard = lock_root_for_mutation(&mut root_lock, &root, "diff", session)?;
    let diff = checkpoints::diff_checkpoint(&ledger, &execution, checkpoint_id)
        .map_err(|error| store_error("checkpoints diff", checkpoint_id, error))?;
    let value = serde_json::to_value(&diff)
        .map(|value| serde_json::json!({ "session": session, "diff": value }))
        .map_err(|error| {
            CliError::failed(format!(
                "checkpoints diff({session}): checkpoint diff failed to serialize: {error}"
            ))
        })?;
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

/// Exact user-turn count over transcript JSON, via the app-side entry point
/// that runs the engine's own predicate (`is_user_turn_prompt`). This
/// replaces a JSON approximation that counted image-only turns the engine
/// treats as non-prompt and could wedge `checkpoints rewind` in a
/// code-restored state (the CLI pre-check passed, the store's authoritative
/// recount refused, and nothing changed on retry). The values were just
/// serialized from typed messages, so a deserialization failure is a
/// transcript-integrity error, not a parse hiccup.
fn count_user_turns_exact(messages: &serde_json::Value) -> Result<u32, CliError> {
    let empty = Vec::new();
    let array = messages.as_array().unwrap_or(&empty);
    checkpoints::count_user_turns_in_json(array)
        .map_err(|error| CliError::failed(format!("code session transcript is malformed: {error}")))
}

fn parse_rfc3339_epoch_secs(value: &str) -> Option<i64> {
    // The rewind sidecar timestamps are written by
    // `chrono::Utc::now().to_rfc3339()`, so the stale-checkpoint cutoff
    // parses through the same crate instead of a hand-rolled decoder —
    // field ranges, month lengths and offsets then agree with the writer
    // by construction, and a hand-corrupted sidecar degrades to `None`.
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|stamp| stamp.timestamp())
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
                    note!(
                        "[pinvou-cli] stale checkpoint reconciliation failed (cleanup only): {error:#}"
                    );
                }
            }
        }
        Err(error) => {
            note!("[pinvou-cli] reading rewind backups failed (reconciliation skipped): {error:#}");
        }
    }

    let entries = checkpoints::list_checkpoints(&ledger)
        .map_err(|error| store_error("checkpoints rewind", session, error))?;
    let loaded = store
        .load(session)
        .map_err(|error| store_error("checkpoints rewind", session, error))?;
    let messages = serde_json::to_value(&loaded.messages)
        .map_err(|error| CliError::failed(format!("code checkpoints rewind: {error}")))?;
    let total_turns = count_user_turns_exact(&messages)?;
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
        note!(
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
    if count_user_turns_exact(&messages)? != record.kept_turns {
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
        "rewoundTurns": count_user_turns_exact(&removed)?,
        "rewoundAt": record.rewound_at,
    })))
}

/// Failure report for the transcript-restore step of `checkpoints undo`, when
/// the working tree has already been restored. `condition_broken` says whether
/// the undo precondition no longer holds (new turns, edits, or the bound
/// checkpoint vanished): that failure is not retryable, every other one is.
/// Selected by the re-derived undo state, never by the store's message text.
fn undo_restore_failure(
    condition_broken: bool,
    session: &str,
    checkpoint_id: Option<&str>,
    detail: &str,
) -> CliError {
    let rollback_point = checkpoint_id.unwrap_or("-");
    if condition_broken {
        CliError::failed(format!(
            "code_checkpoints_undo_condition_changed({session}): the working tree was already \
             restored to rollback point {rollback_point}, but restoring the transcript failed: \
             {detail}. This is not retryable; the rewind record was not consumed but the \
             truncated messages remain in the rewind backup — handle manually (new turns or \
             edits landed after the rewind)"
        ))
    } else {
        CliError::failed(format!(
            "code checkpoints undo({session}): the working tree was already restored to \
             rollback point {rollback_point}, but restoring the transcript failed: {detail}. \
             The rewind record was not consumed, so `checkpoints undo` can be retried"
        ))
    }
}

/// `code checkpoints undo <session>`: headless mirror of `undo_last_rewind`.
/// Restores the working tree and rewrites the transcript, so it requires
/// `--yes` like `rewind`.
fn checkpoints_undo(session: &str, yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
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
        let detail = format!("{error:#}");
        // The store fails *non-retryably* when the transcript changed after
        // the rewind (new turns produced, turn content edited, or the bound
        // checkpoint vanished). Retrying cannot succeed and would steer
        // the user wrong, so mirror the GUI's condition_broken classification
        // (app/commands/checkpoints.rs undo_last_rewind): report that the
        // record was NOT left consumable-by-retry and the truncated messages
        // remain in the rewind backup for manual handling.
        //
        // The discriminator is the re-derived undo state, never the store's
        // message text (the GUI's `detail.contains("不可反悔")` breaks the
        // moment the store's copy changes; an arbitrary rewording must not
        // reclassify an IO failure as non-retryable or vice versa).
        // `resolve_undo_state` re-validates exactly the preconditions the
        // store guards — kept turn count, truncated revision, and the bound
        // PreRestore checkpoint — so `Ok(None)` on the retry means the
        // precondition genuinely no longer holds; `Ok(Some(_))` means the
        // state is still undoable and the failure is something else (IO
        // etc.), which leaves the record unconsumed and retryable. Every
        // other failure falls to the retryable branch, matching the GUI.
        let condition_broken = matches!(resolve_undo_state(&store, &ledger, session), Ok(None));
        undo_restore_failure(condition_broken, session, checkpoint_id.as_deref(), &detail)
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
    fn undo_failure_classification_is_independent_of_the_store_message_text() {
        // The condition_broken branch is selected by the re-derived undo
        // state, never by the store's message. Prove the classification
        // cannot change when the copy does: the two messages below carry
        // identical failure semantics under arbitrarily different text —
        // one phrased as the store's current precondition violation, one
        // reworded beyond recognition (and, in the negative case, phrased
        // exactly like a precondition violation while the state says
        // retryable). Only the branch flag may decide, so the reports must
        // come out the same within each row no matter the wording.
        let condition_changed = "回退后已产生新轮次，不可反悔";
        let reworded = "the operator declined: cannot proceed (totally different copy)";
        for detail in [condition_changed, reworded] {
            let broken = undo_restore_failure(true, "s-1", Some("cp-9"), detail);
            let message = broken.to_string();
            assert!(
                message.contains("code_checkpoints_undo_condition_changed"),
                "condition broken must say so regardless of wording: {message}"
            );
            assert!(
                message.contains("This is not retryable"),
                "condition broken must not invite a retry regardless of wording: {message}"
            );

            let retryable = undo_restore_failure(false, "s-1", Some("cp-9"), detail);
            let message = retryable.to_string();
            assert!(
                !message.contains("condition_changed"),
                "a retryable failure must not be labelled condition_changed: {message}"
            );
            assert!(
                message.contains("can be retried"),
                "a retryable failure must say so regardless of wording: {message}"
            );
        }
    }

    #[test]
    fn strip_login_code_removes_the_exact_value_before_the_heuristic() {
        // A short, non-secret-shaped code survives `redact_secret`'s
        // heuristic; the exact-value strip must take it out of the echoed
        // transcript everywhere it appears, including punctuation-wrapped.
        let transcript = "auth code 'SRCRT-123' received\nok SRCRT-123\n";
        assert_eq!(
            strip_login_code(transcript, Some(" SRCRT-123 ")),
            "auth code '[REDACTED]' received\nok [REDACTED]\n",
            "the value is trimmed like the stdin write before matching"
        );
        // No code flow: the transcript is untouched.
        assert_eq!(
            strip_login_code(transcript, None),
            transcript,
            "a flow without --code must not rewrite the transcript"
        );
        // Degenerate empty code: the strip must not shred the transcript
        // (replace on "" would interleave [REDACTED] between every char).
        assert_eq!(strip_login_code(transcript, Some("   ")), transcript);
    }

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
