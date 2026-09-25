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
//! so same-process semantics stay identical to the GUI commands. Because
//! that sidecar holds the card's full pending-body injection text, `delete`
//! sweeps every sidecar that still references the deleted card (the CLI
//! equivalent of the GUI's `remove_persona_from_all` cascade) and reports
//! the cleared session ids as `cleared_sessions`.
//!
//! One-sided sweep, disclosed: the GUI's own persona delete never touches
//! `persona_equipped.json` (the file is a CLI concept and the name appears
//! nowhere in `pinvou3-app`), so a card deleted in the desktop app leaves the
//! CLI's sidecars behind, and the CLI cannot sweep them afterwards either —
//! `delete` gates on the card existing. The CLI therefore makes that state
//! reachable from its own side: `active` reports the orphaned sidecar instead
//! of degrading to "none", and `unequip` clears it without consulting the card
//! pool.
//!
//! Field note (headless deviation): `create`/`update` expose only the GUI
//! dialog's name/description/body fields — the department is fixed to
//! "specialized" and the emoji/color take the card defaults.

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
    /// The canonical spelling, used both to filter `PersonaSummary::source`
    /// and to echo the applied filter back in the JSON `source` field.
    ///
    /// `Builtin` renders as `builtin`, never as its deprecated `embedded`
    /// input alias (see `parse`): the value has to match what the cards
    /// themselves carry, or `--source embedded` would answer
    /// `{"source": "embedded", "personas": [{"source": "builtin"}, ...]}` and
    /// contradict itself.
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
                // `embedded` is a deprecated INPUT alias for `builtin`, kept
                // for compatibility only. The published CLI reference used to
                // advertise `--source embedded|user`; it now reads
                // `builtin|user|all` (`docs/pinvou-cli.md`), which is the
                // value the feature layer stamps on a card and the one this
                // command echoes back in `source`. The alias stays anyway:
                // scripts written against the older reference are already out
                // there, breaking them buys nothing, it costs one match arm,
                // and it cannot collide because `embedded` is not a card
                // source. It is deliberately not re-documented — the only
                // spelling the reference teaches is `builtin`.
                Some("builtin" | "embedded") => SourceFilter::Builtin,
                Some("user") => SourceFilter::User,
                Some("all") => SourceFilter::All,
                Some(other) => {
                    return Err(CliError::usage(format!(
                        "personas list --source must be builtin (alias: embedded), user, \
                         or all (got {other})"
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

/// Shared implementation in `support::parse_family_flags`; `family`
/// only names this family in error messages.
fn parse_flags<'a>(
    values: &'a [String],
    value_flags: &[&str],
    boolean_flags: &[&str],
) -> Result<(Vec<(&'a str, &'a str)>, Vec<&'a str>), CliError> {
    crate::support::parse_family_flags(values, value_flags, boolean_flags, "personas")
}

fn option<'a>(options: &'a [(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    crate::support::family_option(options, name)
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
    CliError::failed(format!(
        "personas {context}: {}",
        translate_persona_error(&error.to_string())
    ))
}

/// The feature layer's error strings are Chinese (GUI copy); the CLI is an
/// English tool, so the known messages are translated at this boundary and
/// anything unrecognized passes through unchanged rather than being dropped.
fn translate_persona_error(message: &str) -> String {
    match message {
        "只能删除自制卡" => "only self-made personas can be deleted".to_owned(),
        "只能编辑自制卡" => "only self-made personas can be edited".to_owned(),
        "卡牌名称不能为空" => "the persona name must not be empty".to_owned(),
        "卡牌不存在" => "the persona does not exist".to_owned(),
        other => {
            for (prefix, english) in [
                ("非法卡 id: ", "invalid persona id: "),
                ("建目录失败: ", "cannot create the personas directory: "),
                ("序列化失败: ", "cannot serialize the persona: "),
                ("写卡失败: ", "cannot write the persona: "),
            ] {
                if let Some(rest) = other.strip_prefix(prefix) {
                    return format!("{english}{rest}");
                }
            }
            other.to_owned()
        }
    }
}

/// One human-mode `personas list` row: five tab-separated columns
/// (id, source, name, dept, description).
///
/// Every cell here is attacker-reachable text, which is why the whole row goes
/// through the column collapse rather than a chosen subset (same rule as the
/// `sessions subagents` row: the row contract must not depend on which cell
/// happened to look machine-made). `create_user_persona` only rejects an empty
/// trimmed name, the CLI takes `--name`/`--description` verbatim from argv,
/// and `load_user_cards` deserializes whatever `~/.pinvou3/user/personas/*.json`
/// contains — id, dept and source included. A tab would invent a sixth column
/// and a newline would split one card across two rows for whoever is cutting
/// the output on `\t`, and an ESC would reach the terminal. Human mode only:
/// the JSON payload keeps the real strings (`serde_json` escapes everything
/// below 0x20, so it is already safe to read back). Sibling rule and sibling
/// test in `projects.rs`.
fn persona_row(summary: &PersonaSummary) -> String {
    let cell = crate::support::collapse_control_characters;
    format!(
        "{}\t{}\t{}\t{}\t{}",
        cell(&summary.id),
        cell(&summary.source),
        cell(&summary.name),
        cell(&summary.dept),
        cell(if summary.description.is_empty() {
            "-"
        } else {
            &summary.description
        }),
    )
}

/// One human-mode `personas active` row: three tab-separated columns
/// (id, name, source). Same cells, same untrusted sources, same rule as
/// [`persona_row`] — a name safe in `list` and raw in `active` would be the
/// CLI contradicting itself one command apart.
fn active_row(summary: &PersonaSummary) -> String {
    let cell = crate::support::collapse_control_characters;
    format!(
        "{}\t{}\t{}",
        cell(&summary.id),
        cell(&summary.name),
        cell(&summary.source),
    )
}

fn list(source: SourceFilter, output: OutputMode) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    let mut summaries: Vec<PersonaSummary> = all_summaries();
    if source != SourceFilter::All {
        let wanted = source.as_str();
        summaries.retain(|summary| summary.source == wanted);
    }
    let human = summaries
        .iter()
        .map(persona_row)
        .collect::<Vec<_>>()
        .join("\n");
    // A serialization failure is a failure, not an empty deck. The previous
    // fallback answered `{"personas": []}` with exit 0 — a caller scripting
    // "the pool is empty, seed it" cannot tell that apart from a genuinely
    // empty pool, and acts on a deck it was never shown.
    let value = serde_json::to_value(&summaries)
        .map(|personas| json_entries(personas, source))
        .map_err(|error| CliError::failed(format!("personas list: {error}")))?;
    Ok(success(render(output, human, &value)))
}

fn json_entries(personas: serde_json::Value, source: SourceFilter) -> serde_json::Value {
    serde_json::json!({ "source": source.as_str(), "personas": personas })
}

/// Mirror of `read_persona_body`: the full card body for the detail view.
/// Unknown ids exit 1 (GUI: "未知专家面具: {id}").
fn show(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
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
    // See `summary_value`: an empty object with exit 0 would hand the caller a
    // null `.id` for a card that was really created.
    let value = summary_value(&summary, "create")?;
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
    let value = summary_value(&summary, "update")?;
    Ok(success(render(
        output,
        format!("updated {}", summary.id),
        &value,
    )))
}

/// Mirror of `delete_persona`: user cards only ("只能删除自制卡" → exit 1).
/// After the card is gone, every per-session equip sidecar that still
/// references it is swept (see the equip-state note): the sidecar holds the
/// deleted card's full injection body, and leaving it behind would make
/// `active` report a persona that no longer exists.
fn delete(id: &str, yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    sandbox_home()?;
    // The feature delete ignores remove-file errors (the GUI cascade treats
    // a missing card as already-deleted), but a CLI caller deleting an id
    // that is not there must not be told "deleted" — gate on existence like
    // `personas show` does.
    get(id).ok_or_else(|| CliError::failed(format!("unknown persona: {id}")))?;
    delete_user_persona(id).map_err(|error| persona_error("delete", error))?;
    let (cleared_sessions, sidecar_errors) = clear_equipped_sidecars(id)?;
    // The delete has committed by the time the sweep runs, so a sidecar the
    // sweep cannot remove must not flip the outcome to a failure that
    // claims nothing was deleted — and it must not be swallowed either:
    // a failed rerun cannot reach the sweep again (the card is gone), so
    // the stuck sessions are named right here.
    if !sidecar_errors.is_empty() {
        for error in &sidecar_errors {
            note!("warning: personas delete: stale persona sidecar: {error}");
        }
        note!(
            "warning: personas delete: {} session sidecar(s) could not be removed; \
             unequip them per session (the persona card itself is deleted)",
            sidecar_errors.len()
        );
    }
    let human = if cleared_sessions.is_empty() {
        format!("deleted {id}")
    } else {
        format!(
            "deleted {id} (cleared the equipped-persona sidecar on {} session{})",
            cleared_sessions.len(),
            if cleared_sessions.len() == 1 { "" } else { "s" }
        )
    };
    let value = serde_json::json!({
        "id": id,
        "action": "deleted",
        "cleared_sessions": cleared_sessions,
        "sidecar_errors": sidecar_errors,
    });
    Ok(success(render(output, human, &value)))
}

/// Delete-time sweep of the CLI's own equip persistence: scan
/// `$PINVOU3_HOME/sessions/<id>/persona_equipped.json` (only ids that pass
/// `valid_session_id`), and where the sidecar's `persona_id` matches the
/// deleted card, remove the file. Sidecars for other personas are left
/// untouched. Reads are bounded and tolerant — a missing, unreadable, or
/// corrupt sidecar is skipped, the same tolerance `active` applies — but the
/// bound must cover the largest legal sidecar: `pending_body` holds the
/// injection-wrapped body (body cap 4 MiB plus a few hundred bytes of wrapper
/// text), serialized as a JSON string whose worst-case escape expansion is
/// 6 bytes per input byte (`\uXXXX` for control bytes). A cap anywhere below
/// that would silently skip exactly the sidecars carrying the biggest
/// legitimate personas, leaving stale `persona_equipped.json` behind while
/// delete still reports success — so a sidecar that exists but cannot be
/// inspected (over the cap, unreadable) is recorded in `sidecar_errors` and
/// warned instead of skipped silently, while a missing one (the normal
/// unequipped case) stays silent. A matching sidecar that cannot be removed
/// is reported the same way rather than failing the delete after the fact.
fn clear_equipped_sidecars(persona_id: &str) -> Result<(Vec<String>, Vec<String>), CliError> {
    let sessions_dir = sandbox_home()?.join("sessions");
    let entries = match std::fs::read_dir(&sessions_dir) {
        Ok(entries) => entries,
        // No sessions directory yet: nothing can be equipped anywhere. Any
        // other listing failure (permissions, ...) must surface as a sweep
        // error — a silent empty result would report success while ghost
        // sidecars survive.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), Vec::new()));
        }
        Err(error) => {
            return Err(CliError::failed(format!(
                "personas delete: cannot list {}: {error}",
                sessions_dir.display()
            )));
        }
    };
    let mut cleared = Vec::new();
    let mut sweep_errors = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                // A per-entry failure is a listing failure, and the block
                // above already argues what that means here: the sweep cannot
                // say whether the session behind this entry held a sidecar for
                // the deleted card, so dropping it (`entries.flatten()`) would
                // report success while a ghost survives. Name it instead —
                // there is no id to attach it to, which is exactly the point.
                sweep_errors.push(format!(
                    "<unreadable entry under {}>: {error}",
                    sessions_dir.display()
                ));
                continue;
            }
        };
        let Some(session_id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !valid_session_id(&session_id) {
            continue;
        }
        let path = entry.path().join("persona_equipped.json");
        let raw = match crate::support::read_text_file_capped(
            &path,
            MAX_SIDECAR_BYTES,
            "personas delete",
        ) {
            Ok(raw) => raw,
            Err(error) => {
                // The file exists but cannot be inspected (over the cap,
                // permissions, vanished mid-read): it may be a ghost the
                // sweep cannot clear, so surface it instead of a silent
                // success. A vanished (missing) sidecar is the normal
                // unequipped case and stays silent.
                if path.exists() {
                    sweep_errors.push(format!("{session_id}: {error}"));
                }
                continue;
            }
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        if value.get("persona_id").and_then(serde_json::Value::as_str) == Some(persona_id) {
            match std::fs::remove_file(&path) {
                Ok(()) => cleared.push(session_id),
                Err(error) => sweep_errors.push(format!("{session_id}: {error}")),
            }
        }
    }
    // read_dir order is arbitrary; a stable output keeps scripts and tests
    // deterministic.
    cleared.sort();
    sweep_errors.sort();
    Ok((cleared, sweep_errors))
}

/// Session ids join onto paths (the equip sidecar below), so apply the same
/// `[A-Za-z0-9_-]` restriction the sessions family enforces before any path
/// use; anything else is a usage error, never a traversal.
fn valid_session_id(id: &str) -> bool {
    crate::support::valid_session_id(id)
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
    let raw =
        crate::support::read_text_file_capped(&path, MAX_SIDECAR_BYTES, "personas equip").ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get("persona_id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
}

/// 4 MiB body cap × worst-case JSON escape expansion + wrapper/envelope.
const MAX_SIDECAR_BYTES: usize = 4 * 1024 * 1024 * 6 + 1024;
/// The body budget the sidecar cap is computed from — the same 4 MiB the
/// CLI's own persona write paths enforce.
const MAX_EQUIP_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Refuses a card whose RAW body is over the budget the sidecar cap is
/// computed from.
///
/// The delete sweep's capped read only covers bodies up to this budget; a
/// larger one (possible on a persona the desktop app wrote, whose writer has
/// no cap) would equip fine and then survive the delete as a ghost. Refuse at
/// equip time instead.
///
/// The cap is charged to `card.body`, not to `equip_body_injection(card)`.
/// `MAX_EQUIP_BODY_BYTES` is the same 4 MiB `read_body` enforces on the raw
/// body at `personas create`/`update`, and `MAX_SIDECAR_BYTES` is documented
/// as "body cap 4 MiB plus a few hundred bytes of wrapper text" — so charging
/// the wrapper (several hundred fixed bytes plus the card name) against the
/// body's own number made a card accepted at exactly the documented maximum
/// impossible to equip, with a message blaming the body for exceeding a limit
/// it does not exceed. The wrapper is still bounded: the serialized-sidecar
/// check in [`persist_equipped_persona`] covers the whole envelope, and that
/// is the one the sweep's read bound actually depends on.
fn require_equippable_body(card: &PersonaCard) -> Result<(), CliError> {
    if card.body.len() > MAX_EQUIP_BODY_BYTES {
        return Err(CliError::failed(format!(
            "personas equip: the persona body is {} bytes and exceeds the 4 MiB body budget the \
             session sidecar is sized for; shrink the body before equipping",
            card.body.len()
        )));
    }
    Ok(())
}

/// Persists the equip state for the next CLI invocation. The raw-body budget
/// is checked by [`require_equippable_body`] before the body is wrapped.
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
    // The sweep cap covers the whole serialized sidecar, not just the body:
    // the injection wrapper, an uncapped persona id and an uncapped card name
    // (the app's slugifier imposes no length limit on either) could otherwise
    // push a legal body's sidecar past the delete sweep's read bound and
    // recreate the ghost `require_equippable_body` prevents.
    if bytes.len() > MAX_SIDECAR_BYTES {
        return Err(CliError::failed(
            "personas equip: the serialized equip state exceeds the sidecar budget; \
             shrink the persona body or use a shorter persona",
        ));
    }
    // Stage + rename (same discipline as the app's atomic writes): a
    // concurrent `active` read must never observe a torn sidecar. Owner-only:
    // the sidecar embeds the card's full injection body, which for a
    // user-authored persona is text the user wrote and no other account on the
    // machine has a reason to read.
    crate::artifacts::atomic_write(&path, &bytes, crate::artifacts::WriteVisibility::OwnerOnly)
        .map_err(|error| CliError::failed(format!("cannot save session persona sidecar: {error}")))
}

/// Mirror of `equip_persona`: resolve the card, persist the equipped state on
/// the session sidecar and return the summary. Honest scope: the pending-body
/// injection store is process memory — only the desktop app's turn loop
/// consumes it — so a CLI equip records intent on the sidecar; no turn (GUI
/// or CLI) reads that file today, and a GUI equip is invisible to this
/// command. Revealed in the output below so the command cannot be mistaken
/// for live persona injection.
fn equip(session_id: &str, persona_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    // Usage before Failed: character validation first (a traversal id is a
    // usage error, via the sidecar-path check below), then session
    // existence, then the persona lookup — so a malformed session id reports
    // the usage error (exit 2) even when the persona id is unknown too, and
    // an unknown but well-formed session id fails instead of materializing a
    // stray `sessions/<bogus-id>/` directory for a session that does not
    // exist.
    //
    // That order includes `open_store()`, which runs `sandbox_home()` and
    // `SessionStore::boot()` — both Failed-class. Booting it first made the
    // exit-2 contract above depend on the store happening to boot cleanly;
    // `unequip` and `active` already validate before booting, and this lane
    // now matches them.
    equip_state_path(session_id)?;
    let store = open_store()?;
    store.load(session_id).map_err(|error| {
        CliError::failed(format!(
            "personas equip: session {session_id} does not exist ({error})"
        ))
    })?;
    let card = get(persona_id)
        .ok_or_else(|| CliError::failed(format!("unknown persona: {persona_id}")))?;
    require_equippable_body(&card)?;
    let summary = card.summary();
    let injection = equip_body_injection(&card);
    store.set_pending_persona_body(session_id, Some(injection.clone()));
    store.set_active_persona(session_id, Some(persona_id.to_owned()));
    persist_equipped_persona(session_id, persona_id, &injection)?;
    let mut value = summary_value(&summary, "equip")?;
    value["session_id"] = serde_json::json!(session_id);
    value["applies_to_next_turn"] = serde_json::json!(false);
    value["note"] = serde_json::json!(
        "equip state is recorded on the session sidecar; persona injection into turns happen only inside the running desktop app — equip the session there for live injection"
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
/// a missing sidecar is the already-unequipped case, but any other removal
/// failure must not be reported as success — the next `active` would still
/// resolve the persona).
fn unequip(session_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let path = equip_state_path(session_id)?;
    let store = open_store()?;
    // Same session-existence gate as `equip`: a well-formed but unknown id
    // must fail instead of reporting a successful unequip that cleared
    // nothing.
    store.load(session_id).map_err(|error| {
        CliError::failed(format!(
            "personas unequip: session {session_id} does not exist ({error})"
        ))
    })?;
    store.set_active_persona(session_id, None);
    store.set_pending_persona_body(session_id, None);
    if let Err(error) = std::fs::remove_file(&path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(CliError::failed(format!(
                "personas unequip: cannot remove the session persona sidecar: {error}"
            )));
        }
    }
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
///
/// Orphan reporting: the sidecar can outlive its card. The CLI's delete-time
/// sweep only runs on a CLI `personas delete`, and the desktop app's own
/// delete never touches `persona_equipped.json` (the filename appears nowhere
/// in `pinvou3-app`), so deleting a card in the app leaves every CLI sidecar
/// on disk carrying that card's full injection body — and a later
/// `personas delete <id> --yes` exits 1 on the existence gate before the sweep
/// can run. Degrading that state to `null` made the only remaining evidence
/// invisible. It is reported instead; `personas unequip <session-id>` clears
/// it without consulting the card pool, so the state is both visible and
/// clearable from the CLI alone.
fn active(session_id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    // Same gates as equip/unequip: reject ids that cannot name a session
    // directory before any path use, and fail an unknown but well-formed id
    // instead of answering "none" for a typo (the commit that added the
    // unequip gate claimed equip/active parity — this restores it).
    equip_state_path(session_id)?;
    let store = open_store()?;
    store.load(session_id).map_err(|error| {
        CliError::failed(format!(
            "personas active: session {session_id} does not exist ({error})"
        ))
    })?;
    let equipped = equipped_persona_id(session_id).or_else(|| store.active_persona_id(session_id));
    let summary = equipped
        .as_deref()
        .and_then(|persona_id| get(persona_id).map(|card| card.summary()));
    if let (Some(persona_id), None) = (equipped.as_deref(), summary.as_ref()) {
        // Equipped but unresolvable: the card was deleted from the desktop
        // app (or the pool file was removed by hand) while the sidecar stayed.
        // "none" would be a lie — the sidecar is there, it still holds the
        // deleted card's whole injection body at 0600, and nothing else in the
        // CLI reports it.
        return Err(CliError::failed(format!(
            "personas active: session {session_id} has persona {persona_id} equipped but that \
             card no longer exists; its sidecar still holds the card's full injection body — \
             clear it with `pinvou personas unequip {session_id}`"
        )));
    }
    let human = match &summary {
        Some(summary) => active_row(summary),
        None => "none".to_owned(),
    };
    // `null` is this command's answer for "no persona equipped", so it cannot
    // double as the answer for "the equipped persona could not be rendered":
    // a caller branching on `value === null` would unequip, or skip a setup
    // step, for a session that does have a card. Fail instead.
    let value = match &summary {
        Some(summary) => summary_value(summary, "active")?,
        None => serde_json::Value::Null,
    };
    Ok(success(render(output, human, &value)))
}

/// Serializes a `PersonaSummary` for the JSON lane, propagating a failure.
///
/// Every call site previously degraded to `json!({})` (or, in `active`, to
/// `null`) and still exited 0. That turns a serializer failure into a
/// successful *wrong* answer: `.id` reads back as `null`, and a script that
/// keys on the id silently operates on nothing. `monitor snapshot` already
/// treats the identical operation as fallible; this is the same rule for the
/// persona lanes. There is no useful partial answer here — the summary either
/// renders or the command failed.
fn summary_value(summary: &PersonaSummary, context: &str) -> Result<serde_json::Value, CliError> {
    serde_json::to_value(summary)
        .map_err(|error| CliError::failed(format!("personas {context}: {error}")))
}

fn open_store() -> Result<SessionStore, CliError> {
    // Same absolute-path contract as the other families: a relative
    // PINVOU3_HOME would silently resolve against the cwd.
    crate::support::sandbox_home()?;
    SessionStore::boot()
        .map_err(|error| CliError::failed(format!("sessions store unavailable: {error:#}")))
}

/// Read the persona body from `--file` or stdin; bodies are multi-KB
/// markdown, so argv delivery is deliberately not offered. Both lanes are
/// capped at 4 MiB: an unbounded file (`--file /dev/zero`) or stdin
/// (`yes | ...`) would exhaust memory before any validation ran.
fn read_body(source: &BodySource, subcommand: &str) -> Result<String, CliError> {
    const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
    let content = match source {
        BodySource::File(path) => crate::support::read_text_file_capped(
            path,
            MAX_BODY_BYTES,
            &format!("personas {subcommand}"),
        )?,
        BodySource::Stdin => {
            // Reading one byte past the cap distinguishes "at the cap" from
            // "over it".
            let mut content = String::new();
            let stdin = std::io::stdin();
            let mut handle = stdin.lock();
            if let Err(error) = std::io::Read::read_to_string(
                &mut std::io::Read::take(&mut handle, MAX_BODY_BYTES as u64 + 1),
                &mut content,
            ) {
                return Err(CliError::failed(format!(
                    "personas {subcommand}: cannot read stdin: {error}"
                )));
            }
            if content.len() > MAX_BODY_BYTES {
                // Input size is a content error (exit 1), not an invocation
                // error — the same classification as the artifacts/feedback
                // read caps.
                return Err(CliError::failed(format!(
                    "personas {subcommand}: the persona body exceeds the 4 MiB stdin limit"
                )));
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_list(arguments: &[&str]) -> Result<PersonasCommand, CliError> {
        let owned: Vec<String> = arguments.iter().map(|value| (*value).to_owned()).collect();
        parse(&owned)
    }

    /// The CLI reference used to advertise `list [--source embedded|user]`
    /// while the parser only ever accepted `builtin|user|all`, so a script
    /// written against that documentation exited 2. The reference now teaches
    /// `builtin|user|all`; the alias stays as a compatibility input spelling
    /// for the scripts already written, and resolves to the same filter as
    /// `builtin`.
    #[test]
    fn list_source_accepts_the_documented_embedded_alias() {
        assert_eq!(
            parse_list(&["personas", "list", "--source", "embedded"]).unwrap(),
            PersonasCommand::List {
                source: SourceFilter::Builtin,
            }
        );
        assert_eq!(
            parse_list(&["personas", "list", "--source", "builtin"]).unwrap(),
            PersonasCommand::List {
                source: SourceFilter::Builtin,
            }
        );
    }

    /// The alias is an input spelling only: the echoed `source` field has to
    /// keep matching the value the cards themselves carry.
    #[test]
    fn the_embedded_alias_still_reports_the_canonical_source() {
        assert_eq!(SourceFilter::Builtin.as_str(), "builtin");
    }

    /// Unknown values stay a usage error, and the message names the alias so
    /// the accepted set is discoverable from the CLI itself.
    #[test]
    fn list_source_still_rejects_unknown_values() {
        let error = parse_list(&["personas", "list", "--source", "venv"])
            .expect_err("an unknown source must be a usage error");
        assert_eq!(error.exit_code(), crate::ExitCode::Usage);
        assert!(error.to_string().contains("embedded"), "{error}");
    }

    fn summary_fixture(id: &str, name: &str, dept: &str, description: &str) -> PersonaSummary {
        PersonaCard {
            id: id.to_owned(),
            dept: dept.to_owned(),
            name: name.to_owned(),
            description: description.to_owned(),
            emoji: "🃏".to_owned(),
            color: "#7C3AED".to_owned(),
            body: "body".to_owned(),
            source: "user".to_owned(),
            conversational_only: false,
        }
        .summary()
    }

    /// The human `personas list` row is a five-column tab-separated record and
    /// `personas_contract.rs` asserts that shape. Persona names, departments
    /// and descriptions are untrusted — `create_user_persona` only rejects an
    /// empty trimmed name, the CLI takes `--name`/`--description` verbatim
    /// from argv, and `load_user_cards` deserializes arbitrary
    /// `~/.pinvou3/user/personas/*.json` — so without the column collapse one
    /// card would render as two rows with six columns between them and the
    /// ESC would reach the terminal. Sibling rule and sibling test in
    /// `projects.rs`.
    #[test]
    fn list_row_keeps_five_columns_when_the_card_text_carries_control_characters() {
        let row = persona_row(&summary_fixture(
            "user-a\tb",
            "Alpha\tBeta\nGamma\x1b[31m",
            "special\nized",
            "does\tthings\x07",
        ));
        assert_eq!(
            row.lines().count(),
            1,
            "the row must stay one line: {row:?}"
        );
        let columns: Vec<&str> = row.split('\t').collect();
        assert_eq!(columns.len(), 5, "the row must keep five columns: {row:?}");
        assert_eq!(columns[0], "user-a b");
        assert_eq!(columns[1], "user");
        assert_eq!(columns[2], "Alpha Beta Gamma [31m");
        assert_eq!(columns[3], "special ized");
        assert_eq!(columns[4], "does things ");
        assert!(
            !row.contains('\x1b'),
            "ESC must not reach the terminal: {row:?}"
        );
    }

    /// `personas active` renders the same untrusted cells one command away
    /// from `list`; a name that is safe in one and raw in the other would be
    /// the CLI contradicting itself.
    #[test]
    fn active_row_keeps_three_columns_when_the_card_text_carries_control_characters() {
        let row = active_row(&summary_fixture(
            "user-a",
            "Alpha\tBeta\nGamma\x1b[31m",
            "specialized",
            "-",
        ));
        assert_eq!(
            row.lines().count(),
            1,
            "the row must stay one line: {row:?}"
        );
        let columns: Vec<&str> = row.split('\t').collect();
        assert_eq!(columns.len(), 3, "the row must keep three columns: {row:?}");
        assert_eq!(columns[0], "user-a");
        assert_eq!(columns[1], "Alpha Beta Gamma [31m");
        assert_eq!(columns[2], "user");
    }

    /// The collapse is a rendering choice, not a data change: ordinary text
    /// must render byte-for-byte, and an empty description must keep the "-"
    /// placeholder rather than an empty cell.
    #[test]
    fn list_row_leaves_ordinary_text_and_empty_descriptions_untouched() {
        let row = persona_row(&summary_fixture("user-a", "Alpha", "specialized", ""));
        assert_eq!(row, "user-a\tuser\tAlpha\tspecialized\t-");
    }

    /// The equip budget is charged to the RAW body, which is the number
    /// `personas create`/`update` enforce. Charging the injection wrapper
    /// against the same 4 MiB made a card accepted at exactly the documented
    /// maximum impossible to equip — the two commands must agree on what
    /// "4 MiB" means.
    #[test]
    fn a_body_at_exactly_the_documented_cap_is_still_equippable() {
        let mut card = PersonaCard {
            id: "user-cap".to_owned(),
            dept: "specialized".to_owned(),
            name: "Cap".to_owned(),
            description: String::new(),
            emoji: "🃏".to_owned(),
            color: "#7C3AED".to_owned(),
            body: "x".repeat(MAX_EQUIP_BODY_BYTES),
            source: "user".to_owned(),
            conversational_only: false,
        };
        // The wrapper really does push the injection past the raw cap — that
        // is the whole bug, so pin it rather than assuming it.
        assert!(
            equip_body_injection(&card).len() > MAX_EQUIP_BODY_BYTES,
            "the injection wrapper must be what the old check charged for"
        );
        require_equippable_body(&card).expect("a body at exactly the cap must be equippable");
        // One byte over the raw cap is still refused, and the message names
        // the body rather than the sidecar envelope.
        card.body.push('x');
        let error = require_equippable_body(&card).expect_err("over the cap must be refused");
        assert_eq!(error.exit_code(), crate::ExitCode::Failed);
        assert!(error.to_string().contains("4 MiB body budget"), "{error}");
    }

    /// Bug-3 guard: the persona lanes must never publish a placeholder value
    /// on a serialization failure. `PersonaSummary` always renders, so the
    /// assertion is that the helper is `Result`-typed and yields the real
    /// object — the call sites can no longer reach for `json!({})`/`null`
    /// without a compile error.
    #[test]
    fn summary_value_returns_the_real_object_rather_than_a_placeholder() {
        let card = PersonaCard {
            id: "user-test".to_owned(),
            dept: "specialized".to_owned(),
            name: "Test".to_owned(),
            description: "d".to_owned(),
            emoji: "🃏".to_owned(),
            color: "#7C3AED".to_owned(),
            body: "body".to_owned(),
            source: "user".to_owned(),
            conversational_only: false,
        };
        let value = summary_value(&card.summary(), "create").expect("a summary must render");
        assert_eq!(value["id"], "user-test");
        assert!(
            !value.as_object().expect("an object").is_empty(),
            "an empty object would be the old silent-failure answer"
        );
    }
}
