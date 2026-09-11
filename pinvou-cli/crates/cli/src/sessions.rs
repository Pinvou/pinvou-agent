//! `sessions` family: chat / scheduled-run history inspection and metadata
//! management, mirroring `pinvou3-app/src-tauri/src/app/commands/sessions.rs`.
//!
//! Pure-storage only: the store is opened with `SessionStore::boot()` (the
//! same standalone constructor the GUI-side tests use); no Tauri host and no
//! engine is started. When the GUI command layer relies on a `pub(crate)`
//! helper (timeline reader, session-id validator, hidden/pinned sidecar
//! logic), this module mirrors it over the exact same `pinvou3_lib` paths:
//! - list/show/rename/pin/unpin/archive/restore/delete →
//!   `pinvou3_lib::features::sessions::SessionStore` public methods
//!   (`list`, `list_scheduled`, `load`, `set_title`, `set_pinned`,
//!   `set_hidden`, `is_pinned`, `is_hidden`, `delete`, `session_kind`),
//!   the same calls the GUI commands make after their `State<'_, SessionStore>`
//!   extraction.
//! - timeline → `pinvou3_lib::platform::paths::session_timing_events` (the
//!   path used by `features::assistant::timing::read_timeline`, which is
//!   crate-private), parsed with the same tolerant one-JSON-object-per-line
//!   rules and ascending-timestamp ordering.
//! - subagents → `pinvou3_lib::features::multiagent::transcripts::list`, the
//!   exact read-only function behind the GUI `list_subagent_transcripts`
//!   command, with the session ledger root and `engine_epoch_ms = None`
//!   (the CLI never owns a live engine, so non-terminal workers report as
//!   interrupted, matching a stopped GUI process).

use std::io::BufRead;
use std::path::PathBuf;

use crate::support::{render, require_yes, sandbox_home, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::sessions::{SessionKind, SessionStore};

const SHOW_PREVIEW_CHARS: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionsCommand {
    List {
        archived: bool,
        limit: Option<usize>,
    },
    Show {
        id: String,
        last: Option<usize>,
        full: bool,
    },
    Rename {
        id: String,
        title: String,
    },
    Pin {
        id: String,
    },
    Unpin {
        id: String,
    },
    Archive {
        id: String,
    },
    Restore {
        id: String,
    },
    Delete {
        id: String,
        yes: bool,
    },
    Export {
        id: String,
        format: ExportFormat,
        output: Option<PathBuf>,
    },
    Timeline {
        id: String,
    },
    Subagents {
        id: String,
    },
    Folder {
        id: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Json,
}

impl ExportFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Json => "json",
        }
    }
}

/// Flags that carry a value, per subcommand.
const LIST_OPTIONS: &[&str] = &["--limit"];
const SHOW_OPTIONS: &[&str] = &["--last"];
const EXPORT_OPTIONS: &[&str] = &["--format", "--output"];

/// Boolean (valueless) flags, per subcommand.
const LIST_FLAGS: &[&str] = &["--archived"];
const SHOW_FLAGS: &[&str] = &["--full"];
const DELETE_FLAGS: &[&str] = &["--yes"];

pub fn parse(values: &[String]) -> Result<SessionsCommand, CliError> {
    let subcommand = values.get(1).ok_or_else(|| CliError::usage(USAGE))?;
    let rest = &values[2..];
    match subcommand.as_str() {
        "list" => {
            let (options, flags) = parse_flags(rest, LIST_OPTIONS, LIST_FLAGS)?;
            let limit = parse_positive(&options, "--limit")?;
            Ok(SessionsCommand::List {
                archived: flags.contains(&"--archived"),
                limit,
            })
        }
        "show" => {
            let id = require_id(rest.first())?;
            let (options, flags) = parse_flags(&rest[1..], SHOW_OPTIONS, SHOW_FLAGS)?;
            Ok(SessionsCommand::Show {
                id,
                last: parse_positive(&options, "--last")?,
                full: flags.contains(&"--full"),
            })
        }
        "rename" => {
            let id = require_id(rest.first())?;
            let title = rest.get(1..).unwrap_or_default().join(" ");
            if title.trim().is_empty() {
                return Err(CliError::usage("sessions rename requires a title"));
            }
            Ok(SessionsCommand::Rename { id, title })
        }
        "pin" | "unpin" | "archive" | "restore" => {
            let id = require_id(rest.first())?;
            if rest.len() > 1 {
                return Err(CliError::usage(format!(
                    "sessions {subcommand} accepts no options"
                )));
            }
            Ok(match subcommand.as_str() {
                "pin" => SessionsCommand::Pin { id },
                "unpin" => SessionsCommand::Unpin { id },
                "archive" => SessionsCommand::Archive { id },
                _ => SessionsCommand::Restore { id },
            })
        }
        "delete" => {
            let id = require_id(rest.first())?;
            let (_, flags) = parse_flags(&rest[1..], &[], DELETE_FLAGS)?;
            Ok(SessionsCommand::Delete {
                id,
                yes: flags.contains(&"--yes"),
            })
        }
        "export" => {
            let id = require_id(rest.first())?;
            let (options, _) = parse_flags(&rest[1..], EXPORT_OPTIONS, &[])?;
            let format = match option(&options, "--format") {
                None => ExportFormat::Markdown,
                Some("markdown") => ExportFormat::Markdown,
                Some("json") => ExportFormat::Json,
                Some(other) => {
                    return Err(CliError::usage(format!(
                        "sessions export --format must be markdown or json (got {other})"
                    )));
                }
            };
            let output = option(&options, "--output").map(PathBuf::from);
            Ok(SessionsCommand::Export { id, format, output })
        }
        "timeline" | "subagents" | "folder" => {
            let id = require_id(rest.first())?;
            if rest.len() > 1 {
                return Err(CliError::usage(format!(
                    "sessions {subcommand} accepts no options"
                )));
            }
            Ok(match subcommand.as_str() {
                "timeline" => SessionsCommand::Timeline { id },
                "subagents" => SessionsCommand::Subagents { id },
                _ => SessionsCommand::Folder { id },
            })
        }
        _ => Err(CliError::usage(USAGE)),
    }
}

const USAGE: &str = "usage: pinvou sessions <list|show|rename|pin|unpin|archive|restore|delete|export|timeline|subagents|folder>";

fn require_id(value: Option<&String>) -> Result<String, CliError> {
    let id = value
        .ok_or_else(|| CliError::usage("sessions command requires a session id"))?
        .clone();
    if id.is_empty() {
        return Err(CliError::usage("sessions command requires a session id"));
    }
    Ok(id)
}

/// Mirrors the pair-based `named_options` helper in lib.rs, extended with
/// valueless boolean flags: every token must be a known value flag (followed
/// by a non-empty value), a known boolean flag, or nothing.
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
                    "duplicate sessions option {token}"
                )));
            }
            flags.push(token);
            index += 1;
            continue;
        }
        if !value_flags.contains(&token) {
            return Err(CliError::usage(format!(
                "unsupported sessions option: {token}"
            )));
        }
        if options.iter().any(|(name, _)| *name == token) {
            return Err(CliError::usage(format!(
                "duplicate sessions option {token}"
            )));
        }
        let value = values
            .get(index + 1)
            .ok_or_else(|| CliError::usage(format!("sessions option {token} requires a value")))?;
        if value.is_empty() || value.starts_with("--") {
            return Err(CliError::usage(format!(
                "sessions option {token} requires a value"
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

fn parse_positive(options: &[(&str, &str)], name: &str) -> Result<Option<usize>, CliError> {
    match option(options, name) {
        None => Ok(None),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|count| *count > 0)
            .map(Some)
            .ok_or_else(|| CliError::usage(format!("sessions {name} must be a positive integer"))),
    }
}

/// Mirrors `features::sessions::validate_session_id` (crate-private in the
/// app): only `[A-Za-z0-9_-]`, so the id can never traverse out of the
/// sessions root when it is joined onto a path.
fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn open_store() -> Result<SessionStore, CliError> {
    SessionStore::boot()
        .map_err(|error| CliError::failed(format!("sessions store unavailable: {error:#}")))
}

fn store_error(action: &str, id: &str, error: impl std::fmt::Display) -> CliError {
    CliError::failed(format!("sessions {action}({id}): {error:#}"))
}

fn require_existing(store: &SessionStore, id: &str, action: &str) -> Result<(), CliError> {
    store
        .load(id)
        .map(|_| ())
        .map_err(|error| store_error(action, id, error))
}

fn kind_label(store: &SessionStore, id: &str) -> &'static str {
    match store.session_kind(id) {
        Ok(SessionKind::Chat) => "chat",
        Ok(SessionKind::ScheduledRun) => "scheduled-run",
        // A transient profiles-file error must not mislabel a chat session
        // as a scheduled run; surface it as unknown instead.
        Err(_) => "unknown",
    }
}

pub fn execute(command: SessionsCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    match command {
        SessionsCommand::List { archived, limit } => list(archived, limit, output),
        SessionsCommand::Show { id, last, full } => show(&id, last, full, output),
        SessionsCommand::Rename { id, title } => rename(&id, &title, output),
        SessionsCommand::Pin { id } => set_pinned(&id, true, output),
        SessionsCommand::Unpin { id } => set_pinned(&id, false, output),
        SessionsCommand::Archive { id } => set_hidden(&id, true, output),
        SessionsCommand::Restore { id } => set_hidden(&id, false, output),
        SessionsCommand::Delete { id, yes } => delete(&id, yes, output),
        SessionsCommand::Export {
            id,
            format,
            output: destination,
        } => export(&id, format, destination, output),
        SessionsCommand::Timeline { id } => timeline(&id, output),
        SessionsCommand::Subagents { id } => subagents(&id, output),
        SessionsCommand::Folder { id } => folder(&id, output),
    }
}

/// One rendered row of `sessions list`: the same fields the GUI history
/// panels read (`SessionListItem` / `HiddenSessionListItem`), flattened.
struct SessionRow {
    id: String,
    title: String,
    updated_at: String,
    pinned: bool,
    archived: bool,
    kind: &'static str,
}

fn collect_rows(store: &SessionStore, archived: bool) -> Result<Vec<SessionRow>, CliError> {
    let mut metas = store
        .list()
        .map_err(|error| store_error("list", "-", error))?;
    if archived {
        // Same merge as the GUI `list_archived_sessions`: scheduled-run
        // sessions live in the same durable store but need the extra listing.
        let scheduled = store
            .list_scheduled()
            .map_err(|error| store_error("list", "-", error))?;
        metas.extend(scheduled);
        metas.retain(|metadata| store.is_hidden(&metadata.id));
        metas.sort_by_key(|metadata| std::cmp::Reverse(metadata.updated_at));
    } else {
        metas.retain(|metadata| !store.is_hidden(&metadata.id));
    }
    Ok(metas
        .into_iter()
        .map(|metadata| SessionRow {
            id: metadata.id.clone(),
            title: metadata.title.clone(),
            updated_at: metadata.updated_at.to_rfc3339(),
            pinned: store.is_pinned(&metadata.id),
            archived: store.is_hidden(&metadata.id),
            kind: kind_label(store, &metadata.id),
        })
        .collect())
}

fn list(archived: bool, limit: Option<usize>, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    let mut rows = collect_rows(&store, archived)?;
    if let Some(limit) = limit {
        rows.truncate(limit);
    }
    let value = serde_json::json!({
        "sessions": rows.iter().map(|row| serde_json::json!({
            "id": row.id,
            "title": row.title,
            "updated_at": row.updated_at,
            "pinned": row.pinned,
            "archived": row.archived,
            "kind": row.kind,
        })).collect::<Vec<_>>(),
    });
    let human = rows
        .iter()
        .map(|row| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                row.id,
                if row.pinned { "pinned" } else { "-" },
                if row.archived { "archived" } else { "-" },
                row.kind,
                row.updated_at,
                row.title,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(success(render(output, human, &value)))
}

/// Full transcript text of one message, concatenated over its text blocks;
/// non-text blocks render as a `[type]` placeholder. `value` is one entry of
/// the serialized `SavedSession.messages` array.
fn message_text(message: &serde_json::Value) -> String {
    let mut parts = Vec::new();
    if let Some(blocks) = message.get("content").and_then(|value| value.as_array()) {
        for block in blocks {
            match block.get("type").and_then(|value| value.as_str()) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(|value| value.as_str()) {
                        parts.push(text.to_owned());
                    }
                }
                Some(other) => parts.push(format!("[{other}]")),
                None => {}
            }
        }
    }
    parts.join("\n")
}

fn preview(text: &str, full: bool) -> (String, bool) {
    if full || text.chars().count() <= SHOW_PREVIEW_CHARS {
        return (text.to_owned(), false);
    }
    let truncated: String = text.chars().take(SHOW_PREVIEW_CHARS).collect();
    (format!("{truncated} …"), true)
}

fn show(
    id: &str,
    last: Option<usize>,
    full: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    let session = store
        .load(id)
        .map_err(|error| store_error("show", id, error))?;
    let value = serde_json::to_value(&session)
        .map_err(|error| CliError::failed(format!("sessions show({id}): {error}")))?;
    let kind = kind_label(&store, id);
    let mut messages = value
        .get("messages")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    if let Some(last) = last {
        let start = messages.len().saturating_sub(last);
        messages.drain(..start);
    }
    let rendered = messages
        .iter()
        .map(|message| {
            let role = message
                .get("role")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            let (text, truncated) = preview(&message_text(message), full);
            serde_json::json!({
                "role": role,
                "text": text,
                "truncated": truncated,
            })
        })
        .collect::<Vec<_>>();
    let mut human = format!(
        "id: {}\ntitle: {}\nkind: {}\nupdated: {}\nmessages: {}",
        id,
        value
            .pointer("/metadata/title")
            .and_then(|value| value.as_str())
            .unwrap_or(""),
        kind,
        value
            .pointer("/metadata/updated_at")
            .and_then(|value| value.as_str())
            .unwrap_or(""),
        rendered.len(),
    );
    for (index, message) in rendered.iter().enumerate() {
        human.push_str(&format!(
            "\n\n[{}] {}\n{}",
            index + 1,
            message["role"].as_str().unwrap_or("unknown"),
            message["text"].as_str().unwrap_or(""),
        ));
    }
    let json = serde_json::json!({
        "id": id,
        "title": value.pointer("/metadata/title"),
        "kind": kind,
        "updated_at": value.pointer("/metadata/updated_at"),
        "created_at": value.pointer("/metadata/created_at"),
        "message_count": rendered.len(),
        "messages": rendered,
    });
    Ok(success(render(output, human, &json)))
}

fn rename(id: &str, title: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    store
        .set_title(id, title.to_owned())
        .map_err(|error| store_error("rename", id, error))?;
    let value = serde_json::json!({ "id": id, "action": "renamed", "title": title });
    Ok(success(render(
        output,
        format!("renamed {id}: {title}"),
        &value,
    )))
}

/// Mirrors the GUI `set_session_pinned` / `set_session_archived` guards: the
/// session must exist first so the sidecar tables never keep a stale id.
fn set_pinned(id: &str, pinned: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    require_existing(&store, id, "pin")?;
    store.set_pinned(id, pinned);
    let action = if pinned { "pinned" } else { "unpinned" };
    let value = serde_json::json!({ "id": id, "action": action });
    Ok(success(render(output, format!("{action} {id}"), &value)))
}

fn set_hidden(id: &str, hidden: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    require_existing(&store, id, "archive")?;
    store.set_hidden(id, hidden);
    let action = if hidden { "archived" } else { "restored" };
    let value = serde_json::json!({ "id": id, "action": action });
    Ok(success(render(output, format!("{action} {id}"), &value)))
}

fn delete(id: &str, yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    let store = open_store()?;
    // The store delete path mirrors the GUI chat-session cascade (session
    // JSON + artifacts directory) and refuses scheduled-run sessions, which
    // the GUI deletes through their automation only.
    store
        .delete(id)
        .map_err(|error| store_error("delete", id, error))?;
    let value = serde_json::json!({ "id": id, "action": "deleted" });
    Ok(success(render(output, format!("deleted {id}"), &value)))
}

fn export(
    id: &str,
    format: ExportFormat,
    destination: Option<PathBuf>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    let session = store
        .load(id)
        .map_err(|error| store_error("export", id, error))?;
    let value = serde_json::to_value(&session)
        .map_err(|error| CliError::failed(format!("sessions export({id}): {error}")))?;
    let content = match format {
        ExportFormat::Json => serde_json::to_string_pretty(&value)
            .map_err(|error| CliError::failed(format!("sessions export({id}): {error}")))?,
        ExportFormat::Markdown => render_markdown(id, &value),
    };
    match destination {
        Some(path) => {
            let bytes = content.len();
            std::fs::write(&path, content).map_err(|error| {
                CliError::failed(format!(
                    "sessions export({id}): cannot write {}: {error}",
                    path.display()
                ))
            })?;
            let value = serde_json::json!({
                "id": id,
                "format": format.as_str(),
                "output": path.display().to_string(),
                "bytes": bytes,
            });
            Ok(success(render(
                output,
                format!("exported {id} -> {} ({bytes} bytes)", path.display()),
                &value,
            )))
        }
        None => {
            let value = serde_json::json!({
                "id": id,
                "format": format.as_str(),
                "content": content,
            });
            Ok(success(render(output, content, &value)))
        }
    }
}

/// Human-readable transcript with role headers, built from the serialized
/// `SavedSession` (messages carry `role` plus tagged `content` blocks).
fn render_markdown(id: &str, value: &serde_json::Value) -> String {
    let title = value
        .pointer("/metadata/title")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let created = value
        .pointer("/metadata/created_at")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let updated = value
        .pointer("/metadata/updated_at")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let model = value
        .pointer("/metadata/model")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let messages = value
        .get("messages")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = format!(
        "# {title}\n\n- id: {id}\n- created: {created}\n- updated: {updated}\n- model: {model}\n- messages: {}\n",
        messages.len()
    );
    for message in &messages {
        let role = message
            .get("role")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        out.push_str(&format!("\n## {role}\n\n{}\n", message_text(message)));
    }
    out
}

/// Per-turn timing/usage events from `timing_events.jsonl`, tolerating
/// corrupt lines the same way `features::assistant::timing::read_timeline`
/// does: skip anything that is not a JSON object, sort by `timestamp`.
fn timeline(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    if !valid_session_id(id) {
        return Err(CliError::usage("invalid session id"));
    }
    let path = pinvou3_lib::platform::paths::session_timing_events(id);
    let mut events = Vec::new();
    match std::fs::File::open(&path) {
        Ok(file) => {
            // Same 32 MiB cap as `timing::read_timeline` (crate-private): a
            // runaway sidecar must not be read fully into memory. The file
            // may grow after the metadata check, so the limit is re-checked
            // per line like the GUI reader does.
            const MAX_TIMING_FILE_BYTES: u64 = 32 * 1024 * 1024;
            let file_len = file
                .metadata()
                .map_err(|error| {
                    CliError::failed(format!(
                        "sessions timeline({id}): cannot stat {}: {error}",
                        path.display()
                    ))
                })?
                .len();
            if file_len > MAX_TIMING_FILE_BYTES {
                return Err(CliError::failed(format!(
                    "sessions timeline({id}): timing sidecar too large: {file_len} bytes \
                     (limit {MAX_TIMING_FILE_BYTES})"
                )));
            }
            let mut bytes_read = 0_u64;
            for line in std::io::BufReader::new(file).lines() {
                let line = line.map_err(|error| {
                    CliError::failed(format!(
                        "sessions timeline({id}): cannot read {}: {error}",
                        path.display()
                    ))
                })?;
                bytes_read = bytes_read.saturating_add(line.len() as u64 + 1);
                if bytes_read > MAX_TIMING_FILE_BYTES {
                    return Err(CliError::failed(format!(
                        "sessions timeline({id}): timing sidecar grew beyond \
                         {MAX_TIMING_FILE_BYTES} bytes while reading"
                    )));
                }
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                    if value.is_object() {
                        events.push(value);
                    }
                }
            }
            events.sort_by_key(|event| {
                event
                    .get("timestamp")
                    .and_then(|value| value.as_i64())
                    .unwrap_or(0)
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CliError::failed(format!(
                "sessions timeline({id}): cannot read {}: {error}",
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
                    .get("ts")
                    .and_then(|value| value.as_str())
                    .unwrap_or(""),
                event
                    .get("event")
                    .and_then(|value| value.as_str())
                    .unwrap_or(""),
                event
                    .get("turn_id")
                    .and_then(|value| value.as_str())
                    .unwrap_or(""),
                event
                    .get("status")
                    .and_then(|value| value.as_str())
                    .unwrap_or("-"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let value = serde_json::json!({ "id": id, "events": events });
    Ok(success(render(output, human, &value)))
}

/// Read-only subagent transcript listing: the exact
/// `features::multiagent::transcripts::list` call the GUI
/// `list_subagent_transcripts` command makes, with the session ledger root
/// (the GUI's `session_state_root`) and no live-engine epoch.
fn subagents(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    if !valid_session_id(id) {
        return Err(CliError::usage("invalid session id"));
    }
    let store = open_store()?;
    require_existing(&store, id, "subagents")?;
    let ledger = store
        .ledger_root(id)
        .map_err(|error| store_error("subagents", id, error))?;
    let summaries = pinvou3_lib::features::multiagent::transcripts::list(&ledger, None)
        .map_err(|error| store_error("subagents", id, error))?;
    let human = summaries
        .iter()
        .map(|summary| {
            let state = if !summary.has_transcript {
                "queued"
            } else if summary.failed {
                "failed"
            } else if summary.blocked {
                "blocked"
            } else if summary.done {
                "done"
            } else {
                "running"
            };
            format!(
                "{}\t{}\t{}\t{}",
                summary.agent_id,
                state,
                summary.objective.as_deref().unwrap_or("-"),
                summary.error.as_deref().unwrap_or("-"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let value = serde_json::to_value(&summaries)
        .map(|summaries| serde_json::json!({ "id": id, "subagents": summaries }))
        .unwrap_or_else(|_| serde_json::json!({ "id": id, "subagents": [] }));
    Ok(success(render(output, human, &value)))
}

/// Same folder the GUI `reveal_session_folder` resolves: scheduled-run
/// sessions have no per-session runtime directory, so their shared task
/// workspace (the ledger root) is the folder; chat sessions map to
/// `sessions_root()/<id>`.
fn folder(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    if !valid_session_id(id) {
        return Err(CliError::usage("invalid session id"));
    }
    // Validate the sandbox-home contract before touching the store so a bad
    // PINVOU3_HOME is refused instead of resolving session paths against the
    // current working directory.
    let home = sandbox_home()?;
    let store = open_store()?;
    require_existing(&store, id, "folder")?;
    let path = if store.scheduled_profile(id).is_some() {
        store
            .ledger_root(id)
            .map_err(|error| store_error("folder", id, error))?
    } else {
        home.join("sessions").join(id)
    };
    let value = serde_json::json!({ "id": id, "path": path.display().to_string() });
    Ok(success(render(output, path.display().to_string(), &value)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CliCommand, parse_args};

    fn parse(arguments: &[&str]) -> Result<SessionsCommand, CliError> {
        let mut owned: Vec<String> = arguments.iter().map(|value| value.to_string()).collect();
        // arguments start at the subcommand token; parse_args strips argv[0].
        owned.insert(0, "sessions".to_owned());
        owned.insert(0, "pinvou".to_owned());
        sessions_parse(&owned)
    }

    fn sessions_parse(values: &[String]) -> Result<SessionsCommand, CliError> {
        match parse_args(values)?.command() {
            CliCommand::Sessions(command) => Ok(command.clone()),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_every_subcommand() {
        assert_eq!(
            parse(&["list"]).unwrap(),
            SessionsCommand::List {
                archived: false,
                limit: None,
            }
        );
        assert_eq!(
            parse(&["list", "--archived", "--limit", "5"]).unwrap(),
            SessionsCommand::List {
                archived: true,
                limit: Some(5),
            }
        );
        assert_eq!(
            parse(&["show", "s-1"]).unwrap(),
            SessionsCommand::Show {
                id: "s-1".into(),
                last: None,
                full: false,
            }
        );
        assert_eq!(
            parse(&["show", "s-1", "--last", "3", "--full"]).unwrap(),
            SessionsCommand::Show {
                id: "s-1".into(),
                last: Some(3),
                full: true,
            }
        );
        assert_eq!(
            parse(&["rename", "s-1", "new", "title"]).unwrap(),
            SessionsCommand::Rename {
                id: "s-1".into(),
                title: "new title".into(),
            }
        );
        for (name, expected) in [
            ("pin", SessionsCommand::Pin { id: "s-1".into() }),
            ("unpin", SessionsCommand::Unpin { id: "s-1".into() }),
            ("archive", SessionsCommand::Archive { id: "s-1".into() }),
            ("restore", SessionsCommand::Restore { id: "s-1".into() }),
        ] {
            assert_eq!(parse(&[name, "s-1"]).unwrap(), expected, "{name}");
        }
        assert_eq!(
            parse(&["delete", "s-1", "--yes"]).unwrap(),
            SessionsCommand::Delete {
                id: "s-1".into(),
                yes: true,
            }
        );
        // --yes stays parseable-but-unconfirmed: the exit-2 contract is
        // enforced by support::require_yes at execute time.
        assert_eq!(
            parse(&["delete", "s-1"]).unwrap(),
            SessionsCommand::Delete {
                id: "s-1".into(),
                yes: false,
            }
        );
        assert_eq!(
            parse(&["export", "s-1"]).unwrap(),
            SessionsCommand::Export {
                id: "s-1".into(),
                format: ExportFormat::Markdown,
                output: None,
            }
        );
        assert_eq!(
            parse(&["export", "s-1", "--format", "json", "--output", "out.json"]).unwrap(),
            SessionsCommand::Export {
                id: "s-1".into(),
                format: ExportFormat::Json,
                output: Some(PathBuf::from("out.json")),
            }
        );
        assert_eq!(
            parse(&["timeline", "s-1"]).unwrap(),
            SessionsCommand::Timeline { id: "s-1".into() }
        );
        assert_eq!(
            parse(&["subagents", "s-1"]).unwrap(),
            SessionsCommand::Subagents { id: "s-1".into() }
        );
        assert_eq!(
            parse(&["folder", "s-1"]).unwrap(),
            SessionsCommand::Folder { id: "s-1".into() }
        );
    }

    #[test]
    fn rejects_invalid_usage_with_exit_code_two() {
        let invalid = [
            vec!["sessions"],
            vec!["sessions", "bogus"],
            vec!["sessions", "list", "--bogus"],
            vec!["sessions", "list", "--limit", "0"],
            vec!["sessions", "list", "--limit", "x"],
            vec!["sessions", "list", "--limit"],
            vec!["sessions", "list", "--archived", "--archived"],
            vec!["sessions", "show"],
            vec!["sessions", "show", "s-1", "--nope"],
            vec!["sessions", "show", "s-1", "--last"],
            vec!["sessions", "rename"],
            vec!["sessions", "rename", "s-1"],
            vec!["sessions", "rename", "s-1", "   "],
            vec!["sessions", "pin"],
            vec!["sessions", "pin", "s-1", "--extra"],
            vec!["sessions", "delete", "s-1", "--nope"],
            vec!["sessions", "export", "s-1", "--format", "html"],
            vec!["sessions", "export", "s-1", "--format"],
            vec!["sessions", "timeline"],
            vec!["sessions", "timeline", "s-1", "--full"],
        ];
        for arguments in invalid {
            let error = parse(&arguments).expect_err(arguments.join(" ").as_str());
            assert_eq!(error.exit_code(), crate::ExitCode::Usage, "{arguments:?}");
        }
    }
}
