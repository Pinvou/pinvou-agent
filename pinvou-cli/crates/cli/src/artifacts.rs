//! `artifacts` family: cross-session deliverables index plus path-validated
//! text read/write over session artifact files, mirroring
//! `pinvou3-app/src-tauri/src/app/commands/artifacts.rs`.
//!
//! Pure-storage only: no Tauri host, no engine. The GUI command layer is a
//! thin shell over feature/platform helpers, several of which are
//! `pub(crate)`; this module mirrors exactly those rules over the same
//! `pinvou3_lib` paths:
//! - list → mirror of `features::deliverables::list_deliverable_index_impl`
//!   (crate-private): same `sessions_root()` scan, same deliverable
//!   extension whitelist, same category mapping, same newest-mtime-wins
//!   dedupe and ordering. Session existence uses
//!   `pinvou3_lib::features::sessions::SessionStore`.
//! - read/write → the GUI `write_artifact_text` rules (canonical path must
//!   stay inside `sessions_root()/<session-id>/{artifacts,workspace}`,
//!   session id must not start with `_`, markdown-only overwrite of an
//!   existing file, 10 MiB cap) plus the credential-component half of
//!   `platform::path_policy::validate_user_path` (see
//!   [`crosses_sensitive_component`]). Relative paths resolve against the
//!   session's ledger workspace, like the GUI `resolve_artifact_path`. The
//!   overwrite deliberately takes no `--yes`: it is the GUI editor save
//!   semantics (markdown-only, in-ledger, size-capped), not a destructive
//!   whole-store operation.
//!
//! What is deliberately *not* mirrored from `validate_user_path` on this lane
//! is its `BLOCKED_PREFIXES` half (`/etc/shadow`, `/proc/`, `/root/`, …). Those
//! are system locations, and `resolve_session_artifact` has already proved the
//! canonical target is inside this session's own storage — so every one of them
//! is unreachable here by construction, except `/root/`, which is the ordinary
//! home of a root-owned install and would therefore refuse *every* artifact
//! rather than any sensitive one. The component blacklist is the half that
//! still bites inside session storage (an agent can create a file named `.env`
//! or a `.ssh/` directory in its own workspace) and it is mirrored in full.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::support::{collapse_control_characters, read_text_file_capped, render, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::deliverables::{DELIVERABLE_EXTS, deliverable_category};
use pinvou3_lib::features::sessions::{SessionStore, is_aux_session_id};

/// Same cap as the GUI artifact editor (`MAX_EDITABLE_MARKDOWN_BYTES`).
const MAX_EDITABLE_MARKDOWN_BYTES: usize = 10 * 1024 * 1024;

/// The credential-component blocklist, IMPORTED from
/// `platform::path_policy::BLOCKED_COMPONENTS` (the module was widened from
/// `pub(crate)` for exactly this consumer): the lists can no longer drift.
/// The round-27 copy carried a "must change together" warning — an upstream
/// credential addition without a matching CLI edit silently reopened the name
/// on this surface while `files ingest` (which consumes the upstream list via
/// `validate_browsable_path`) stayed closed; that failure class is gone now
/// that both surfaces read one constant.
use pinvou3_lib::platform::path_policy::BLOCKED_COMPONENTS as SENSITIVE_PATH_COMPONENTS;

/// The system-prefix blocklist, imported from
/// `platform::path_policy::BLOCKED_PREFIXES` for the same reason.
///
/// Only consulted for paths the user names freely — today that is
/// `feedback submit --attach/--body-file` and `agent run --attach`. The
/// artifact lanes skip it; see the module docs for why it cannot apply
/// inside session storage.
use pinvou3_lib::platform::path_policy::BLOCKED_PREFIXES as SENSITIVE_PATH_PREFIXES;

/// Mirror of the component half of `path_policy::check_sensitive_components`.
///
/// Takes an already-canonicalized path (the upstream contract) and answers with
/// the blacklisted component it crosses, if any. The comparison reproduces
/// `platform::os::path_component_eq`, which is byte-exact on unix and ASCII
/// case-insensitive on Windows — a case-sensitive compare there would let
/// `ID_RSA` through on a filesystem that treats it as the same file.
pub(crate) fn crosses_sensitive_component(canonical: &Path) -> Option<&'static str> {
    SENSITIVE_PATH_COMPONENTS.iter().copied().find(|blocked| {
        canonical.components().any(|component| {
            let value = component.as_os_str();
            if cfg!(windows) {
                value.to_string_lossy().eq_ignore_ascii_case(blocked)
            } else {
                value == std::ffi::OsStr::new(*blocked)
            }
        })
    })
}

/// Mirror of the whole `path_policy::check_sensitive_components` predicate
/// (components *and* system prefixes), for user-named paths that are not
/// confined to session storage.
pub(crate) fn check_sensitive_path(canonical: &Path) -> Result<(), String> {
    if let Some(blocked) = crosses_sensitive_component(canonical) {
        return Err(format!(
            "{} crosses the credential path component `{blocked}`",
            canonical.display()
        ));
    }
    let text = canonical.to_string_lossy();
    if let Some(prefix) = SENSITIVE_PATH_PREFIXES
        .iter()
        .find(|prefix| text.starts_with(**prefix))
    {
        return Err(format!(
            "{} is in the system-sensitive area {prefix}",
            canonical.display()
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtifactsCommand {
    List {
        session: Option<String>,
    },
    Read {
        session_id: String,
        relative_path: String,
    },
    Write {
        session_id: String,
        relative_path: String,
        file: Option<PathBuf>,
        stdin: bool,
    },
}

pub fn parse(values: &[String]) -> Result<ArtifactsCommand, CliError> {
    const USAGE: &str = "usage: pinvou artifacts <list [--session ID]|read <session-id> <path>|write <session-id> <path> (--file PATH|--stdin)>";
    let subcommand = values.get(1).ok_or_else(|| CliError::usage(USAGE))?;
    let rest = &values[2..];
    match subcommand.as_str() {
        "list" => {
            let mut session = None;
            let mut index = 0;
            while index < rest.len() {
                if rest[index] != "--session" {
                    return Err(CliError::usage(format!(
                        "unsupported artifacts option: {}",
                        rest[index]
                    )));
                }
                if session.is_some() {
                    return Err(CliError::usage("duplicate artifacts option --session"));
                }
                let value = rest.get(index + 1).ok_or_else(|| {
                    CliError::usage("artifacts option --session requires a value")
                })?;
                if value.is_empty() || value.starts_with("--") {
                    return Err(CliError::usage(
                        "artifacts option --session requires a value",
                    ));
                }
                session = Some(value.clone());
                index += 2;
            }
            Ok(ArtifactsCommand::List { session })
        }
        "read" => {
            let (session_id, relative_path) = require_session_and_path(rest)?;
            if rest.len() > 2 {
                return Err(CliError::usage("artifacts read accepts no options"));
            }
            Ok(ArtifactsCommand::Read {
                session_id,
                relative_path,
            })
        }
        "write" => {
            let positional_end = rest
                .iter()
                .position(|value| value == "--file" || value == "--stdin")
                .unwrap_or(rest.len());
            let (session_id, relative_path) =
                require_session_and_path(rest.get(..positional_end).unwrap_or_default())?;
            if positional_end > 2 {
                return Err(CliError::usage(
                    "artifacts write accepts only a session id and a relative path",
                ));
            }
            let mut file = None;
            let mut stdin = false;
            let mut index = positional_end;
            while index < rest.len() {
                match rest[index].as_str() {
                    "--file" => {
                        if file.is_some() || stdin {
                            return Err(CliError::usage(
                                "artifacts write accepts one of --file or --stdin",
                            ));
                        }
                        let value = rest.get(index + 1).ok_or_else(|| {
                            CliError::usage("artifacts write --file requires a path")
                        })?;
                        if value.is_empty() || value.starts_with("--") {
                            return Err(CliError::usage("artifacts write --file requires a path"));
                        }
                        file = Some(PathBuf::from(value));
                        index += 2;
                    }
                    "--stdin" => {
                        if file.is_some() || stdin {
                            return Err(CliError::usage(
                                "artifacts write accepts one of --file or --stdin",
                            ));
                        }
                        stdin = true;
                        index += 1;
                    }
                    other => {
                        return Err(CliError::usage(format!(
                            "unsupported artifacts option: {other}"
                        )));
                    }
                }
            }
            if file.is_none() && !stdin {
                return Err(CliError::usage(
                    "artifacts write requires --file PATH or --stdin",
                ));
            }
            Ok(ArtifactsCommand::Write {
                session_id,
                relative_path,
                file,
                stdin,
            })
        }
        _ => Err(CliError::usage(USAGE)),
    }
}

fn require_session_and_path(rest: &[String]) -> Result<(String, String), CliError> {
    if rest.len() < 2 {
        return Err(CliError::usage(
            "artifacts command requires a session id and a relative path",
        ));
    }
    if rest[0].is_empty() || rest[1].is_empty() {
        return Err(CliError::usage(
            "artifacts command requires a session id and a relative path",
        ));
    }
    // Round-40 review: the session id carries `[A-Za-z0-9_-]`, so a
    // flag-shaped token passes it and dies later as an exit-1
    // `artifact_not_found` (e.g. `artifacts read s-1 --stdin` reading
    // `--stdin` as the path). An option-like token is an argv-decidable
    // mistake — refuse it at parse with the exit-2 usage class, the same
    // discipline `files ingest` applies to its PATH slot.
    for (slot, token) in [("session id", &rest[0]), ("PATH", &rest[1])] {
        if token.starts_with('-') {
            return Err(CliError::usage(format!(
                "artifacts command requires a {slot}, got the option-like token {token:?}"
            )));
        }
    }
    Ok((rest[0].clone(), rest[1].clone()))
}

fn open_store() -> Result<SessionStore, CliError> {
    SessionStore::boot()
        .map_err(|error| CliError::failed(format!("artifacts store unavailable: {error:#}")))
}

/// One row of the cross-session deliverables index (the GUI `DeliverableItem`
/// shape, flattened to owned fields the CLI can render).
#[derive(Clone, Debug)]
struct DeliverableRow {
    name: String,
    path: String,
    ext: String,
    category: String,
    session_id: String,
    source: String,
    mtime: i64,
    size: u64,
}

/// What one index scan produced: the deliverable rows, plus the session
/// records the scan could not read.
///
/// The skipped list is not cosmetic. A session record over the scan cap —
/// or one that cannot be read/parsed at all (permissions, a mid-write
/// record, corrupt JSON) — is dropped from the index, so an incomplete
/// listing is indistinguishable from an empty one — see [`list`], which puts
/// the names into the JSON payload for exactly that reason.
struct DeliverableIndex {
    rows: Vec<DeliverableRow>,
    skipped: Vec<String>,
}

/// Mirror of `features::deliverables::list_deliverable_index_impl`: scan
/// `sessions_root()/*.json`, keep tracked artifacts that still exist on
/// disk, whitelist deliverable extensions, dedupe by physical path keeping
/// the newest mtime, sort by mtime descending then name.
///
/// `only_session` is applied to the FILE NAME, before the record is read.
/// Sessions are stored as `<id>.json`, so a single-session listing has no
/// reason to read — let alone `serde_json`-parse — every other record in the
/// store, each of which may be up to `MAX_LIST_SCAN_BYTES`. One deviation
/// from the pre-short-circuit behavior, disclosed instead of paid for on
/// every scan: a hand-edited record whose filename and parsed `metadata/id`
/// disagree is invisible to the FILTERED lane (the filename gate skips it
/// before the parse), while the unfiltered lane still finds it via the
/// caller's parsed-id re-check. Only externally rewritten stores can hold
/// such records.
fn deliverable_index(only_session: Option<&str>) -> Result<DeliverableIndex, CliError> {
    let sessions_dir = pinvou3_lib::platform::paths::sessions_root();
    let entries = match std::fs::read_dir(&sessions_dir) {
        Ok(entries) => entries,
        // A missing sessions root is a genuinely empty store, not a failure.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DeliverableIndex {
                rows: Vec::new(),
                skipped: Vec::new(),
            });
        }
        // Anything else (EACCES on a locked-down home, `sessions` shadowed by
        // a regular file) must not render as "no deliverables": the
        // `skipped_sessions` disclosure exists precisely so a consumer can
        // tell "empty" from "index incomplete" — an empty EVERYTHING is the
        // one shape that reads as a clean bill.
        Err(error) => {
            return Err(CliError::failed(format!(
                "artifacts list: cannot read the sessions root {}: {error}",
                sessions_dir.display()
            )));
        }
    };
    let mut by_path: HashMap<String, DeliverableRow> = HashMap::new();
    let mut skipped: Vec<String> = Vec::new();
    // `list` only reads the metadata header, but a session record is parsed
    // whole: cap the per-file read like every other family lane so a huge
    // transcript cannot dominate the listing (oversized files are skipped
    // with a note rather than read).
    const MAX_LIST_SCAN_BYTES: u64 = 32 * 1024 * 1024;
    for entry in entries.flatten() {
        let file = entry.path();
        if !file.is_file() || file.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let stem = file
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_owned();
        // Aux side-chat sessions stay outside the deliverable boundary, same
        // as the GUI index this module mirrors
        // (`features::deliverables::list_deliverable_index_impl` skips aux
        // stems before parsing): an aux transcript tracking a deliverable
        // must not surface its paths outside the aux session. The predicate
        // is the store's shared `is_aux_session_id` (imported, not copied),
        // so a case-aliasing fix upstream cannot drift behind a copy.
        if is_aux_session_id(stem.as_str()) {
            continue;
        }
        if only_session.is_some_and(|session| session != stem.as_str()) {
            continue;
        }
        if std::fs::metadata(&file)
            .map(|meta| meta.len())
            .unwrap_or_default()
            > MAX_LIST_SCAN_BYTES
        {
            note!(
                "[artifacts] list skips {} (larger than the {MAX_LIST_SCAN_BYTES}-byte scan cap)",
                file.display()
            );
            // Also reported in the JSON payload: stderr is invisible to a
            // `--output json` consumer, which would otherwise read a truncated
            // index as "this session has no deliverables".
            skipped.push(stem);
            continue;
        }
        let raw = match read_record_capped(&file, MAX_LIST_SCAN_BYTES) {
            RecordRead::Text(raw) => raw,
            RecordRead::TooLarge => {
                // The record passed the stat probe but grew past the cap
                // before this read landed; identical skip contract to the
                // probe above.
                note!(
                    "[artifacts] list skips {} (larger than the {MAX_LIST_SCAN_BYTES}-byte scan cap)",
                    file.display()
                );
                skipped.push(stem);
                continue;
            }
            RecordRead::Unreadable => {
                // Same "incomplete ≠ empty" disclosure as the oversize
                // branch: an EACCES'd or mid-write record is dropped from
                // the index, so an unreadable slice must surface in
                // skipped_sessions instead of vanishing (round-36 review).
                note!("[artifacts] list skips {} (unreadable)", file.display());
                skipped.push(stem);
                continue;
            }
        };
        let Ok(view) = serde_json::from_str::<serde_json::Value>(&raw) else {
            // Corrupt JSON is the same undecidable slice: disclosed, then
            // skipped.
            note!(
                "[artifacts] list skips {} (not a valid session record)",
                file.display()
            );
            skipped.push(stem);
            continue;
        };
        let session_id = view
            .pointer("/metadata/id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_owned();
        let source = view
            .pointer("/metadata/title")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_owned();
        let Some(artifacts) = view.get("artifacts").and_then(|value| value.as_array()) else {
            continue;
        };
        for artifact in artifacts {
            let Some(storage_path) = artifact
                .get("storage_path")
                .and_then(|value| value.as_str())
            else {
                continue;
            };
            let path = PathBuf::from(storage_path);
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_owned();
            if name.is_empty() {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !DELIVERABLE_EXTS.contains(&ext.as_str()) {
                continue;
            }
            let mtime = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or(0);
            let byte_size = artifact
                .get("byte_size")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let path_string = path.to_string_lossy().to_string();
            let row = DeliverableRow {
                name,
                path: path_string.clone(),
                ext: ext.clone(),
                category: deliverable_category(&ext).to_owned(),
                session_id: session_id.clone(),
                source: source.clone(),
                mtime,
                size: if metadata.len() > 0 {
                    metadata.len()
                } else {
                    byte_size
                },
            };
            by_path
                .entry(path_string)
                .and_modify(|current| {
                    if row.mtime >= current.mtime {
                        *current = row.clone();
                    }
                })
                .or_insert(row);
        }
    }
    let mut rows: Vec<DeliverableRow> = by_path.into_values().collect();
    rows.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.name.cmp(&b.name)));
    skipped.sort();
    Ok(DeliverableIndex { rows, skipped })
}

/// Outcome of a capped session-record read for [`deliverable_index`].
#[derive(Debug)]
enum RecordRead {
    /// The record ends within the cap and decodes as UTF-8.
    Text(String),
    /// The record does not end within the cap; the read was cut off before
    /// EOF instead of slurping an unbounded file.
    TooLarge,
    /// The record is unreadable or not valid UTF-8 — the silent skip the
    /// previous uncapped `read_to_string` took on the same conditions.
    Unreadable,
}

/// Reads at most `cap + 1` bytes of `path` and decodes it as UTF-8.
///
/// The stat probe in [`deliverable_index`] only sees the size at stat time,
/// so a record under the cap at probe time that grows past it before the
/// read lands must not be slurped whole by an uncapped `read_to_string`.
/// The take allows one extra byte so a record of exactly `cap` bytes still
/// reaches EOF inside the read and counts as a full read; only a strictly
/// larger file reports [`RecordRead::TooLarge`]. `cap` is a parameter so
/// the boundary can be unit-tested with small fixtures; the production
/// caller passes `MAX_LIST_SCAN_BYTES`.
fn read_record_capped(path: &Path, cap: u64) -> RecordRead {
    let Ok(file) = std::fs::File::open(path) else {
        return RecordRead::Unreadable;
    };
    let mut taken = file.take(cap.saturating_add(1));
    let mut raw = Vec::new();
    if taken.read_to_end(&mut raw).is_err() {
        return RecordRead::Unreadable;
    }
    if raw.len() as u64 > cap {
        return RecordRead::TooLarge;
    }
    match String::from_utf8(raw) {
        Ok(text) => RecordRead::Text(text),
        Err(_) => RecordRead::Unreadable,
    }
}

/// Resolves `<session-id> <relative-path>` to a canonical artifact file,
/// mirroring `resolve_artifact_path_in_workspace` (relative against the
/// ledger workspace) plus containment checks. Read and write differ like
/// the GUI: reads accept the whole session ledger tree — which for
/// `sched-*` sessions lives under `~/.pinvou3/scheduled/<task>/workspace`,
/// outside `sessions_root()` — while writes stay confined to
/// `sessions_root()/<session-id>/{artifacts,workspace}`
/// (`ensure_editable_artifact_path`). Collapsing both into the write
/// containment made scheduled-run artifacts listable but never readable.
fn resolve_session_artifact(
    store: &SessionStore,
    session_id: &str,
    relative_path: &str,
    writable: bool,
) -> Result<PathBuf, CliError> {
    if !crate::support::valid_session_id(session_id) {
        return Err(CliError::usage("invalid session id"));
    }
    let workspace = store
        .ledger_root(session_id)
        .map_err(|error| CliError::failed(format!("artifacts({session_id}): {error:#}")))?;
    let raw = Path::new(relative_path);
    let path = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        workspace.join(raw)
    };
    let canonical = std::fs::canonicalize(&path).map_err(|error| {
        // Scripts key on the machine-readable prefix: only a genuinely
        // missing path is `artifact_not_found`; permissions/loop errors get
        // a distinct prefix instead of misclassifying as not-found.
        if error.kind() == std::io::ErrorKind::NotFound {
            CliError::failed(format!("artifact_not_found: {error}"))
        } else {
            CliError::failed(format!(
                "artifact_not_readable: cannot resolve {}: {error}",
                path.display()
            ))
        }
    })?;
    if !canonical.is_file() {
        return Err(CliError::failed(format!(
            "artifact_not_found: {} is not a file",
            canonical.display()
        )));
    }
    // The credential-component half of the GUI path policy, applied to the
    // canonical target (the upstream contract is "already canonicalized", so
    // this must come after `canonicalize`, not before). The containment checks
    // below keep the CLI inside session storage, which the GUI's read command
    // does not even require — but an agent can create `.env` or `.ssh/id_rsa`
    // inside its own workspace, and there the GUI refuses while the CLI did
    // not. Same refusal on read and write: the risk is the content reaching a
    // model context, which is the read direction.
    if let Some(blocked) = crosses_sensitive_component(&canonical) {
        return Err(CliError::failed(format!(
            "artifact_crosses_sensitive_component: {} crosses `{blocked}`; credential paths are \
             never read or written through this command",
            canonical.display()
        )));
    }
    let sessions_root = pinvou3_lib::platform::paths::sessions_root();
    let sessions_root = std::fs::canonicalize(&sessions_root).map_err(|error| {
        CliError::failed(format!(
            "artifact_outside_session_storage: cannot resolve sessions root({}): {error}",
            sessions_root.display()
        ))
    })?;
    let relative = match canonical.strip_prefix(&sessions_root) {
        Ok(relative) => relative,
        Err(_) => {
            // Outside sessions storage: readable only when it lives inside
            // this session's ledger tree (the scheduled-run workspace
            // case); writes refuse — matching the GUI, where the write
            // command is sessions-root confined and scheduled-run
            // deliverables are read-only.
            if !writable {
                let workspace_canonical = std::fs::canonicalize(&workspace).map_err(|error| {
                    CliError::failed(format!(
                        "artifact_outside_session_storage: cannot resolve workspace {}: {error}",
                        workspace.display()
                    ))
                })?;
                if canonical.strip_prefix(&workspace_canonical).is_ok() {
                    return Ok(canonical);
                }
            }
            return Err(CliError::failed("artifact_outside_session_storage"));
        }
    };
    let mut components = relative.components();
    let session = components
        .next()
        .and_then(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .ok_or_else(|| CliError::failed("artifact_outside_session"))?;
    if session.is_empty() || session.starts_with('_') {
        return Err(CliError::failed("artifact_outside_editable_session"));
    }
    if session != session_id {
        return Err(CliError::failed(format!(
            "artifact_session_mismatch: artifact belongs to session {session}"
        )));
    }
    let area = components
        .next()
        .and_then(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .ok_or_else(|| CliError::failed("artifact_outside_session_artifacts"))?;
    if area != "artifacts" && area != "workspace" {
        return Err(CliError::failed("artifact_outside_session_artifacts"));
    }
    Ok(canonical)
}

pub fn execute(command: ArtifactsCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    // Same absolute-PINVOU3_HOME contract as every other store-opening
    // family: a relative home would silently resolve against the cwd.
    crate::support::sandbox_home()?;
    match command {
        ArtifactsCommand::List { session } => list(session, output),
        ArtifactsCommand::Read {
            session_id,
            relative_path,
        } => read(&session_id, &relative_path, output),
        ArtifactsCommand::Write {
            session_id,
            relative_path,
            file,
            stdin,
        } => write(&session_id, &relative_path, file.as_deref(), stdin, output),
    }
}

fn list(session: Option<String>, output: OutputMode) -> Result<CliOutcome, CliError> {
    let DeliverableIndex { mut rows, skipped } = deliverable_index(session.as_deref())?;
    if let Some(session) = session.as_deref() {
        // Kept even though the scan already short-circuited on the filename:
        // the reported `session_id` comes from the record's `metadata/id`, and
        // this is what makes the two agree if a record is ever stored under a
        // filename that does not match its own id.
        rows.retain(|row| row.session_id == session);
    }
    let human = rows
        .iter()
        .map(|row| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                // The cells are NOT machine-made here: `name` and `path` are
                // read off the session record's `storage_path`, whose filename
                // part may legally contain `\t` or `\n` (a POSIX filename
                // allows both), and `session_id` comes from the record's
                // `metadata/id` JSON field. Either would split the row into
                // two lines or invent a seventh column for whoever cuts on
                // `\t`, and ESC must not reach the terminal — the same
                // reasons `sessions list` collapses its id and title.
                // `ext` is whitelisted (`DELIVERABLE_EXTS`) and `category` is
                // derived from it, so both are machine-made in practice, but
                // by house rule (see the personas renderer) they go through
                // the column collapse too rather than a chosen subset; a
                // future whitelist change must not re-open the row. JSON
                // keeps the verbatim bytes and `size` is numeric.
                // (sync: `sessions.rs` list, `projects.rs` list)
                crate::support::collapse_control_characters(&row.name),
                crate::support::collapse_control_characters(&row.ext),
                crate::support::collapse_control_characters(&row.category),
                row.size,
                crate::support::collapse_control_characters(&row.session_id),
                crate::support::collapse_control_characters(&row.path)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let value = serde_json::json!({
        "artifacts": rows.iter().map(|row| serde_json::json!({
            "name": row.name,
            "path": row.path,
            "ext": row.ext,
            "category": row.category,
            "session_id": row.session_id,
            "source": row.source,
            "mtime": row.mtime,
            "size": row.size,
        })).collect::<Vec<_>>(),
        // Always present, empty in the normal case: a consumer that has to
        // tell "no deliverables" from "index incomplete" needs a key it can
        // read unconditionally, not one that only appears on the bad day.
        "skipped_sessions": skipped,
    });
    Ok(success(render(output, human, &value)))
}

fn read(session_id: &str, relative_path: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    // Read lane: the ledger-tree containment applies (scheduled-run
    // workspaces readable), matching the GUI's read path. Bounded like the
    // write lane — a multi-gigabyte deliverable fails cleanly instead of
    // being slurped whole into memory.
    let path = resolve_session_artifact(&store, session_id, relative_path, false)?;
    let content =
        read_text_file_capped(&path, MAX_EDITABLE_MARKDOWN_BYTES, "artifact_read_failed")?;
    let value = serde_json::json!({
        "session_id": session_id,
        "path": path.display().to_string(),
        "content": content,
    });
    // Stdout is a terminal and the content is agent-writable: the human arm
    // gets the same block sanitizer `sessions show`/`export` apply at this
    // boundary (newlines/indent survive, ESC/CR and the rest of the control
    // range do not). JSON and the stored file keep the verbatim bytes.
    let human = crate::support::collapse_block_control_characters(&content);
    Ok(success(render(output, human, &value)))
}

fn write(
    session_id: &str,
    relative_path: &str,
    file: Option<&Path>,
    stdin: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    // Both lanes are capped at the save limit BEFORE the read: an unbounded
    // file (`--file /dev/zero`) or stdin (`yes | ...`) would load into memory
    // and only then hit the size check. Reading one byte past the cap
    // distinguishes "at the cap" from "over it".
    let content = match (file, stdin) {
        (Some(file), false) => {
            // Same wire code as the stdin lane for the same over-cap
            // failure: the shared reader answers a generic
            // `{action}: {path} exceeds …` message, and a script keying on
            // the GUI-derived wire code (the one the stdin lane and the
            // post-read re-check below answer) would miss the file lane —
            // the lane scripts are more likely to drive. Pre-sized here so
            // the reader never sees the over-cap file; an unreadable or
            // vanished file still falls to the reader's own error, and a
            // file that grows between this stat and the read is caught by
            // the reader's cap (rare, same honest failure either way).
            if std::fs::metadata(file)
                .map(|metadata| metadata.len() > MAX_EDITABLE_MARKDOWN_BYTES as u64)
                .unwrap_or(false)
            {
                return Err(CliError::failed("markdown_artifact_is_too_large_to_save"));
            }
            // Round-37 review: the source is a user-named path, so it takes
            // the same credential-path policy every sibling ingest lane
            // applies (`agent run --attach`, `feedback --attach`) — without
            // it, `artifacts write s-1 notes.md --file ~/.ssh/id_rsa` copied
            // credential content into model-readable session storage.
            let resolved = std::fs::canonicalize(file).map_err(|error| {
                CliError::failed(format!(
                    "artifacts write: cannot resolve {}: {error}",
                    file.display()
                ))
            })?;
            crate::artifacts::check_sensitive_path(&resolved).map_err(|reason| {
                CliError::failed(format!("artifacts write: refusing --file: {reason}"))
            })?;
            // Read the CANONICAL path — it is what was policy-checked
            // (round-41 review); the original spelling would let a symlink
            // swapped between the two calls route unverified content past
            // the refusal into model-readable session storage.
            crate::support::read_text_file_capped(
                &resolved,
                MAX_EDITABLE_MARKDOWN_BYTES,
                "artifacts write",
            )?
        }
        (None, true) => {
            let mut content = String::new();
            std::io::Read::read_to_string(
                &mut std::io::Read::take(
                    std::io::stdin().lock(),
                    MAX_EDITABLE_MARKDOWN_BYTES as u64 + 1,
                ),
                &mut content,
            )
            .map_err(|error| {
                CliError::failed(format!("artifacts write cannot read stdin: {error}"))
            })?;
            if content.len() > MAX_EDITABLE_MARKDOWN_BYTES {
                return Err(CliError::failed("markdown_artifact_is_too_large_to_save"));
            }
            content
        }
        // Unreachable via parse; kept total so execute stays total too.
        (Some(_), true) | (None, false) => {
            return Err(CliError::usage(
                "artifacts write requires exactly one of --file or --stdin",
            ));
        }
    };
    let store = open_store()?;
    // Write lane: sessions-root confinement only (scheduled-run artifacts
    // are read-only here, like the GUI).
    let path = resolve_session_artifact(&store, session_id, relative_path, true)?;
    // Same md-only overwrite rule as the GUI `write_artifact_text`: only
    // existing .md/.markdown files inside a session may be edited.
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "md" && ext != "markdown" {
        return Err(CliError::failed("only_markdown_artifacts_can_be_edited"));
    }
    if content.len() > MAX_EDITABLE_MARKDOWN_BYTES {
        return Err(CliError::failed("markdown_artifact_is_too_large_to_save"));
    }
    // Stage + rename (the GUI writes atomically under its lifecycle lock): a
    // crash mid-write must not leave a truncated deliverable behind. A
    // deliverable is no more sensitive than the session directory that holds
    // it, so the mode stays at the process umask — unlike the feedback bundle
    // and the persona sidecar, which carry user-authored text.
    pinvou3_lib::platform::atomic_write(&path, content.as_bytes()).map_err(|error| {
        CliError::failed(format!(
            "artifact_write_failed({}): {error}",
            path.display()
        ))
    })?;
    let bytes = content.len();
    let value = serde_json::json!({
        "session_id": session_id,
        "path": path.display().to_string(),
        "bytes": bytes,
    });
    // A POSIX filename may carry control characters and the path is
    // agent-chosen, so the human line collapses it like the list rows;
    // JSON keeps the verbatim path.
    Ok(success(render(
        output,
        format!(
            "wrote {} ({bytes} bytes)",
            collapse_control_characters(&path.display().to_string())
        ),
        &value,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_args;

    fn parse(arguments: &[&str]) -> Result<ArtifactsCommand, CliError> {
        let mut owned: Vec<String> = arguments.iter().map(|value| value.to_string()).collect();
        owned.insert(0, "pinvou".to_owned());
        match parse_args(owned)?.command() {
            crate::CliCommand::Artifacts(command) => Ok(command.clone()),
            _other => panic!(
                "parsed an unexpected command family; the fixture argv does not match the test"
            ),
        }
    }

    #[test]
    fn parses_every_subcommand() {
        assert_eq!(
            parse(&["artifacts", "list"]).unwrap(),
            ArtifactsCommand::List { session: None }
        );
        assert_eq!(
            parse(&["artifacts", "list", "--session", "s-1"]).unwrap(),
            ArtifactsCommand::List {
                session: Some("s-1".into()),
            }
        );
        assert_eq!(
            parse(&["artifacts", "read", "s-1", "artifacts/report.md"]).unwrap(),
            ArtifactsCommand::Read {
                session_id: "s-1".into(),
                relative_path: "artifacts/report.md".into(),
            }
        );
        assert_eq!(
            parse(&[
                "artifacts",
                "write",
                "s-1",
                "artifacts/report.md",
                "--file",
                "in.md"
            ])
            .unwrap(),
            ArtifactsCommand::Write {
                session_id: "s-1".into(),
                relative_path: "artifacts/report.md".into(),
                file: Some(PathBuf::from("in.md")),
                stdin: false,
            }
        );
        assert_eq!(
            parse(&[
                "artifacts",
                "write",
                "s-1",
                "artifacts/report.md",
                "--stdin"
            ])
            .unwrap(),
            ArtifactsCommand::Write {
                session_id: "s-1".into(),
                relative_path: "artifacts/report.md".into(),
                file: None,
                stdin: true,
            }
        );
    }

    #[test]
    fn rejects_invalid_usage_with_exit_code_two() {
        let invalid = [
            vec!["artifacts"],
            vec!["artifacts", "bogus"],
            vec!["artifacts", "list", "--nope"],
            vec!["artifacts", "list", "--session"],
            vec!["artifacts", "list", "--session", "--other"],
            vec!["artifacts", "read"],
            vec!["artifacts", "read", "s-1"],
            vec!["artifacts", "read", "s-1", "a.md", "--extra"],
            vec![
                "artifacts",
                "write",
                "s-1",
                "a.md",
                "extra.md",
                "--file",
                "in.md",
            ],
            vec!["artifacts", "write", "s-1"],
            vec!["artifacts", "write", "s-1", "a.md"],
            vec![
                "artifacts",
                "write",
                "s-1",
                "a.md",
                "--file",
                "a.md",
                "--stdin",
            ],
            vec!["artifacts", "write", "s-1", "a.md", "--file"],
            vec!["artifacts", "write", "s-1", "a.md", "--bogus"],
        ];
        for arguments in invalid {
            let error = parse(&arguments).expect_err(arguments.join(" ").as_str());
            assert_eq!(error.exit_code(), crate::ExitCode::Usage, "{arguments:?}");
        }
    }

    fn scratch(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "pinvou-cli-atomic-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn atomic_write_replaces_the_target_with_the_new_content() {
        // The widened app writer (`platform::filesystem::atomic_write`, the
        // one implementation behind this lane since the local copy was
        // deleted) must leave exactly the new content and no staging residue.
        let dir = scratch("happy");
        let target = dir.join("report.md");
        std::fs::write(&target, b"old").unwrap();
        pinvou3_lib::platform::atomic_write(&target, b"new").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        // Nothing staged is left behind: neither the tmp nor the Windows
        // state machine's backup sibling.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| {
                let name = name.to_string_lossy();
                name.contains(".tmp-") || name.contains(".bak-")
            })
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capped_record_read_cuts_off_past_the_cap() {
        let dir = scratch("capped-read");
        let record = dir.join("s-1.json");
        let body = r#"{"metadata":{"id":"s-1"}}"#;
        std::fs::write(&record, body).unwrap();
        let size = std::fs::metadata(&record).unwrap().len();

        match read_record_capped(&record, size) {
            RecordRead::Text(raw) => assert_eq!(raw, body),
            other => panic!("a record of exactly the cap must read whole, got {other:?}"),
        }
        // One byte below the file size, the record cannot end within the
        // cap: the read must report TooLarge, not return truncated text.
        match read_record_capped(&record, size - 1) {
            RecordRead::TooLarge => {}
            other => panic!("a record past the cap must read as TooLarge, got {other:?}"),
        }
        // Unreadable-as-text content keeps the silent-skip shape the old
        // `read_to_string` error path had.
        std::fs::write(&record, b"\xff\xfe not utf8").unwrap();
        assert!(matches!(
            read_record_capped(&record, size),
            RecordRead::Unreadable
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The mirror of `platform::path_policy` must cover every entry of both
    /// upstream lists, and must not fire on an ordinary deliverable — a
    /// too-eager substring match here would make `artifacts read` refuse
    /// normal files (`environment.md` is not `.env`).
    #[test]
    fn sensitive_path_mirror_matches_the_upstream_blacklists() {
        for component in SENSITIVE_PATH_COMPONENTS {
            let path = PathBuf::from("/home/u/.pinvou3/sessions/s-1/workspace").join(component);
            assert_eq!(
                crosses_sensitive_component(&path),
                Some(*component),
                "{component} must be refused"
            );
            assert!(check_sensitive_path(&path).is_err(), "{component}");
        }
        for prefix in SENSITIVE_PATH_PREFIXES {
            let path = PathBuf::from(format!("{prefix}probe"));
            assert!(
                check_sensitive_path(&path).is_err(),
                "{prefix} must be refused"
            );
        }
        // Names that merely contain a blacklisted string are not components.
        for benign in [
            "/home/u/.pinvou3/sessions/s-1/workspace/environment.md",
            "/home/u/.pinvou3/sessions/s-1/workspace/id_rsa_notes.md",
            "/home/u/.pinvou3/sessions/s-1/artifacts/credentials.json.md",
            "/home/u/.pinvou3/sessions/s-1/artifacts/sshkeys.md",
        ] {
            let path = PathBuf::from(benign);
            assert_eq!(crosses_sensitive_component(&path), None, "{benign}");
            assert!(check_sensitive_path(&path).is_ok(), "{benign}");
        }
    }

    /// The feedback bundle and the persona sidecar hold user-authored text;
    /// the default umask (~0644) publishes it to every account on the machine.
    #[cfg(unix)]
    #[test]
    fn owner_only_writes_land_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("mode");
        let target = dir.join("secret.json");
        pinvou3_lib::platform::atomic_write_private(&target, b"{}").unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "got {mode:o}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
