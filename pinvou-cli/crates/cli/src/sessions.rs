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
//!
//! Cross-process caveat: the GUI serializes its mutations behind in-process
//! locks that a separate CLI process cannot see. A CLI `rename`/`pin` on a
//! session the GUI is ACTIVELY streaming rewrites the whole transcript JSON
//! from a snapshot read moments earlier, so the engine's newest messages
//! can be lost (the store's own `set_title` comment names this hazard).
//! `archive`/`restore` race the same way, and a `delete` of a streaming
//! session can leave the GUI engine rebuilding a zombie transcript when its
//! in-flight turn commits. Avoid mutations on a session the desktop app is
//! currently writing; the last-writer-wins windows on the sidecar
//! registries (viewed/pinned state) are cosmetic by comparison.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::support::{render, require_yes, sandbox_home, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::codex_acp::SessionAgentStore;
use pinvou3_lib::features::sessions::{SessionKind, SessionStore};

const SHOW_PREVIEW_CHARS: usize = 200;

/// Human-mode sanitizer for MODEL-authored text rendered as a BLOCK
/// (transcript bodies, the markdown export on stdout) rather than as one
/// cell of a tab-separated row.
///
/// Why not `support::collapse_control_characters` here: that one flattens
/// every control character including `\n` and `\t`, which is right for a
/// single-line column but would destroy the layout of the very transcript
/// the caller asked to read — a full dump legitimately spans many lines and
/// indents code blocks. So newline and tab survive, and everything else in
/// the C0/C1 control range collapses to a space. The characters that matter
/// are the ones this keeps out: ESC (terminal escape sequences — cursor
/// moves, colour, window-title rewrites, and on some terminals clipboard or
/// response injection), CR (redraws the current line, so earlier output can
/// be silently overwritten), BEL, and the remaining C0/DEL noise. Those are
/// attacker-controlled in a way the layout is not: the text comes from the
/// model and from tool results the model saw.
///
/// JSON mode needs no equivalent — `serde_json` escapes everything below
/// 0x20 — so this stays strictly a human-rendering choice and the stored
/// transcript, the `--output PATH` file, and the JSON payload keep the
/// verbatim bytes.
fn collapse_display_control_characters(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\n' | '\t' => ch,
            _ if ch.is_control() => ' ',
            _ => ch,
        })
        .collect()
}

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
                return Err(CliError::usage(format!(
                    "sessions rename requires a title\n{RENAME_OUTPUT_NOTE}"
                )));
            }
            // A title whose first token looks like a flag ("--limit") would be
            // swallowed as a flag by every other subcommand, so reject titles
            // that LOOK like flags; a title that merely CONTAINS "--" ("War
            // and Peace -- annotated") is legitimate text — the shell has
            // already stripped any quotes by the time the CLI sees the token.
            if title.starts_with('-') {
                return Err(CliError::usage(format!(
                    "sessions rename takes a plain title and cannot accept one that looks \
                     like a flag; retitling text like `--limit` must go through the desktop \
                     app instead (got '{title}')\n{RENAME_OUTPUT_NOTE}"
                )));
            }
            // The global `--output` scan no longer strips `--output json|human`
            // out of the middle of the line (parse_args claims a mode only in
            // the leading run or at the very end of argv). A pair that is
            // still visible in the title therefore has ordinary title words
            // AFTER it — it cannot be the caller asking for JSON output
            // (that pair would have ended the line and been consumed there).
            // Refuse it instead of renaming to an unintended title: exit 2,
            // nothing stored. A `--output` with a non-mode value ("see
            // --output now") is ordinary text and stays allowed.
            if rest
                .windows(2)
                .skip(1)
                .any(|pair| pair[0] == "--output" && (pair[1] == "json" || pair[1] == "human"))
            {
                return Err(CliError::usage(format!(
                    "sessions rename cannot accept a title containing the global `--output \
                     json|human` flag pair followed by more title words (`--output json` as \
                     the LAST two tokens of the line is the legal way to ask for JSON \
                     output); retitling to such text must go through the desktop app \
                     instead (got '{title}')\n{RENAME_OUTPUT_NOTE}"
                )));
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
            if rest.first().is_none() {
                // Missing id: show the export-specific usage so the global
                // --output collision note is disclosed at the point of
                // invocation (see EXPORT_USAGE).
                return Err(CliError::usage(EXPORT_USAGE));
            }
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

/// Export-specific usage. The note discloses the global `--output` mode
/// scan: `parse_args` claims `--output json|human` where a global flag is
/// legal (the leading run and the very end of the line, lib.rs), so
/// `sessions export s-1 --output json` still selects JSON stdout instead
/// of writing a file named "json".
const EXPORT_USAGE: &str = "usage: pinvou sessions export <id> [--format markdown|json] \
[--output PATH]\n\
note: the global --output flag claims the values 'json' and 'human' wherever a global flag is \
legal, so `--output json` prints JSON stdout instead of writing a file named 'json'; spell such \
a destination as --output ./json (or use any other path)";

/// Rename's share of the same disclosure. `export` loses a FILE NAME to the
/// global scan; `rename` used to lose two WORDS out of the middle of a
/// title: before the round-18 fix, `parse_args` stripped `--output json`
/// from ANYWHERE in argv, so `sessions rename s-1 see --output json now`
/// stored "see now" and exited 0. The scan now leaves the pair alone inside
/// family input, so the title arrives intact as "see --output json now":
/// rename refuses it (the flag-shaped-title rule) because a title that
/// begins with a flag token would be swallowed as a flag by every other
/// subcommand. The disclosure rides on the usage errors a caller fighting
/// the collision does reach.
const RENAME_OUTPUT_NOTE: &str = "note: the global --output flag claims the values 'json' and \
'human' only where a global flag is legal (the leading run or the last two tokens of the line); \
a title that starts like a flag, or that contains the `--output json|human` pair with more \
title words after it, is refused rather than silently edited — retitle through the desktop app \
instead";

fn require_id(value: Option<&String>) -> Result<String, CliError> {
    let id = value
        .ok_or_else(|| CliError::usage("sessions command requires a session id"))?
        .clone();
    if id.is_empty() {
        return Err(CliError::usage("sessions command requires a session id"));
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
    crate::support::parse_family_flags(values, value_flags, boolean_flags, "sessions")
}

fn option<'a>(options: &'a [(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    crate::support::family_option(options, name)
}

fn parse_positive(options: &[(&str, &str)], name: &str) -> Result<Option<usize>, CliError> {
    crate::support::parse_family_positive::<usize>(options, name, "sessions")
}

fn open_store() -> Result<SessionStore, CliError> {
    // Same absolute-path contract as `sessions folder`: a relative
    // PINVOU3_HOME would silently resolve against the cwd.
    crate::support::sandbox_home()?;
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
    // Every id-taking subcommand rejects an invalid id with the same usage
    // error (exit 2), whether or not the store layer would also catch it.
    let named_id = |id: &str| crate::support::require_valid_session_id(id, "sessions");
    match command {
        SessionsCommand::List { archived, limit } => list(archived, limit, output),
        SessionsCommand::Show { id, last, full } => {
            named_id(&id)?;
            show(&id, last, full, output)
        }
        SessionsCommand::Rename { id, title } => {
            named_id(&id)?;
            rename(&id, &title, output)
        }
        SessionsCommand::Pin { id } => {
            named_id(&id)?;
            set_pinned(&id, true, output)
        }
        SessionsCommand::Unpin { id } => {
            named_id(&id)?;
            set_pinned(&id, false, output)
        }
        SessionsCommand::Archive { id } => {
            named_id(&id)?;
            set_hidden(&id, true, output)
        }
        SessionsCommand::Restore { id } => {
            named_id(&id)?;
            set_hidden(&id, false, output)
        }
        SessionsCommand::Delete { id, yes } => {
            named_id(&id)?;
            delete(&id, yes, output)
        }
        SessionsCommand::Export {
            id,
            format,
            output: destination,
        } => {
            named_id(&id)?;
            export(&id, format, destination, output)
        }
        SessionsCommand::Timeline { id } => {
            named_id(&id)?;
            timeline(&id, output)
        }
        SessionsCommand::Subagents { id } => {
            named_id(&id)?;
            subagents(&id, output)
        }
        SessionsCommand::Folder { id } => {
            named_id(&id)?;
            folder(&id, output)
        }
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
                // The id is NOT machine-made here: `store.list()` reports the
                // ids it scanned off `sessions/*.json` filenames, and a POSIX
                // filename may contain `\t` or `\n` (only ids arriving via
                // argv pass `require_valid_session_id`). A hand-placed or
                // restored-from-backup file is enough to break the row, so the
                // id gets the same collapse as the title rather than a comment
                // claiming it cannot.
                crate::support::collapse_control_characters(&row.id),
                if row.pinned { "pinned" } else { "-" },
                if row.archived { "archived" } else { "-" },
                row.kind,
                row.updated_at,
                // Titles are stored verbatim and legitimately contain newlines
                // (the GUI's attachment marker embeds "\n\n") or a tab, either
                // of which would split this row into two lines or invent a
                // seventh column for whoever is cutting on \t. The full
                // collapse (tab and newline included, unlike the
                // transcript-body sanitizer above) is what keeps the row
                // contract; JSON keeps the real title. The remaining columns
                // are genuinely machine-made (fixed markers, a fixed kind
                // label, an RFC3339 timestamp).
                crate::support::collapse_control_characters(&row.title),
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
    let mut session = store
        .load(id)
        .map_err(|error| store_error("show", id, error))?;
    let kind = kind_label(&store, id);
    // The session log is the source of truth for the total (the metadata
    // counter can lag behind hand-seeded or replayed transcripts).
    let total_messages = session.messages.len();
    // Window FIRST, on the owned transcript, and serialize only what survives.
    // Serializing the whole `SavedSession` up front (and then cloning its
    // `messages` array out of the result) materialized two more full copies of
    // a transcript that `--last 1` is about to throw away — plus the journal,
    // which this command never renders. Draining first makes the peak scale
    // with the WINDOW instead of with the transcript. The two `to_value` calls
    // below produce exactly the bytes the single whole-session call produced
    // for these two subtrees, so the output shape is unchanged (in particular
    // the timestamps keep serde's spelling, not `to_rfc3339`'s).
    if let Some(last) = last {
        let start = session.messages.len().saturating_sub(last);
        session.messages.drain(..start);
    }
    let metadata = serde_json::to_value(&session.metadata)
        .map_err(|error| CliError::failed(format!("sessions show({id}): {error}")))?;
    let messages = serde_json::to_value(&session.messages)
        .map_err(|error| CliError::failed(format!("sessions show({id}): {error}")))?;
    drop(session);
    // Take the array by value; `as_array().cloned()` would reintroduce a copy
    // of the very window this function just narrowed.
    let messages = match messages {
        serde_json::Value::Array(messages) => messages,
        _other => Vec::new(),
    };
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
        "id: {}\ntitle: {}\nkind: {}\nupdated: {}\nmessages: {} (showing {})",
        id,
        crate::support::collapse_control_characters(
            metadata
                .get("title")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
        ),
        kind,
        metadata
            .get("updated_at")
            .and_then(|value| value.as_str())
            .unwrap_or(""),
        total_messages,
        rendered.len(),
    );
    // The transcript body is the real injection surface of this family: every
    // byte is model-authored (or tool output the model echoed back) and it is
    // printed straight to a terminal. Block sanitizer, not the column one —
    // a transcript is meant to keep its lines and indentation; see
    // [`collapse_display_control_characters`]. The JSON payload below is
    // untouched, so a consumer that wants the verbatim bytes asks for JSON.
    for (index, message) in rendered.iter().enumerate() {
        human.push_str(&format!(
            "\n\n[{}] {}\n{}",
            index + 1,
            // `role` is a plain `String` on the deserialized message, not an
            // enum: a transcript can carry `assistant\x1b]0;pwned\x07` and the
            // header would hand the terminal the exact OSC sequence the body
            // sanitizer on the next line exists to stop. It is a one-line
            // header cell, so it takes the COLUMN collapse (newline and tab
            // included) rather than the block one — a role that spans lines
            // would push the body out of its own header.
            crate::support::collapse_control_characters(
                message["role"].as_str().unwrap_or("unknown")
            ),
            collapse_display_control_characters(message["text"].as_str().unwrap_or("")),
        ));
    }
    let json = serde_json::json!({
        "id": id,
        "title": metadata.get("title"),
        "kind": kind,
        "updated_at": metadata.get("updated_at"),
        "created_at": metadata.get("created_at"),
        // Session total (GUI `message_count` semantics) — `--last` only
        // windows the rendered messages below.
        "message_count": total_messages,
        "shown_message_count": rendered.len(),
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
    // The echo goes through the same collapse as the list/show title columns:
    // a title is argv here, but it is rendered back on one line and the CLI
    // must not be the place where the same string is safe in one command and
    // raw in another.
    Ok(success(render(
        output,
        format!(
            "renamed {id}: {}",
            crate::support::collapse_control_characters(title)
        ),
        &value,
    )))
}

/// Error for a sidecar toggle that reported success in memory but never
/// reached the durable registry. Same shape as the connectors disabled-mirror
/// verification: name the sidecar, state what the durable file still says,
/// and tell the caller what to do about it.
///
/// `verb` is the SUBCOMMAND the caller typed (`pin`/`unpin`/`archive`/
/// `restore`), because that is what `sessions <verb>(<id>)` must name; handing
/// it the past participle rendered `sessions unpinned(s-1): ...`, a command
/// spelling that does not exist.
fn sidecar_not_persisted(
    verb: &str,
    id: &str,
    sidecar: &str,
    still: &str,
) -> Result<CliOutcome, CliError> {
    // The sidecar registries live next to the transcripts, so a read-only or
    // full sessions directory is the failure a caller can actually act on.
    let location = crate::support::sandbox_home()
        .map(|home| home.join("sessions").display().to_string())
        .unwrap_or_else(|_| "the sessions directory".to_owned());
    Err(CliError::failed(format!(
        "sessions {verb}({id}): the {sidecar} sidecar did not persist the change (the session \
         is still {still}); check that {location} is writable and retry the command"
    )))
}

/// Mirrors the GUI `set_session_pinned` / `set_session_archived` guards: the
/// session must exist first so the sidecar tables never keep a stale id.
fn set_pinned(id: &str, pinned: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    // Two labels on purpose: `verb` is the subcommand the caller typed and is
    // what every `sessions <verb>(<id>)` error prefix must carry (an `unpin`
    // failure reported itself as `sessions pin(...)` before), while `action`
    // is the past participle the success line and the JSON `action` field use.
    let (verb, action) = if pinned {
        ("pin", "pinned")
    } else {
        ("unpin", "unpinned")
    };
    require_existing(&store, id, verb)?;
    if store.set_pinned(id, pinned).is_err() {
        return sidecar_not_persisted(
            verb,
            id,
            "pinned-sessions",
            if pinned { "unpinned" } else { "pinned" },
        );
    }
    let value = serde_json::json!({ "id": id, "action": action });
    Ok(success(render(output, format!("{action} {id}"), &value)))
}

fn set_hidden(id: &str, hidden: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    // Same verb/participle split as `set_pinned`: a `restore` failure reported
    // itself as `sessions archive(...)` before.
    let (verb, action) = if hidden {
        ("archive", "archived")
    } else {
        ("restore", "restored")
    };
    require_existing(&store, id, verb)?;
    if store.set_hidden(id, hidden).is_err() {
        return sidecar_not_persisted(
            verb,
            id,
            "hidden-sessions",
            if hidden { "visible" } else { "archived" },
        );
    }
    // `set_hidden(id, true)` is TWO durable writes: it clears the pin first
    // (`features/sessions/sidecars.rs`, so an archived session cannot keep a
    // pinned slot) and the pin registry is a different file with its own
    // failure mode. Verifying only the hidden flag let a half-landed archive —
    // hidden on disk, still pinned on disk — exit 0, and the stale pin then
    // keeps the session exempt from retention forever. `restore` has no such
    // side effect, so only the archive direction is re-checked.
    if hidden && store.is_pinned(id) {
        return sidecar_not_persisted(verb, id, "pinned-sessions", "pinned");
    }
    let value = serde_json::json!({ "id": id, "action": action });
    Ok(success(render(output, format!("{action} {id}"), &value)))
}

fn delete(id: &str, yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    let store = open_store()?;
    // Unknown ids exit 1 like every other sessions command (show/export/
    // rename gate on existence): the store's own delete treats NotFound as
    // success for the GUI cascade, but a CLI caller asking to delete a
    // session that is not there must not be told "deleted".
    require_existing(&store, id, "delete")?;
    // The store delete path mirrors the GUI chat-session cascade (session
    // JSON + artifacts directory) and refuses scheduled-run sessions, which
    // the GUI deletes through their automation only.
    store
        .delete(id)
        .map_err(|error| store_error("delete", id, error))?;
    // Second half of the GUI cascade (`app/commands/sessions.rs` calls
    // `acp_pool.agents().remove(&id)` after a successful chat delete): drop the
    // session's record from `~/.pinvou3/session-agents.json`. `store.delete`
    // only sweeps `sessions/<id>/` (the per-session `code-session.json` sidecar
    // included) — the INDEX record survives it, and the app's boot-time
    // `backfill_missing_code_session_sidecars` runs over exactly
    // "record says code-session, sidecar missing" and re-creates
    // `sessions/<deleted-id>/code-session.json`, resurrecting a directory for a
    // session that no longer exists. `SessionAgentStore::remove` also re-sweeps
    // that sidecar, which is idempotent here.
    //
    // The index file is only consulted when it exists: `remove` persists
    // unconditionally, and a `sessions delete` in a home that never ran an ACP
    // session must not be the thing that creates `session-agents.json`.
    let agents = SessionAgentStore::load_or_empty();
    if agents.path().exists() {
        agents.remove(id).map_err(|error| {
            // The transcript is already gone, so this cannot roll back — but it
            // must not be silent either (the next app boot would rebuild the
            // ghost directory). Mirrors the GUI, which also propagates this
            // failure after the delete has committed.
            CliError::failed(format!(
                "sessions delete({id}): the session was deleted but its record in {} could not \
                 be removed ({error:#}); the desktop app will re-create \
                 sessions/{id}/code-session.json on its next start until that record is gone",
                agents.path().display()
            ))
        })?;
    }
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
    // Consumed, not borrowed: an export renders the WHOLE transcript, so the
    // `Value` cannot be narrowed the way `show` narrows its window — but
    // moving the session in lets it be freed as soon as the `Value` exists,
    // instead of being held alive alongside both the `Value` and the rendered
    // `content` string until the end of this function.
    let value = serde_json::to_value(session)
        .map_err(|error| CliError::failed(format!("sessions export({id}): {error}")))?;
    let content = match format {
        ExportFormat::Json => serde_json::to_string_pretty(&value)
            .map_err(|error| CliError::failed(format!("sessions export({id}): {error}")))?,
        ExportFormat::Markdown => render_markdown(id, &value),
    };
    match destination {
        Some(path) => {
            // An existing destination is refused, not overwritten: the
            // transcript store lives in plain files under the same root, so
            // a silent `fs::write` could destroy a stored session (or any
            // other file the user pointed at) with exit 0. `create_new`
            // makes the check and the write one atomic step, so a
            // destination created (or swapped onto a symlink) after a
            // plain exists() probe can no longer be truncated.
            let bytes = content.len();
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                // A transcript is the whole conversation — system prompt,
                // every turn, tool calls and their results — so a default
                // umask file (~0644) would publish it to every local user.
                // Same 0600 contract as `code providers export`. Unlike that
                // path no follow-up `set_permissions` is needed: `mode` only
                // applies at create time, and `create_new` guarantees this
                // call is the create (a pre-existing destination is refused
                // below instead of being reused).
                options.mode(0o600);
            }
            // Non-unix: no POSIX mode bits; ACL tightening is out of scope
            // here exactly as it is for `code providers export`.
            //
            // The create and the body write are kept as SEPARATE failures.
            // Fused through `and_then`, the error handler could not tell which
            // of the two failed and still deleted the destination — while only
            // a create this call actually made may be deleted. The
            // classification needed splitting too: `AlreadyExists` is the
            // exclusive-create refusal on unix, but Windows `CREATE_NEW`
            // against an existing DIRECTORY reports ERROR_ACCESS_DENIED
            // (`PermissionDenied`), so the refusal is decided by "the path is
            // already there", with the raw error only as the fallback message.
            let mut file = match options.open(&path) {
                Ok(file) => file,
                Err(error) => {
                    let already_there =
                        error.kind() == std::io::ErrorKind::AlreadyExists || path.exists();
                    return Err(CliError::failed(if already_there {
                        format!(
                            "sessions export({id}): refusing to overwrite {}; choose a \
                             destination that does not exist yet",
                            path.display()
                        )
                    } else {
                        // Nothing was created, so nothing is removed here.
                        format!(
                            "sessions export({id}): cannot create {}: {error}",
                            path.display()
                        )
                    }));
                }
            };
            if let Err(error) = file.write_all(content.as_bytes()) {
                // The exclusive create DID succeed and the body failed
                // (ENOSPC, quota): this call owns the destination, so drop the
                // truncated file — a retry becomes possible and no half-written
                // transcript masquerades as an export. Close the handle first
                // so Windows can unlink it.
                drop(file);
                let _ = std::fs::remove_file(&path);
                return Err(CliError::failed(format!(
                    "sessions export({id}): cannot write {}: {error}",
                    path.display()
                )));
            }
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
            // Stdout is a terminal here, and the markdown body is the model's
            // own text (`render_markdown` interleaves role headers with raw
            // message text). Sanitizing at this one boundary covers both
            // formats and leaves `render_markdown` itself verbatim, which is
            // what the `--output PATH` arm and the JSON `content` field above
            // must keep: a file and a JSON string are not a terminal, and an
            // export that silently differs from the stored transcript would
            // be a worse bug than the one being fixed.
            Ok(success(render(
                output,
                collapse_display_control_characters(&content),
                &value,
            )))
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
/// Event whitelist from `timing::parse_timeline_line` (base events plus the
/// benchmark observation events the CLI build enables).
fn is_timeline_event(value: &serde_json::Value) -> bool {
    const EVENTS: [&str; 12] = [
        "user_start",
        "assistant_done",
        "context_snapshot",
        "engine_turn_started",
        "first_message_delta",
        "first_tool_call_started",
        "first_tool_call_completed",
        "turn_started",
        "first_delta",
        "tool_call_started",
        "tool_call_completed",
        "model_request_metric",
    ];
    value.is_object()
        && value
            .get("turn_id")
            .and_then(|v| v.as_str())
            .is_some_and(|turn| !turn.trim().is_empty())
        && value
            .get("event")
            .and_then(|v| v.as_str())
            .is_some_and(|event| EVENTS.contains(&event))
        && value.get("timestamp").and_then(|v| v.as_i64()).is_some()
        && value
            .get("ts")
            .and_then(|v| v.as_str())
            .is_some_and(|ts| !ts.trim().is_empty())
}

fn timeline(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    if !crate::support::valid_session_id(id) {
        return Err(CliError::usage("invalid session id"));
    }
    // A missing sidecar and a missing session both read as empty output, so
    // gate on the session like list/show do.
    require_existing(&open_store()?, id, "timeline")?;
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
                // Same line contract as `timing::parse_timeline_line`
                // (this crate builds pinvou3-lib with benchmark-hooks, so
                // the observation events are part of the whitelist):
                // non-empty turn_id, known event, integer timestamp,
                // non-empty ts. The GUI reader drops anything else, and so
                // must the CLI — a stray JSON object would otherwise sort to
                // the front of the timeline.
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                    if is_timeline_event(&value) {
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
    // Every cell comes out of the JSONL sidecar, which is only as trustworthy
    // as whatever wrote it: `event` is whitelisted above but `ts`, `turn_id`
    // and `status` are free strings. One embedded tab or newline would add a
    // column or split a row, so each cell goes through the column collapse.
    let cell = |event: &serde_json::Value, key: &str, fallback: &str| {
        crate::support::collapse_control_characters(
            event
                .get(key)
                .and_then(|value| value.as_str())
                .unwrap_or(fallback),
        )
    };
    let human = events
        .iter()
        .map(|event| {
            format!(
                "{}\t{}\t{}\t{}",
                cell(event, "ts", ""),
                cell(event, "event", ""),
                cell(event, "turn_id", ""),
                cell(event, "status", "-"),
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
    if !crate::support::valid_session_id(id) {
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
                // Every terminal non-success lands here — failed, cancelled and
                // interrupted alike — because the foundation defines
                // `failed = done && status != Completed`
                // (`features/multiagent/transcripts.rs`), so `failed` already
                // implies `done` and there is no second `failed` arm to reach.
                // The human column prints the foundation's own status name
                // rather than flattening all three into "failed"; the fallback
                // is the interrupted projection, which is what a listing with
                // no live engine (this CLI) reports for a worker that never
                // reached a terminal status.
                summary.status.as_deref().unwrap_or("interrupted")
            } else if summary.blocked {
                "blocked"
            } else if summary.done {
                "done"
            } else {
                "running"
            };
            // `objective` is verbatim from the model's own `agent` tool call
            // and `error` carries whatever the failing worker reported, so
            // both are untrusted here. They are also the two rightmost cells
            // of a tab-separated row: an embedded tab would invent a fifth
            // column and a newline would turn one subagent into two rows, so
            // the whole row goes through the column collapse (agent_id and
            // state included — the row contract must not depend on which
            // cell happened to be machine-made).
            let cell = crate::support::collapse_control_characters;
            format!(
                "{}\t{}\t{}\t{}",
                cell(&summary.agent_id),
                cell(state),
                cell(summary.objective.as_deref().unwrap_or("-")),
                cell(summary.error.as_deref().unwrap_or("-")),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    // A failed serialization is a host failure, not an empty roster: telling
    // a script "no subagents dispatched" when the listing could not be
    // rendered turns a reportable error into a wrong answer (same propagation
    // as `show` and `export` above).
    let serialized = serde_json::to_value(&summaries)
        .map_err(|error| CliError::failed(format!("sessions subagents({id}): {error}")))?;
    let value = serde_json::json!({ "id": id, "subagents": serialized });
    Ok(success(render(output, human, &value)))
}

/// Same folder the GUI `reveal_session_folder` resolves: scheduled-run
/// sessions have no per-session runtime directory, so their shared task
/// workspace (the ledger root) is the folder; chat sessions map to
/// `sessions_root()/<id>`.
fn folder(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    if !crate::support::valid_session_id(id) {
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
            _other => panic!(
                "parsed an unexpected command family; the fixture argv does not match the test"
            ),
        }
    }

    #[test]
    fn rename_accepts_titles_containing_flags_but_not_flag_shaped_ones() {
        // A title that merely CONTAINS "--" is legitimate text; this
        // acceptance previously existed only as a comment — pin it.
        assert_eq!(
            parse(&["rename", "s-1", "War", "and", "Peace --", "annotated"]).unwrap(),
            SessionsCommand::Rename {
                id: "s-1".into(),
                title: "War and Peace -- annotated".into(),
            }
        );
        // A title that LOOKS like a flag is rejected as a usage error; the
        // message must not pretend quoting could help (the shell strips
        // quotes before argv).
        let error = parse(&["rename", "s-1", "--limit"]).unwrap_err();
        assert!(error.to_string().contains("plain title"), "{error}");
    }

    #[test]
    fn export_usage_discloses_the_global_output_collision() {
        // `sessions export --output json` silently flips the GLOBAL output
        // mode to json instead of writing a file named "json"; the export
        // usage text must surface the ./ workaround at the point of
        // invocation instead of leaving it as a lib.rs code comment.
        let error = parse(&["export"]).unwrap_err();
        assert_eq!(error.exit_code(), crate::ExitCode::Usage);
        assert!(error.to_string().contains("--output ./json"), "{error}");
    }

    #[test]
    fn rename_usage_discloses_the_global_output_collision() {
        // `sessions rename s-1 see --output json now` used to lose two words
        // of the title to the global --output scan (which stripped the pair
        // from ANYWHERE in argv) and store "see now" with exit 0. The scan
        // now claims the mode only at a legal global flag position (the
        // leading run / the very end of the line); a pair still visible in
        // the title has title words after it, so rename refuses it: exit 2,
        // nothing stored.
        let error = parse(&["rename", "s-1", "see", "--output", "json", "now"])
            .expect_err("a title containing a global flag pair must be refused");
        assert_eq!(error.exit_code(), crate::ExitCode::Usage, "{error}");
        assert!(
            error
                .to_string()
                .contains("cannot accept a title containing"),
            "the refusal must name the pair rule: {error}"
        );
        // The other legal spelling of the same request (pair at the END of
        // the line) still works: mode applied, title from the words before.
        assert_eq!(
            parse(&["rename", "s-1", "see", "--output", "json"]).unwrap(),
            SessionsCommand::Rename {
                id: "s-1".into(),
                title: "see".into(),
            }
        );
        for arguments in [vec!["rename", "s-1"], vec!["rename", "s-1", "--limit"]] {
            let error = parse(&arguments).expect_err("a usage error");
            assert_eq!(error.exit_code(), crate::ExitCode::Usage, "{arguments:?}");
            assert!(
                error
                    .to_string()
                    .contains("refused rather than silently edited"),
                "{arguments:?} must disclose the --output collision rule: {error}"
            );
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

    /// Usage rejections, each pinned to the reason it is rejected FOR.
    ///
    /// The fixtures start at the SUBCOMMAND token: `parse` prepends
    /// "pinvou" and "sessions" itself. Spelling the family again here would
    /// double-prefix every argv into `pinvou sessions sessions …`, which
    /// dies on the unknown-subcommand arm before reaching the shape under
    /// test — twenty cases passing for one wrong reason. The expected
    /// message fragment is what keeps that from silently happening again:
    /// exit code 2 alone cannot tell the two failures apart.
    #[test]
    fn rejects_invalid_usage_with_exit_code_two() {
        let invalid: [(Vec<&str>, &str); 20] = [
            (vec![], "usage: pinvou sessions"),
            (vec!["bogus"], "usage: pinvou sessions"),
            (vec!["list", "--bogus"], "unsupported sessions option"),
            (
                vec!["list", "--limit", "0"],
                "sessions --limit must be a positive integer",
            ),
            (
                vec!["list", "--limit", "x"],
                "sessions --limit must be a positive integer",
            ),
            (
                vec!["list", "--limit"],
                "sessions option --limit requires a value",
            ),
            (
                vec!["list", "--archived", "--archived"],
                "duplicate sessions option --archived",
            ),
            (vec!["show"], "requires a session id"),
            (vec!["show", "s-1", "--nope"], "unsupported sessions option"),
            (
                vec!["show", "s-1", "--last"],
                "sessions option --last requires a value",
            ),
            (vec!["rename"], "requires a session id"),
            (vec!["rename", "s-1"], "sessions rename requires a title"),
            (
                vec!["rename", "s-1", "   "],
                "sessions rename requires a title",
            ),
            (vec!["pin"], "requires a session id"),
            (
                vec!["pin", "s-1", "--extra"],
                "sessions pin accepts no options",
            ),
            (
                vec!["delete", "s-1", "--nope"],
                "unsupported sessions option",
            ),
            (
                vec!["export", "s-1", "--format", "html"],
                "sessions export --format must be markdown or json",
            ),
            (
                vec!["export", "s-1", "--format"],
                "sessions option --format requires a value",
            ),
            (vec!["timeline"], "requires a session id"),
            (
                vec!["timeline", "s-1", "--full"],
                "sessions timeline accepts no options",
            ),
        ];
        for (arguments, expected) in invalid {
            let error = parse(&arguments).expect_err(arguments.join(" ").as_str());
            assert_eq!(error.exit_code(), crate::ExitCode::Usage, "{arguments:?}");
            assert!(
                error.to_string().contains(expected),
                "{arguments:?} was rejected for the wrong reason: {error}"
            );
        }
    }

    /// The transcript sanitizer must keep the layout of the output it
    /// protects: a transcript dump is meant to span lines and to indent, so
    /// collapsing `\n`/`\t` the way the column sanitizer does would destroy
    /// the very thing the caller asked to read. Everything else in the
    /// control range — ESC above all, plus CR, which redraws the current
    /// line and can hide earlier output — must not reach the terminal.
    #[test]
    fn transcript_sanitizer_keeps_layout_and_drops_escapes() {
        assert_eq!(
            collapse_display_control_characters("line1\n\tindented\n"),
            "line1\n\tindented\n"
        );
        assert_eq!(
            collapse_display_control_characters("safe\x1b[2J\x1b]0;pwned\x07done"),
            "safe [2J ]0;pwned done"
        );
        assert_eq!(
            collapse_display_control_characters("visible\rhidden"),
            "visible hidden"
        );
        // Non-control text, including multi-byte characters, is untouched.
        assert_eq!(
            collapse_display_control_characters("已完成 — ok"),
            "已完成 — ok"
        );
    }
}
