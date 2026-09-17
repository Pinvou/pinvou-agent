//! Project-layer commands: cross-store composition and session existence
//! checks live in this layer; the `features::projects` core itself depends on
//! neither sessions nor codex_acp (dependency-direction constraint).
//!
//! Phase 0 exposes: list/create/update/delete/move. Directory rebinding
//! (`rebind_workspace_root`) belongs to Phase 4; its fences (active-turn
//! rejection, baseline recapture) need AcpPool write-path integration and did
//! not ship with this layer.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::features::codex_acp::{AcpPool, CodexWorkspaceKind, SessionAgentStore};
use crate::features::projects::{
    DeleteProjectReport, EnsureFolderOutcome, MoveSessionOutcome, Project, ProjectStore,
    SessionAssignments,
};
use crate::features::sessions::SessionStore;

use super::sessions::ensure_chat_session;

/// Project events only emit locally: the projects domain is desktop-only per
/// the bridge contract; the remote-control forwarding whitelist has no such
/// event, so forwarding would only be rejected by the relay and log a
/// rejection every time (review #447 finding 11: do not forward before a
/// consumer exists).
fn emit_project_event(app: &AppHandle, event: &str, action: &str) {
    let _ = app.emit(event, serde_json::json!({ "action": action }));
    // Desktop webview channel only. The projects domain is desktop-exclusive
    // (the Web bridge lacks the whole domain); not forwarded until the remote
    // side formally supports project lists — consistent with remote_control's
    // ruling for code-session events; forwarding an event outside the
    // RUST_FORWARDED_EVENTS whitelist gets rejected and logged.
}

/// Root availability (whether the directory is still on disk) — the frontend
/// renders "folder unavailable · rebind" from this, but a project is never
/// auto-deleted based on it.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectRootStatus {
    pub path: PathBuf,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectListItem {
    pub id: String,
    pub name: String,
    pub roots: Vec<ProjectRootStatus>,
    pub position: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// `Some("folder")` = project auto-materialized from a folder (frontend
    /// badge); None = manual.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// The project's remembered primary folder (§9.3 default cwd when
    /// creating a session from the project channel); None = not recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_primary_root: Option<PathBuf>,
    /// Number of explicitly assigned sessions; auto-grouped member counts are
    /// computed by the frontend's group resolution (Phase 1).
    pub assigned_session_count: usize,
}

impl ProjectListItem {
    fn from_project(project: &Project, assigned_session_count: usize) -> Self {
        Self {
            id: project.id.clone(),
            name: project.name.clone(),
            roots: project
                .roots
                .iter()
                .map(|path| ProjectRootStatus {
                    path: path.clone(),
                    available: path.is_dir(),
                })
                .collect(),
            position: project.position,
            created_at: project.created_at,
            updated_at: project.updated_at,
            origin: project.origin.clone(),
            last_primary_root: project.last_primary_root.clone(),
            assigned_session_count,
        }
    }
}

/// list_projects response: project list + the full assignment map (input for
/// the frontend's three-tier group resolution; null assignment = explicit
/// move-out) + the anti-materialization exclusion list (§3, canonical keys;
/// for the management panel to view/revoke).
#[derive(Debug, Clone, Serialize)]
pub struct ProjectListResponse {
    pub projects: Vec<ProjectListItem>,
    pub assignments: SessionAssignments,
    pub never_materialize_roots: Vec<String>,
}

/// Project list ordered by position, with each root's availability, explicit
/// member counts, and the assignment map.
#[tauri::command]
pub async fn list_projects(store: State<'_, ProjectStore>) -> Result<ProjectListResponse, String> {
    let assignments = store.assignments_snapshot();
    let projects = store
        .list()
        .iter()
        .map(|project| {
            ProjectListItem::from_project(project, store.assigned_session_ids(&project.id).len())
        })
        .collect();
    Ok(ProjectListResponse {
        projects,
        assignments,
        never_materialize_roots: store.never_materialize_roots(),
    })
}

/// Create a project. roots may be empty (pure-label project); non-empty
/// roots each pass absoluteness/overlap validation.
#[tauri::command]
pub async fn create_project(
    name: String,
    roots: Option<Vec<PathBuf>>,
    app: AppHandle,
    store: State<'_, ProjectStore>,
) -> Result<ProjectListItem, String> {
    let project = store
        .create_project(name, roots.unwrap_or_default())
        .map_err(|e| format!("create_project: {e:#}"))?;
    emit_project_event(&app, "projects:list_changed", "created");
    Ok(ProjectListItem::from_project(&project, 0))
}

/// Update a project: name and roots are optional patches; None keeps the
/// current value.
///
/// Member expulsion on root removal (§4): sessions under a removed root that
/// have no assignment entry yet are written as explicit move-outs (None) and
/// stay in Ungrouped — preventing tier-② grouping or ensure materialization
/// from immediately overturning the removal; existing entries (explicitly
/// assigned to this/another project, already moved out) are untouched.
/// Removal does not affect in-flight sessions (snapshot semantics §6/§9.5);
/// this only writes logical-layer assignments and never touches any session's
/// working-directory binding.
#[tauri::command]
pub async fn update_project(
    project_id: String,
    name: Option<String>,
    roots: Option<Vec<PathBuf>>,
    last_primary_root: Option<PathBuf>,
    app: AppHandle,
    store: State<'_, ProjectStore>,
    sessions: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
) -> Result<ProjectListItem, String> {
    let project = match roots {
        Some(roots) => replace_project_roots_and_expel(
            &store,
            &sessions,
            acp_pool.agents(),
            &project_id,
            name,
            roots,
        )?,
        None => store
            .update_project(&project_id, name, None)
            .map_err(|e| format!("update_project({project_id}): {e:#}"))?,
    };
    // Remembered primary folder (§9.2): explicit patch, must be a roots
    // member (validated by the store).
    let project = match last_primary_root {
        Some(root) => store
            .set_last_primary_root(&project_id, &root)
            .map_err(|e| format!("update_project({project_id}) primary root: {e:#}"))?,
        None => project,
    };
    let count = store.assigned_session_ids(&project_id).len();
    emit_project_event(&app, "projects:list_changed", "updated");
    Ok(ProjectListItem::from_project(&project, count))
}

/// `update_project`'s roots-replacement path (extracted for command-level
/// tests): the payload is first normalized to the store's canonical form and
/// removed is computed from the normalized result — comparing the raw invoke
/// payload directly would only key-fold the strings (see store.rs
/// `removed_roots`) while stored roots are canonicalized; the same directory
/// spelled differently (macOS /var→/private/var, symlinked home, autofs)
/// would misjudge a no-op edit as a removal and hard-expel every auto member
/// under that root (review #484 B3). Root replacement and member expulsion
/// complete under the same store lock in the same persist: on failure the
/// retry's removed recomputation still hits, so the expel is never
/// permanently skipped (review #484 B3).
pub(crate) fn replace_project_roots_and_expel(
    store: &ProjectStore,
    sessions: &SessionStore,
    agents: &SessionAgentStore,
    project_id: &str,
    name: Option<String>,
    roots: Vec<PathBuf>,
) -> Result<Project, String> {
    let normalized = ProjectStore::normalize_roots(&roots)
        .map_err(|e| format!("update_project({project_id}): {e:#}"))?;
    let previous_roots = store.get(project_id).map(|project| project.roots);
    let removed = previous_roots
        .map(|previous| crate::features::projects::removed_roots(&previous, &normalized))
        .unwrap_or_default();
    let mut expel_session_ids = Vec::new();
    for root in &removed {
        // Same enumeration as delete_project: sessions under the removed
        // roots in both binding stores; the store side skips ids that already
        // have assignment entries per tier-① semantics.
        for (session_id, _) in agents.sessions_under_workspace(root) {
            expel_session_ids.push(session_id);
        }
        for (session_id, _) in sessions.workspace_bindings_under(root) {
            expel_session_ids.push(session_id);
        }
    }
    expel_session_ids.sort();
    expel_session_ids.dedup();
    store
        .update_project_and_expel(project_id, name, normalized, &expel_session_ids)
        .map_err(|e| format!("update_project({project_id}): {e:#}"))
}

/// Delete a project: all members (explicitly assigned + auto-grouped) are
/// written as explicit move-outs, stay in Ungrouped, and do not revive with
/// the folder's next auto-materialization; sessions created later in that
/// folder auto-group as usual. Sessions themselves are never deleted; the
/// affected session ids are returned for the frontend to prompt.
#[tauri::command]
pub async fn delete_project(
    project_id: String,
    app: AppHandle,
    store: State<'_, ProjectStore>,
    sessions: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
) -> Result<DeleteProjectReport, String> {
    // Auto-grouped member enumeration: sessions under any of the project's
    // roots in both binding stores. Ids that already have assignment entries
    // are skipped by the store side per tier-① semantics (explicitly assigned
    // elsewhere / already moved out are not members of this project).
    let roots = store
        .get(&project_id)
        .map(|project| project.roots.clone())
        .unwrap_or_default();
    let mut expel_session_ids = Vec::new();
    for root in &roots {
        for (session_id, _) in acp_pool.agents().sessions_under_workspace(root) {
            expel_session_ids.push(session_id);
        }
        for (session_id, _) in sessions.workspace_bindings_under(root) {
            expel_session_ids.push(session_id);
        }
    }
    let report = store
        .delete_project(&project_id, &expel_session_ids)
        .map_err(|e| format!("delete_project({project_id}): {e:#}"))?;
    emit_project_event(&app, "projects:list_changed", "deleted");
    Ok(report)
}

/// Move a session's assignment (a pure filing operation, allowed for running
/// sessions too). `project_id = None` means explicit move-out; with
/// `add_workspace_root = true` the session's bound project directory is also
/// adopted as a target root — temporary sessions have no project directory,
/// so that combination errors. The physical layer (working-directory
/// binding) is never touched.
#[tauri::command]
pub async fn move_session_to_project(
    session_id: String,
    project_id: Option<String>,
    add_workspace_root: Option<bool>,
    app: AppHandle,
    store: State<'_, ProjectStore>,
    sessions: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
) -> Result<MoveSessionOutcome, String> {
    // Confirm the session exists first so the assignment table keeps no
    // invalid ids (same convention as set_session_pinned); scheduled-run
    // sessions are rejected with the same rule as the sibling commands so run
    // records never enter the assignment table.
    ensure_chat_session(&sessions, &session_id, "move_session_to_project")
        .map_err(|e| format!("move_session_to_project({session_id}): {e}"))?;
    let workspace_root = if add_workspace_root.unwrap_or(false) {
        if project_id.is_none() {
            return Err(
                "move_session_to_project: add_workspace_root requires project_id".to_string(),
            );
        }
        // Unified workspace detection (cross-mode fusion): code/ACP sessions
        // go through the agent record; plain bound sessions fall back to the
        // dual-root signal — execution root ≠ ledger root ⇒ bound, and the
        // execution root is the bound directory (#445 binding semantics).
        // Both absence modes — an agent record that exists but is not
        // project-shaped (e.g. temporary) and a missing record (Err) — fall
        // through to the same fallback, so Ok(Temporary) does not short-
        // circuit into an error (review #452 finding 4).
        let detected = match acp_pool.workspace_info(&session_id) {
            Ok(info) if info.workspace_kind == CodexWorkspaceKind::Project => {
                Some(PathBuf::from(info.workspace_path))
            }
            _ => sessions
                .session_roots(&session_id)
                .ok()
                // The bound flag is authoritative: never substitute a
                // ledger != execution path comparison (documented contract of
                // SessionRoots::bound; review #464 MINOR 7).
                .filter(|roots| roots.bound)
                .map(|roots| roots.execution),
        };
        match detected {
            Some(path) => Some(path),
            None => {
                return Err(format!(
                    "move_session_to_project({session_id}): session has no project folder to add"
                ));
            }
        }
    } else {
        None
    };
    let outcome = store
        .move_session_to_project(
            &session_id,
            project_id.as_deref(),
            workspace_root.as_deref(),
        )
        .map_err(|e| format!("move_session_to_project({session_id}): {e:#}"))?;
    emit_project_event(&app, "projects:list_changed", "moved");
    Ok(outcome)
}

/// Folder-project auto-materialization (Codex-client-style adoption): roots
/// are aggregated by the frontend from session-list workspaces (client-
/// driven, isomorphic to Codex `project/import` being initiated by the
/// desktop). Idempotent: covered roots are reused / conflicts reported per
/// root; the list-changed broadcast only fires when something was created.
#[tauri::command]
pub async fn ensure_folder_projects(
    roots: Vec<PathBuf>,
    app: AppHandle,
    store: State<'_, ProjectStore>,
) -> Result<Vec<EnsureFolderOutcome>, String> {
    let outcomes = store
        .ensure_folder_roots(&roots)
        .map_err(|e| format!("ensure_folder_projects: {e:#}"))?;
    if outcomes
        .iter()
        .any(|outcome| matches!(outcome, EnsureFolderOutcome::Created { .. }))
    {
        emit_project_event(&app, "projects:list_changed", "folder_ensured");
    }
    Ok(outcomes)
}

/// Anti-materialization exclusion list (§3): `never = true` means "never
/// auto-create a project for this folder again", `false` revokes. Visible,
/// revocable, idempotent; affects only future auto-materialization and leaves
/// existing projects untouched. Returns the updated exclusion list; the
/// panel/sidebar refresh via projects:list_changed.
#[tauri::command]
pub async fn projects_set_never_materialize(
    root: PathBuf,
    never: bool,
    app: AppHandle,
    store: State<'_, ProjectStore>,
) -> Result<Vec<String>, String> {
    let roots = store
        .set_never_materialize(&root, never)
        .map_err(|e| format!("projects_set_never_materialize: {e:#}"))?;
    emit_project_event(&app, "projects:list_changed", "never_materialize_changed");
    Ok(roots)
}

/// Result report of `align_session_to_project`. When applied=false, reason
/// distinguishes `no_project` (no assignment), `no_change` (keychain already
/// matches the project), and `write_skipped` (the binding store wrote
/// nothing: agent record gone / sidecar unreadable; disk unchanged).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AlignOutcome {
    pub session_id: String,
    /// Keychain snapshot after alignment (or the current one when unchanged):
    /// primary slot = session cwd.
    pub roots: Vec<PathBuf>,
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Align to project (§6/§9.7 explicit action): replace the session's
/// keychain snapshot with **all** of its assigned project's roots as they are
/// **at that moment** — primary slot = the session's own cwd (no door change,
/// §9.2), additional roots = project roots minus cwd, order preserved.
/// Project resolution uses the same rule as the frontend: explicit
/// assignment wins, tier-② multi-hits are adopted by the smallest position
/// (resolve_session_project).
///
/// Written to both stores (agent record / plain binding sidecar, the same
/// dual-store shift as rebind); live engines are pushed via Op::SyncSession
/// and it takes effect next turn. Fence: an active turn (ACP prompt / native
/// turn / scheduled round) rejects with the typed ALIGN_BUSY error;
/// temporary/scheduled sessions without a bound workspace get
/// ALIGN_NO_WORKSPACE. Idempotent: a keychain identical to the current one
/// yields applied=false, reason=no_change.
#[tauri::command]
pub async fn align_session_to_project(
    session_id: String,
    app: AppHandle,
    store: State<'_, ProjectStore>,
    sessions: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
    engines: State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<AlignOutcome, String> {
    // Active-turn fence (same rule as the rebind gate): a running
    // prompt/turn/scheduled round rejects.
    let busy = acp_pool.is_turn_active(&session_id).await
        || engines.is_turn_active(&session_id)
        || engines.is_scheduled_turn_running(&session_id);
    let outcome = align_session_keychain(
        &session_id,
        AlignStores {
            projects: &store,
            sessions: &sessions,
            agents: acp_pool.agents(),
        },
        busy,
    )?;
    if !outcome.applied {
        // Nothing was written (no_project / no_change / write_skipped): no
        // live-engine push, no event — runtime state must not diverge from
        // disk (review #484 B3).
        return Ok(outcome);
    }

    // Push the new root set to a live engine (takes effect next turn); a
    // push failure does not block — the next spawn/resume backfills the same
    // snapshot from the binding store.
    if let Some(engine) = engines.handle_for(&session_id).await {
        match sessions.load(&session_id) {
            Ok(saved) => {
                if let Err(error) = engine
                    .sync_session(session_id.clone(), saved.messages)
                    .await
                {
                    eprintln!(
                        "[projects] align_session_to_project: push roots to live engine failed: {error:#}"
                    );
                }
            }
            Err(error) => {
                eprintln!(
                    "[projects] align_session_to_project: load session for live push failed: {error:#}"
                );
            }
        }
    }
    // List surfacing refresh: workspace_roots ships with list_sessions / the
    // code-session list, and the chat lane's bridge layer refetches through
    // the existing session:list_changed subscription (the code lane is
    // refreshed by the frontend's align call site itself via
    // refreshSessions).
    super::sessions::emit_session_event(&app, "session:list_changed", &session_id, "aligned");
    Ok(outcome)
}

/// The three stores `align_session_to_project` depends on (extracted so
/// command-level tests can bypass Tauri State/AppHandle).
pub(crate) struct AlignStores<'a> {
    pub projects: &'a ProjectStore,
    pub sessions: &'a SessionStore,
    pub agents: &'a SessionAgentStore,
}

/// The align command's decision-and-persist core: the command wrapper owns
/// active-turn probing (`busy`), the live-engine push, and event emission;
/// this only reads stores, writes both binding stores, and reports `applied`
/// honestly.
///
/// Difference from rebind (review #484: align has no post-write busy
/// recheck): rebind's recheck targets the window where a turn starts between
/// the entry fence and the multi-file migration and executes against the old
/// directory; align's write is a single atomic record/sidecar rewrite and
/// never moves the session's own cwd (the keychain only swaps additional
/// roots, §9.2 no door change) — a turn started inside the fence window
/// still executes against the same cwd, the new root set takes effect next
/// turn via SyncSession, and a retry has no value, so there is no post-write
/// recheck.
pub(crate) fn align_session_keychain(
    session_id: &str,
    stores: AlignStores<'_>,
    busy: bool,
) -> Result<AlignOutcome, String> {
    let session_roots = stores
        .sessions
        .session_roots(session_id)
        .map_err(|e| format!("align_session_to_project: {e:#}"))?;
    if !session_roots.bound {
        return Err(format!(
            "ALIGN_NO_WORKSPACE: 临时会话没有绑定工作区，无法对齐到项目 (session has no bound workspace)"
        ));
    }
    let cwd = session_roots.execution;

    // Current snapshot: agent record (code/ACP) or plain binding sidecar.
    let agent_bound = stores.agents.get(session_id).workspace_path.is_some();
    let current = if agent_bound {
        stores.agents.session_workspace_roots(session_id)
    } else {
        stores.sessions.session_workspace_roots(session_id)
    };
    if !agent_bound
        && stores
            .sessions
            .session_workspace_binding(session_id)
            .is_none()
    {
        return Err(format!(
            "ALIGN_NO_WORKSPACE: 临时会话没有绑定工作区，无法对齐到项目 (session has no bound workspace)"
        ));
    }

    let Some(project) = stores.projects.resolve_session_project(session_id, &cwd) else {
        return Ok(AlignOutcome {
            session_id: session_id.to_string(),
            roots: current,
            applied: false,
            reason: Some("no_project".to_string()),
        });
    };
    let next = ProjectStore::keychain_for_workspace(&cwd, &project.roots);
    let same = current.len() == next.len() && current.iter().zip(next.iter()).all(|(a, b)| a == b);
    if same {
        return Ok(AlignOutcome {
            session_id: session_id.to_string(),
            roots: next,
            applied: false,
            reason: Some("no_change".to_string()),
        });
    }

    if busy {
        return Err(
            "ALIGN_BUSY: 会话有活动回合，对齐被拒绝，请空闲后重试 (active turn in progress)"
                .to_string(),
        );
    }

    // Dual-store write: agent record (native code sessions also rewrite the
    // sidecar) / plain sidecar. Ok(false) = the store wrote nothing (agent
    // record gone / sidecar unreadable): must not claim applied, and the
    // command layer skips the live-engine push and event accordingly
    // (review #484 B3).
    let written = if agent_bound {
        stores
            .agents
            .set_session_workspace_roots(session_id, next.clone())
            .map_err(|e| format!("align_session_to_project: {e:#}"))?
    } else {
        stores
            .sessions
            .set_session_workspace_roots(session_id, next.clone())
            .map_err(|e| format!("align_session_to_project: {e:#}"))?
    };
    if !written {
        return Ok(AlignOutcome {
            session_id: session_id.to_string(),
            roots: current,
            applied: false,
            reason: Some("write_skipped".to_string()),
        });
    }
    Ok(AlignOutcome {
        session_id: session_id.to_string(),
        roots: next,
        applied: true,
        reason: None,
    })
}

/// Result report of rebind_workspace_root: per-session results + affected
/// projects. Rebinding is idempotent, and failed entries can be retried
/// directly (the candidate snapshot includes retry entries that are "already
/// under `to` but whose metadata is not synced"; already-succeeded parts
/// rerun as no-ops).
#[derive(Debug, Clone, Serialize)]
pub struct RebindWorkspaceReport {
    pub rebound_session_ids: Vec<String>,
    pub failed_session_ids: Vec<String>,
    pub affected_project_ids: Vec<String>,
    /// Sessions found by the post-migration recheck to have entered an
    /// active turn: their bindings have shifted, but the turn may still be
    /// executing against the old directory; the frontend uses this to prompt
    /// for one retry when idle if necessary.
    #[serde(default)]
    pub post_busy_session_ids: Vec<String>,
}

/// `from` input validation: the empty string and filesystem roots (Unix `/`,
/// Windows drive roots — neither has a parent) are rejected. An empty prefix
/// matches every record under folded-key matching — `from=""` is a wholesale
/// rewrite and `from="/"` with confirm-existing is a wholesale relocation;
/// neither is rebind semantics (review #463 minor).
fn validate_rebind_from(from: &Path) -> Result<(), String> {
    if from.as_os_str().is_empty() || from.parent().is_none() {
        return Err(format!(
            "rebind_workspace_root: from 必须是非根目录的路径，收到 {}",
            from.display()
        ));
    }
    Ok(())
}

/// `to` must not sit inside `from` (equality is handled by the caller
/// first): rebinding shifts by prefix, and a target inside the old directory
/// deepens on every rerun (/a/x → /a/x/new/x → …), breaking idempotence
/// (review #451 finding 6). Compared on folded keys, so case/separator
/// differences cannot escape.
fn reject_nested_rebind_target(from: &Path, to_key: &Path) -> Result<(), String> {
    let from_canon = std::fs::canonicalize(from).unwrap_or_else(|_| from.to_path_buf());
    let from_key = crate::platform::os::filesystem_path_identity_key(
        &crate::platform::os::platform_compat_path(&from_canon.to_string_lossy()).to_string_lossy(),
    );
    let to_key_str = crate::platform::os::filesystem_path_identity_key(&to_key.to_string_lossy());
    let from_trim = from_key.trim_end_matches('/');
    let to_trim = to_key_str.trim_end_matches('/');
    if !from_trim.is_empty()
        && (to_trim == from_trim || to_trim.starts_with(&format!("{from_trim}/")))
    {
        return Err(
            "REBIND_NESTED_TARGET: 新目录不能位于旧目录内部（会造成递归加深） (nested target rejected)"
                .to_string(),
        );
    }
    Ok(())
}

/// The old directory still on disk = not a broken-link scenario, so explicit
/// strong confirmation is required. Errors express their type with a stable
/// marker prefix; the frontend escalates to a strong warning based on it and
/// never matches human copy (finding 11).
fn require_confirm_existing(from: &Path, confirm_existing: Option<bool>) -> Result<(), String> {
    if from.is_dir() && !confirm_existing.unwrap_or(false) {
        return Err(
            "REBIND_OLD_ROOT_EXISTS: 原目录仍存在，需在界面确认后重试 (original folder still exists)"
                .to_string(),
        );
    }
    Ok(())
}

/// Directory rebind (broken-link repair): after a project folder is
/// physically moved/deleted, shift every binding under the `from` prefix —
/// project roots, session workspaces (three places: index/sidecar/metadata),
/// and assignment derivation — wholesale to `to`. Unlike "move assignment"
/// this is a physical-layer write, hence the fences:
/// - `to` must exist and be a directory (validated by
///   validate_codex_project_workspace);
/// - while the old directory `from` still exists, `confirm_existing = true`
///   is required (the frontend has strongly confirmed);
/// - if any affected session has an active turn (ACP prompt, native Engine
///   turn, or scheduled round), the whole operation is rejected;
/// - after the shift a project root must not nest within its own project,
///   otherwise everything errors and rolls back.
/// Historical paths inside transcripts are not rewritten; workspace baselines
/// are recaptured per session (failures only log; baselines can be
/// re-derived).
#[tauri::command]
pub async fn rebind_workspace_root(
    from: PathBuf,
    to: PathBuf,
    confirm_existing: Option<bool>,
    app: AppHandle,
    store: State<'_, ProjectStore>,
    sessions: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
    engines: State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<RebindWorkspaceReport, String> {
    let to_key = crate::features::codex_acp::validate_codex_project_workspace(&to)
        .map_err(|e| format!("rebind_workspace_root: 目标目录不可用: {e:#}"))?;
    if from == to_key {
        return Ok(RebindWorkspaceReport {
            rebound_session_ids: Vec::new(),
            failed_session_ids: Vec::new(),
            affected_project_ids: Vec::new(),
            post_busy_session_ids: Vec::new(),
        });
    }
    validate_rebind_from(&from)?;
    reject_nested_rebind_target(&from, &to_key)?;
    require_confirm_existing(&from, confirm_existing)?;

    // Snapshot of the affected set (shared by the active-turn fence and the
    // metadata replay), must be taken before the rewrite (review #463 M1):
    // the rewrite only returns what this run rewrote — sessions shifted by a
    // previous run whose set_workspace failed no longer match `from`, and
    // without a snapshot they could never be retried. The candidate set spans
    // both binding stores: agent records (code/ACP, including orphan sidecars
    // outside the index, M6) and plain-session binding sidecars (unify:
    // grouping follows binding, so plain bound sessions are equally in rebind
    // scope); retry candidates "already under `to` but metadata not synced"
    // are also included.
    let mut affected = acp_pool.agents().sessions_under_workspace(&from);
    affected.extend(sessions.workspace_bindings_under(&from));
    for (session_id, path) in acp_pool
        .agents()
        .sessions_under_workspace(&to_key)
        .into_iter()
        .chain(sessions.workspace_bindings_under(&to_key))
    {
        if affected.iter().any(|(sid, _)| *sid == session_id) {
            continue;
        }
        // A session whose metadata matches its binding is normal; one whose
        // metadata cannot be read (orphan/corrupt) is also treated as a
        // candidate — the metadata loop classifies it.
        let needs_metadata_sync = match sessions.load(&session_id) {
            Ok(session) => session.metadata.workspace != path,
            Err(_) => true,
        };
        if needs_metadata_sync {
            affected.push((session_id, path));
        }
    }

    // Active-turn fence: if any affected session is running a
    // prompt/turn/scheduled round, reject and retry when idle. Scheduled
    // rounds are only recorded in scheduled_running_sessions; not counting
    // them would miss in-flight rounds in the spawn→submit window (same rule
    // as the rewind gate, M5).
    let mut busy_ids = Vec::new();
    for (session_id, _) in &affected {
        if acp_pool.is_turn_active(session_id).await
            || engines.is_turn_active(session_id)
            || engines.is_scheduled_turn_running(session_id)
        {
            busy_ids.push(session_id.clone());
        }
    }
    if !busy_ids.is_empty() {
        return Err(format!(
            "rebind_workspace_root: 会话正在运行，稍后重试: {}",
            busy_ids.join(", ")
        ));
    }

    // Order: project roots → session bindings (index + sidecar, both binding
    // stores) → metadata → baseline. Every step is idempotent, and a failed
    // retry only completes the unfinished parts. The metadata loop is driven
    // by the snapshot above, computing the target path per candidate (shifted
    // under the from prefix; retry candidates already under `to` stay
    // as-is).
    let affected_project_ids = store
        .rebind_roots(&from, &to_key)
        .map_err(|e| format!("rebind_workspace_root: {e:#}"))?;
    let code_rebound = acp_pool
        .agents()
        .rebind_workspace_prefix(&from, &to_key)
        .map_err(|e| format!("rebind_workspace_root: {e:#}"))?;
    let code_rebound_ids: std::collections::HashSet<&str> = code_rebound
        .iter()
        .map(|(session_id, _)| session_id.as_str())
        .collect();
    let plain_rebind = sessions
        .rebind_workspace_bindings(&from, &to_key)
        .map_err(|e| format!("rebind_workspace_root: {e:#}"))?;
    // Entries whose sidecar write failed still point at `from`: the metadata
    // loop skips them (otherwise binding/metadata diverge), and the failure
    // list folds into the report (review #464 MAJOR 4).
    let plain_failed: std::collections::HashSet<&str> = plain_rebind
        .failed_session_ids
        .iter()
        .map(String::as_str)
        .collect();
    let mut rebound_session_ids = Vec::new();
    let mut failed_session_ids = plain_rebind.failed_session_ids.clone();
    for (session_id, bound_path) in &affected {
        if plain_failed.contains(session_id.as_str()) {
            continue;
        }
        let Some(new_path) = SessionAgentStore::rebind_target_path(bound_path, &from, &to_key)
        else {
            continue;
        };
        // Orphans (session JSON no longer exists) have no metadata to write
        // and count as succeeded; a corrupt JSON is not an orphan —
        // set_workspace's load parse failure goes into failed and is
        // retryable (review #463 minor: the orphan classification only
        // accepts NotFound, not every load error).
        if sessions.durable_session_record_is_absent(session_id) {
            rebound_session_ids.push(session_id.clone());
            continue;
        }
        match sessions.set_workspace(session_id, new_path.clone()) {
            Ok(()) => rebound_session_ids.push(session_id.clone()),
            Err(error) => {
                eprintln!("[projects] rebind set_workspace({session_id}) failed: {error:#}");
                failed_session_ids.push(session_id.clone());
            }
        }
        // Baseline recapture is only for code sessions: plain sessions do
        // not consume workspace baselines, and no code-lane sidecar is
        // created for them. Runs under spawn_blocking: a non-git directory
        // synchronously walks tens of thousands of entries and must not run
        // serially on the async command thread (same idiom as codex.rs).
        // Best-effort; failures only log.
        if code_rebound_ids.contains(session_id.as_str()) {
            let baseline_session_id = session_id.clone();
            let baseline_root = new_path.clone();
            match tauri::async_runtime::spawn_blocking(move || {
                crate::features::codex_acp::workspace::capture_baseline(
                    &baseline_session_id,
                    &baseline_root,
                )
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    eprintln!("[projects] rebind capture_baseline({session_id}) failed: {error:#}")
                }
                Err(error) => {
                    eprintln!(
                        "[projects] rebind capture_baseline({session_id}) task failed: {error}"
                    )
                }
            }
        }
    }

    emit_project_event(&app, "projects:list_changed", "rebound");
    for session_id in &rebound_session_ids {
        super::sessions::emit_session_event(
            &app,
            "session:list_changed",
            session_id,
            "workspace_rebound",
        );
    }
    // Post-migration busy recheck (finding 5): the entry fence and the
    // multi-file migration are not a critical section — a turn may start
    // during the migration and execute against the old directory. The
    // bindings have shifted; this only reports honestly, and the frontend
    // prompts for one retry when idle if necessary. Scheduled rounds follow
    // the same rule as the entry fence (M5).
    let mut post_busy_session_ids = Vec::new();
    for (session_id, _) in &affected {
        if acp_pool.is_turn_active(session_id).await
            || engines.is_turn_active(session_id)
            || engines.is_scheduled_turn_running(session_id)
        {
            post_busy_session_ids.push(session_id.clone());
        }
    }
    Ok(RebindWorkspaceReport {
        rebound_session_ids,
        failed_session_ids,
        affected_project_ids,
        post_busy_session_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wire-shape lock: the bridge's applySnapshot consumes the snapshot by
    /// the projects/assignments keys, and a serde rename would silently drop
    /// every snapshot (review finding 41); never_materialize_roots (§3
    /// exclusion list) is in the snapshot too.
    #[test]
    fn project_list_response_wire_keys_are_stable() {
        let value = serde_json::to_value(ProjectListResponse {
            projects: Vec::new(),
            assignments: SessionAssignments::default(),
            never_materialize_roots: Vec::new(),
        })
        .expect("serialize ProjectListResponse");
        let object = value.as_object().expect("response serializes as an object");
        assert!(object.contains_key("projects"));
        assert!(object.contains_key("assignments"));
        assert!(object.contains_key("never_materialize_roots"));
    }

    #[test]
    fn rebind_from_rejects_empty_and_root() {
        assert!(
            validate_rebind_from(Path::new("")).is_err(),
            "empty string is a wholesale rewrite"
        );
        // Platform roots (Unix `/`, Windows drive roots) have no parent and
        // must be rejected.
        let root = std::env::temp_dir()
            .canonicalize()
            .ok()
            .and_then(|p| p.ancestors().last().map(|a| a.to_path_buf()))
            .expect("temp dir must have a root ancestor");
        assert!(
            validate_rebind_from(&root).is_err(),
            "filesystem root {root:?} with confirm-existing is a wholesale relocation"
        );
        let normal = std::env::temp_dir().join("pinvou3-rebind-from-check");
        assert!(validate_rebind_from(&normal).is_ok());
    }

    #[test]
    fn rebind_rejects_target_nested_inside_from() {
        let from = Path::new("/a/b");
        assert!(reject_nested_rebind_target(from, Path::new("/a/b/c")).is_err());
        assert!(reject_nested_rebind_target(from, Path::new("/a/b")).is_err());
        assert!(
            reject_nested_rebind_target(from, Path::new("/a/bc")).is_ok(),
            "directory boundary: a sibling prefix must not false-hit"
        );
        assert!(reject_nested_rebind_target(from, Path::new("/a")).is_ok());
    }

    #[test]
    fn rebind_requires_confirm_when_old_root_exists() {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-rebind-confirm-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let error = require_confirm_existing(&dir, None).unwrap_err();
        assert!(
            error.starts_with("REBIND_OLD_ROOT_EXISTS"),
            "stable marker prefix: the frontend escalates to a strong warning based on it, never matching human copy"
        );
        assert!(require_confirm_existing(&dir, Some(false)).is_err());
        assert!(require_confirm_existing(&dir, Some(true)).is_ok());
        let missing = dir.join("gone");
        assert!(
            require_confirm_existing(&missing, None).is_ok(),
            "broken-link scenario (directory gone from disk) needs no confirmation"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // ── Command-level tests for update_project / align_session_to_project
    //    (review #484 B3) ──

    use crate::platform::paths::tests::ENV_LOCK;

    fn unique_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "pinvou3-projects-cmd-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    /// Same shape as sessions/tests.rs's isolated_store: point PINVOU3_HOME
    /// at a dedicated directory inside the process-wide env lock, then boot.
    /// The home directory is deliberately not deleted (the directory must
    /// stay on disk after the guard drops, matching the lifetime of the
    /// session records under test).
    fn isolated_session_store() -> (SessionStore, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let home = unique_dir("home");
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };
        let store = SessionStore::boot_with_scheduled_root(home.join("scheduled")).expect("boot");
        (store, guard)
    }

    fn project_store_in(dir: &Path) -> ProjectStore {
        ProjectStore::from_paths(dir.join("projects.json"))
    }

    fn create_plain_bound_session(
        sessions: &SessionStore,
        bound: &Path,
        roots: Vec<PathBuf>,
    ) -> String {
        let session = sessions
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create session");
        let id = session.metadata.id;
        sessions
            .bind_session_workspace_with_roots(&id, bound.to_path_buf(), roots)
            .expect("bind workspace");
        id
    }

    /// align's active-turn fence: rejects when busy and writes nothing.
    #[test]
    fn align_busy_fence_rejects_without_writing() {
        let (sessions, _g) = isolated_session_store();
        let project_root = unique_dir("busy-proj");
        let extra = unique_dir("busy-extra");
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        let store = project_store_in(&unique_dir("busy-store"));
        store
            .create_project("p".to_string(), vec![project_root.clone(), extra.clone()])
            .expect("create project");
        // The current snapshot has only the primary root, which differs from
        // the project keychain [primary, extra] → not no_change.
        let session_id =
            create_plain_bound_session(&sessions, &project_root, vec![project_root.clone()]);
        let agents = SessionAgentStore::for_test(unique_dir("busy-agents").join("agents.json"));

        let error = align_session_keychain(
            &session_id,
            AlignStores {
                projects: &store,
                sessions: &sessions,
                agents: &agents,
            },
            true,
        )
        .expect_err("busy fence must reject");
        assert!(error.starts_with("ALIGN_BUSY"), "typed error: {error}");
        assert_eq!(
            sessions.session_workspace_roots(&session_id),
            vec![project_root.clone()],
            "the on-disk snapshot is unchanged after the fence rejects"
        );
        let _ = std::fs::remove_dir_all(&project_root);
        let _ = std::fs::remove_dir_all(&extra);
    }

    /// Plain binding channel: align writes the sidecar, preserving the
    /// binding path and bound_at.
    #[test]
    fn align_writes_plain_binding_sidecar() {
        let (sessions, _g) = isolated_session_store();
        let project_root = unique_dir("plain-proj");
        let extra = unique_dir("plain-extra");
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        let store = project_store_in(&unique_dir("plain-store"));
        store
            .create_project("p".to_string(), vec![project_root.clone(), extra.clone()])
            .expect("create project");
        let session_id =
            create_plain_bound_session(&sessions, &project_root, vec![project_root.clone()]);
        let agents = SessionAgentStore::for_test(unique_dir("plain-agents").join("agents.json"));

        let outcome = align_session_keychain(
            &session_id,
            AlignStores {
                projects: &store,
                sessions: &sessions,
                agents: &agents,
            },
            false,
        )
        .expect("align");
        assert!(outcome.applied);
        assert_eq!(outcome.reason, None);
        assert_eq!(
            outcome.roots,
            vec![project_root.clone(), extra.clone()],
            "primary slot = session cwd, additional roots follow the project"
        );
        assert_eq!(
            sessions.session_workspace_roots(&session_id),
            outcome.roots,
            "sidecar snapshot replaced"
        );
        assert_eq!(
            sessions.session_workspace_binding(&session_id).as_deref(),
            Some(project_root.as_path()),
            "the binding path is not rewritten by alignment (no door change)"
        );
        // Read the sidecar directly to double-check: bound_at is not lost.
        let sidecar_path = crate::platform::paths::sessions_root()
            .join(&session_id)
            .join("workspace-binding.json");
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&sidecar_path).expect("sidecar"))
                .expect("parse sidecar");
        assert!(raw.get("bound_at").is_some(), "bound_at preserved");
        let _ = std::fs::remove_dir_all(&project_root);
        let _ = std::fs::remove_dir_all(&extra);
    }

    /// Agent (native code session) channel: align writes both the index
    /// record and the authoritative sidecar.
    #[test]
    fn align_writes_agent_record_and_code_session_sidecar() {
        let (sessions, _g) = isolated_session_store();
        let cwd = unique_dir("agent-cwd");
        let extra = unique_dir("agent-extra");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        let agents_dir = unique_dir("agent-store");
        std::fs::create_dir_all(&agents_dir).unwrap();
        let agents = SessionAgentStore::for_test(agents_dir.join("session-agents.json"));
        agents
            .bind_code_native_session(
                "code-1",
                CodexWorkspaceKind::Project,
                Some(cwd.clone()),
                vec![cwd.clone()],
            )
            .expect("bind code session");
        // session_roots goes through the production-style resolver: native
        // code sessions' project binding.
        let agents_for_resolver = agents.clone();
        *sessions.execution_root_resolver.write() =
            Some(std::sync::Arc::new(move |session_id: &str| {
                agents_for_resolver.code_project_workspace(session_id)
            }));
        let store = project_store_in(&unique_dir("agent-projects"));
        store
            .create_project("p".to_string(), vec![cwd.clone(), extra.clone()])
            .expect("create project");

        let outcome = align_session_keychain(
            "code-1",
            AlignStores {
                projects: &store,
                sessions: &sessions,
                agents: &agents,
            },
            false,
        )
        .expect("align");
        assert!(outcome.applied);
        assert_eq!(outcome.roots, vec![cwd.clone(), extra.clone()]);
        assert_eq!(
            agents.session_workspace_roots("code-1"),
            vec![cwd.clone(), extra.clone()],
            "index record replaced"
        );
        // The authoritative sidecar is rewritten in sync (native code
        // sessions recover from it after the auxiliary index is lost; a
        // missed write would revive the old value).
        let sidecar_path = agents_dir
            .join("sessions")
            .join("code-1")
            .join("code-session.json");
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&sidecar_path).expect("code-session sidecar"))
                .expect("parse sidecar");
        let roots = raw["workspace_roots"].as_array().expect("roots array");
        assert_eq!(roots.len(), 2, "sidecar snapshot dual-written: {raw}");
        let _ = std::fs::remove_dir_all(&cwd);
        let _ = std::fs::remove_dir_all(&extra);
        let _ = std::fs::remove_dir_all(&agents_dir);
    }

    /// Ok(false) is no longer swallowed: when the binding store writes
    /// nothing (sidecar unreadable), report applied=false /
    /// reason=write_skipped, and the command layer skips the live-engine push
    /// and the session:list_changed event accordingly (review #484 B3-2).
    #[test]
    fn align_reports_write_skipped_when_store_declines() {
        let (sessions, _g) = isolated_session_store();
        let project_root = unique_dir("skip-proj");
        let extra = unique_dir("skip-extra");
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        let store = project_store_in(&unique_dir("skip-store"));
        store
            .create_project("p".to_string(), vec![project_root.clone(), extra.clone()])
            .expect("create project");
        let session_id =
            create_plain_bound_session(&sessions, &project_root, vec![project_root.clone()]);
        // The binding path is already in the in-memory cache; corrupting the
        // on-disk sidecar makes the write side's read fail with Ok(false),
        // while the entry's binding-existence check still hits the cache.
        let sidecar_path = crate::platform::paths::sessions_root()
            .join(&session_id)
            .join("workspace-binding.json");
        std::fs::write(&sidecar_path, "not json").expect("corrupt sidecar");
        let agents = SessionAgentStore::for_test(unique_dir("skip-agents").join("agents.json"));

        let outcome = align_session_keychain(
            &session_id,
            AlignStores {
                projects: &store,
                sessions: &sessions,
                agents: &agents,
            },
            false,
        )
        .expect("align");
        assert!(
            !outcome.applied,
            "must not claim applied when nothing was written"
        );
        assert_eq!(outcome.reason.as_deref(), Some("write_skipped"));
        let _ = std::fs::remove_dir_all(&project_root);
        let _ = std::fs::remove_dir_all(&extra);
    }

    /// AlignOutcome wire shape: reason only appears when applied=false
    /// (skip_serializing_if); no reason key when applied.
    #[test]
    fn align_outcome_wire_shape_is_stable() {
        let applied = serde_json::to_value(AlignOutcome {
            session_id: "s1".to_string(),
            roots: vec![PathBuf::from("/a")],
            applied: true,
            reason: None,
        })
        .expect("serialize");
        let object = applied.as_object().expect("object");
        for key in ["session_id", "roots", "applied"] {
            assert!(object.contains_key(key), "missing wire key {key}");
        }
        assert!(
            !object.contains_key("reason"),
            "reason only appears on failure"
        );
        let skipped = serde_json::to_value(AlignOutcome {
            session_id: "s1".to_string(),
            roots: Vec::new(),
            applied: false,
            reason: Some("no_change".to_string()),
        })
        .expect("serialize");
        assert_eq!(skipped["reason"], "no_change");
    }

    /// B3-1: a no-op root edit whose payload spelling differs from the
    /// stored form (symlink spelling, same shape as macOS
    /// /var→/private/var) must not be misjudged as a removal — removed is
    /// computed from the normalized roots, and members are not hard-expelled.
    /// Directory symlinks on Windows need privileges; unix/macOS cover this
    /// (no cfg: the architecture guard bans platform conditional compilation
    /// outside the adapter layer).
    #[test]
    fn update_noop_roots_edit_via_symlink_does_not_expel() {
        if std::env::consts::OS == "windows" {
            return;
        }
        let (sessions, _g) = isolated_session_store();
        let base = unique_dir("symlink-base");
        let real = base.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = base.join("link");
        let status = std::process::Command::new("ln")
            .arg("-s")
            .arg(&real)
            .arg(&link)
            .status()
            .expect("spawn ln");
        assert!(status.success(), "ln -s must succeed on unix-likes");
        let canonical = real.canonicalize().unwrap();

        let store = project_store_in(&base.join("store"));
        let project = store
            .create_project("p".to_string(), vec![real.clone()])
            .expect("create project");
        assert_eq!(project.roots, vec![canonical.clone()]);
        let session_id = create_plain_bound_session(&sessions, &canonical, vec![canonical.clone()]);
        let agents = SessionAgentStore::for_test(base.join("agents").join("agents.json"));

        // The payload uses the symlink spelling: after normalization it lands
        // back on the stored form and removed is empty.
        let updated = replace_project_roots_and_expel(
            &store,
            &sessions,
            &agents,
            &project.id,
            None,
            vec![link.clone()],
        )
        .expect("no-op roots edit");
        assert_eq!(updated.roots, vec![canonical.clone()]);
        assert_eq!(
            store.assignment_of(&session_id),
            None,
            "a no-op edit must not write move-out entries (a misjudged removal would hard-expel auto members)"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// B3-2/B3-3: on root removal, auto members from both binding stores are
    /// written as explicit move-outs in the same transaction; a retry
    /// (removed already empty) is idempotent and overturns nothing.
    #[test]
    fn update_expels_members_from_both_binding_stores_atomically() {
        let (sessions, _g) = isolated_session_store();
        let root_a = unique_dir("expel-a");
        let root_b = unique_dir("expel-b");
        std::fs::create_dir_all(&root_a).unwrap();
        std::fs::create_dir_all(&root_b).unwrap();
        let agents_dir = unique_dir("expel-agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        let agents = SessionAgentStore::for_test(agents_dir.join("session-agents.json"));
        agents
            .bind_code_native_session(
                "code-under-a",
                CodexWorkspaceKind::Project,
                Some(root_a.clone()),
                vec![root_a.clone()],
            )
            .expect("bind code session");
        let plain_id = create_plain_bound_session(&sessions, &root_a, vec![root_a.clone()]);

        let store = project_store_in(&unique_dir("expel-store"));
        let project = store
            .create_project("p".to_string(), vec![root_a.clone(), root_b.clone()])
            .expect("create project");

        let updated = replace_project_roots_and_expel(
            &store,
            &sessions,
            &agents,
            &project.id,
            None,
            vec![root_b.clone()],
        )
        .expect("replace roots");
        assert_eq!(updated.roots, vec![root_b.canonicalize().unwrap()]);
        for id in [&plain_id, "code-under-a"] {
            assert_eq!(
                store.assignment_of(id),
                Some(None),
                "{id} should be written as an explicit move-out"
            );
        }
        // Retry: removed recomputes as empty, no expel in the transaction,
        // existing move-out entries untouched.
        let again = replace_project_roots_and_expel(
            &store,
            &sessions,
            &agents,
            &project.id,
            None,
            vec![root_b.clone()],
        )
        .expect("retry");
        assert_eq!(again.roots, vec![root_b.canonicalize().unwrap()]);
        assert_eq!(store.assignment_of(&plain_id), Some(None));
        let _ = std::fs::remove_dir_all(&root_a);
        let _ = std::fs::remove_dir_all(&root_b);
        let _ = std::fs::remove_dir_all(&agents_dir);
    }
}
