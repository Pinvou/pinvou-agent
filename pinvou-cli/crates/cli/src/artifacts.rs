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
//! - read/write → mirror of `platform::path_policy::validate_user_path` +
//!   the GUI `write_artifact_text` rules (absolute path, canonical path must
//!   stay inside `sessions_root()/<session-id>/{artifacts,workspace}`,
//!   session id must not start with `_`, markdown-only overwrite of an
//!   existing file, 10 MiB cap). Relative paths resolve against the
//!   session's ledger workspace, like the GUI `resolve_artifact_path`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::support::{render, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::sessions::SessionStore;

/// Same cap as the GUI artifact editor (`MAX_EDITABLE_MARKDOWN_BYTES`).
const MAX_EDITABLE_MARKDOWN_BYTES: usize = 10 * 1024 * 1024;

const DELIVERABLE_EXTS: &[&str] = &[
    "pptx", "ppt", "docx", "doc", "pdf", "html", "htm", "xlsx", "xls", "md", "csv", "png", "jpg",
    "jpeg", "svg", "gif", "webp", "zip",
];

fn deliverable_category(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" | "mhtml" | "mht" => "web",
        "ppt" | "pptx" | "odp" | "dps" => "ppt",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "heic" => "img",
        _ => "doc",
    }
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
    Ok((rest[0].clone(), rest[1].clone()))
}

fn open_store() -> Result<SessionStore, CliError> {
    SessionStore::boot()
        .map_err(|error| CliError::failed(format!("artifacts store unavailable: {error:#}")))
}

/// Mirrors the GUI `valid_session_id` guard: ids are joined onto filesystem
/// paths, so anything outside `[A-Za-z0-9_-]` is rejected outright.
fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
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

/// Mirror of `features::deliverables::list_deliverable_index_impl`: scan
/// `sessions_root()/*.json`, keep tracked artifacts that still exist on
/// disk, whitelist deliverable extensions, dedupe by physical path keeping
/// the newest mtime, sort by mtime descending then name.
fn deliverable_index() -> Vec<DeliverableRow> {
    let sessions_dir = pinvou3_lib::platform::paths::sessions_root();
    let entries = match std::fs::read_dir(&sessions_dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut by_path: HashMap<String, DeliverableRow> = HashMap::new();
    for entry in entries.flatten() {
        let file = entry.path();
        if !file.is_file() || file.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(view) = serde_json::from_str::<serde_json::Value>(&raw) else {
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
    let mut out: Vec<DeliverableRow> = by_path.into_values().collect();
    out.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Resolves `<session-id> <relative-path>` to a canonical artifact file
/// under `sessions_root()/<session-id>/{artifacts,workspace}`, mirroring
/// `resolve_artifact_path_in_workspace` (relative against the ledger
/// workspace) plus `ensure_editable_artifact_path` containment checks.
fn resolve_session_artifact(
    store: &SessionStore,
    session_id: &str,
    relative_path: &str,
) -> Result<PathBuf, CliError> {
    if !valid_session_id(session_id) {
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
    let canonical = std::fs::canonicalize(&path)
        .map_err(|error| CliError::failed(format!("artifact_not_found: {error}")))?;
    if !canonical.is_file() {
        return Err(CliError::failed(format!(
            "artifact_not_found: {} is not a file",
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
    let relative = canonical
        .strip_prefix(&sessions_root)
        .map_err(|_| CliError::failed("artifact_outside_session_storage"))?;
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
    let mut rows = deliverable_index();
    if let Some(session) = session.as_deref() {
        rows.retain(|row| row.session_id == session);
    }
    let human = rows
        .iter()
        .map(|row| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                row.name, row.ext, row.category, row.size, row.session_id, row.path
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
    });
    Ok(success(render(output, human, &value)))
}

fn read(session_id: &str, relative_path: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    let path = resolve_session_artifact(&store, session_id, relative_path)?;
    let content = std::fs::read_to_string(&path).map_err(|error| {
        CliError::failed(format!("artifact_read_failed({}): {error}", path.display()))
    })?;
    let value = serde_json::json!({
        "session_id": session_id,
        "path": path.display().to_string(),
        "content": content,
    });
    Ok(success(render(output, content, &value)))
}

fn write(
    session_id: &str,
    relative_path: &str,
    file: Option<&Path>,
    stdin: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let content = match (file, stdin) {
        (Some(file), false) => std::fs::read_to_string(file).map_err(|error| {
            CliError::failed(format!(
                "artifacts write cannot read --file {}: {error}",
                file.display()
            ))
        })?,
        (None, true) => {
            let mut content = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut content).map_err(
                |error| CliError::failed(format!("artifacts write cannot read stdin: {error}")),
            )?;
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
    let path = resolve_session_artifact(&store, session_id, relative_path)?;
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
    // Temp + rename (the GUI writes atomically under its lifecycle lock): a
    // crash mid-write must not leave a truncated deliverable behind.
    {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = path.with_extension(format!("md.tmp.{}.{}", std::process::id(), nonce));
        std::fs::write(&tmp, &content).map_err(|error| {
            CliError::failed(format!(
                "artifact_write_failed({}): {error}",
                path.display()
            ))
        })?;
        if let Err(error) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(CliError::failed(format!(
                "artifact_write_failed({}): {error}",
                path.display()
            )));
        }
    }
    let bytes = content.len();
    let value = serde_json::json!({
        "session_id": session_id,
        "path": path.display().to_string(),
        "bytes": bytes,
    });
    Ok(success(render(
        output,
        format!("wrote {} ({bytes} bytes)", path.display()),
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
            other => panic!("unexpected command: {other:?}"),
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
}
