//! `projects` family: the project layer (logical session grouping),
//! mirroring `pinvou3-app/src-tauri/src/app/commands/projects.rs`.
//!
//! Every operation calls the same `pinvou3_lib::features` store methods the
//! GUI commands call (`list`, `create_project`, `update_project`,
//! `delete_project`, `move_session_to_project`, `assignments_snapshot`,
//! `assigned_session_ids`, and for `rebind`: `begin_rebind`,
//! `plan_rebind_roots`, `rebind_roots`, `rebind_source_display`,
//! `SessionAgentStore::rebind_workspace_prefix`,
//! `SessionStore::rebind_workspace_bindings`, `SessionStore::set_workspace`). The list JSON is a
//! CLI-shaped superset of the GUI wire DTO: the GUI's `ProjectListItem`
//! deliberately omits per-root `available` and the `created_at`/`updated_at`
//! timestamps from the wire, while the CLI renders them (`id`, `name`,
//! `roots` with per-root `path`, `position`, `created_at`, `updated_at`,
//! `assigned_session_count`) plus the full `assignments` map. Additive only:
//! every GUI field name is preserved. Pure storage: no Tauri host, no
//! engine.
//!
//! The per-root `available` field the CLI adds back costs one `stat(2)` per
//! root per list row — the cost the GUI dropped the field to avoid. See
//! [`project_item`] for why that is kept and why it is not memoized.
//!
//! Headless deviations, disclosed:
//! - `move` always passes `add_workspace_root = None`: folding the session's
//!   bound workspace folder into the target roots resolves through the
//!   desktop app's ACP pool, which a one-shot CLI does not boot.
//! - `move` without a project id writes the store's explicit-ungroup entry,
//!   which is irreversible to "auto-grouped" on either surface, so it is
//!   gated the way the GUI gates it: the session must currently RESOLVE to a
//!   project. Tier 1 of that resolution is the store's own assignments map;
//!   tier 2 (auto-grouping by the session's bound workspace directory) lives
//!   only in the frontend and is reproduced in [`resolved_project_id`], with
//!   its one deviation documented on [`path_is_under_root`].
//! - The GUI can set a project's roots back to the empty list through
//!   `update_project`; the CLI treats `--root` absence as "keep the current
//!   roots" and offers no clear-roots flag.
//! - `move` keeps the GUI's product decision that assignments only accept
//!   chat sessions (scheduled-run sessions are managed from Scheduled), and
//!   proves session existence through a real `SessionStore::load` because
//!   `session_kind` alone reports Chat for unknown ids without touching the
//!   disk.
//! - `rebind` is the storage half of the GUI's `rebind_workspace_root`
//!   (the broken-link repair after a project directory moved): the same
//!   store writes through the same public store APIs and under the same
//!   rebind gate — codex lane (`session-agents.json` index + code-session
//!   sidecars, including off-index orphans), plain-chat lane
//!   (`workspace-binding.json` sidecars + legacy global table), SavedSession
//!   metadata replay, baseline recapture, and project roots last. What is
//!   NOT reproducible headless is the desktop-process half, and every run
//!   discloses that on stderr: the active-turn fence, the post-migration
//!   busy recheck and the idle-gated runtime eviction need the app's
//!   `AcpPool`/`EnginePool`, so a live desktop app's active turns against
//!   the old directory are not reclaimed from here — close the app or
//!   re-run the rebind from the desktop for that half. `--yes` is required
//!   unconditionally, which subsumes the GUI's confirm-existing escalation
//!   (the GUI demands strong confirmation only while the old directory
//!   still exists); the dialog's post-busy retry plumbing
//!   (`previous_post_busy_session_ids`) has no headless counterpart. The
//!   GUI's pre-rewrite plain-lane snapshot and its post-pass fence rescan
//!   live behind `pub(crate)` store helpers, so the metadata replay is
//!   driven by the lanes' outcomes (what each rewrote, plus what each
//!   reported failed) instead of a pre-rewrite snapshot: every pre-existing
//!   under-`from` binding still reaches the report through the rebound or
//!   failed arm, only by a different route.
//!
//! Error copy: the feature's own error strings are English and pass through
//! prefixed with `projects <subcommand>`; unknown ids and operational
//! failures exit 1, usage errors exit 2, and the destructive `delete`, the
//! irreversible ungrouping `move`, and the wholesale `rebind` all require
//! `--yes` before any store access (deleting a project never deletes
//! sessions — affected sessions are only unassigned).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::support::{render, require_yes, sandbox_home, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::codex_acp::{
    CodexWorkspaceKind, SessionAgentStore, validate_codex_project_workspace,
};
use pinvou3_lib::features::projects::{
    Project, ProjectStore, RebindRootsError, rebind_source_display,
};
use pinvou3_lib::features::sessions::{SessionKind, SessionStore};

const USAGE: &str = "usage: pinvou projects <list|create|update|delete|move|rebind>";

/// Disclosure carried by every `projects move` usage error, because the
/// asymmetry is not guessable from the command shape: omitting the project id
/// does not "clear" the assignment back to its default, it writes an EXPLICIT
/// ungroup entry (`store.rs`: "null = 显式移出，阻止自动归组复活") whose whole
/// purpose is to stop auto-grouping from putting the session back. Nothing in
/// either surface turns that entry back into "no entry": `delete_project`
/// only filters assignments that name a project, and `forget_session` /
/// `retain_sessions` only fire when the session itself is deleted. The only
/// way out is another `move <session> <project>`.
const MOVE_UNGROUP_NOTE: &str = "note: `projects move <session>` without a project id writes an \
EXPLICIT ungroup that permanently opts the session out of auto-grouping; it cannot be reverted \
to \"grouped automatically\" — only another `projects move <session> <project>` overwrites it, \
and it requires --yes";

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
        yes: bool,
    },
    Rebind {
        from: PathBuf,
        to: PathBuf,
        yes: bool,
    },
}

/// Flags that carry a value, per subcommand. `--root` is the one repeatable
/// flag (the GUI create/update take a root list), so it is pre-extracted
/// before the single-use flags go through the shared parser.
const CREATE_OPTIONS: &[&str] = &["--name"];
const UPDATE_OPTIONS: &[&str] = &["--name"];

/// Boolean (valueless) flags, per subcommand.
const DELETE_FLAGS: &[&str] = &["--yes"];
const MOVE_FLAGS: &[&str] = &["--yes"];
const REBIND_FLAGS: &[&str] = &["--yes"];

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
            // Split the session-id tail into the project positional and the
            // flag region, positional first like every sibling subcommand
            // (delete/show parse their flags after the id). Omitting the
            // project id is the ungroup form — the store's None arm, the
            // same entry the GUI's move picker offers as ungrouped. Its
            // remaining tokens are only `--yes`; with a project id the line
            // ends there. What the None arm WRITES is irreversible, so every
            // usage error on this lane carries the disclosure.
            let (project_id, flag_tail): (Option<String>, &[String]) = match rest.get(1) {
                // No project id: ungroup form, all remaining tokens are flags.
                None => (None, &rest[1..]),
                // The confirmation flag is the only legal non-positional
                // after the session id, but a flag BEFORE the positional
                // (`--yes prj-1`) is malformed.
                Some(token) if token == "--yes" => match rest.get(2) {
                    Some(_) => {
                        return Err(CliError::usage(format!(
                            "projects move: invalid project id\n{MOVE_UNGROUP_NOTE}"
                        )));
                    }
                    None => (None, &rest[1..]),
                },
                // Explicit empty or other flag-shaped project token.
                Some(id) if id.is_empty() || id.starts_with("--") => {
                    return Err(CliError::usage(format!(
                        "projects move: invalid project id\n{MOVE_UNGROUP_NOTE}"
                    )));
                }
                // A project id: the line ends there, no flags this form takes.
                Some(id) => {
                    if rest.len() > 2 {
                        return Err(CliError::usage(format!(
                            "projects move accepts no options\n{MOVE_UNGROUP_NOTE}"
                        )));
                    }
                    (Some(id.clone()), &[])
                }
            };
            let (_, flags) = parse_flags(flag_tail, &[], MOVE_FLAGS)?;
            Ok(ProjectsCommand::Move {
                session_id,
                project_id,
                yes: flags.contains(&"--yes"),
            })
        }
        "rebind" => {
            // Two path positionals, flags after them like every sibling
            // subcommand. A flag-shaped token before a positional is
            // malformed (the shared parser would otherwise eat it as a
            // flag), but an EMPTY positional parses through: the
            // execute-level validation mirrors the GUI command's rules
            // verbatim and its message is the single place that names
            // empty, relative and filesystem-root `from` alike.
            let from = require_path(rest.first(), "rebind <from> <to>")?;
            let to = require_path(rest.get(1), "rebind <from> <to>")?;
            let (_, flags) = parse_flags(&rest[2..], &[], REBIND_FLAGS)?;
            Ok(ProjectsCommand::Rebind {
                from,
                to,
                yes: flags.contains(&"--yes"),
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

/// Positional path token for `rebind`: required, and never flag-shaped (an
/// empty token deliberately parses through — see the parse arm).
fn require_path(value: Option<&String>, tail: &str) -> Result<PathBuf, CliError> {
    let value = value.ok_or_else(|| CliError::usage(format!("projects {tail} are required")))?;
    if value.starts_with("--") {
        return Err(CliError::usage(format!(
            "projects rebind: expected a path, got flag-shaped {value:?}"
        )));
    }
    Ok(PathBuf::from(value))
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
            yes,
        } => move_session(&session_id, project_id.as_deref(), yes, output),
        ProjectsCommand::Rebind { from, to, yes } => rebind(&from, &to, yes, output),
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
///
/// Cost of the `available` field, at the point of action: `is_dir()` is one
/// `stat(2)` per root per row, and the GUI deliberately dropped the field from
/// its wire DTO to avoid exactly that ("连带省去列表路径的逐个 is_dir() stat",
/// `app/commands/projects.rs`). On a network mount each of those stats can
/// block for as long as the mount takes to answer, so a `projects list` over
/// an unreachable NFS/SMB root is as slow as the mount, not as slow as the
/// store. The CLI keeps the field anyway — a headless caller has no other way
/// to learn a root went missing — and it is NOT memoized across rows on
/// purpose: `validate_roots` rejects a root that is the same as, or nested
/// under, a root of any other project, so no two rows in one listing can ever
/// stat the same path and a cache would only add bookkeeping.
///
/// A serialization failure is propagated rather than degraded to
/// `json!({})`/`json!([])`: that placeholder answers a caller's `.id` with
/// `null` under exit 0, which is a successful WRONG answer — the same rule
/// `personas::summary_value` spells out, applied here so the PR has one rule.
fn project_item(
    project: &Project,
    assigned_session_count: usize,
    context: &str,
) -> Result<serde_json::Value, CliError> {
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
    let mut item = serde_json::to_value(project)
        .map_err(|error| CliError::failed(format!("projects {context}: {error}")))?;
    // Already `Vec<Value>`: wrapping it directly skips a `to_value` round-trip
    // that could only ever succeed.
    item["roots"] = serde_json::Value::Array(roots);
    item["assigned_session_count"] = serde_json::json!(assigned_session_count);
    Ok(item)
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
        items.push(project_item(project, count, "list")?);
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
    let value = project_item(&project, 0, "create")?;
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
    let value = project_item(&project, count, "update")?;
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

/// The directory a session is bound to, for tier-2 auto-grouping.
///
/// Same union the app's own execution-root resolver builds (`lib.rs`) and the
/// same two sources the frontend's session items carry: an ACP/code session's
/// project workspace comes from the agent index (`session-agents.json`), a
/// bound plain work session's comes from its `workspace-binding.json`
/// sidecar. A session with neither has no project directory at all, which is
/// the frontend's `hasProjectWorkspace(item) === false` — it cannot be
/// auto-grouped.
///
/// The agent record is read directly rather than through
/// `code_project_workspace`, which additionally requires code MODE: an ACP
/// session in plain mode still carries a project workspace, and the frontend
/// lists it.
fn session_workspace_path(sessions: &SessionStore, session_id: &str) -> Option<PathBuf> {
    let record = SessionAgentStore::load_or_empty().get(session_id);
    if record.workspace_kind == CodexWorkspaceKind::Project
        && let Some(path) = record.workspace_path
    {
        return Some(path);
    }
    sessions.session_workspace_binding(session_id)
}

/// Containment rule behind tier-2 grouping.
///
/// The frontend compares strings and guards the boundary by hand
/// (`a.startsWith(b) && a[b.length] === '/'`, after stripping trailing
/// separators). `Path::starts_with` is the same rule expressed in components:
/// it matches whole components only, so `/srv/appdata` is not under `/srv/app`,
/// and trailing separators stop mattering. An empty root is excluded — for the
/// frontend that case degenerates to "any absolute path", which would make one
/// hand-edited blank root swallow every session.
///
/// Known limit: the frontend also case-folds when BOTH sides look like Windows
/// paths. This comparison does not, so on Windows a root and a workspace that
/// differ only in case resolve as unrelated. That can only make the gate below
/// more permissive (it never refuses a move the GUI would allow), which is the
/// safe direction for an irreversible write.
fn path_is_under_root(path: &Path, root: &Path) -> bool {
    !root.as_os_str().is_empty() && path.starts_with(root)
}

/// The project a session currently resolves to, mirroring the frontend's
/// `resolveSessionProjectId` (`features/projects/projectGrouping.js`): tier 1
/// is the explicit assignment, tier 2 is auto-grouping by the session's bound
/// workspace directory, longest root winning so a nested project root cannot
/// be stolen by a shallower one.
///
/// The Rust store exposes tier 1 only, and only as the whole map: the
/// per-session `assignment_of` is `#[cfg(test)]`, so the production reader is
/// `assignments_snapshot` — the same call the command layer ships to the
/// frontend alongside `list_projects`. Tier 2 lives in the frontend, so it is
/// reproduced here from the store's own data rather than left unchecked. The
/// one deviation is the case folding documented on [`path_is_under_root`].
fn resolved_project_id(
    store: &ProjectStore,
    sessions: &SessionStore,
    session_id: &str,
) -> Option<String> {
    let projects = store.list();
    match store.assignments_snapshot().get(session_id).cloned() {
        // Assigned to a project that still exists: that is the answer.
        Some(Some(project_id)) if projects.iter().any(|project| project.id == project_id) => {
            return Some(project_id);
        }
        // An explicit ungroup entry resolves to "no project", full stop — it
        // exists precisely to stop tier 2 from answering.
        Some(None) => return None,
        // A dangling id (its project was deleted out from under the entry) and
        // "no entry at all" both fall through to tier 2, exactly as the
        // frontend does.
        Some(Some(_)) | None => {}
    }
    let workspace = session_workspace_path(sessions, session_id)?;
    let mut best: Option<(&str, usize)> = None;
    for project in &projects {
        for root in &project.roots {
            if !path_is_under_root(&workspace, root) {
                continue;
            }
            let depth = root.as_os_str().len();
            if best.is_none_or(|(_, best_depth)| depth > best_depth) {
                best = Some((project.id.as_str(), depth));
            }
        }
    }
    best.map(|(project_id, _)| project_id.to_owned())
}

/// Mirror of `move_session_to_project` (storage-only subset): the assignment
/// write is the exact store call, gated by the GUI's chat-session checks.
fn move_session(
    session_id: &str,
    project_id: Option<&str>,
    yes: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    // Session ids join onto store paths inside the session store, so apply
    // the same `[A-Za-z0-9_-]` restriction the sessions family enforces
    // before any store use; anything else is a usage error, never a
    // traversal. Usage wins over Failed: a malformed session id reports the
    // usage error even when the project id is unknown too.
    crate::support::require_valid_session_id(session_id, "projects move")?;
    // Omitting the project id is the ungrouping form, whose write is
    // irreversible on both surfaces (neither can turn the explicit entry
    // back into "auto-grouped"). Every other irreversible write in this
    // crate already carries the `support::require_yes` confirmation gate
    // (`projects delete`, `sessions delete`), so the ungrouping move gets
    // the same gate: same exit class (usage, exit 2), same copy shape, and
    // before any store access — with the reason spelled out via
    // MOVE_UNGROUP_NOTE.
    if project_id.is_none() {
        require_yes(yes)?;
    }
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
    // Ungrouping is gated the way the GUI gates it. `MoveToProjectDialog.jsx`
    // renders its "remove from project" entry `aria-disabled` unless the
    // session RESOLVES to a project, and the store's `None` arm is not a
    // "clear" but an explicit, irreversible opt-out of auto-grouping — so
    // running it on a session that is already ungrouped pins a state the user
    // never chose and neither surface can undo. The CLI had no such gate.
    if project_id.is_none() && resolved_project_id(&store, &sessions, session_id).is_none() {
        return Err(CliError::failed(format!(
            "projects move: session {session_id} is not in a project, so there is nothing to \
             move it out of; the desktop app disables this action for the same reason\n\
             {MOVE_UNGROUP_NOTE}"
        )));
    }
    // The GUI's add_workspace_root lane needs the ACP pool to resolve the
    // session's workspace record; see the module header for the disclosed
    // headless deviation.
    let outcome = store
        .move_session_to_project(session_id, project_id, None)
        .map_err(|error| project_error("move", error))?;
    // Same no-placeholder rule as `project_item`: a degraded `json!({})` would
    // answer `project_id`/`added_root` with `null` under exit 0.
    let mut value = serde_json::to_value(&outcome)
        .map_err(|error| CliError::failed(format!("projects move: {error}")))?;
    value["session_id"] = serde_json::json!(session_id);
    let human = match project_id {
        Some(project_id) => format!("moved {session_id} into {project_id}"),
        None => format!("moved {session_id} out of its project"),
    };
    Ok(success(render(output, human, &value)))
}

// ─────────────────────────────── rebind ────────────────────────────────

/// Mirror of the GUI `rebind_workspace_root`'s `from` rule: empty, relative,
/// and filesystem-root paths are all rejected by one message. An empty prefix
/// matches every record under folded-key matching (a full rewrite), a root
/// `from` relocates everything, and a relative `from` diverges the storage
/// lanes (the projects lane absolutizes it through ancestor resolution while
/// the codex lane folds it raw) — none of these is a rebind.
fn validate_rebind_from(from: &Path) -> Result<(), CliError> {
    if from.as_os_str().is_empty() || !from.is_absolute() || from.parent().is_none() {
        return Err(CliError::usage(format!(
            "projects rebind: from must be an absolute, non-root directory, got {}",
            from.display()
        )));
    }
    Ok(())
}

/// Mirror of the GUI `rebind_workspace_root`'s `to` rule: a filesystem root
/// (Unix `/`, Windows drive root — no parent) would translate every binding
/// onto the root, a mass relocation rather than a rebind.
fn validate_rebind_to(to: &Path) -> Result<(), CliError> {
    if to.parent().is_none() {
        return Err(CliError::usage(
            "projects rebind: the destination cannot be a filesystem root (REBIND_TO_ROOT)",
        ));
    }
    Ok(())
}

/// Mirror of the GUI's `reject_nested_rebind_target` (equality is handled by
/// the caller first): a target inside the old directory deepens on every
/// rerun (/a/x → /a/x/new/x → …), breaking idempotency. The GUI compares
/// folded identity keys through `platform::os::path_identity_is_same_or_nested`,
/// which is `pub(crate)` to the app crate; the CLI compares components with
/// `Path::starts_with` — both operands are already in the entry-normalized
/// resolved display form, so the match is whole-component and the one
/// deviation is Windows case-only spellings, where the app's canonicalize at
/// validation time already removes the case difference in practice.
fn rebind_target_is_same_or_nested(to_display: &Path, from: &Path) -> bool {
    to_display.starts_with(from)
}

/// Mirror of `SessionStore::durable_session_record_is_absent` (the helper is
/// `pub(crate)` to the app crate, the layout is the store's own:
/// `<sessions root>/<id>.json`): only NotFound counts as absent, so a corrupt
/// record is never mistaken for an orphan and stays a retryable failure.
fn session_record_is_absent(session_id: &str) -> bool {
    let record = pinvou3_lib::platform::paths::sessions_root().join(format!("{session_id}.json"));
    matches!(
        std::fs::metadata(&record),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

/// Mirror of `rebind_workspace_root` (storage half, review #463/#464 lineage):
/// after a project directory physically moved, translate every binding under
/// the `from` prefix onto `to` — project roots, the codex lane
/// (`session-agents.json` index + code-session sidecars), plain-chat
/// `workspace-binding.json` sidecars (+ the legacy global table), and the
/// SavedSession metadata — through exactly the store methods the GUI command
/// calls, with the same write order (session lanes first, project roots
/// LAST, so an interrupted run still shows the old root and a rerun
/// converges; every step is idempotent).
///
/// Deviations, all disclosed: the desktop-process half (active-turn fence,
/// post-migration busy recheck, idle-gated runtime eviction) needs the app's
/// runtime pools and is skipped — the stderr note on every run says so; the
/// confirmation is `--yes`, required unconditionally, which subsumes the
/// GUI's confirm-existing escalation; and the metadata replay is driven by
/// the lanes' outcomes instead of a pre-rewrite snapshot (see the module
/// header). Per-session failures never abort the run: like the GUI, they are
/// reported (`failed_session_ids`) and a rerun converges them; only a store
/// call failing outright exits 1.
fn rebind(from: &Path, to: &Path, yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    // Argument-shape validation first (pure path checks, no store access —
    // the same order `move` uses for its session-id gate): a malformed
    // argument is reported even when --yes is also missing.
    validate_rebind_from(from)?;
    validate_rebind_to(to)?;
    // The rewrite is wholesale (old paths are gone from every store record
    // afterwards; retries only converge forward), so it carries the family's
    // confirmation gate before any store access — the same class as
    // `delete` and the ungrouping `move`.
    require_yes(yes)?;
    let store = open_store()?;
    let sessions = open_session_store()?;
    let agents = SessionAgentStore::load_or_empty();
    // Rebind-side critical section (`begin_rebind`, the same token the GUI
    // command takes): root-accepting project writers hold `rebind_fence` and
    // refuse to commit while this is held, which is what makes the
    // multi-store rewrite exclusive against them. Held until return.
    let _rebind_gate = store
        .begin_rebind()
        .map_err(|error| project_error("rebind", error))?;
    // `to` must exist, be a directory, and enters the lanes in its canonical
    // display form (the GUI's REBIND_TO_UNUSABLE).
    let to_display = validate_codex_project_workspace(to).map_err(|error| {
        CliError::failed(format!(
            "projects rebind: the destination is not a usable project directory \
             (REBIND_TO_UNUSABLE): {error:#}"
        ))
    })?;
    // Input-contract normalization (`rebind_source_display`): `from` is
    // resolved ONCE so all three storage lanes match in one domain — an
    // alias spelling (macOS /var/x vs /private/var/x) must not half-migrate.
    let from_display = rebind_source_display(from);
    if from_display == to_display {
        // Same short-circuit as the store lanes and the GUI: a rename onto
        // itself is a no-op success, not an error.
        return rebind_report(output, Vec::new(), Vec::new(), Vec::new());
    }
    if rebind_target_is_same_or_nested(&to_display, &from_display) {
        return Err(CliError::failed(
            "projects rebind: the destination cannot sit inside the original folder \
             (REBIND_TO_NESTED)",
        ));
    }
    // Root pre-flight (REBIND_ROOTS_CONFLICT): the roots commit LAST, so an
    // overlap conflict must be rejected before any session binding moves.
    store
        .plan_rebind_roots(&from_display, &to_display)
        .map_err(|error| {
            CliError::failed(format!(
                "projects rebind: rebinding would produce overlapping project roots \
                 (REBIND_ROOTS_CONFLICT): {error:#}"
            ))
        })?;
    // Codex lane: the store's own batch rewrites the helper index and the
    // authoritative code-session sidecars (off-index orphans included) and
    // reports both what moved and what is finally stale.
    let prefix_outcome = agents
        .rebind_workspace_prefix(&from_display, &to_display)
        .map_err(|error| project_error("rebind", error))?;
    // Plain-chat lane: the store's own batch moves the binding sidecars and
    // the in-memory cache, and translates the legacy global table BEFORE the
    // sidecars move; its failures and legacy resurrections reach the report
    // below.
    let plain_rebind = sessions
        .rebind_workspace_bindings(&from_display, &to_display)
        .map_err(|error| project_error("rebind", error))?;
    // Finally-stale sidecars of both lanes: no self-healing path remains in
    // this run, so they must be reported as failed for a rerun to converge
    // them via the on-disk prefix scan.
    let mut final_stale = prefix_outcome.sidecar_final_stale.clone();
    for session_id in &plain_rebind.failed_session_ids {
        if !final_stale.contains(session_id) {
            final_stale.push(session_id.clone());
        }
    }
    // SavedSession metadata replay over the union of what the two lanes
    // actually rewrote (the GUI's metadata_rebind_targets minus the
    // pre-rewrite snapshot — see the module header): every pre-existing
    // under-`from` binding either moved (rebound arm) or failed (failed
    // arm via final_stale).
    let mut metadata_targets = prefix_outcome.affected.clone();
    for entry in &plain_rebind.rebound {
        if !metadata_targets.iter().any(|(id, _)| id == &entry.0) {
            metadata_targets.push(entry.clone());
        }
    }
    // Only code-lane sessions consume workspace baselines.
    let code_rebound_ids: HashSet<String> = prefix_outcome
        .affected
        .iter()
        .map(|(session_id, _)| session_id.clone())
        .collect();
    let mut rebound_session_ids = Vec::new();
    let mut failed_session_ids = Vec::new();
    for (session_id, bound_path) in &metadata_targets {
        // The lanes hand back already-translated paths; the to-prefix arm of
        // rebind_target_path passes them through unchanged.
        let Some(new_path) =
            SessionAgentStore::rebind_target_path(bound_path, &from_display, &to_display)
        else {
            continue;
        };
        let finally_stale = final_stale.iter().any(|sid| sid == session_id);
        // An orphan (session JSON gone) has no metadata to write; a corrupt
        // JSON is NOT an orphan — set_workspace's load failure lands in
        // failed and is retryable.
        if session_record_is_absent(session_id) {
            if finally_stale {
                failed_session_ids.push(session_id.clone());
            } else {
                rebound_session_ids.push(session_id.clone());
            }
            continue;
        }
        match sessions.set_workspace(session_id, new_path.clone()) {
            Ok(()) => {
                // Indexed session whose sidecar failed both passes: the
                // binding moved but the authoritative sidecar still holds
                // the old path — honestly count it as failed.
                if finally_stale {
                    failed_session_ids.push(session_id.clone());
                } else {
                    rebound_session_ids.push(session_id.clone());
                }
            }
            Err(error) => {
                // Only the root cause is echoed (the chain embeds session
                // ids in store paths — CodeQL cleartext-logging); the id
                // reaches the user through the report's failed list.
                note!(
                    "[projects] rebind set_workspace failed: {}",
                    error.root_cause()
                );
                failed_session_ids.push(session_id.clone());
            }
        }
        // Baseline recapture, code-lane only, best-effort: the git
        // fingerprint is derivable again, a failure never blocks the rebind.
        if code_rebound_ids.contains(session_id)
            && let Err(error) =
                pinvou3_lib::features::codex_acp::workspace::capture_baseline(session_id, &new_path)
        {
            note!("[projects] rebind capture_baseline failed: {error:#}");
        }
    }
    // Store-side fence hits the outcome union never saw (a sidecar left
    // under `from` that neither lane's rebound set names): reported, never
    // silently dropped — otherwise a boot restore could resurrect the old
    // path while the run claims success.
    for session_id in &final_stale {
        if !metadata_targets.iter().any(|(id, _)| id == session_id)
            && !failed_session_ids.contains(session_id)
        {
            failed_session_ids.push(session_id.clone());
        }
    }
    // Legacy-table honesty: when the legacy global table survived the write,
    // every session it would resurrect at the next boot joins the failures,
    // independently of this run's rebound set (on a retry nothing is left to
    // rewrite, so the rebound set alone would claim a false full success).
    if plain_rebind.legacy_sync_failed {
        for session_id in &plain_rebind.legacy_resurrection_ids {
            if !failed_session_ids.contains(session_id) {
                failed_session_ids.push(session_id.clone());
            }
        }
    }
    // Project roots LAST: the overlap invariant was pre-flighted above and is
    // revalidated under the store's write lock here.
    let affected_project_ids = store
        .rebind_roots(&from_display, &to_display)
        .map_err(|error| match error {
            RebindRootsError::Overlap(context) => CliError::failed(format!(
                "projects rebind: rebinding would produce overlapping project roots \
                     (REBIND_ROOTS_CONFLICT): {context:#}"
            )),
            RebindRootsError::Other(context) => project_error("rebind", context),
        })?;
    rebind_report(
        output,
        rebound_session_ids,
        failed_session_ids,
        affected_project_ids,
    )
}

/// Report shape: the store-reachable subset of the GUI's
/// `RebindWorkspaceReport` (no `post_busy_session_ids` — that field is
/// produced by the desktop runtime pools this command does not see). A
/// partial run still exits 0 with the failure list visible in both
/// renderings, the GUI's report-and-continue semantics.
fn rebind_report(
    output: OutputMode,
    rebound_session_ids: Vec<String>,
    failed_session_ids: Vec<String>,
    affected_project_ids: Vec<String>,
) -> Result<CliOutcome, CliError> {
    // Scope disclosure, every run and both output modes: stderr only, so a
    // `--output json` consumer's stdout parse is unaffected.
    note!(
        "note: `projects rebind` migrates stored bindings only; a live desktop app's \
         active turns against the old directory are not reclaimed from here — close \
         the app or re-run the rebind from the desktop for that half"
    );
    let mut human = format!(
        "rebound {} session(s) ({} failed), updated roots of {} project(s)",
        rebound_session_ids.len(),
        failed_session_ids.len(),
        affected_project_ids.len(),
    );
    if !failed_session_ids.is_empty() {
        human.push_str("; failed sessions (a rerun converges them): ");
        human.push_str(&failed_session_ids.join(", "));
    }
    let value = serde_json::json!({
        "rebound_session_ids": rebound_session_ids,
        "failed_session_ids": failed_session_ids,
        "affected_project_ids": affected_project_ids,
    });
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

    /// Tier-2 containment must match on whole components, the way the
    /// frontend's hand-written boundary check does. A shallower root that is
    /// only a string prefix of the workspace ("/srv/app" vs "/srv/appdata")
    /// must NOT capture it, or the ungroup gate would believe a session is
    /// grouped when the GUI shows it ungrouped.
    #[test]
    fn tier_two_containment_matches_components_not_string_prefixes() {
        let workspace = PathBuf::from("/srv/appdata/repo");
        assert!(path_is_under_root(&workspace, Path::new("/srv/appdata")));
        assert!(path_is_under_root(
            &workspace,
            Path::new("/srv/appdata/repo")
        ));
        assert!(path_is_under_root(&workspace, Path::new("/srv/appdata/")));
        assert!(!path_is_under_root(&workspace, Path::new("/srv/app")));
        assert!(!path_is_under_root(
            &workspace,
            Path::new("/srv/appdata/repo/sub")
        ));
        // A blank root degenerates to "every absolute path" in the frontend's
        // string comparison; one hand-edited empty root must not swallow every
        // session here.
        assert!(!path_is_under_root(&workspace, Path::new("")));
    }
}
