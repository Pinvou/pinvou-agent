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
use crate::features::projects::{
    DeleteProjectReport, EnsureFolderOutcome, MoveSessionOutcome, Project, ProjectStore,
    SessionAssignments,
};
use crate::features::sessions::SessionStore;

use super::sessions::ensure_chat_session;

/// 项目事件只走本地 emit:projects 域按 bridge 契约是桌面端专属,
/// remote-control 的转发白名单没有该事件,转发只会被中继拒绝并每次
/// 刷一条拒绝日志(评审 #447 finding 11:在消费方出现前不转发)。
fn emit_project_event(app: &AppHandle, event: &str, action: &str) {
    let _ = app.emit(event, serde_json::json!({ "action": action }));
    // 只走桌面 webview 通道。projects 域桌面独占(Web 桥整域缺席),远程端
    // 正式支持项目列表之前不转发——与 remote_control 对代码会话事件的
    // 同类裁决一致;转发不在 RUST_FORWARDED_EVENTS 白名单内会被拒并刷日志。
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
    /// `Some("folder")` = 按文件夹自动物化的项目(前端徽标);None = 手工。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
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
            origin: project.origin.clone(),
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
        // 统一工作区探测(跨模式融合):代码/ACP 会话走 agent 记录;普通绑定
        // 会话回落到双根信号——执行根≠账本根 ⇒ 已绑定,执行根即绑定目录
        // (#445 的绑定语义)。agent 记录存在但非项目形态(如临时)与记录缺失
        // (Err)两种缺席模式都穿透到同一回退,不让 Ok(Temporary) 短路成错误
        // (评审 #452 finding 4)。
        let detected = match acp_pool.workspace_info(&session_id) {
            Ok(info) if info.workspace_kind == CodexWorkspaceKind::Project => {
                Some(PathBuf::from(info.workspace_path))
            }
            _ => sessions
                .session_roots(&session_id)
                .ok()
                .filter(|roots| roots.execution != roots.ledger)
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

/// 文件夹项目自动物化(Codex 客户端式收编):roots 由前端从会话列表的
/// 工作区聚合(客户端驱动,与 Codex `project/import` 由桌面端发起同构)。
/// 幂等:覆盖复用 / 墓碑跳过 / 冲突逐根上报;有新建才广播列表变更。
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

/// rebind_workspace_root 的结果汇报:逐会话结果 + 受影响项目。重绑定幂等,
/// 失败项可直接重试(已成功的部分重跑为空操作)。
#[derive(Debug, Clone, Serialize)]
pub struct RebindWorkspaceReport {
    pub rebound_session_ids: Vec<String>,
    pub failed_session_ids: Vec<String>,
    pub affected_project_ids: Vec<String>,
    /// 迁移完成后复查发现已进入活跃回合的会话:它们的绑定已平移,但回合
    /// 可能仍对着旧目录执行,前端据此提示必要时空闲后重试一次。
    #[serde(default)]
    pub post_busy_session_ids: Vec<String>,
}

/// 目录重绑定(修断链):项目文件夹被物理移走/删除后,把一切以 `from` 为
/// 前缀的绑定——项目 root、会话工作区(索引/sidecar/元数据三处)、归属
/// 派生——整体平移到 `to`。与"移动归属"不同,这是物理层写操作,故有栅栏:
/// - `to` 必须存在且是目录(经 validate_codex_project_workspace 校验);
/// - 旧目录 `from` 仍存在时须 `confirm_existing = true`(前端已强确认);
/// - 受影响会话任一有活跃回合(ACP prompt 或原生 Engine turn)即整体拒绝;
/// - 平移后项目 root 不得与其它项目重叠,违者整体报错回滚。
/// transcript 里的历史路径不改写;workspace baseline 逐会话重采集(失败仅
/// 记日志,baseline 可再派生)。
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
    // `to` 不得位于 `from` 之内:重绑定按前缀平移,目标在旧目录内部时重跑会
    // 不断加深 (/a/x → /a/x/new/x → …),幂等性被破坏(评审 #451 finding 6)。
    {
        let from_canon = std::fs::canonicalize(&from).unwrap_or_else(|_| from.clone());
        let from_key = crate::platform::os::filesystem_path_identity_key(
            &crate::platform::os::platform_compat_path(&from_canon.to_string_lossy())
                .to_string_lossy(),
        );
        let to_key_str =
            crate::platform::os::filesystem_path_identity_key(&to_key.to_string_lossy());
        let from_trim = from_key.trim_end_matches('/');
        let nested = to_key_str.trim_end_matches('/') == from_trim
            || to_key_str
                .trim_end_matches('/')
                .starts_with(&format!("{from_trim}/"));
        if !from_trim.is_empty() && nested {
            return Err(
                "rebind_workspace_root: 新目录不能位于旧目录内部（会造成递归加深）".to_string(),
            );
        }
    }
    // 旧目录仍在磁盘上 = 非断链场景,要求显式强确认。错误以稳定标记前缀
    // 表达类型,前端据此升级强警告,不匹配人类文案(finding 11)。
    if from.is_dir() && !confirm_existing.unwrap_or(false) {
        return Err(
            "REBIND_OLD_ROOT_EXISTS: 原目录仍存在，需在界面确认后重试 (original folder still exists)"
                .to_string(),
        );
    }

    // 活跃回合栅栏:受影响会话任一在跑 prompt/turn 就拒绝,等空闲后重试。
    // 候选集跨两类绑定存储:agent 记录(代码/ACP) + 普通会话绑定 sidecar。
    let mut guard_candidates = acp_pool.agents().sessions_under_workspace(&from);
    guard_candidates.extend(sessions.workspace_bindings_under(&from));
    let mut busy_ids = Vec::new();
    for (session_id, _) in &guard_candidates {
        if acp_pool.is_turn_active(session_id).await || engines.is_turn_active(session_id) {
            busy_ids.push(session_id.clone());
        }
    }
    if !busy_ids.is_empty() {
        return Err(format!(
            "rebind_workspace_root: 会话正在运行，稍后重试: {}",
            busy_ids.join(", ")
        ));
    }

    // 顺序:项目 root → 会话绑定(索引+sidecar,两类存储) → 元数据 → baseline。
    // 每步幂等,失败重试只补未完成部分。
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
    let plain_rebound = sessions
        .rebind_workspace_bindings(&from, &to_key)
        .map_err(|e| format!("rebind_workspace_root: {e:#}"))?;
    let mut rebound_session_ids = Vec::new();
    let mut failed_session_ids = Vec::new();
    for (session_id, new_path) in code_rebound.iter().chain(plain_rebound.iter()) {
        // 孤儿 sidecar(会话 JSON 已不存在)没有元数据可写,按成功计。
        if sessions.load(session_id).is_err() {
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
        // baseline 重采集只对代码会话:普通会话不消费 workspace baseline,
        // 不为它们创建代码车道专属 sidecar。best-effort,失败仅记日志。
        if code_rebound_ids.contains(session_id.as_str()) {
            if let Err(error) =
                crate::features::codex_acp::workspace::capture_baseline(session_id, new_path)
            {
                eprintln!("[projects] rebind capture_baseline({session_id}) failed: {error:#}");
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
    // 迁移后忙碌复查(finding 5):入口栅栏与多文件迁移不是互斥区,回合可能
    // 在迁移期间启动、对着旧目录执行。绑定已平移,这里只如实上报,前端提示
    // 必要时空闲后重试一次。
    let mut post_busy_session_ids: Vec<String> = Vec::new();
    for (session_id, _) in &guard_candidates {
        if acp_pool.is_turn_active(session_id).await || engines.is_turn_active(session_id) {
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
