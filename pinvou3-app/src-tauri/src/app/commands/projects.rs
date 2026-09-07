//! 项目层命令:跨 store 组合与会话存在性校验在此层完成,
//! `features::projects` 本体不依赖 sessions/codex_acp(依赖方向约束)。
//!
//! Phase 0 暴露:list/create/update/delete/move。目录重绑定
//! (`rebind_workspace_root`)属 Phase 4,其栅栏(活跃回合拒绝、baseline
//! 重采集)需要 AcpPool 写路径集成,不随本层首发。

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::features::codex_acp::{AcpPool, CodexWorkspaceKind};
use crate::features::projects::{DeleteProjectReport, MoveSessionOutcome, Project, ProjectStore};
use crate::features::sessions::SessionStore;

use super::sessions::ensure_chat_session;

/// 项目事件只走本地 emit:projects 域按 bridge 契约是桌面端专属,
/// remote-control 的转发白名单没有该事件,转发只会被中继拒绝并每次
/// 刷一条拒绝日志(评审 #447 finding 11:在消费方出现前不转发)。
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

/// 项目列表,按 position 有序,含每个 root 的可用性与显式成员数。
#[tauri::command]
pub async fn list_projects(store: State<'_, ProjectStore>) -> Result<Vec<ProjectListItem>, String> {
    Ok(store
        .list()
        .iter()
        .map(|project| {
            ProjectListItem::from_project(project, store.assigned_session_ids(&project.id).len())
        })
        .collect())
}

/// 创建项目。roots 可为空(纯标签项目);非空时逐个过绝对性/重叠校验。
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

/// 更新项目:名称与 roots 均为可选补丁,None 保持不变。
#[tauri::command]
pub async fn update_project(
    project_id: String,
    name: Option<String>,
    roots: Option<Vec<PathBuf>>,
    app: AppHandle,
    store: State<'_, ProjectStore>,
) -> Result<ProjectListItem, String> {
    let project = store
        .update_project(&project_id, name, roots)
        .map_err(|e| format!("update_project({project_id}): {e:#}"))?;
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
    // 先确认会话存在,避免归属表残留无效 id(同 set_session_pinned 惯例);
    // scheduled-run 会话与兄弟命令同口径拒绝,防止运行记录被写进归属表。
    ensure_chat_session(&sessions, &session_id, "move_session_to_project")
        .map_err(|e| format!("move_session_to_project({session_id}): {e}"))?;
    let workspace_root = if add_workspace_root.unwrap_or(false) {
        if project_id.is_none() {
            return Err(
                "move_session_to_project: add_workspace_root requires project_id".to_string(),
            );
        }
        // 普通 chat 会话在池里没有工作区记录,底层会报"not an ACP session"——
        // 语义上该组合只是"没有可加的目录",错误信息按此表述,避免误导排查方向。
        let info = acp_pool.workspace_info(&session_id).map_err(|e| {
            format!(
                "move_session_to_project({session_id}): session has no resolvable workspace record to add as a root (normal chat sessions have none): {e:#}"
            )
        })?;
        if info.workspace_kind != CodexWorkspaceKind::Project {
            return Err(format!(
                "move_session_to_project({session_id}): temporary session has no project folder to add"
            ));
        }
        Some(PathBuf::from(info.workspace_path))
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
