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
    DeleteProjectReport, EnsureFolderOutcome, MoveSessionOutcome, Project, ProjectStore,
    RebindRootsError, SessionAssignments, removed_roots,
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

/// 项目 root 的 wire 形态（仅路径）。曾带有 `available`（root 是否仍在磁盘上，
/// 供"文件夹不可用·重新绑定"渲染），但该 UI 从未落地、前端也从未读取该字段，
/// 连带省去列表路径的逐个 is_dir() stat。
#[derive(Debug, Clone, Serialize)]
pub struct ProjectRootStatus {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectListItem {
    pub id: String,
    pub name: String,
    pub roots: Vec<ProjectRootStatus>,
    pub position: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// `Some("folder")` = 按文件夹自动物化的项目;None = 手工创建。仅作数据
    /// 溯源,不参与分组判定与删除语义。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// 项目记住的主文件夹(§9.3:从项目入口新建会话时的默认 cwd);None = 未记录。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_primary_root: Option<PathBuf>,
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
                .map(|path| ProjectRootStatus { path: path.clone() })
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

/// list_projects 响应:项目列表 + 全量归属映射(前端三层分组解析的原料,
/// null 归属 = 显式移出)。
#[derive(Debug, Clone, Serialize)]
pub struct ProjectListResponse {
    pub projects: Vec<ProjectListItem>,
    pub assignments: SessionAssignments,
    /// 反物化排除表(§3,canonical 键;管理面板据此展示与撤销)。
    pub never_materialize_roots: Vec<String>,
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
        never_materialize_roots: store.never_materialize_roots(),
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
    last_primary_root: Option<PathBuf>,
    app: AppHandle,
    store: State<'_, ProjectStore>,
    sessions: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
) -> Result<ProjectListItem, String> {
    // Gated only when roots can change: a rename cannot re-add a from-prefixed
    // root, and fencing it would reject a harmless rename for the whole rebind.
    let _fence = if roots.is_some() {
        Some(store.rebind_fence()?)
    } else {
        None
    };
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
    drop(_fence);
    // 记住的主文件夹(§9.2):显式补丁,必须是 roots 成员(由 store 校验)。
    let project = match last_primary_root {
        Some(root) => match store.set_last_primary_root(&project_id, &root) {
            Ok(updated) => updated,
            Err(e) => {
                // The roots replacement (when present) already persisted: the
                // frontend must observe it even though the command fails, so
                // the event goes out before the error does (review #484
                // MINOR — a half-committed write must not leave a stale
                // sidebar).
                emit_project_event(&app, "projects:list_changed", "updated");
                return Err(format!("update_project({project_id}) primary root: {e}"));
            }
        },
        None => project,
    };
    let count = store.assigned_session_ids(&project_id).len();
    emit_project_event(&app, "projects:list_changed", "updated");
    Ok(ProjectListItem::from_project(&project, count))
}

/// `update_project` 的 roots 替换路径(抽出来供命令级测试):
/// 先把握荷归一化到 store 的 canonical 形态,再用归一化结果算 removed——
/// 直接比较 invoke 原始载荷只会做键折叠(见 store.rs `removed_roots`),而存储
/// 的 roots 是 canonicalize 过的;同一个目录换个拼法(macOS /var→/private/var、
/// symlink 化的 home、autofs)会把「空操作编辑」误判为移除,把该根下所有自动
/// 成员硬移出(评审 #484 B3)。root 替换与成员移出在同一把 store 锁、同一次
/// persist 内完成:失败后重试重算的 removed 仍然命中,移出不会被永久跳过。
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
        .map(|previous| removed_roots(&previous, &normalized))
        .unwrap_or_default();
    let mut expel_session_ids = Vec::new();
    for root in &removed {
        // 与 delete_project 同一套枚举:两个绑定 store 里位于被移除根下的会话;
        // store 侧按 tier-① 语义跳过已有归属条目的 id。扫描失败必须上报,不能
        // 静默漏掉移出(那正是「根移除了、成员却复活」的成因)。
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

/// 删除项目:会话只被解绑(回落自动/隐式分组),永不删除;返回受影响会话
/// id 供前端提示。
#[tauri::command]
pub async fn delete_project(
    project_id: String,
    app: AppHandle,
    store: State<'_, ProjectStore>,
    sessions: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
) -> Result<DeleteProjectReport, String> {
    // 自动归组成员的枚举:两个绑定 store 中位于该项目任一 root 下的会话。已有
    // 归属条目的 id 由 store 侧按 tier-① 语义跳过(显式归属他处/已移出不算成员)。
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
    // Rebind fence FIRST, before the workspace detection below: a rebind
    // starting and completing inside the detection window would otherwise let
    // this command commit a stale `from`-prefixed root — exactly the
    // broken-link state the fence exists to prevent (review #464 round-7 B3).
    // Root-accepting writer under the same fence: `add_workspace_root` adds a
    // directory to a project and the store re-validates overlap, so committing
    // mid-rebind can both re-add a `from`-prefixed root and bind a session
    // under `from` after the rebind's candidate snapshot (review #464 round-6
    // finding 6).
    let _fence = store.rebind_fence()?;
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

/// 文件夹项目自动物化(Codex 客户端式收编):roots 由前端从会话列表的
/// workspace 聚合而来(客户端驱动,同 Codex `project/import` 由桌面端发起)。
/// 幂等:已锚定的根复用 / 冲突按根汇报;仅在有新建时广播 list-changed。
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

/// 反物化排除表(§3):`never = true` 表示"此文件夹不再自动建项目",`false`
/// 撤销。可见、可撤销、幂等;只影响未来的自动物化,既有项目不受影响。返回更新
/// 后的排除表;面板/侧栏通过 projects:list_changed 刷新。
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

/// `align_session_to_project` 的结果汇报。applied=false 时 reason 区分
/// `no_project`(无归属)、`no_change`(钥匙串已与项目一致)、
/// `write_skipped`(绑定 store 未写入:agent 记录消失 / sidecar 不可读,
/// 磁盘未变)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AlignOutcome {
    pub session_id: String,
    /// 对齐后的钥匙串快照(未变更时为当前快照):主槽 = 会话 cwd。
    pub roots: Vec<PathBuf>,
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// 对齐到项目(§6/§9.7 显式动作):把会话的钥匙串快照替换为它所属项目
/// **此刻**的全部 roots —— 主槽 = 会话自身 cwd(不换门,§9.2),附加根 =
/// 项目 roots 去掉 cwd,顺序保持。项目解析与前端同一规则:显式归属优先,
/// tier-② 多命中取 position 最小者(resolve_session_project)。
///
/// 双 store 写入(agent 记录 / 纯绑定 sidecar,与 rebind 同一套双写);
/// 存活引擎经 `Op::SyncSession` 推送,下个回合生效。围栏:活动回合(ACP
/// prompt / 原生回合 / 定时轮)以类型化 ALIGN_BUSY 拒绝;无绑定工作区的
/// 临时/定时会话得到 ALIGN_NO_WORKSPACE。幂等:钥匙串与当前完全一致时返回
/// applied=false、reason=no_change。
#[tauri::command]
pub async fn align_session_to_project(
    session_id: String,
    app: AppHandle,
    store: State<'_, ProjectStore>,
    sessions: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
    engines: State<'_, crate::features::assistant::engine_pool::EnginePool>,
) -> Result<AlignOutcome, String> {
    // 活动回合围栏(与 rebind 门同规则):运行中的 prompt/回合/定时轮一律拒绝。
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
        // 什么都没写(no_project / no_change / write_skipped):不推送存活引擎、
        // 不发事件——运行态不得与磁盘分叉(评审 #484 B3)。
        return Ok(outcome);
    }

    // 把新根集推给存活引擎(下个回合生效);推送失败不阻断——下一次
    // spawn/resume 会从绑定 store 回填同一份快照。
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
    // 列表透出刷新:workspace_roots 随 list_sessions / code 会话列表下发,chat
    // 车道由 bridge 层经既有 session:list_changed 订阅重取(code 车道由前端
    // align 调用点自己 refreshSessions)。
    super::sessions::emit_session_event(&app, "session:list_changed", &session_id, "aligned");
    Ok(outcome)
}

/// `align_session_to_project` 依赖的三个 store(抽出来让命令级测试绕过
/// Tauri State/AppHandle)。
pub(crate) struct AlignStores<'a> {
    pub projects: &'a ProjectStore,
    pub sessions: &'a SessionStore,
    pub agents: &'a SessionAgentStore,
}

/// align 命令的「判定并落盘」内核:命令壳负责活动回合探测(`busy`)、存活引擎
/// 推送与事件发射;这里只读 store、写两个绑定 store,并如实汇报 `applied`。
///
/// 与 rebind 的差别(评审 #484:align 无写入后 busy 复核):rebind 的复核针对
/// 「围栏到多文件迁移之间开启的回合会按旧目录执行」这个窗口;align 的写入是单
/// 条记录/sidecar 的原子重写,且从不移动会话自身 cwd(钥匙串只换附加根,
/// §9.2 不换门)——围栏窗口内开始的回合仍在同一 cwd 上执行,新根集下个回合经
/// SyncSession 生效,重试没有价值,故无写入后复核。
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
        return Err(
            "ALIGN_NO_WORKSPACE: session has no bound workspace (temporary sessions cannot align)"
                .to_string(),
        );
    }
    let cwd = session_roots.execution;

    // 当前快照:agent 记录(code/ACP)或纯绑定 sidecar。
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
        return Err(
            "ALIGN_NO_WORKSPACE: session has no bound workspace (temporary sessions cannot align)"
                .to_string(),
        );
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
            "ALIGN_BUSY: an active turn is in progress, retry when the session is idle".to_string(),
        );
    }

    // 双 store 写入:agent 记录(原生代码会话同时重写 sidecar)/ 纯 sidecar。
    // Ok(false) = store 什么都没写(agent 记录消失 / sidecar 不可读):不得声称
    // applied,命令层据此跳过存活引擎推送与事件(评审 #484 B3)。
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

/// Result report of rebind_workspace_root: per-session outcomes + affected
/// projects. Rebind is idempotent and failures can be retried directly (the
/// candidate snapshot includes retry items "already under to but metadata
/// not synced"; the succeeded parts rerun as no-ops).
#[derive(Debug, Clone, Default, Serialize)]
pub struct RebindWorkspaceReport {
    pub rebound_session_ids: Vec<String>,
    pub failed_session_ids: Vec<String>,
    pub affected_project_ids: Vec<String>,
    /// Sessions found in an active turn by the post-migration recheck or
    /// skipped by the idle-gated eviction: their bindings moved, but a turn
    /// may still execute against the old directory. The frontend keeps the
    /// dialog open on a non-empty list (also when nothing failed) and its
    /// retry feeds these ids back as `previous_post_busy_session_ids`, so the
    /// next run can tell a carryover post-busy session (moved by an earlier
    /// run, runtime still resident with the old cwd — must be reclaimed and,
    /// when busy again, honestly reported again) apart from a healthy
    /// to-lane session (nothing ever moved — must stay unreported). Closing
    /// the dialog instead would leave the instruction with no entry point,
    /// because the unavailable-root badge disappears once the root moved
    /// (review #463 M2 + round-8 MAJOR-2 + F-Major).
    #[serde(default)]
    pub post_busy_session_ids: Vec<String>,
}

/// `from` validation: empty, relative, and filesystem-root paths are
/// rejected. An empty prefix matches every record under folded-key matching
/// (a full rewrite), a root `from` with confirm-existing relocates
/// everything, and a relative `from` diverges the storage lanes (the
/// projects lane absolutizes it through ancestor resolution while the codex
/// lane folds it raw) — none of these is a rebind (review #463 minor). Not
/// reachable from the UI (the frontend always passes a stored project root),
/// so the copy stays backend prose.
fn validate_rebind_from(from: &Path) -> Result<(), String> {
    if from.as_os_str().is_empty() || !from.is_absolute() || from.parent().is_none() {
        return Err(format!(
            "rebind_workspace_root: from must be an absolute, non-root directory, got {}",
            from.display()
        ));
    }
    Ok(())
}

/// `to` = filesystem root (Unix `/`, Windows drive root — both have no
/// parent) is rejected: translating every binding onto the filesystem root
/// is a mass relocation, not a rebind (review #463 minor). Existence and
/// directory-ness are checked separately by
/// `validate_codex_project_workspace`. Carries a stable marker because the
/// folder picker can land here, and the message must not be Chinese-only for
/// en/ja users (review #463 round-8 M4).
fn validate_rebind_to(to: &Path) -> Result<(), String> {
    if to.parent().is_none() {
        return Err("REBIND_TO_ROOT: the destination cannot be a filesystem root".to_string());
    }
    Ok(())
}

/// `to` must not sit inside `from` (equality is handled by the caller
/// first): rebind translates by prefix, and a target inside the old
/// directory deepens on every rerun (/a/x → /a/x/new/x → …), breaking
/// idempotency (review #451 finding 6). Compared on folded keys so case /
/// separator differences cannot evade it, through the shared component
/// predicate (review #463 round-8 elegance). Reachable from the picker
/// (choosing a subfolder of the old folder), hence the typed marker (M4).
fn reject_nested_rebind_target(from: &Path, to_display: &Path) -> Result<(), String> {
    let from_key = crate::platform::os::filesystem_path_identity_key(
        &crate::platform::os::platform_compat_path(&from.to_string_lossy()).to_string_lossy(),
    );
    let to_display_str =
        crate::platform::os::filesystem_path_identity_key(&to_display.to_string_lossy());
    if crate::platform::os::path_identity_is_same_or_nested(
        to_display_str.trim_end_matches('/'),
        from_key.trim_end_matches('/'),
    ) {
        return Err(
            "REBIND_TO_NESTED: the destination cannot sit inside the original folder".to_string(),
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
        // Marker hygiene (round-8 should-fix 2): the tail after the marker is
        // diagnostics prose and crosses logs / the web bridge — keep it
        // English, like the other six markers.
        return Err(
            "REBIND_OLD_ROOT_EXISTS: original folder still exists; confirm in the dialog to proceed"
                .to_string(),
        );
    }
    Ok(())
}

/// Directory rebind (broken-link repair): after a project folder is
/// physically moved/deleted, every binding under the `from` prefix — project
/// roots, session workspaces (codex index / code-session sidecar / plain-chat
/// workspace-binding sidecar / SavedSession metadata), derived assignments —
/// is translated onto `to`. Unlike "move assignment" this is a physical-layer
/// write, so it carries fences:
/// - `to` must exist and be a directory (validated by
///   validate_codex_project_workspace) and must not be the filesystem root;
/// - `from` must be an absolute, non-root path;
/// - while the old directory `from` still exists, `confirm_existing = true`
///   is required (the frontend has strong-confirmed);
/// - if any affected session has an active turn (ACP prompt, native Engine
///   turn, or scheduled round) the whole rebind is rejected;
/// - project roots must not end up overlapping other projects, or the whole
///   rebind fails and rolls back.
/// Historical paths in transcripts are not rewritten; the workspace baseline
/// is recaptured per session (failures are only logged — the baseline is
/// derivable again).
///
/// Write order: session bindings (codex lane, then the plain-chat binding
/// sidecars) → SavedSession metadata (+ baseline) → project roots LAST
/// (review #463 round-8 M3). The old order committed the roots first, so a
/// crash before the session lanes left roots already moved (the unavailable
/// badge — the only rebind entry — gone) with bindings still under `from`, a
/// state no retry could reach. With the roots last, an interrupted run still
/// shows the old root as unavailable and the badge reruns the remaining
/// lanes; the root rewrite itself is pre-flighted by
/// `ProjectStore::plan_rebind_roots` so an overlap conflict aborts before any
/// session binding is touched.
///
/// Storage-form invariant (review #463 m1/B1): `from` is normalized once at
/// this entry via `rebind_source_display` — project roots are stored in
/// `root_display` (canonical) form and session bindings were canonicalized
/// at bind time, while the storage lanes match in different domains (the
/// projects store resolves symlinked ancestors, the codex/session lanes fold
/// lexically). Normalizing at the entry pins every lane to one resolved form,
/// so an alias caller (macOS `/var/x` vs the stored `/private/var/x`) cannot
/// half-migrate. `rebind_workspace_root` is a public command; this
/// normalization is part of its input contract.
///
/// Known residual (review #463 round-8 minor): the codex restore/backfill
/// lane copies `workspace_path` verbatim, so a record written before the
/// canonicalization convention keeps its raw spelling and a later alias-form
/// rebind will not match it. Only records bound by the current binary are
/// covered by the invariant above.
#[tauri::command]
pub async fn rebind_workspace_root(
    from: PathBuf,
    to: PathBuf,
    confirm_existing: Option<bool>,
    // Post-busy ids from the dialog's previous run, fed back on its retry
    // (review #463 F-Major). They never widen the candidate set: only ids
    // that independently land in this run's to-lane retry population are
    // honored, so a stale or forged list cannot make an unrelated session an
    // eviction candidate or a report entry.
    previous_post_busy_session_ids: Option<Vec<String>>,
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
    let to_display = crate::features::codex_acp::validate_codex_project_workspace(&to)
        .map_err(|e| format!("REBIND_TO_UNUSABLE: {e:#}"))?;
    // Normalize `from` once for all three storage lanes (review #463 B1, see
    // the docblock). Resolved through the deepest existing ancestor, so a
    // vanished directory behind a symlinked ancestor (macOS /var) still
    // resolves into the stored key domain.
    let from = crate::features::projects::rebind_source_display(&from);
    if from == to_display {
        return Ok(RebindWorkspaceReport::default());
    }
    reject_nested_rebind_target(&from, &to_display)?;
    require_confirm_existing(&from, confirm_existing)?;
    // Root pre-flight (review #463 round-8 M3): the roots are committed last,
    // so a rewrite that cannot succeed — an overlap conflict with another
    // project's territory — must be rejected here, before any session binding
    // has been translated. Reachable from the picker (choosing a folder that
    // is or contains another project's root), hence the typed marker so the
    // copy is localized (round-8 M4).
    store
        .plan_rebind_roots(&from, &to_display)
        .map_err(|e| format!("REBIND_ROOTS_CONFLICT: {e:#}"))?;

    // Affected-set snapshot (shared by the active-turn fence and the
    // metadata replay), taken BEFORE any rewrite (review #463 M1): the
    // return of rebind_workspace_prefix is only the set this run rewrote — a
    // session translated by a previous run whose set_workspace failed no
    // longer matches `from` and could never be retried without the snapshot.
    // sessions_under_workspace includes off-index orphan sidecars (M6);
    // retry candidates "already under to but metadata not synced" are folded
    // in too, so a failed rerun converges.
    let mut affected = acp_pool.agents().sessions_under_workspace(&from);
    // Plain-chat working-directory bindings (review #463 round-8 B1): a chat
    // created with a `workspace_path` carries only
    // `sessions/<id>/workspace-binding.json` (plus the in-memory cache) — no
    // codex index record and no code-session sidecar — and the execution-root
    // resolver reads its cwd from exactly that binding. Without this lane the
    // next turn would keep using the vanished folder and recreate it.
    let plain_bindings_under_from = sessions.workspace_bindings_under(&from);
    for (session_id, path) in &plain_bindings_under_from {
        if !affected.iter().any(|(sid, _)| sid == session_id) {
            affected.push((session_id.clone(), path.clone()));
        }
    }
    // Post-busy sessions of a previous run land here on retry (review #463
    // M2): their metadata was already synced in run 1, so they are absent
    // from `affected` — without feeding them back as explicit eviction
    // candidates, the documented "retry once when idle" remedy would be a
    // no-op (they would never re-enter rebound_session_ids and never be
    // evicted). A known, accepted coarseness: healthy sessions created
    // directly under `to` also land here, and reclaiming their idle runtime is
    // a harmless lazy respawn (rebuilt on the next send). It is NOT the same
    // work the idle reaper does: the reaper never touches the session the user
    // has open, so this tail is the only thing that reclaims a stranded
    // runtime within a session's own lifetime.
    //
    // The two populations must stay apart in the REPORT (round-8 minor 5 +
    // F-Major): a healthy to-lane session is never reported post-busy —
    // nothing was translated for it, so "retry when idle" would be a
    // guaranteed no-op — while a carryover post-busy session of a previous
    // run (its bindings moved then, its old-cwd runtime is still resident)
    // MUST be reported again when the eviction refuses, or the dialog would
    // close claiming full success while the next turn resurrects the
    // vanished folder. The dialog feeds its previous report's post-busy ids
    // back as `previous_post_busy_session_ids`; only ids that independently
    // land in this run's to-lane retry population are honored as carryover
    // (see carryover_post_busy_candidates), so the fed-back list cannot
    // widen the eviction or report sets.
    let mut retry_evict_candidates: Vec<String> = Vec::new();
    for (session_id, path) in acp_pool.agents().sessions_under_workspace(&to_display) {
        admit_rebind_retry_candidate(
            session_id,
            path,
            &sessions,
            &mut affected,
            &mut retry_evict_candidates,
        );
    }
    for (session_id, path) in sessions.workspace_bindings_under(&to_display) {
        admit_rebind_retry_candidate(
            session_id,
            path,
            &sessions,
            &mut affected,
            &mut retry_evict_candidates,
        );
    }
    let carryover_post_busy = carryover_post_busy_candidates(
        &retry_evict_candidates,
        &previous_post_busy_session_ids.unwrap_or_default(),
    );

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

    // Order: session bindings (index + code-session sidecars) → plain-chat
    // binding sidecars → metadata → baseline → project roots LAST. Every step
    // is idempotent; a failed retry only completes the unfinished parts. The
    // metadata loop is driven by the snapshot UNION the plain lane's rebound
    // set (round-7 should-fix): entries the pre-rewrite snapshot never
    // contained but this run's plain batch just moved must get the
    // metadata.workspace replay and a report entry too, not silently wait for
    // a rerun to converge them. `rebind_target_path` returns the rebound path
    // as-is (to-prefix arm), so the same per-candidate logic serves both;
    // metadata_rebind_targets dedupes the union by session id against the
    // full snapshot (see its doc for why the codex rebound set would be the
    // wrong key).
    let prefix_outcome = acp_pool
        .agents()
        .rebind_workspace_prefix(&from, &to_display)
        .map_err(|e| format!("rebind_workspace_root: {e:#}"))?;
    // Plain-chat binding sidecars (review #463 round-8 B1) via the unified
    // batch (#464): it moves the sidecars AND the in-memory cache in one pass,
    // translates the legacy global table BEFORE the sidecars move so every
    // crash window heals forward, and names the sessions a surviving table
    // would resurrect at the next boot. A sidecar whose write failed stays on
    // disk with the old path, is reported below, and the next run's `from`
    // scan still matches it.
    let plain_rebind = sessions
        .rebind_workspace_bindings(&from, &to_display)
        .map_err(|e| format!("rebind_workspace_root: {e:#}"))?;
    let binding_final_stale: Vec<String> = plain_rebind.failed_session_ids.clone();
    // Finally-stale sidecar list (Major 2): an orphan rewrite failure, or an
    // indexed session whose rewrite + retry passes both failed — no
    // self-healing path remains in this run (backfill only fills missing
    // sidecars, and boot restore skips sidecars while the index is intact).
    // Not reporting them would let a restore with a damaged index resurrect
    // old directories and silently undo the rebind. Stale sidecars still sit
    // under the from prefix; once counted as failed, a user rerun converges
    // them via the on-disk prefix scan. Both storage lanes contribute: the
    // codex lane's outcome plus the plain-chat bindings that failed above.
    let mut final_stale = prefix_outcome.sidecar_final_stale.clone();
    for session_id in binding_final_stale {
        if !final_stale.contains(&session_id) {
            final_stale.push(session_id);
        }
    }
    // Plain-chat lane post-pass fence (review #463 F1 — the asymmetric half
    // of round-8 minor 8): the codex lane re-scans index records still under
    // `from` after its rewrite, but nothing re-scanned the plain-chat binding
    // sidecars. A chat created+bound under `from` DURING this run
    // (create_session is not serialized by the rebind gate) was absent from
    // the pre-rewrite snapshot, so the loop above never rewrote it and the
    // run would report full success while its binding still points at the
    // vanished folder — its next turn would recreate it. Folding the re-scan
    // hits into final_stale routes them through the shared fence below:
    // reported as failed (hence also eviction candidates), and a rerun
    // converges them via the same on-disk scan.
    plain_lane_fence_rescan(&sessions, &from, &mut final_stale);
    // Code-lane rebound set: only these consume workspace baselines, so the
    // recapture below is gated to them (#464 unify).
    let code_rebound_ids: std::collections::HashSet<String> = prefix_outcome
        .affected
        .iter()
        .map(|(session_id, _)| session_id.clone())
        .collect();
    let mut rebound_session_ids = Vec::new();
    let mut failed_session_ids = Vec::new();
    for (session_id, bound_path) in
        metadata_rebind_targets(&affected, &prefix_outcome.affected, &plain_rebind.rebound).iter()
    {
        let Some(new_path) = SessionAgentStore::rebind_target_path(bound_path, &from, &to_display)
        else {
            continue;
        };
        // An orphan (session JSON already gone) has no metadata to write; a
        // corrupt JSON is NOT an orphan — set_workspace's load parse failure
        // lands in failed and is retryable (review #463 minor: the orphan
        // classification accepts only NotFound, not any load error).
        if sessions.durable_session_record_is_absent(session_id) {
            if final_stale.iter().any(|sid| sid == session_id) {
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
                if final_stale.iter().any(|sid| sid == session_id) {
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
        // Baseline recapture is gated to code sessions (#464 unify): plain
        // bound sessions do not consume workspace baselines, so no code-lane
        // sidecar is created for them.
        if code_rebound_ids.contains(session_id.as_str()) {
            // Baseline recapture: best-effort, the git fingerprint is derivable
            // again, and a failure does not block the rebind. Runs on
            // spawn_blocking: a non-git directory synchronously walks tens of
            // thousands of entries and must not run serially on the async
            // command thread (same idiom as session creation in codex.rs).
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

    // Project roots LAST (review #463 round-8 M3): committing them earlier
    // made an interrupted run unreachable — the root would already sit at
    // `to` (available, so no badge) while the session lanes still pointed at
    // `from`, and re-adding the old root to retry collides with the moved one
    // and rolls back. With the roots last, a crash anywhere above still shows
    // the old root as unavailable and the badge reruns the remaining lanes
    // (every step is idempotent). The overlap invariant was pre-flighted
    // before any write and is revalidated under the write lock here.
    //
    // This retry entry exists exactly while `from` is unavailable, which is
    // also the only state in which the badge (the sole rebind entry) is
    // rendered — so the reorder restores the entry for every interrupted run
    // that the user could have started in the first place. In the
    // strong-confirm path (`from` reappeared after the badge was shown, see
    // require_confirm_existing) the folder is available again and no badge is
    // rendered either way; the dialog that drove the run is still open and its
    // retry covers that window.
    //
    // Residual (review #463 round-8 MINOR-3, on record): the pre-flight reads a
    // snapshot and the commit revalidates under the write lock, so a project
    // mutation landing between the two (the rebind gate serializes rebinds
    // only) fails HERE — after the session lanes are durable. The user gets the
    // localized conflict marker and a dialog that stays open with a retry, and
    // a rerun converges once the overlap is resolved, but the per-session
    // detail of this run is not reported alongside the error.
    // Only a genuine overlap conflict carries the localized conflict marker
    // (round-8 review M3): persist and other infrastructure failures must
    // surface as ordinary errors, or the user is told to resolve a
    // "conflict" that no resolution fixes.
    let affected_project_ids =
        store
            .rebind_roots(&from, &to_display)
            .map_err(|error| match error {
                RebindRootsError::Overlap(context) => format!("REBIND_ROOTS_CONFLICT: {context:#}"),
                RebindRootsError::Other(context) => format!("rebind_workspace_root: {context:#}"),
            })?;

    // Post-pass fence hits that the pre-rewrite snapshot never saw (review
    // #463 round-8 MINOR-1): a session created under `from` by a concurrent
    // writer during this run is not in `affected`, so the metadata loop never
    // visited it and its `sidecar_final_stale` entry would otherwise be
    // dropped. It has no metadata to replay here, but it must still be
    // reported — otherwise the run claims success while an index record still
    // sits under `from` and a boot restore can resurrect the old path. Done
    // before the reclaim tail so these sessions are eviction candidates too.
    fold_unreported_fence_hits(&final_stale, &affected, &mut failed_session_ids);

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
    // successful reclaim also resets the per-session shell state, which pins
    // its cwd at construction (M1/M2). Candidates are this run's rebound
    // sessions, the sessions it could not finish, and the fed-back retry
    // candidates (M2).
    let post_busy: std::collections::HashSet<String> =
        post_busy_session_ids.iter().cloned().collect();
    for session_id in collect_eviction_candidates(
        &rebound_session_ids,
        &failed_session_ids,
        &retry_evict_candidates,
    ) {
        if post_busy.contains(&session_id) {
            continue;
        }
        let acp_idle = acp_pool.evict_if_idle_for_rebind(&session_id).await;
        let engine_idle = engines.evict_if_idle_for_rebind(&session_id).await;
        if !acp_idle || !engine_idle {
            // A turn started between the recheck and the eviction and the
            // pools refused to kill it. Surface the session so the user can
            // retry once it is idle again — but only when something was
            // actually translated for it (review #463 round-8 minor +
            // F-Major): THIS run's rebound/failed sessions, or a carryover
            // post-busy session a previous run moved (fed back by the
            // dialog's retry — its old-cwd runtime is still resident, so an
            // unreported refusal would close the dialog on a false full
            // success and the next turn would resurrect the vanished
            // folder). A healthy to-lane session needs nothing, so "retry
            // when idle" would be a guaranteed no-op and it stays
            // unreported; its reclaim stays best-effort here and falls back
            // to the idle reaper.
            let touched_this_run = rebound_session_ids.iter().any(|sid| sid == &session_id)
                || failed_session_ids.iter().any(|sid| sid == &session_id);
            if (touched_this_run || carryover_post_busy.contains(&session_id))
                && !post_busy_session_ids.contains(&session_id)
            {
                post_busy_session_ids.push(session_id);
            }
        }
    }
    // The plain sidecars and metadata moved, but if the legacy global table
    // could not be synced, the next boot migration would re-bind the old paths
    // over the fresh sidecars — the report must not claim success (#464
    // round-5 blocker 1). The list is driven by the entries the surviving
    // table would actually resurrect, not by this run's rewrite log: on a
    // retry nothing is left to rewrite and the rebound set is empty while the
    // stale table is still there (#464 round-6 blocking 1).
    if plain_rebind.legacy_sync_failed {
        merge_legacy_resurrections_into_failures(
            &mut failed_session_ids,
            &plain_rebind.legacy_resurrection_ids,
        );
    }
    Ok(RebindWorkspaceReport {
        rebound_session_ids,
        failed_session_ids,
        affected_project_ids,
        post_busy_session_ids,
    })
}

/// Admits a session found under the `to` prefix into the rebind (review #463
/// M2 + round-8 B1): metadata that does not match the binding is a candidate
/// left unfinished by an earlier run, so it joins `affected` and gets its
/// metadata replayed; metadata that already matches means the session is
/// healthy from the rebind's point of view, and it becomes a pure eviction
/// candidate instead — that is how a post-busy session of a previous run
/// reaches the idle-gated reclaim tail. A session whose metadata cannot be
/// read (orphan/corrupt) is treated as a candidate too; the metadata loop
/// classifies it.
fn admit_rebind_retry_candidate(
    session_id: String,
    path: PathBuf,
    sessions: &SessionStore,
    affected: &mut Vec<(String, PathBuf)>,
    retry_evict_candidates: &mut Vec<String>,
) {
    if affected.iter().any(|(sid, _)| *sid == session_id) {
        return;
    }
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

/// Eviction candidates for the rebind tail: sessions rebound in this run,
/// sessions this run could not finish (`failed_session_ids` — their binding
/// lanes may already have moved even though the metadata write failed, so a
/// resident runtime would otherwise keep the old cwd with nothing reclaiming
/// it; review #463 round-8 MAJOR-1), plus `to`-lane sessions whose metadata is
/// already synced (post-busy sessions from a previous run — review #463 M2),
/// deduplicated, order preserved.
fn collect_eviction_candidates(
    rebound_session_ids: &[String],
    failed_session_ids: &[String],
    retry_evict_candidates: &[String],
) -> Vec<String> {
    let mut candidates = rebound_session_ids.to_vec();
    for session_id in failed_session_ids.iter().chain(retry_evict_candidates) {
        if !candidates.contains(session_id) {
            candidates.push(session_id.clone());
        }
    }
    candidates
}

/// Carryover post-busy sessions of a previous run (review #463 F-Major): the
/// dialog's retry feeds its previous report's post-busy ids back, and only
/// the intersection with THIS run's to-lane retry population is honored.
/// Honoring the raw list would let a stale or forged id mark an unrelated
/// session as reportable; intersecting keeps the fed-back list purely
/// reclassifying — every honored id is a session this run would have
/// reclaimed anyway, so the eviction set never widens. A fed-back id whose
/// metadata drifted out of sync lands in `affected` instead of the retry
/// population and is handled by the metadata loop, so it drops out of the
/// carryover set deliberately (it is no longer "already converged").
fn carryover_post_busy_candidates(
    retry_evict_candidates: &[String],
    previous_post_busy_session_ids: &[String],
) -> std::collections::HashSet<String> {
    let fed_back: std::collections::HashSet<&str> = previous_post_busy_session_ids
        .iter()
        .map(String::as_str)
        .collect();
    retry_evict_candidates
        .iter()
        .filter(|session_id| fed_back.contains(session_id.as_str()))
        .cloned()
        .collect()
}

/// Folds post-pass fence hits into the failed list (review #463 round-8
/// MINOR-1 + F1): sessions still under `from` after the rewrite that the
/// pre-rewrite snapshot never saw (created by a concurrent writer during the
/// run) have no metadata replayed by this run, but they must be reported —
/// otherwise the run claims success while a binding still sits under `from`
/// and the next turn (or a boot restore) resurrects the old path. Already
/// listed sessions (in `affected`, classified by the metadata loop, or
/// already failed) are not duplicated.
fn fold_unreported_fence_hits(
    final_stale: &[String],
    affected: &[(String, PathBuf)],
    failed_session_ids: &mut Vec<String>,
) {
    for session_id in final_stale {
        if !affected.iter().any(|(sid, _)| sid == session_id)
            && !failed_session_ids.contains(session_id)
        {
            failed_session_ids.push(session_id.clone());
        }
    }
}

/// Plain-chat lane post-pass fence (review #463 F1 — the asymmetric half of
/// round-8 minor 8): the codex lane re-scans its index records still under
/// `from` after the rewrite; this re-scan does the same for the plain-chat
/// binding sidecars. A chat created+bound under `from` during the run
/// (create_session is not serialized by the rebind gate) was not in the
/// pre-rewrite snapshot, so the rewrite loop never visited it — folding the
/// hit into `final_stale` lets the shared fence report it as failed (and the
/// eviction tail reclaim it), and a rerun converges it via the same scan.
/// A binding whose rewrite succeeded no longer matches `from` (sidecar and
/// cache both moved), and a failed rewrite is already listed, so the
/// contains-guard keeps this a pure addition of NEW hits.
fn plain_lane_fence_rescan(sessions: &SessionStore, from: &Path, final_stale: &mut Vec<String>) {
    for (session_id, _) in sessions.workspace_bindings_under(from) {
        if !final_stale.contains(&session_id) {
            final_stale.push(session_id);
        }
    }
}

/// Round-8 should-fix 8: the round-6-B1 user-facing contract — when the
/// legacy global table could not be synced, every session that table would
/// resurrect at the next boot joins the report's failure list (deduplicated),
/// so the dialog stays open with an honest retry instead of closing on a
/// false success. Extracted from the command body for testability: the body
/// needs the Tauri harness, this merge is pure.
fn merge_legacy_resurrections_into_failures(
    failed_session_ids: &mut Vec<String>,
    legacy_resurrection_ids: &[String],
) {
    for session_id in legacy_resurrection_ids {
        if !failed_session_ids.contains(session_id) {
            failed_session_ids.push(session_id.clone());
        }
    }
}

/// Metadata replay targets for one rebind run: the pre-rewrite snapshot
/// first (it already contains every binding under `from` visible before the
/// rewrites started — the union happens at snapshot time, projects.rs
/// `affected`), then the codex lane's rebound set ONLY for entries the
/// snapshot never contained (a session created+bound under `from` between
/// the snapshot and the codex rewrite — round-8 review M1: it was rewritten
/// but neither metadata-synced nor reported, invisible to the post-pass
/// fence because its record no longer sits under `from`), then the plain
/// batch's rebound set under the same rule. Deduplication is by session id
/// against the FULL snapshot (and the arms accumulated before each), not the
/// codex lane's rebound set: the lanes are structurally blind to each other,
/// so keying a filter on the codex outcome would run the loop body twice for
/// every plain chat — `set_workspace` twice and, worse, the id twice in
/// `rebound_session_ids` (no dedup downstream), so the dialog would claim
/// "Rebound 2N" for N plain chats (round-8 review finding 1).
fn metadata_rebind_targets(
    affected: &[(String, PathBuf)],
    codex_rebound: &[(String, PathBuf)],
    plain_rebound: &[(String, PathBuf)],
) -> Vec<(String, PathBuf)> {
    let mut targets = affected.to_vec();
    for arm in [codex_rebound, plain_rebound] {
        for entry in arm {
            if !targets.iter().any(|(id, _)| id == &entry.0) {
                targets.push(entry.clone());
            }
        }
    }
    targets
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
            never_materialize_roots: Vec::new(),
        })
        .expect("serialize ProjectListResponse");
        let object = value.as_object().expect("response serializes as an object");
        assert!(object.contains_key("projects"));
        assert!(object.contains_key("assignments"));
        assert!(object.contains_key("never_materialize_roots"));
    }

    /// Real stores under one isolated home: the align kernel reads
    /// `session_roots`/bindings through SessionStore, the agent lane through
    /// SessionAgentStore, and project resolution through ProjectStore, so the
    /// outcome lattice can only be pinned against the real trio.
    fn align_fixtures() -> (
        tempfile::TempDir,
        std::sync::MutexGuard<'static, ()>,
        SessionStore,
        SessionAgentStore,
        ProjectStore,
    ) {
        let guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).expect("create home");
        // SAFETY: platform::paths::tests::ENV_LOCK is held; env writes are
        // serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };
        let sessions =
            SessionStore::boot_with_scheduled_root(home.join("scheduled")).expect("session store");
        let agents = SessionAgentStore::load().expect("agent store");
        let projects = ProjectStore::from_paths(home.join("projects-store.json"));
        // Production parity (lib.rs): the plain-lane roots authority resolves
        // agent-record bindings too — without this the align kernel would
        // see a native code session as unbound.
        let resolver_agents = agents.clone();
        sessions.set_execution_root_resolver(std::sync::Arc::new(move |session_id: &str| {
            resolver_agents.get(session_id).workspace_path.clone()
        }));
        (tmp, guard, sessions, agents, projects)
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
    fn merge_legacy_resurrections_dedupes_into_failures() {
        let mut failed = vec!["already-failed".to_string()];
        merge_legacy_resurrections_into_failures(
            &mut failed,
            &[
                "resurrected-a".to_string(),
                "already-failed".to_string(),
                "resurrected-b".to_string(),
            ],
        );
        assert_eq!(
            failed,
            vec![
                "already-failed".to_string(),
                "resurrected-a".to_string(),
                "resurrected-b".to_string()
            ]
        );
        // An empty resurrection set (a table this process never parsed) adds
        // nothing — legacy_sync_failed alone still failed the run upstream.
        merge_legacy_resurrections_into_failures(&mut failed, &[]);
        assert_eq!(failed.len(), 3);
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
    fn metadata_rebind_targets_dedupes_late_arms_against_full_snapshot() {
        // Round-8 finding 1 + M1: the snapshot already contains every binding
        // under `from` visible before the rewrites; the codex and plain arms
        // contribute ONLY entries the snapshot never contained. Deduping the
        // plain arm against the codex rebound set (structurally blind to
        // plain chats) would double-report every pure plain chat, and
        // dropping the codex arm entirely would lose the rewritten newcomer
        // (metadata never synced, id never reported).
        let affected = vec![
            ("code".to_string(), PathBuf::from("/from")),
            ("plain-in-snapshot".to_string(), PathBuf::from("/from/sub")),
        ];
        let codex_rebound = vec![
            // Created+bound between the snapshot and the codex rewrite (M1):
            // absent from the snapshot, joins with its translated path.
            ("codex-newcomer".to_string(), PathBuf::from("/from/deep")),
            // Snapshot member the codex lane rewrote: must NOT repeat.
            ("code".to_string(), PathBuf::from("/from")),
        ];
        let plain_rebound = vec![
            // Already in the snapshot: must NOT repeat (would double-report).
            ("plain-in-snapshot".to_string(), PathBuf::from("/to/sub")),
            // Created+bound during the run: absent from the snapshot, joins.
            ("midrun".to_string(), PathBuf::from("/to/midrun")),
            // Already contributed by the codex arm: the later arm must not
            // repeat it either.
            ("codex-newcomer".to_string(), PathBuf::from("/to/deep")),
        ];
        let targets = metadata_rebind_targets(&affected, &codex_rebound, &plain_rebound);
        let ids: Vec<&str> = targets.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["code", "plain-in-snapshot", "codex-newcomer", "midrun"]
        );
        // The snapshot's own path wins for duplicated ids: the loop's
        // rebind_target_path translates it, while a rebound arm's value is
        // already the `to` path (as-is arm) — keeping the snapshot entry
        // preserves the retry semantics documented for the snapshot.
        assert_eq!(
            targets.iter().find(|(id, _)| id == "plain-in-snapshot"),
            Some(&("plain-in-snapshot".to_string(), PathBuf::from("/from/sub"))),
        );
        // The codex newcomer keeps its pre-rewrite path so the loop's
        // rebind_target_path translates it (or passes it through as-is when
        // the lane already recorded the translated form).
        assert_eq!(
            targets.iter().find(|(id, _)| id == "codex-newcomer"),
            Some(&("codex-newcomer".to_string(), PathBuf::from("/from/deep"))),
        );
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
        let error = validate_rebind_to(&root).unwrap_err();
        assert!(
            error.starts_with("REBIND_TO_ROOT"),
            "typed marker: the picker can land here and the copy is localized (round-8 M4)"
        );
        let normal = std::env::temp_dir().join("pinvou3-rebind-to-check");
        assert!(validate_rebind_to(&normal).is_ok());
    }

    #[test]
    fn eviction_candidates_merge_rebound_failed_and_retry_without_duplicates() {
        // review #463 M2 + round-8 MAJOR-1: this run's rebound sessions, the
        // ones it failed to finish, and the fed-back retry (post-busy)
        // population are all eviction candidates, deduplicated, rebound first.
        let rebound = vec!["s1".to_string(), "s2".to_string()];
        let failed = vec!["s2".to_string(), "s4".to_string()];
        let retry = vec!["s4".to_string(), "s3".to_string()];
        assert_eq!(
            collect_eviction_candidates(&rebound, &failed, &retry),
            vec![
                "s1".to_string(),
                "s2".to_string(),
                "s4".to_string(),
                "s3".to_string()
            ]
        );
        assert!(collect_eviction_candidates(&[], &[], &[]).is_empty());
        assert_eq!(
            collect_eviction_candidates(&[], &[], &["s9".to_string()]),
            vec!["s9".to_string()],
            "a pure retry run (nothing rebound) still evicts the fed-back candidates"
        );
        assert_eq!(
            collect_eviction_candidates(&[], &["s8".to_string()], &[]),
            vec!["s8".to_string()],
            "a session whose metadata write failed is still reclaimed: its binding lanes may have moved"
        );
    }

    #[test]
    fn carryover_candidates_honor_only_the_to_lane_retry_population() {
        // review #463 F-Major: the dialog's retry feeds its previous report's
        // post-busy ids back, and only ids that independently land in THIS
        // run's to-lane retry population are honored as carryover. The
        // fed-back list reclassifies reporting; it must never widen the
        // eviction or report sets.
        let retry = vec!["carryover".to_string(), "healthy".to_string()];
        let fed_back = vec!["carryover".to_string(), "forged".to_string()];
        let carryover = carryover_post_busy_candidates(&retry, &fed_back);
        assert!(
            carryover.contains("carryover"),
            "a fed-back id in the retry population is a carryover post-busy session"
        );
        assert!(
            !carryover.contains("healthy"),
            "a healthy to-lane session stays unreported even though it is an eviction candidate"
        );
        assert!(
            !carryover.contains("forged"),
            "a fed-back id outside the retry population is ignored — the list cannot widen the sets"
        );
        assert!(
            carryover_post_busy_candidates(&retry, &[]).is_empty(),
            "a first run (nothing fed back) has no carryover population"
        );
    }

    #[test]
    fn fold_unreported_fence_hits_reports_only_new_unaffected_hits() {
        // review #463 round-8 MINOR-1 + F1: a fence hit the pre-rewrite
        // snapshot never saw must be reported as failed; sessions already
        // classified by the metadata loop (in `affected`) or already failed
        // must not be duplicated.
        let affected = vec![("seen".to_string(), PathBuf::from("/from/seen"))];
        let mut failed = vec!["already-failed".to_string()];
        let final_stale = vec![
            "seen".to_string(),
            "already-failed".to_string(),
            "concurrent".to_string(),
        ];
        fold_unreported_fence_hits(&final_stale, &affected, &mut failed);
        assert_eq!(
            failed,
            vec!["already-failed".to_string(), "concurrent".to_string()],
            "only the fence hit the snapshot never saw is appended"
        );
        // Idempotent: a second fold (the dead duplicate loop this helper
        // replaced, review #463 N1) can never push anything.
        fold_unreported_fence_hits(&final_stale, &affected, &mut failed);
        assert_eq!(failed.len(), 2);
    }

    /// Isolated SessionStore for the plain-lane fence test, borrowing the
    /// process-wide env lock (same idiom as features::sessions::tests).
    fn isolated_session_store() -> (SessionStore, std::sync::MutexGuard<'static, ()>) {
        let guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-rebind-fence-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
        let store = SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot");
        (store, guard)
    }

    #[test]
    fn plain_lane_fence_rescan_reports_chat_created_during_the_run() {
        // review #463 F1 regression: a chat created+bound under `from` DURING
        // the rebind run (create_session is not serialized by the rebind
        // gate) was absent from the pre-rewrite snapshot, so the rewrite loop
        // never visited it; without the re-scan the run reported full success
        // while its binding still pointed at the vanished folder.
        let (store, _g) = isolated_session_store();
        let from =
            std::env::temp_dir().join(format!("pinvou3-rebind-fence-from-{}", std::process::id()));
        let to =
            std::env::temp_dir().join(format!("pinvou3-rebind-fence-to-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
        std::fs::create_dir_all(from.join("nested")).expect("create from dir");
        std::fs::create_dir_all(&to).expect("create to dir");

        // Pre-rewrite snapshot population: chat A is bound under `from` and
        // the rewrite loop moves it (sidecar + cache) onto `to`.
        let chat_a = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create chat A");
        store
            .bind_session_workspace(&chat_a.metadata.id, from.join("nested"))
            .expect("bind A under from");
        let affected = vec![(chat_a.metadata.id.clone(), from.join("nested"))];
        assert!(store.rebind_workspace_binding(&chat_a.metadata.id, to.join("nested")));

        // The concurrent writer: chat B is created+bound under `from` while
        // the run is in flight — the snapshot never saw it.
        let chat_b = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create chat B");
        store
            .bind_session_workspace(&chat_b.metadata.id, from.join("nested"))
            .expect("bind B under from");

        let mut final_stale = Vec::new();
        plain_lane_fence_rescan(&store, &from, &mut final_stale);
        assert_eq!(
            final_stale,
            vec![chat_b.metadata.id.clone()],
            "the re-scan finds exactly the binding created during the run; the rewritten one no longer matches"
        );
        let mut failed_session_ids = Vec::new();
        fold_unreported_fence_hits(&final_stale, &affected, &mut failed_session_ids);
        assert_eq!(
            failed_session_ids,
            vec![chat_b.metadata.id.clone()],
            "the fence hit is reported as failed, so the run cannot claim full success"
        );

        let _ = std::fs::remove_dir_all(&from);
        let _ = std::fs::remove_dir_all(&to);
    }

    #[test]
    fn rebind_rejects_target_nested_inside_from() {
        let from = Path::new("/a/b");
        let nested = reject_nested_rebind_target(from, Path::new("/a/b/c")).unwrap_err();
        assert!(
            nested.starts_with("REBIND_TO_NESTED"),
            "typed marker: the picker can land here and the copy is localized (round-8 M4)"
        );
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
