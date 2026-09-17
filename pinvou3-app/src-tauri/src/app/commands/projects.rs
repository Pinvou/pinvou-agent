//! Project-layer commands: cross-store composition and session-existence
//! checks live here; `features::projects` itself does not depend on
//! sessions/codex_acp (dependency-direction constraint).
//!
//! Exposed: list/create/update/delete/move, plus the directory rebind
//! (`rebind_workspace_root`) with its fences (active-turn rejection, busy
//! recheck, idle-gated runtime eviction, baseline recapture).

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::features::codex_acp::{AcpPool, CodexWorkspaceKind, SessionAgentStore};
use crate::features::projects::{
    DeleteProjectReport, MoveSessionOutcome, Project, ProjectStore, SessionAssignments,
};
use crate::features::sessions::SessionStore;

use super::sessions::ensure_chat_session;

/// Project events are only emitted locally: the projects domain is
/// desktop-only per the bridge contract and is not forwarded until
/// remote-control officially supports project lists (review #447 finding 11:
/// do not forward before a consumer exists). The actual rejection point is
/// the `policy.events` gate (publish_event_inner in remote_control/manager),
/// logged via `forward_local_event`; `RUST_FORWARDED_EVENTS` only dedupes
/// echoes from Frontend sources and is unrelated to this decision (review
/// #463 minor: a previous comment attributed this incorrectly).
fn emit_project_event(app: &AppHandle, event: &str, action: &str) {
    let _ = app.emit(event, serde_json::json!({ "action": action }));
}

/// root 的可用性(目录是否仍在磁盘上)——前端据此渲染"文件夹不可用·重新绑定",
/// 但绝不据此自动删项目。
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
    /// 显式归属的会话数;自动归组的成员数由前端分组解析计算(Phase 1)。
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
            assigned_session_count,
        }
    }
}

/// list_projects 响应:项目列表 + 全量归属映射(前端三层分组解析的原料,
/// null 归属 = 显式移出)。
#[derive(Debug, Clone, Serialize)]
pub struct ProjectListResponse {
    pub projects: Vec<ProjectListItem>,
    pub assignments: SessionAssignments,
}

/// 项目列表,按 position 有序,含每个 root 的可用性、显式成员数与归属映射。
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
    })
}

/// 创建项目。roots 可为空(纯标签项目);非空时逐个过绝对性/重叠校验。
#[tauri::command]
pub async fn create_project(
    name: String,
    roots: Option<Vec<PathBuf>>,
    app: AppHandle,
    store: State<'_, ProjectStore>,
) -> Result<ProjectListItem, String> {
    // Root-accepting writer: refuses to commit into an in-flight rebind, which
    // would otherwise re-add a `from`-prefixed root after the rebind's snapshot
    // (review #464 round-6 finding 6). The fence is released when the write is
    // done, not when the command returns.
    let _fence = store.rebind_fence()?;
    let project = store
        .create_project(name, roots.unwrap_or_default())
        .map_err(|e| format!("create_project: {e:#}"))?;
    drop(_fence);
    emit_project_event(&app, "projects:list_changed", "created");
    Ok(ProjectListItem::from_project(&project, 0))
}

/// 更新项目:名称与 roots 均为可选补丁,None 保持不变。
#[tauri::command]
pub async fn update_project(
    project_id: String,
    name: Option<String>,
    roots: Option<Vec<PathBuf>>,
    app: AppHandle,
    store: State<'_, ProjectStore>,
) -> Result<ProjectListItem, String> {
    // Gated only when roots can change: a rename cannot re-add a from-prefixed
    // root, and fencing it would reject a harmless rename for the whole rebind.
    let _fence = if roots.is_some() {
        Some(store.rebind_fence()?)
    } else {
        None
    };
    let project = store
        .update_project(&project_id, name, roots)
        .map_err(|e| format!("update_project({project_id}): {e:#}"))?;
    drop(_fence);
    let count = store.assigned_session_ids(&project_id).len();
    emit_project_event(&app, "projects:list_changed", "updated");
    Ok(ProjectListItem::from_project(&project, count))
}

/// 删除项目:会话只被解绑(回落自动/隐式分组),永不删除;返回受影响会话
/// id 供前端提示。
#[tauri::command]
pub async fn delete_project(
    project_id: String,
    app: AppHandle,
    store: State<'_, ProjectStore>,
) -> Result<DeleteProjectReport, String> {
    let report = store
        .delete_project(&project_id)
        .map_err(|e| format!("delete_project({project_id}): {e:#}"))?;
    emit_project_event(&app, "projects:list_changed", "deleted");
    Ok(report)
}

/// 移动会话归属(纯归档操作,运行中的会话同样允许)。
/// `project_id = None` 表示显式移出;`add_workspace_root = true` 时把该会话
/// 绑定的项目目录顺带加为目标 root——临时会话没有项目目录,该组合报错。
/// 物理层(工作目录绑定)永不触碰。
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
    // 先确认会话真实存在:session_kind 对未知 id 不触盘、直接返回 Chat,
    // 单靠它会把无效 id 写进归属表,因此像 set_session_pinned 一样补一次
    // 真实 load。scheduled-run 的拒绝是归属域自身的产品决策(归属表只收
    // chat 会话)——兄弟元数据命令(delete/rename/pin)并不拒绝
    // scheduled-run,此处不与它们同口径。
    ensure_chat_session(&sessions, &session_id, "move_session_to_project")
        .map_err(|e| format!("move_session_to_project({session_id}): {e}"))?;
    sessions
        .load(&session_id)
        .map_err(|e| format!("move_session_to_project({session_id}): {e:#}"))?;
    let workspace_root = if add_workspace_root.unwrap_or(false) {
        if project_id.is_none() {
            return Err(
                "move_session_to_project: add_workspace_root requires project_id".to_string(),
            );
        }
        // Unified workspace probing (cross-mode fusion): code/ACP sessions
        // resolve via the agent record; plain bound sessions fall back to the
        // dual-root signal — execution root != ledger root ⇒ bound, with the
        // execution root as the bound directory (#445 binding semantics).
        // Both absence modes — an agent record that exists but is not
        // project-shaped (e.g. temporary) and a missing record (Err) — fall
        // through to the same fallback, never letting Ok(Temporary)
        // short-circuit into an error (review #452 finding 4).
        let detected = match acp_pool.workspace_info(&session_id) {
            Ok(info) if info.workspace_kind == CodexWorkspaceKind::Project => {
                Some(PathBuf::from(info.workspace_path))
            }
            _ => sessions
                .session_roots(&session_id)
                .ok()
                // The bound flag is the authoritative verdict: never
                // substitute a ledger != execution path comparison for it
                // (documented contract of SessionRoots::bound; review #464
                // MINOR 7).
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
    // Root-accepting writer under the same fence: `add_workspace_root` adds a
    // directory to a project and the store re-validates overlap, so committing
    // mid-rebind can both re-add a `from`-prefixed root and bind a session
    // under `from` after the rebind's candidate snapshot (review #464 round-6
    // finding 6).
    let _fence = store.rebind_fence()?;
    let outcome = store
        .move_session_to_project(
            &session_id,
            project_id.as_deref(),
            workspace_root.as_deref(),
        )
        .map_err(|e| format!("move_session_to_project({session_id}): {e:#}"))?;
    drop(_fence);
    emit_project_event(&app, "projects:list_changed", "moved");
    Ok(outcome)
}

/// Result report of rebind_workspace_root: per-session outcomes + affected
/// projects. Rebind is idempotent and failures can be retried directly (the
/// candidate snapshot includes retry items "already under to but metadata
/// not synced"; the succeeded parts rerun as no-ops).
#[derive(Debug, Clone, Serialize)]
pub struct RebindWorkspaceReport {
    pub rebound_session_ids: Vec<String>,
    pub failed_session_ids: Vec<String>,
    pub affected_project_ids: Vec<String>,
    /// Sessions found in an active turn by the post-migration recheck or
    /// skipped by the idle-gated eviction: their bindings moved, but a turn
    /// may still execute against the old directory. The frontend suggests
    /// one retry when idle — the retry feeds these sessions back as explicit
    /// eviction candidates, so the remedy is real (review #463 M2).
    #[serde(default)]
    pub post_busy_session_ids: Vec<String>,
}

/// `from` validation: empty, relative, and filesystem-root paths are
/// rejected. An empty prefix matches every record under folded-key matching
/// (a full rewrite), a root `from` with confirm-existing relocates
/// everything, and a relative `from` diverges the storage lanes (the
/// projects lane absolutizes it through ancestor resolution while the codex
/// lane folds it raw) — none of these is a rebind (review #463 minor).
fn validate_rebind_from(from: &Path) -> Result<(), String> {
    if from.as_os_str().is_empty() || !from.is_absolute() || from.parent().is_none() {
        return Err(format!(
            "rebind_workspace_root: from 必须是绝对的非根目录路径，收到 {}",
            from.display()
        ));
    }
    Ok(())
}

/// `to` = filesystem root (Unix `/`, Windows drive root — both have no
/// parent) is rejected: translating every binding onto the filesystem root
/// is a mass relocation, not a rebind (review #463 minor). Existence and
/// directory-ness are checked separately by
/// `validate_codex_project_workspace`.
fn validate_rebind_to(to: &Path) -> Result<(), String> {
    if to.parent().is_none() {
        return Err(format!(
            "rebind_workspace_root: to 不能是文件系统根，收到 {}",
            to.display()
        ));
    }
    Ok(())
}

/// `to` must not sit inside `from` (equality is handled by the caller
/// first): rebind translates by prefix, and a target inside the old
/// directory deepens on every rerun (/a/x → /a/x/new/x → …), breaking
/// idempotency (review #451 finding 6). Compared on folded keys so case /
/// separator differences cannot evade it.
fn reject_nested_rebind_target(from: &Path, to_key: &Path) -> Result<(), String> {
    // No second resolution here (review #464 round-6 nit follow-up): `from`
    // reaches this point already in display form — `rebind_workspace_root`
    // normalizes it through `rebind_source_display` before validating, and
    // that is the same `root_display` chain. Re-resolving it was redundant,
    // and on a path that does not exist it resolved through the drive root
    // (`\\?\E:\a\b`), which broke both the lexical callers and the pattern
    // the check is written against. The symlinked-ancestor case is covered
    // where it belongs: the store's `covered_workspace_skip_survives_symlinked_ancestor`.
    let from_key = crate::platform::os::filesystem_path_identity_key(&from.to_string_lossy());
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

/// Old directory still on disk = not a broken-link scenario, so an explicit
/// strong confirmation is required. The error carries a stable marker
/// prefix; the frontend escalates to a strong warning by prefix and never
/// matches human copy (finding 11).
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
/// physically moved/deleted, every binding under the `from` prefix — project
/// roots, session workspaces (index/sidecar/metadata), derived assignments —
/// is translated onto `to`. Unlike "move assignment" this is a physical-layer
/// write, so it carries fences:
/// - `to` must exist and be a directory (validated by
///   validate_codex_project_workspace) and must not be the filesystem root;
/// - `from` must be an absolute, non-root path;
/// - while the old directory `from` still exists, `confirm_existing = true`
///   is required (the frontend has strong-confirmed);
/// - if any affected session has an active turn (ACP prompt, native Engine
///   turn, or scheduled round) the whole rebind is rejected;
/// - after translation, project roots must not overlap other projects, or
///   the whole rebind fails and rolls back.
/// Historical paths in transcripts are not rewritten; the workspace baseline
/// is recaptured per session (failures are only logged — the baseline is
/// derivable again).
///
/// Storage-form invariant (review #463 m1/B1): `from` is normalized once at
/// this entry via `rebind_source_display` — project roots are stored in
/// `root_display` (canonical) form and session bindings were canonicalized
/// at bind time, while the three storage lanes match in different domains
/// (the projects store resolves symlinked ancestors, the codex/session lanes
/// fold lexically). Normalizing at the entry pins every lane to one resolved
/// form, so an alias caller (macOS `/var/x` vs the stored `/private/var/x`)
/// cannot half-migrate. `rebind_workspace_root` is a public command; this
/// normalization is part of its input contract.
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
    // Concurrency fence (Minor 10): a rebind writes across three stores, so
    // concurrent calls are serialized. The token is held until the command
    // returns; Drop clears it.
    let _rebind_gate = store.begin_rebind()?;
    validate_rebind_from(&from)?;
    validate_rebind_to(&to)?;
    let to_key = crate::features::codex_acp::validate_codex_project_workspace(&to)
        .map_err(|e| format!("rebind_workspace_root: 目标目录不可用: {e:#}"))?;
    // Normalize `from` once for all three storage lanes (review #463 B1, see
    // the docblock). Resolved through the deepest existing ancestor, so a
    // vanished directory behind a symlinked ancestor (macOS /var) still
    // resolves into the stored key domain.
    let from = crate::features::projects::rebind_source_display(&from);
    if from == to_key {
        return Ok(RebindWorkspaceReport {
            rebound_session_ids: Vec::new(),
            failed_session_ids: Vec::new(),
            affected_project_ids: Vec::new(),
            post_busy_session_ids: Vec::new(),
        });
    }
    reject_nested_rebind_target(&from, &to_key)?;
    require_confirm_existing(&from, confirm_existing)?;

    // Snapshot of the affected set (shared by the active-turn fence and the
    // metadata replay), taken before any rewrite (review #463 M1): the
    // rewrite returns only the set this run rewrote; sessions translated by a
    // previous run whose set_workspace failed no longer match `from`, so
    // without a snapshot they can never be retried.
    // The candidate set spans both binding stores: agent records (code/ACP,
    // including off-index orphan sidecars, M6) and plain-session binding
    // sidecars (unify: grouping follows binding, so plain bound sessions are
    // also in rebind scope); retry candidates "already under to but with
    // metadata not yet synced" are included as well.
    let mut affected = acp_pool.agents().sessions_under_workspace(&from);
    let plain_affected = sessions
        .workspace_bindings_under(&from)
        .map_err(|e| format!("rebind_workspace_root: scan workspace bindings: {e:#}"))?;
    affected.extend(plain_affected);
    // Cross-lane dedup (review #464 round-6 finding 7): the two stores are
    // independent, so nothing structurally prevents one session id from
    // appearing in both lanes — and it would then be processed twice with
    // independently translated paths, surfacing duplicate ids in
    // rebound_session_ids/failed_session_ids. The `to`-lane loop below already
    // models this dedup; the `from` lane needs it too. First occurrence wins,
    // and the codex lane is scanned first: for a code session its agent record
    // is the authoritative binding, and the metadata loop's baseline recapture
    // is derived from that same set.
    let mut seen_affected: std::collections::HashSet<String> = std::collections::HashSet::new();
    affected.retain(|(session_id, _)| seen_affected.insert(session_id.clone()));
    // Post-busy sessions of a previous run land here on retry (review #463
    // M2): their metadata was already synced in run 1, so they are absent
    // from `affected` — without feeding them back as explicit eviction
    // candidates, the documented "retry once when idle" remedy would be a
    // no-op (they would never re-enter rebound_session_ids and never be
    // evicted). A known, accepted coarseness: healthy sessions created
    // directly under `to` also land here; evicting their idle runtime is a
    // harmless lazy-respawn (the same thing the idle reaper does routinely).
    let mut retry_evict_candidates: Vec<String> = Vec::new();
    for (session_id, path) in acp_pool
        .agents()
        .sessions_under_workspace(&to_key)
        .into_iter()
        .chain(
            sessions
                .workspace_bindings_under(&to_key)
                .map_err(|e| format!("rebind_workspace_root: scan workspace bindings: {e:#}"))?,
        )
    {
        if affected.iter().any(|(sid, _)| *sid == session_id) {
            continue;
        }
        // A session whose metadata matches its binding is healthy; one whose
        // metadata cannot be read (orphan/corrupt) is treated as a candidate
        // too — the metadata loop classifies it.
        let needs_metadata_sync = match sessions.load(&session_id) {
            Ok(session) => session.metadata.workspace != path,
            Err(_) => true,
        };
        if needs_metadata_sync {
            affected.push((session_id, path));
        } else {
            retry_evict_candidates.push(session_id);
        }
    }

    // Active-turn fence: if any affected session is running a prompt/turn/
    // scheduled round, reject and let the user retry when idle. Scheduled
    // rounds are only recorded in scheduled_running_sessions; not counting
    // them would miss an in-flight round in the spawn→submit window (same
    // semantics as the rewind gate, M5).
    // Known trade-off (review #463 Minor 9, on record): the busy check reads
    // the runtime's busy/configuring flags; a flag stuck set (process died
    // without resetting) keeps rejecting until restart. On the ACP side
    // is_turn_active folds in configuring, so the config-sync window is also
    // covered by the rejection. There is deliberately no stale escape hatch,
    // to avoid reclaiming a session with an in-flight turn by mistake.
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
        // Typed marker (Minor 7): a busy rejection is the fence's normal
        // high-frequency path; the frontend maps it to i18n copy by stable
        // prefix, and only session ids follow the marker (same convention as
        // REBIND_OLD_ROOT_EXISTS).
        return Err(format!("REBIND_SESSIONS_BUSY: {}", busy_ids.join(", ")));
    }

    // Order: project roots → session bindings (index + sidecar, both binding
    // stores) → metadata → baseline. Each step is idempotent; a failed retry
    // only fills in the unfinished parts. The metadata loop is driven by the
    // snapshot above and computes the target path per candidate (translate
    // under the from prefix; retry candidates already under to stay as-is).
    let affected_project_ids = store
        .rebind_roots(&from, &to_key)
        .map_err(|e| format!("rebind_workspace_root: {e:#}"))?;
    // Finally-stale sidecar list (Major 2): an orphan rewrite failure, or an
    // indexed session whose rewrite + retry passes both failed — no
    // self-healing path remains in this run (backfill only fills missing
    // sidecars, and boot restore skips sidecars while the index is intact).
    // Not reporting them would let a restore with a damaged index resurrect
    // old directories and silently undo the rebind. Stale sidecars still sit
    // under the from prefix; once counted as failed, a user rerun converges
    // them via the on-disk prefix scan.
    let prefix_outcome = acp_pool
        .agents()
        .rebind_workspace_prefix(&from, &to_key)
        .map_err(|e| format!("rebind_workspace_root: {e:#}"))?;
    // Baseline re-capture applies to code sessions only (the loop below gates
    // on this): plain bound sessions do not consume workspace baselines, so do
    // not create code-lane-only sidecars for them (review #464).
    let code_rebound_ids: std::collections::HashSet<&str> = prefix_outcome
        .affected
        .iter()
        .map(|(session_id, _)| session_id.as_str())
        .collect();
    let plain_rebind = sessions
        .rebind_workspace_bindings(&from, &to_key)
        .map_err(|e| format!("rebind_workspace_root: {e:#}"))?;
    // Entries whose sidecar writes failed still have bindings pointing at
    // from: the metadata loop skips them (otherwise bindings and metadata
    // would diverge), and the failure list is merged into the report
    // (review #464 MAJOR 4).
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
        // An orphan (session JSON already gone) has no metadata to write; a
        // corrupt JSON is NOT an orphan — set_workspace's load parse failure
        // lands in failed and is retryable (review #463 minor: the orphan
        // classification accepts only NotFound, not any load error).
        if sessions.durable_session_record_is_absent(session_id) {
            if prefix_outcome
                .sidecar_final_stale
                .iter()
                .any(|sid| sid == session_id)
            {
                // Orphan sidecar persist failed: a restart would resurrect
                // the old directory, so report it as failed — a rerun retries
                // the same sidecar (m2).
                failed_session_ids.push(session_id.clone());
            } else {
                rebound_session_ids.push(session_id.clone());
            }
            continue;
        }
        match sessions.set_workspace(session_id, new_path.clone()) {
            Ok(()) => {
                // Indexed session whose sidecar failed both passes: the
                // binding moved but the authoritative sidecar still holds the
                // old path, so honestly count it as failed to trigger a user
                // rerun (Major 2).
                if prefix_outcome
                    .sidecar_final_stale
                    .iter()
                    .any(|sid| sid == session_id)
                {
                    failed_session_ids.push(session_id.clone());
                } else {
                    rebound_session_ids.push(session_id.clone());
                }
            }
            Err(error) => {
                // CodeQL cleartext-logging (review #463 round 7): the error
                // chain embeds the session id (sessions/<id>.json paths), so
                // only the root cause — which never carries paths or ids —
                // is logged; the id reaches the user through the report's
                // failed list instead.
                eprintln!(
                    "[projects] rebind set_workspace failed: {}",
                    error.root_cause()
                );
                failed_session_ids.push(session_id.clone());
            }
        }
        // Baseline recapture is gated to code sessions (unify): plain bound
        // sessions do not consume workspace baselines, so no code-lane
        // sidecar is created for them. Best-effort, the git fingerprint is
        // derivable again, and a failure does not block the rebind. Runs on
        // spawn_blocking: a non-git directory synchronously walks tens of
        // thousands of entries and must not run serially on the async
        // command thread (same idiom as session creation in codex.rs).
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
                    // Same CodeQL constraint as set_workspace above: the chain
                    // embeds sessions/<id>/…json.tmp paths; log the root cause
                    // only.
                    eprintln!(
                        "[projects] rebind capture_baseline failed: {}",
                        error.root_cause()
                    )
                }
                Err(error) => {
                    eprintln!("[projects] rebind capture_baseline task failed: {error}")
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
    // multi-file migration are not mutually exclusive, so a turn may have
    // started — against the old directory — during the migration. Bindings
    // are already moved; report honestly and let the frontend suggest one
    // retry when idle. Scheduled rounds share the entry-fence semantics
    // (M5).
    let mut post_busy_session_ids = Vec::new();
    for (session_id, _) in &affected {
        if acp_pool.is_turn_active(session_id).await
            || engines.is_turn_active(session_id)
            || engines.is_scheduled_turn_running(session_id)
        {
            post_busy_session_ids.push(session_id.clone());
        }
    }
    // Idle-gated runtime reclaim (review #463 M1/M2 + eviction-tail TOCTOU):
    // rebind only translates stored bindings — resident processes still hold
    // the cwd captured at spawn (ACP get_or_spawn's reuse branch does not
    // compare workspaces; the native engine's PreparedRuntimeModel rebuild
    // key excludes the workspace), so the next turn would keep executing in
    // the vanished folder while the UI promises the new one. Both pools
    // reclaim through their idle-aware primitives: the recheck and the
    // removal are atomic, so a turn that starts after the recheck above is
    // NOT killed — the session is reported as post-busy instead. A
    // successful reclaim also resets the per-session shell manager, which
    // pins its cwd at construction (M1). Candidates are this run's rebound
    // sessions plus the fed-back retry candidates (M2).
    let post_busy: std::collections::HashSet<String> =
        post_busy_session_ids.iter().cloned().collect();
    for session_id in collect_eviction_candidates(&rebound_session_ids, &retry_evict_candidates) {
        if post_busy.contains(&session_id) {
            continue;
        }
        let acp_idle = acp_pool.evict_if_idle_for_rebind(&session_id).await;
        let engine_idle = engines.evict_if_idle_for_rebind(&session_id).await;
        if !acp_idle || !engine_idle {
            // A turn started between the recheck and the eviction and the
            // pools refused to kill it. Surface the session so the user can
            // retry once it is idle again.
            if !post_busy_session_ids.contains(&session_id) {
                post_busy_session_ids.push(session_id);
            }
        }
    }
    // The plain sidecars and metadata moved, but if the legacy global table
    // could not be synced, the next boot migration would re-bind the old
    // paths over the fresh sidecars — the report must not claim success
    // (review #464 round-5 blocker 1).
    //
    // The failure list is driven by the sessions the surviving table would
    // actually resurrect, NOT by this run's rewrite log: on a retry the
    // bindings already sit under `to`, nothing is rewritten, `rebound` is
    // empty — and a `rebound`-driven merge reported full success while the
    // stale table was still on disk (review #464 round-6 blocking 1). A
    // retry therefore keeps naming the same sessions until the legacy write
    // succeeds or the file disappears, which is the retry loop's only exit.
    if plain_rebind.legacy_sync_failed {
        for session_id in &plain_rebind.legacy_resurrection_ids {
            if !failed_session_ids.contains(session_id) {
                failed_session_ids.push(session_id.clone());
            }
        }
    }
    Ok(RebindWorkspaceReport {
        rebound_session_ids,
        failed_session_ids,
        affected_project_ids,
        post_busy_session_ids,
    })
}

/// Eviction candidates for the rebind tail: sessions rebound in this run
/// plus `to`-lane sessions whose metadata is already synced (post-busy
/// sessions from a previous run — review #463 M2), deduplicated, order
/// preserved.
fn collect_eviction_candidates(
    rebound_session_ids: &[String],
    retry_evict_candidates: &[String],
) -> Vec<String> {
    let mut candidates = rebound_session_ids.to_vec();
    for session_id in retry_evict_candidates {
        if !candidates.contains(session_id) {
            candidates.push(session_id.clone());
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wire-shape lock: the bridge's applySnapshot consumes the snapshot by
    /// the projects/assignments keys, so a serde rename would silently drop
    /// every snapshot (review finding 41).
    #[test]
    fn project_list_response_wire_keys_are_stable() {
        let value = serde_json::to_value(ProjectListResponse {
            projects: Vec::new(),
            assignments: SessionAssignments::default(),
        })
        .expect("serialize ProjectListResponse");
        let object = value.as_object().expect("response serializes as an object");
        assert!(object.contains_key("projects"));
        assert!(object.contains_key("assignments"));
    }

    #[test]
    fn rebind_from_rejects_empty_and_root() {
        assert!(
            validate_rebind_from(Path::new("")).is_err(),
            "空串是全量重写"
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
            "文件系统根 {root:?} 配合 confirm-existing 是全量重安置"
        );
        let normal = std::env::temp_dir().join("pinvou3-rebind-from-check");
        assert!(validate_rebind_from(&normal).is_ok());
    }

    #[test]
    fn rebind_from_rejects_relative_paths() {
        // review #463 minor: a relative `from` diverges the storage lanes
        // (the projects lane absolutizes it through ancestor resolution, the
        // codex lane folds it raw), so the entry rejects it outright.
        assert!(validate_rebind_from(Path::new("relative/dir")).is_err());
        assert!(validate_rebind_from(Path::new("./also-relative")).is_err());
    }

    #[test]
    fn rebind_to_rejects_filesystem_root() {
        // review #463 minor: rebinding onto the filesystem root is a mass
        // relocation, not a rebind.
        let root = std::env::temp_dir()
            .canonicalize()
            .ok()
            .and_then(|p| p.ancestors().last().map(|a| a.to_path_buf()))
            .expect("temp dir must have a root ancestor");
        assert!(validate_rebind_to(&root).is_err());
        let normal = std::env::temp_dir().join("pinvou3-rebind-to-check");
        assert!(validate_rebind_to(&normal).is_ok());
    }

    #[test]
    fn eviction_candidates_merge_rebound_and_retry_without_duplicates() {
        // review #463 M2: the retry (post-busy) population is fed back as
        // explicit eviction candidates alongside this run's rebound sessions,
        // deduplicated, rebound first.
        let rebound = vec!["s1".to_string(), "s2".to_string()];
        let retry = vec!["s2".to_string(), "s3".to_string()];
        assert_eq!(
            collect_eviction_candidates(&rebound, &retry),
            vec!["s1".to_string(), "s2".to_string(), "s3".to_string()]
        );
        assert!(collect_eviction_candidates(&[], &[]).is_empty());
        assert_eq!(
            collect_eviction_candidates(&[], &["s9".to_string()]),
            vec!["s9".to_string()],
            "a pure retry run (nothing rebound) still evicts the fed-back candidates"
        );
    }

    #[test]
    fn rebind_rejects_target_nested_inside_from() {
        let from = Path::new("/a/b");
        assert!(reject_nested_rebind_target(from, Path::new("/a/b/c")).is_err());
        assert!(reject_nested_rebind_target(from, Path::new("/a/b")).is_err());
        assert!(
            reject_nested_rebind_target(from, Path::new("/a/bc")).is_ok(),
            "目录边界:sibling 前缀不得误命中"
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
            "稳定标记前缀:前端据此升级强警告,不匹配人类文案"
        );
        assert!(require_confirm_existing(&dir, Some(false)).is_err());
        assert!(require_confirm_existing(&dir, Some(true)).is_ok());
        let missing = dir.join("gone");
        assert!(
            require_confirm_existing(&missing, None).is_ok(),
            "断链场景(目录已不在盘上)无需确认"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
