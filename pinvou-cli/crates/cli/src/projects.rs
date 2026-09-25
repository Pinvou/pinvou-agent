//! `projects` family: the project layer (logical session grouping),
//! mirroring `pinvou3-app/src-tauri/src/app/commands/projects.rs`.
//!
//! Every operation calls the same `pinvou3_lib::features::projects` store
//! methods the GUI commands call (`list`, `create_project`,
//! `update_project`, `delete_project`, `move_session_to_project`,
//! `assignments_snapshot`, `assigned_session_ids`). The list JSON is a
//! CLI-shaped superset of the GUI wire DTO: the GUI's `ProjectListItem`
//! deliberately omits per-root `available` and the `created_at`/`updated_at`
//! timestamps from the wire, while the CLI renders them (`id`, `name`,
//! `roots` with per-root `path`, `position`, `created_at`, `updated_at`,
//! `assigned_session_count`) plus the full `assignments` map. Additive only:
//! every GUI field name is preserved. Pure storage: no Tauri host, no
//! engine.
//!
//! Headless deviations, disclosed:
//! - `move` always passes `add_workspace_root = None`: folding the session's
//!   bound workspace folder into the target roots resolves through the
//!   desktop app's ACP pool, which a one-shot CLI does not boot.
//! - The GUI can set a project's roots back to the empty list through
//!   `update_project`; the CLI treats `--root` absence as "keep the current
//!   roots" and offers no clear-roots flag.
//! - `move` keeps the GUI's product decision that assignments only accept
//!   chat sessions (scheduled-run sessions are managed from Scheduled), and
//!   proves session existence through a real `SessionStore::load` because
//!   `session_kind` alone reports Chat for unknown ids without touching the
//!   disk.
//!
//! Error copy: the feature's own error strings are English and pass through
//! prefixed with `projects <subcommand>`; unknown ids and operational
//! failures exit 1, usage errors exit 2, and destructive `delete` requires
//! `--yes` before any store access (deleting a project never deletes
//! sessions — affected sessions are only unassigned).

use std::path::PathBuf;

use crate::support::{render, require_yes, sandbox_home, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::projects::{Project, ProjectStore};
use pinvou3_lib::features::sessions::{SessionKind, SessionStore};

const USAGE: &str = "usage: pinvou projects <list|create|update|delete|move>";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectsCommand {
    List,
    Create {
        name: String,
        roots: Vec<PathBuf>,
    },
    Update {
        id: String,
        name: Option<String>,
        roots: Option<Vec<PathBuf>>,
    },
    Delete {
        id: String,
        yes: bool,
    },
    Move {
        session_id: String,
        project_id: Option<String>,
    },
}

/// Flags that carry a value, per subcommand. `--root` is the one repeatable
/// flag (the GUI create/update take a root list), so it is pre-extracted
/// before the single-use flags go through the shared parser.
const CREATE_OPTIONS: &[&str] = &["--name"];
const UPDATE_OPTIONS: &[&str] = &["--name"];

/// Boolean (valueless) flags, per subcommand.
const DELETE_FLAGS: &[&str] = &["--yes"];

pub fn parse(values: &[String]) -> Result<ProjectsCommand, CliError> {
    let subcommand = values.get(1).ok_or_else(|| CliError::usage(USAGE))?;
    let rest = &values[2..];
    match subcommand.as_str() {
        "list" => {
            parse_flags(rest, &[], &[])?;
            Ok(ProjectsCommand::List)
        }
        "create" => {
            let (rest, roots) = extract_roots(rest)?;
            let (options, _) = parse_flags(&rest, CREATE_OPTIONS, &[])?;
            let name = option(&options, "--name")
                .ok_or_else(|| CliError::usage("projects create requires --name N"))?
                .to_owned();
            if name.trim().is_empty() {
                return Err(CliError::usage("projects create --name must not be empty"));
            }
            Ok(ProjectsCommand::Create { name, roots })
        }
        "update" => {
            let id = require_id(rest.first(), "update")?;
            let (rest, roots) = extract_roots(&rest[1..])?;
            let (options, _) = parse_flags(&rest, UPDATE_OPTIONS, &[])?;
            if option(&options, "--name").is_some_and(|name| name.trim().is_empty()) {
                return Err(CliError::usage("projects update --name must not be empty"));
            }
            let roots = if roots.is_empty() { None } else { Some(roots) };
            Ok(ProjectsCommand::Update {
                id,
                name: option(&options, "--name").map(str::to_owned),
                roots,
            })
        }
        "delete" => {
            let id = require_id(rest.first(), "delete")?;
            let (_, flags) = parse_flags(&rest[1..], &[], DELETE_FLAGS)?;
            Ok(ProjectsCommand::Delete {
                id,
                yes: flags.contains(&"--yes"),
            })
        }
        "move" => {
            let session_id = require_id(rest.first(), "move")?;
            // Omitting the project id moves the session out of its project —
            // the store's None arm, the same entry the GUI's move picker
            // offers as ungrouped. An explicit empty or flag-shaped token
            // stays a usage error.
            let project_id = match rest.get(1) {
                None => None,
                Some(id) if id.is_empty() || id.starts_with("--") => {
                    return Err(CliError::usage("projects move: invalid project id"));
                }
                Some(id) => Some(id.clone()),
            };
            if rest.len() > 2 {
                return Err(CliError::usage("projects move accepts no options"));
            }
            Ok(ProjectsCommand::Move {
                session_id,
                project_id,
            })
        }
        _ => Err(CliError::usage(USAGE)),
    }
}

/// Pre-extracts the repeatable `--root PATH` flags the shared single-use
/// parser cannot express; remaining tokens flow into `parse_family_flags`
/// and malformed `--root` values reproduce the shared parser's error copy.
fn extract_roots(values: &[String]) -> Result<(Vec<String>, Vec<PathBuf>), CliError> {
    let mut rest = Vec::new();
    let mut roots = Vec::new();
    let mut index = 0;
    while index < values.len() {
        if values[index] == "--root" {
            let value = values
                .get(index + 1)
                .ok_or_else(|| CliError::usage("projects option --root requires a value"))?;
            if value.is_empty() || value.starts_with("--") {
                return Err(CliError::usage("projects option --root requires a value"));
            }
            roots.push(PathBuf::from(value));
            index += 2;
            continue;
        }
        rest.push(values[index].clone());
        index += 1;
    }
    Ok((rest, roots))
}

fn require_id(value: Option<&String>, subcommand: &str) -> Result<String, CliError> {
    let id = value
        .ok_or_else(|| CliError::usage(format!("projects {subcommand} requires an id")))?
        .clone();
    if id.is_empty() {
        return Err(CliError::usage(format!(
            "projects {subcommand} requires an id"
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
    crate::support::parse_family_flags(values, value_flags, boolean_flags, "projects")
}

fn option<'a>(options: &'a [(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    crate::support::family_option(options, name)
}

// ─────────────────────────────── execute ───────────────────────────────

pub fn execute(command: ProjectsCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    match command {
        ProjectsCommand::List => list(output),
        ProjectsCommand::Create { name, roots } => create(&name, roots, output),
        ProjectsCommand::Update { id, name, roots } => update(&id, name, roots, output),
        ProjectsCommand::Delete { id, yes } => delete(&id, yes, output),
        ProjectsCommand::Move {
            session_id,
            project_id,
        } => move_session(&session_id, project_id.as_deref(), output),
    }
}

fn project_error(context: &str, error: impl std::fmt::Display) -> CliError {
    CliError::failed(format!("projects {context}: {error:#}"))
}

fn open_store() -> Result<ProjectStore, CliError> {
    // Same absolute-path contract as the other families: a relative
    // PINVOU3_HOME would silently resolve against the cwd. The store boot
    // itself never fails (corrupt files degrade to the empty state).
    sandbox_home()?;
    Ok(ProjectStore::boot())
}

fn open_session_store() -> Result<SessionStore, CliError> {
    sandbox_home()?;
    SessionStore::boot()
        .map_err(|error| CliError::failed(format!("sessions store unavailable: {error:#}")))
}

/// One list row in the GUI `ProjectListItem` shape: the stored project with
/// per-root availability and the explicit member count (`from_project`).
fn project_item(project: &Project, assigned_session_count: usize) -> serde_json::Value {
    let roots = project
        .roots
        .iter()
        .map(|path| {
            serde_json::json!({
                "path": path.display().to_string(),
                "available": path.is_dir(),
            })
        })
        .collect::<Vec<_>>();
    let mut item = serde_json::to_value(project).unwrap_or_else(|_| serde_json::json!({}));
    item["roots"] = serde_json::to_value(roots).unwrap_or_else(|_| serde_json::json!([]));
    item["assigned_session_count"] = serde_json::json!(assigned_session_count);
    item
}

fn roots_human(project: &Project) -> String {
    if project.roots.is_empty() {
        return "-".to_owned();
    }
    project
        .roots
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// One human-mode `projects list` row: four tab-separated columns
/// (id, name, member count, roots).
///
/// Two of the four cells carry strings this CLI does not control. The project
/// name is stored as typed — `features::projects::store` only trims the ends,
/// so a tab, a newline or an ESC survives into the store file and back out
/// here — and root paths are filesystem paths, where the same bytes are
/// legal. A tab would invent a fifth column and a newline would split one
/// project across two rows for whoever is cutting the output on `\t`, so both
/// cells go through the column collapse. Human mode only: the JSON payload
/// keeps the real name and paths (`serde_json` escapes everything below
/// 0x20, so it is already safe to read back).
fn project_row(project: &Project, assigned_session_count: usize) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        project.id,
        crate::support::collapse_control_characters(&project.name),
        assigned_session_count,
        crate::support::collapse_control_characters(&roots_human(project)),
    )
}

/// Mirror of `list_projects`: projects in store order (position, id) with
/// per-root availability and the full assignment map.
fn list(output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    let assignments = store.assignments_snapshot();
    let projects = store.list();
    let mut rows = Vec::new();
    let mut items = Vec::new();
    for project in &projects {
        let count = store.assigned_session_ids(&project.id).len();
        rows.push(project_row(project, count));
        items.push(project_item(project, count));
    }
    let value = serde_json::json!({
        "projects": items,
        "assignments": assignments,
    });
    Ok(success(render(output, rows.join("\n"), &value)))
}

/// Mirror of `create_project`: roots are optional (a pure-label project);
/// the GUI response carries an explicit member count of zero for a fresh
/// project.
fn create(name: &str, roots: Vec<PathBuf>, output: OutputMode) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    let project = store
        .create_project(name.to_owned(), roots)
        .map_err(|error| project_error("create", error))?;
    let value = project_item(&project, 0);
    Ok(success(render(
        output,
        format!("created {}", project.id),
        &value,
    )))
}

/// Mirror of `update_project`: name and roots are optional patches; a root
/// list replaces the stored roots wholesale, absence keeps them.
fn update(
    id: &str,
    name: Option<String>,
    roots: Option<Vec<PathBuf>>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    let store = open_store()?;
    let project = store
        .update_project(id, name, roots)
        .map_err(|error| project_error("update", error))?;
    let count = store.assigned_session_ids(&project.id).len();
    let value = project_item(&project, count);
    Ok(success(render(
        output,
        format!("updated {}", project.id),
        &value,
    )))
}

/// Mirror of `delete_project`: `--yes` is checked before any store access;
/// affected sessions are only unassigned (they fall back to implicit
/// grouping), never deleted.
fn delete(id: &str, yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    require_yes(yes)?;
    let store = open_store()?;
    let report = store
        .delete_project(id)
        .map_err(|error| project_error("delete", error))?;
    let value = serde_json::json!({
        "id": id,
        "action": "deleted",
        "affected_session_ids": report.affected_session_ids,
    });
    Ok(success(render(output, format!("deleted {id}"), &value)))
}

/// Mirror of `move_session_to_project` (storage-only subset): the assignment
/// write is the exact store call, gated by the GUI's chat-session checks.
fn move_session(
    session_id: &str,
    project_id: Option<&str>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    // Session ids join onto store paths inside the session store, so apply
    // the same `[A-Za-z0-9_-]` restriction the sessions family enforces
    // before any store use; anything else is a usage error, never a
    // traversal. Usage wins over Failed: a malformed session id reports the
    // usage error even when the project id is unknown too.
    crate::support::require_valid_session_id(session_id, "projects move")?;
    let sessions = open_session_store()?;
    match sessions
        .session_kind(session_id)
        .map_err(|error| project_error("move", error))?
    {
        SessionKind::ScheduledRun => {
            return Err(CliError::failed(format!(
                "projects move: session {session_id} is a scheduled run; \
                 scheduled-run sessions are managed from Scheduled"
            )));
        }
        SessionKind::Chat => {}
    }
    sessions.load(session_id).map_err(|error| {
        CliError::failed(format!(
            "projects move: session {session_id} does not exist ({error})"
        ))
    })?;
    let store = open_store()?;
    // The GUI's add_workspace_root lane needs the ACP pool to resolve the
    // session's workspace record; see the module header for the disclosed
    // headless deviation.
    let outcome = store
        .move_session_to_project(session_id, project_id, None)
        .map_err(|error| project_error("move", error))?;
    let mut value = serde_json::to_value(&outcome).unwrap_or_else(|_| serde_json::json!({}));
    value["session_id"] = serde_json::json!(session_id);
    let human = match project_id {
        Some(project_id) => format!("moved {session_id} into {project_id}"),
        None => format!("moved {session_id} out of its project"),
    };
    Ok(success(render(output, human, &value)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_fixture(name: &str, roots: Vec<PathBuf>) -> Project {
        let now = chrono::Utc::now();
        Project {
            id: "prj-fixture".to_owned(),
            name: name.to_owned(),
            roots,
            position: 0,
            created_at: now,
            updated_at: now,
        }
    }

    /// The human `projects list` row is a four-column tab-separated record
    /// and `projects_contract.rs` asserts that shape. A name or a root path
    /// carrying a tab, a newline or an ESC must not be able to break it:
    /// the store keeps such a name verbatim (it only trims the ends), so
    /// without the column collapse a single project would render as two rows
    /// with five columns between them, and the ESC would reach the terminal.
    #[test]
    fn list_row_keeps_four_columns_when_the_name_and_roots_carry_control_characters() {
        let row = project_row(
            &project_fixture(
                "Alpha\tBeta\nGamma\x1b[31m",
                vec![PathBuf::from("/tmp/one\ttwo"), PathBuf::from("/tmp/three")],
            ),
            2,
        );
        assert_eq!(
            row.lines().count(),
            1,
            "the row must stay one line: {row:?}"
        );
        let columns: Vec<&str> = row.split('\t').collect();
        assert_eq!(columns.len(), 4, "the row must keep four columns: {row:?}");
        assert_eq!(columns[0], "prj-fixture");
        assert_eq!(columns[1], "Alpha Beta Gamma [31m");
        assert_eq!(columns[2], "2");
        assert_eq!(columns[3], "/tmp/one two, /tmp/three");
        assert!(
            !row.contains('\x1b'),
            "ESC must not reach the terminal: {row:?}"
        );
    }

    /// The collapse is a rendering choice, not a data change: a name without
    /// control characters must render byte-for-byte, and a project with no
    /// roots must keep the "-" placeholder rather than an empty cell.
    #[test]
    fn list_row_leaves_ordinary_names_and_empty_roots_untouched() {
        let row = project_row(&project_fixture("Alpha", Vec::new()), 0);
        assert_eq!(row, "prj-fixture\tAlpha\t0\t-");
    }
}
