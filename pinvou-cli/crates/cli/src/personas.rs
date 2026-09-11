//! `personas` family: the expert-card pool and per-session equip state,
//! mirroring `pinvou3-app/src-tauri/src/app/commands/personas.rs`.
//!
//! Every operation calls the same public `pinvou3_lib::features::personas`
//! functions the GUI commands call (`all_summaries`, `get`,
//! `create_user_persona`, `update_user_persona`, `delete_user_persona`,
//! `equip_body_injection`) plus the `SessionStore` persona sidecars
//! (`set_pending_persona_body` / `set_active_persona` / `active_persona_id`)
//! used by `equip_persona` / `unequip_persona` / `get_active_persona`.
//! Pure storage only: no Tauri host, no engine.
//!
//! Error-copy note: the GUI returns Chinese messages ("未知专家面具",
//! "只能删除自制卡", "卡牌不存在"); the CLI surfaces the same failure
//! conditions as exit-code 1 errors with English copy (developer tool, all
//! CLI output is English per the CLI spec).
//!
//! Equip-state note (headless deviation): the GUI keeps `active_persona` /
//! `pending_persona_body` in `SessionModeState`, which is deliberately
//! memory-only over the app's lifetime. The CLI is one process per
//! invocation, so `equip` additionally persists the state to the per-session
//! sidecar `~/.pinvou3/sessions/<id>/persona_equipped.json` (the directory
//! that already hosts the other per-session sidecars) and `active` reads it
//! back; `unequip` removes it. The in-memory `SessionStore` calls still run
//! so same-process semantics stay identical to the GUI commands.

use std::path::PathBuf;

use crate::support::{render, require_yes, sandbox_home, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::personas::{
    PersonaCard, PersonaSummary, all_summaries, create_user_persona, delete_user_persona,
    equip_body_injection, get, update_user_persona,
};
use pinvou3_lib::features::sessions::SessionStore;

const USAGE: &str = "usage: pinvou personas <list|show|create|update|delete|equip|unequip|active>";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceFilter {
    Builtin,
    User,
    All,
}

impl SourceFilter {
    fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::User => "user",
            Self::All => "all",
        }
    }
}

/// Persona body input: `--file BODY.md` or `--stdin` (never argv — bodies
/// are multi-KB markdown).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodySource {
    File(PathBuf),
    Stdin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersonasCommand {
    List {
        source: SourceFilter,
    },
    Show {
        id: String,
    },
    Create {
        name: String,
        description: Option<String>,
        body: BodySource,
    },
    Update {
        id: String,
        name: Option<String>,
        description: Option<String>,
        body: Option<BodySource>,
    },
    Delete {
        id: String,
        yes: bool,
    },
    Equip {
        session_id: String,
        persona_id: String,
    },
    Unequip {
        session_id: String,
    },
    Active {
        session_id: String,
    },
}

/// Flags that carry a value, per subcommand.
const CREATE_OPTIONS: &[&str] = &["--name", "--description", "--file"];
const UPDATE_OPTIONS: &[&str] = &["--name", "--description", "--file"];
const LIST_OPTIONS: &[&str] = &["--source"];

/// Boolean (valueless) flags, per subcommand.
const DELETE_FLAGS: &[&str] = &["--yes"];
const CREATE_FLAGS: &[&str] = &["--stdin"];
const UPDATE_FLAGS: &[&str] = &["--stdin"];

pub fn parse(values: &[String]) -> Result<PersonasCommand, CliError> {
    let subcommand = values.get(1).ok_or_else(|| CliError::usage(USAGE))?;
    let rest = &values[2..];
    match subcommand.as_str() {
        "list" => {
            let (options, _) = parse_flags(rest, LIST_OPTIONS, &[])?;
            let source = match option(&options, "--source") {
                None => SourceFilter::All,
                Some("builtin") => SourceFilter::Builtin,
                Some("user") => SourceFilter::User,
                Some("all") => SourceFilter::All,
                Some(other) => {
                    return Err(CliError::usage(format!(
                        "personas list --source must be builtin, user, or all (got {other})"
                    )));
                }
            };
            Ok(PersonasCommand::List { source })
        }
        "show" | "delete" => {
            let id = require_id(rest.first(), subcommand)?;
            let (_, flags) = parse_flags(
                &rest[1..],
                &[],
                if subcommand == "delete" {
                    DELETE_FLAGS
                } else {
                    &[]
                },
            )?;
            if subcommand == "show" {
                Ok(PersonasCommand::Show { id })
            } else {
                Ok(PersonasCommand::Delete {
                    id,
                    yes: flags.contains(&"--yes"),
                })
            }
        }
        "create" => {
            let (options, flags) = parse_flags(rest, CREATE_OPTIONS, CREATE_FLAGS)?;
            let name = option(&options, "--name")
                .ok_or_else(|| CliError::usage("personas create requires --name N"))?
                .to_owned();
            if name.trim().is_empty() {
                return Err(CliError::usage("personas create --name must not be empty"));
            }
            let body = body_source(&options, &flags, "create")?.ok_or_else(|| {
                CliError::usage("personas create requires a body via --file BODY.md or --stdin")
            })?;
            Ok(PersonasCommand::Create {
                name,
                description: option(&options, "--description").map(str::to_owned),
                body,
            })
        }
        "update" => {
            let id = require_id(rest.first(), "update")?;
            let (options, flags) = parse_flags(&rest[1..], UPDATE_OPTIONS, UPDATE_FLAGS)?;
            let body = body_source(&options, &flags, "update")?;
            Ok(PersonasCommand::Update {
                id,
                name: option(&options, "--name").map(str::to_owned),
                description: option(&options, "--description").map(str::to_owned),
                body,
            })
        }
        "equip" => {
            let session_id = require_id(rest.first(), "equip")?;
            let persona_id = rest
                .get(1)
                .filter(|id| !id.is_empty() && !id.starts_with("--"))
                .ok_or_else(|| CliError::usage("personas equip requires a persona id"))?
                .clone();
            if rest.len() > 2 {
                return Err(CliError::usage("personas equip accepts no options"));
            }
            Ok(PersonasCommand::Equip {
                session_id,
                persona_id,
            })
        }
        "unequip" | "active" => {
            let session_id = require_id(rest.first(), subcommand)?;
            if rest.len() > 1 {
                return Err(CliError::usage(format!(
                    "personas {subcommand} accepts no options"
                )));
            }
            Ok(if subcommand == "unequip" {
                PersonasCommand::Unequip { session_id }
            } else {
                PersonasCommand::Active { session_id }
            })
        }
        _ => Err(CliError::usage(USAGE)),
    }
}

/// Body source resolution shared by create/update: exactly one of `--file`
/// or `--stdin`. `create` requires a body (the CLI contract is stricter
/// than the GUI dialog, which allows an empty body through); `update` treats
/// it as optional (keep the stored body when absent).
fn body_source(
    options: &[(&str, &str)],
    flags: &[&str],
    subcommand: &str,
) -> Result<Option<BodySource>, CliError> {
    let file = option(options, "--file").map(PathBuf::from);
    let stdin = flags.contains(&"--stdin");
    if file.is_some() && stdin {
        return Err(CliError::usage(
            "use only one of --file or --stdin for the persona body",
        ));
    }
    let body = match (file, stdin) {
        (Some(path), false) => Some(BodySource::File(path)),
        (None, true) => Some(BodySource::Stdin),
        (None, false) => {
            if subcommand == "create" {
                return Err(CliError::usage(
                    "personas create requires a body via --file BODY.md or --stdin",
                ));
            }
            None
        }
        (Some(_), true) => unreachable!("file+stdin rejected above"),
    };
    Ok(body)
}

fn require_id(value: Option<&String>, subcommand: &str) -> Result<String, CliError> {
    let id = value
        .ok_or_else(|| CliError::usage(format!("personas {subcommand} requires an id")))?
        .clone();
    if id.is_empty() {
        return Err(CliError::usage(format!(
            "personas {subcommand} requires an id"
        )));
    }
    Ok(id)
}

/// Mirrors the pair-based flag parser used by the sessions family.
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
                    "duplicate personas option {token}"
                )));
            }
            flags.push(token);
            index += 1;
            continue;
        }
        if !value_flags.contains(&token) {
            return Err(CliError::usage(format!(
                "unsupported personas option: {token}"
            )));
        }
        if options.iter().any(|(name, _)| *name == token) {
            return Err(CliError::usage(format!(
                "duplicate personas option {token}"
            )));
        }
        let value = values
            .get(index + 1)
            .ok_or_else(|| CliError::usage(format!("personas option {token} requires a value")))?;
        if value.is_empty() || value.starts_with("--") {
            return Err(CliError::usage(format!(
                "personas option {token} requires a value"
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

// ─────────────────────────────── execute ───────────────────────────────

pub fn execute(command: PersonasCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    match command {
        PersonasCommand::List { source } => list(source, output),
        PersonasCommand::Show { id } => show(&id, output),
        PersonasCommand::Create {
            name,
            description,
            body,
        } => create(&name, description.as_deref(), &body, output),
        PersonasCommand::Update {
            id,
            name,
            description,
            body,
        } => update(&id, name, description, body, output),
        PersonasCommand::Delete { id, yes } => delete(&id, yes, output),
        PersonasCommand::Equip {
            session_id,
            persona_id,
        } => equip(&session_id, &persona_id, output),
        PersonasCommand::Unequip { session_id } => unequip(&session_id, output),
        PersonasCommand::Active { session_id } => active(&session_id, output),
    }
}

fn persona_error(context: &str, error: impl std::fmt::Display) -> CliError {
    CliError::failed(format!("personas {context}: {error}"))
}

fn list(source: SourceFilter, output: OutputMode) -> Result<CliOutcome, CliError> {
    let mut summaries: Vec<PersonaSummary> = all_summaries();
    if source != SourceFilter::All {
        let wanted = source.as_str();
        summaries.retain(|summary| summary.source == wanted);
    }
    let human = summaries
        .iter()
        .map(|summary| {
            format!(
                "{}\t{}\t{}\t{}\t{}",
                summary.id,
                summary.source,
                summary.name,
                summary.dept,
                if summary.description.is_empty() {
                    "-"
                } else {
                    &summary.description
                },
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let value = serde_json::to_value(&summaries)
        .map(|personas| json_entries(personas, source))
        .unwrap_or_else(|_| json_entries(serde_json::Value::Array(Vec::new()), source));
    Ok(success(render(output, human, &value)))
}

fn json_entries(personas: serde_json::Value, source: SourceFilter) -> serde_json::Value {
    serde_json::json!({ "source": source.as_str(), "personas": personas })
}

/// Mirror of `read_persona_body`: the full card body for the detail view.
/// Unknown ids exit 1 (GUI: "未知专家面具: {id}").
fn show(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let card = get(id).ok_or_else(|| CliError::failed(format!("unknown persona: {id}")))?;
    let human = card.body.clone();
    let value = serde_json::json!({
        "id": card.id,
        "name": card.name,
        "dept": card.dept,
        "source": card.source,
        "description": card.description,
        "body": card.body,
    });
    Ok(success(render(output, human, &value)))
}

/// Mirror of `create_persona` + `PersonaInput::into_card`: user cards get the
/// GUI's defaults (dept falls back to "specialized", emoji/color to the
/// card-defaults) and a generated `user-<slug>-<nanos>` id.
fn create(
    name: &str,
    description: Option<&str>,
    body: &BodySource,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    let card = PersonaCard {
        id: String::new(),
        dept: "specialized".to_owned(),
        name: name.to_owned(),
        description: description.unwrap_or_default().to_owned(),
        emoji: "🃏".to_owned(),
        color: "#7C3AED".to_owned(),
        body: read_body(body, "create")?,
        source: "user".to_owned(),
        conversational_only: false,
    };
    let summary = create_user_persona(card).map_err(|error| persona_error("create", error))?;
    let value = serde_json::to_value(&summary).unwrap_or_else(|_| serde_json::json!({}));
    Ok(success(render(
        output,
        format!("created {}", summary.id),
        &value,
    )))
}

/// Mirror of `update_persona`: fetch the full current card, overlay the
/// provided fields, hand the complete card back to `update_user_persona`
/// (which enforces the user- prefix rule: builtin cards exit 1).
fn update(
    id: &str,
    name: Option<String>,
    description: Option<String>,
    body: Option<BodySource>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    let mut card = get(id).ok_or_else(|| CliError::failed(format!("unknown persona: {id}")))?;
    if let Some(name) = name {
        card.name = name;
    }
    if let Some(description) = description {
        card.description = description;
    }
    if let Some(body) = body {
        card.body = read_body(&body, "update")?;
    }
    let summary = update_user_persona(card).map_err(|error| persona_error("update", error))?;
    let value = serde_json::to_value(&summary).unwrap_or_else(|_| serde_json::json!({}));
    Ok(success(render(
        output,
        format!("updated {}", summary.id),
        &value,
    )))
}

/// Mirror of `delete_persona`: user cards only ("只能删除自制卡" → exit 1).
fn delete(id: &str, yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    sandbox_home()?;
    delete_user_persona(id).map_err(|error| persona_error("delete", error))?;
    let value = serde_json::json!({ "id": id, "action": "deleted" });
    Ok(success(render(output, format!("deleted {id}"), &value)))
}

/// Session ids join onto paths (the equip sidecar below), so apply the same
/// `[A-Za-z0-9_-]` restriction the sessions family enforces before any path
/// use; anything else is a usage error, never a traversal.
fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Per-session equip state file; see the module-level equip-state note for
/// why the CLI needs a file where the GUI keeps the state in memory only.
fn equip_state_path(session_id: &str) -> Result<PathBuf, CliError> {
    if !valid_session_id(session_id) {
        return Err(CliError::usage(format!(
            "personas session id must use only letters, digits, '-' or '_': {session_id}"
        )));
    }
    Ok(sandbox_home()?
        .join("sessions")
        .join(session_id)
        .join("persona_equipped.json"))
}

/// Reads the persisted equip state; a missing, unreadable, or corrupt sidecar
/// degrades to "no persona equipped" (the same tolerance as the GUI restart
/// path, where the memory-only state is simply gone).
fn equipped_persona_id(session_id: &str) -> Option<String> {
    let path = equip_state_path(session_id).ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get("persona_id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
}

/// Persists the equip state for the next CLI invocation.
fn persist_equipped_persona(
    session_id: &str,
    persona_id: &str,
    pending_body: &str,
) -> Result<(), CliError> {
    let path = equip_state_path(session_id)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| {
            CliError::failed(format!(
                "cannot create session persona sidecar directory: {error}"
            ))
        })?;
    }
    let payload = serde_json::json!({ "persona_id": persona_id, "pending_body": pending_body });
    let bytes = serde_json::to_vec(&payload).map_err(|error| {
        CliError::failed(format!("cannot serialize session persona sidecar: {error}"))
    })?;
    // Temp + rename (same discipline as the app's atomic writes): a
    // concurrent `active` read must never observe a torn sidecar.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = path.with_extension(format!("json.tmp.{}.{}", std::process::id(), nonce));
    std::fs::write(&tmp, &bytes).map_err(|error| {
        CliError::failed(format!("cannot save session persona sidecar: {error}"))
    })?;
    if let Err(error) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CliError::failed(format!(
            "cannot save session persona sidecar: {error}"
        )));
    }
    Ok(())
}

/// Mirror of `equip_persona`: resolve the card, persist the equipped state on
/// the session sidecar and return the summary. Honest scope: the pending-body
/// injection store is process memory — only the desktop app's turn loop
/// consumes it — so a CLI equip records intent on the sidecar; no turn (GUI
/// or CLI) reads that file today, and a GUI equip is invisible to this
/// command. Revealed in the output below so the command cannot be mistaken
/// for live persona injection.
fn equip(session_id: &str, persona_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let card = get(persona_id)
        .ok_or_else(|| CliError::failed(format!("unknown persona: {persona_id}")))?;
    let store = open_store()?;
    let summary = card.summary();
    let injection = equip_body_injection(&card);
    store.set_pending_persona_body(session_id, Some(injection.clone()));
    store.set_active_persona(session_id, Some(persona_id.to_owned()));
    persist_equipped_persona(session_id, persona_id, &injection)?;
    let mut value = serde_json::to_value(&summary).unwrap_or_else(|_| serde_json::json!({}));
    value["session_id"] = serde_json::json!(session_id);
    value["applies_to_next_turn"] = serde_json::json!(false);
    value["note"] = serde_json::json!(
        "equip state is recorded on the session sidecar; persona injection into turns happens          only inside the running desktop app — equip the session there for live injection"
    );
    Ok(success(render(
        output,
        format!(
            "equipped {persona_id} on {session_id} (recorded; injection happens in the \
             desktop app's turns)"
        ),
        &value,
    )))
}

/// Mirror of `unequip_persona`: clear both the active id and the pending
/// body so nothing is injected on the next turn (memory + persisted sidecar;
/// a missing sidecar is the already-unequipped case).
fn unequip(session_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let path = equip_state_path(session_id)?;
    let store = open_store()?;
    store.set_active_persona(session_id, None);
    store.set_pending_persona_body(session_id, None);
    let _ = std::fs::remove_file(&path);
    let value = serde_json::json!({ "session_id": session_id, "action": "unequipped" });
    Ok(success(render(
        output,
        format!("unequipped persona on {session_id}"),
        &value,
    )))
}

/// Mirror of `get_active_persona`: the equipped card's summary, or null. The
/// persisted sidecar is the source of truth across CLI invocations; the
/// in-memory store stays as the fallback for state set in this process.
fn active(session_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    // Reject ids that cannot name a session directory before any path use.
    equip_state_path(session_id)?;
    let store = open_store()?;
    let summary = equipped_persona_id(session_id)
        .or_else(|| store.active_persona_id(session_id))
        .and_then(|persona_id| get(&persona_id).map(|card| card.summary()));
    let human = match &summary {
        Some(summary) => format!("{}\t{}\t{}", summary.id, summary.name, summary.source),
        None => "none".to_owned(),
    };
    let value = match serde_json::to_value(&summary) {
        Ok(value) => value,
        Err(_) => serde_json::Value::Null,
    };
    Ok(success(render(output, human, &value)))
}

fn open_store() -> Result<SessionStore, CliError> {
    SessionStore::boot()
        .map_err(|error| CliError::failed(format!("sessions store unavailable: {error:#}")))
}

/// Read the persona body from `--file` or stdin; bodies are multi-KB
/// markdown, so argv delivery is deliberately not offered.
fn read_body(source: &BodySource, subcommand: &str) -> Result<String, CliError> {
    let content = match source {
        BodySource::File(path) => std::fs::read_to_string(path).map_err(|error| {
            CliError::failed(format!(
                "personas {subcommand}: cannot read {}: {error}",
                path.display()
            ))
        })?,
        BodySource::Stdin => {
            let mut content = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut content).map_err(
                |error| {
                    CliError::failed(format!("personas {subcommand}: cannot read stdin: {error}"))
                },
            )?;
            content
        }
    };
    if content.trim().is_empty() {
        return Err(CliError::usage(format!(
            "personas {subcommand}: the persona body must not be empty"
        )));
    }
    Ok(content)
}
