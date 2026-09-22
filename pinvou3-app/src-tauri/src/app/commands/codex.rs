//! 主页“代码”模式使用的 Codex ACP Tauri 命令。
//!
//! 这里只保留传输边界与会话元数据编排；Codex 进程、ACP 协议、权限和事件适配
//! 均由 `features::codex_acp` 领域模块负责。

use anyhow::Context;
use deepseek_tui::session_manager::SessionMetadata;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::State;

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::codex_acp::reader_window::{self, ReaderOpenRequest};
use crate::features::codex_acp::workspace::{
    self, WorkspaceBranches, WorkspaceChanges, WorkspaceDiff, WorkspaceEntry, WorkspaceListing,
    WorkspacePreview,
};
use crate::features::codex_acp::{
    AcpAgentDescriptor, AcpEventEnvelope, AcpPool, AgentBackend, CodexAcpPendingElicitation,
    CodexAcpPendingPermission, CodexAcpSessionInfo, CodexAcpStatus, CodexAcpWorkspaceInfo,
    CodexWorkspaceKind, SessionAgentStore, validate_codex_project_workspace,
};
use crate::features::projects::ProjectStore;
use crate::features::sessions::{SessionKind, SessionStore};

#[derive(Debug, Clone, Serialize)]
pub struct CodexAcpSessionListItem {
    #[serde(flatten)]
    pub metadata: SessionMetadata,
    pub pinned: bool,
    pub pinned_at: Option<String>,
    #[serde(flatten)]
    pub workspace: CodexAcpWorkspaceInfo,
    /// 钥匙串快照(§6):从 codex-acp 权威存储投影(session-agents 记录,原生
    /// 代码会话与 ACP 会话分别经 `bind_code_native_session` / `set_acp_workspace`
    /// 写入;普通 SessionStore 的 workspace-binding.json sidecar 作兜底,与
    /// lib.rs 注入引擎的 resolver 同一取数顺序)。metadata 里那份只有 fork
    /// 原生快照流才写。空 = 单根语义(仅 cwd)。Web 投影会把它降级为末级目录名,
    /// 与 workspace.workspace_path 同一套主机路径纪律。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub workspace_roots: Vec<String>,
    pub agent_id: String,
    pub agent_name: String,
}

fn ensure_codex_workspace_root(
    kind: CodexWorkspaceKind,
    path: &std::path::Path,
) -> anyhow::Result<()> {
    if kind == CodexWorkspaceKind::Temporary {
        std::fs::create_dir_all(path)
            .with_context(|| format!("创建 Codex 临时工作目录失败: {}", path.display()))?;
    }
    Ok(())
}

pub(crate) type WorkspaceBindingVerifier<'a> =
    dyn Fn(&Path) -> Result<(), String> + Send + Sync + 'a;

fn verify_workspace_binding(
    project_workspace: Option<&Path>,
    verifier: Option<&WorkspaceBindingVerifier<'_>>,
) -> Result<(), String> {
    let Some(verifier) = verifier else {
        return Ok(());
    };
    let workspace = project_workspace
        .ok_or_else(|| "Authorized workspace binding requires a project directory".to_string())?;
    verifier(workspace)
}

fn rollback_created_code_session(session_id: &str, store: &SessionStore, acp_pool: &AcpPool) {
    if let Err(error) = acp_pool.agents().remove(session_id) {
        log::warn!("[codex_acp] rollback Session Agent binding {session_id} failed: {error:#}");
    }
    if let Err(error) = store.delete(session_id) {
        log::warn!("[codex_acp] rollback Session {session_id} failed: {error:#}");
    }
}

#[tauri::command]
pub async fn list_acp_agents(
    _acp_pool: State<'_, AcpPool>,
) -> Result<Vec<AcpAgentDescriptor>, String> {
    list_acp_agents_for_pool().await
}

/// The agent catalog is static; the pool handle is no longer consulted.
pub(crate) async fn list_acp_agents_for_pool() -> Result<Vec<AcpAgentDescriptor>, String> {
    Ok(AcpPool::agent_catalog())
}

#[tauri::command]
pub async fn get_acp_agent_status(
    agent_id: String,
    recheck: Option<bool>,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpStatus, String> {
    get_acp_agent_status_for_pool(agent_id, recheck, &acp_pool).await
}

pub(crate) async fn get_acp_agent_status_for_pool(
    agent_id: String,
    recheck: Option<bool>,
    acp_pool: &AcpPool,
) -> Result<CodexAcpStatus, String> {
    // recheck=true 时忽略探测缓存强制重探测：用户在 App 外手动安装/升级
    // CLI 后点击「重新检测」必须拿到最新状态。轮询调用不传，保持读缓存。
    if recheck.unwrap_or(false) {
        let pool = acp_pool.clone();
        return pool
            .recheck_agent_status(&agent_id)
            .await
            .map_err(|error| format!("重新检测 ACP Agent 状态失败: {error:#}"));
    }
    acp_pool
        .status_for_agent(&agent_id)
        .await
        .map_err(|error| format!("读取 ACP Agent 状态失败: {error:#}"))
}

/// 统一的 ACP Agent 安装入口：按 status.install_action 分派（官方脚本或原来源
/// brew/npm 升级），完成后返回最新状态。action 提供时经合法性校验后优先。
#[tauri::command]
pub async fn install_acp_agent(
    agent: String,
    action: Option<String>,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpStatus, String> {
    acp_pool
        .install_agent(&agent, action.as_deref())
        .await
        .map_err(|error| format!("安装 ACP Agent 失败: {error:#}"))
}

#[tauri::command]
pub async fn login_acp_agent(
    agent_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpStatus, String> {
    acp_pool
        .login_agent(&agent_id)
        .await
        .map_err(|error| format!("登录 ACP Agent 失败: {error:#}"))
}

#[tauri::command]
pub async fn switch_acp_agent_account(
    agent_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpStatus, String> {
    acp_pool
        .switch_agent_account(&agent_id)
        .await
        .map_err(|error| format!("切换 ACP Agent 账号失败: {error:#}"))
}

#[tauri::command]
pub fn open_acp_agent_login_url(
    agent_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    acp_pool
        .open_agent_login_url(&agent_id)
        .map_err(|error| format!("打开 ACP Agent 授权页面失败: {error:#}"))
}

#[tauri::command]
pub async fn submit_acp_agent_login_code(
    agent_id: String,
    code: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    acp_pool
        .submit_agent_login_code(&agent_id, &code)
        .await
        .map_err(|error| format!("提交 ACP Agent 授权码失败: {error:#}"))
}

#[tauri::command]
pub async fn get_codex_acp_session_info(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpSessionInfo, String> {
    acp_pool
        .session_info(&session_id)
        .await
        .map_err(|error| format!("读取 Codex ACP 会话信息失败: {error:#}"))
}

#[tauri::command]
pub async fn set_codex_acp_model(
    session_id: String,
    model_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpSessionInfo, String> {
    acp_pool
        .set_model(&session_id, &model_id)
        .await
        .map_err(|error| format!("切换 Codex 模型失败: {error:#}"))
}

/// 发送未经 Pinvou skill、persona 或知识库 prompt 注入的原始用户消息。
#[tauri::command]
pub async fn codex_acp_prompt(
    session_id: String,
    message: String,
    attachments: Option<Vec<crate::features::files::file_ingest::IngestResult>>,
    workspace_references: Option<Vec<String>>,
    store: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    codex_acp_prompt_with_attachments(
        session_id,
        message,
        attachments.unwrap_or_default(),
        workspace_references.unwrap_or_default(),
        &store,
        &acp_pool,
    )
    .await
}

pub(crate) async fn codex_acp_prompt_with_attachments(
    session_id: String,
    message: String,
    attachments: Vec<crate::features::files::file_ingest::IngestResult>,
    workspace_references: Vec<String>,
    store: &SessionStore,
    acp_pool: &AcpPool,
) -> Result<(), String> {
    let message = message.trim().to_string();
    if message.is_empty() && attachments.is_empty() && workspace_references.is_empty() {
        return Err("empty message".to_string());
    }
    if !acp_pool.is_acp(&session_id) {
        return Err("当前会话不是 ACP 会话".to_string());
    }
    let title_source = if message.is_empty() {
        attachments
            .first()
            .map(|attachment| attachment.basename.as_str())
            .or_else(|| workspace_references.first().map(String::as_str))
            .unwrap_or("附件")
    } else {
        message.as_str()
    };
    super::sessions::apply_default_session_title(store, &session_id, title_source)?;
    // Timing registration lives inside `AcpPool::send_message`, after busy
    // admission succeeds but before the prompt task is spawned: the spawned
    // turn can complete before `send_message` returns, so registering here
    // (after the await) would race that finish and drop the completion.
    acp_pool
        .send_message(&session_id, message, attachments, workspace_references)
        .await
        .map_err(|error| format!("ACP Agent send failed: {error:#}"))?;
    Ok(())
}

// 会话内浏览走 session_id（解析会话工作区并校验可用性）；
// 会话前（draft）浏览直接校验并规范化调用方给出的项目路径，无需 ACP 会话。
fn codex_workspace_root(
    session_id: Option<&str>,
    workspace_path: Option<&str>,
    acp_pool: &AcpPool,
) -> Result<std::path::PathBuf, String> {
    if let Some(session_id) = session_id.filter(|id| !id.is_empty()) {
        // 原生代码会话与 ACP 会话一样可以在会话内浏览工作区（workspace_info 已支持）。
        if !acp_pool.is_acp(session_id) && !acp_pool.agents().is_code_session(session_id) {
            return Err("当前会话不是 ACP 会话".to_string());
        }
        let info = acp_pool
            .workspace_info(session_id)
            .map_err(|error| format!("读取 Codex 工作目录失败: {error:#}"))?;
        if !info.workspace_available {
            return Err(format!("Codex 工作目录不可用: {}", info.workspace_path));
        }
        return Ok(std::path::PathBuf::from(info.workspace_path));
    }
    if let Some(path) = workspace_path.filter(|path| !path.trim().is_empty()) {
        return validate_codex_project_workspace(std::path::Path::new(path))
            .map_err(|error| format!("Codex 工作目录不可用: {error:#}"));
    }
    Err("缺少会话或工作区路径".to_string())
}

#[tauri::command]
pub async fn list_codex_workspace(
    session_id: Option<String>,
    relative_path: Option<String>,
    workspace_path: Option<String>,
    acp_pool: State<'_, AcpPool>,
) -> Result<WorkspaceListing, String> {
    let root = codex_workspace_root(session_id.as_deref(), workspace_path.as_deref(), &acp_pool)?;
    tauri::async_runtime::spawn_blocking(move || {
        workspace::list_workspace(&root, relative_path.as_deref())
            .map_err(|error| format!("读取 Codex 工作区失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("读取 Codex 工作区任务失败: {error}"))?
}

#[tauri::command]
pub async fn search_codex_workspace(
    session_id: Option<String>,
    query: String,
    workspace_path: Option<String>,
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<WorkspaceEntry>, String> {
    let root = codex_workspace_root(session_id.as_deref(), workspace_path.as_deref(), &acp_pool)?;
    tauri::async_runtime::spawn_blocking(move || {
        workspace::search_workspace(&root, &query)
            .map_err(|error| format!("搜索 Codex 工作区失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("搜索 Codex 工作区任务失败: {error}"))?
}

#[tauri::command]
pub async fn preview_codex_workspace_file(
    session_id: Option<String>,
    relative_path: String,
    workspace_path: Option<String>,
    acp_pool: State<'_, AcpPool>,
) -> Result<WorkspacePreview, String> {
    let root = codex_workspace_root(session_id.as_deref(), workspace_path.as_deref(), &acp_pool)?;
    tauri::async_runtime::spawn_blocking(move || {
        workspace::preview_workspace_file(&root, &relative_path)
            .map_err(|error| format!("预览 Codex 工作区文件失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("预览 Codex 工作区文件任务失败: {error}"))?
}

#[tauri::command]
pub async fn open_codex_workspace_resource(
    session_id: String,
    resource_path: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    let root = codex_workspace_root(Some(&session_id), None, &acp_pool)?;
    let path = workspace::resolve_workspace_resource(&root, &resource_path)
        .map_err(|error| format!("打开 Codex 工作区资源失败: {error:#}"))?;
    crate::platform::os::open_target(
        crate::platform::os::external_application_path(&path),
        "Codex 工作区资源",
    )
}

#[tauri::command]
pub async fn get_codex_workspace_changes(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<WorkspaceChanges, String> {
    let root = codex_workspace_root(Some(&session_id), None, &acp_pool)?;
    tauri::async_runtime::spawn_blocking(move || {
        workspace::workspace_changes(&session_id, &root)
            .map_err(|error| format!("读取 Codex 工作区更改失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("读取 Codex 工作区更改任务失败: {error}"))?
}

#[tauri::command]
pub async fn get_codex_workspace_diff(
    session_id: String,
    relative_path: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<WorkspaceDiff, String> {
    let root = codex_workspace_root(Some(&session_id), None, &acp_pool)?;
    tauri::async_runtime::spawn_blocking(move || {
        workspace::workspace_diff(&session_id, &root, &relative_path)
            .map_err(|error| format!("读取 Codex 文件差异失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("读取 Codex 文件差异任务失败: {error}"))?
}

#[tauri::command]
pub async fn list_codex_workspace_branches(
    session_id: Option<String>,
    workspace_path: Option<String>,
    acp_pool: State<'_, AcpPool>,
) -> Result<WorkspaceBranches, String> {
    let root = codex_workspace_root(session_id.as_deref(), workspace_path.as_deref(), &acp_pool)?;
    tauri::async_runtime::spawn_blocking(move || {
        workspace::workspace_branches(&root)
            .map_err(|error| format!("读取 Codex 工作区分支失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("读取 Codex 工作区分支任务失败: {error}"))?
}

#[tauri::command]
pub async fn checkout_codex_workspace_branch(
    session_id: Option<String>,
    workspace_path: Option<String>,
    branch: String,
    mode: String,
    commit_message: Option<String>,
    acp_pool: State<'_, AcpPool>,
) -> Result<WorkspaceBranches, String> {
    let root = codex_workspace_root(session_id.as_deref(), workspace_path.as_deref(), &acp_pool)?;
    let mode = workspace::BranchSwitchMode::parse(&mode)
        .map_err(|error| format!("切换 Codex 工作区分支失败: {error:#}"))?;
    // Cross-session guard: reject the switch while any session bound to this
    // workspace (including sessions other than the caller) has a turn in
    // flight, so a running agent cannot land later edits on the wrong branch.
    // The frontend only gates the current session; this backend check consults
    // the shared assistant::timing in-flight registry for both ACP and native
    // code sessions. Best-effort: a turn admitted after the in-task recheck
    // below can still race the git commands.
    let workspace_sessions = acp_pool.agents().code_sessions_in_workspace(&root);
    let running = workspace_sessions
        .iter()
        .filter(|session_id| crate::features::assistant::timing::has_active_turn(session_id))
        .count();
    if running > 0 {
        return Err(format!(
            "该工作区有 {running} 个会话正在运行，请等待运行结束后再切换分支"
        ));
    }
    tauri::async_runtime::spawn_blocking(move || {
        // Recheck inside the task to narrow the admission window between the
        // outer check and the first git mutation: a turn started in between
        // would otherwise run concurrently with the checkout.
        let running = workspace_sessions
            .iter()
            .filter(|session_id| crate::features::assistant::timing::has_active_turn(session_id))
            .count();
        if running > 0 {
            return Err(format!(
                "该工作区有 {running} 个会话正在运行，请等待运行结束后再切换分支"
            ));
        }
        workspace::checkout_workspace_branch(&root, &branch, mode, commit_message.as_deref())
            .map_err(|error| format!("切换 Codex 工作区分支失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("切换 Codex 工作区分支任务失败: {error}"))?
}

#[tauri::command]
pub async fn open_codex_workspace_file(
    session_id: Option<String>,
    relative_path: String,
    workspace_path: Option<String>,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    let root = codex_workspace_root(session_id.as_deref(), workspace_path.as_deref(), &acp_pool)?;
    let path = workspace::resolve_workspace_file(&root, &relative_path)
        .map_err(|error| format!("打开 Codex 工作区文件失败: {error:#}"))?;
    crate::platform::os::open_target(
        crate::platform::os::external_application_path(&path),
        "Codex 工作区文件",
    )
}

#[tauri::command]
pub async fn reveal_codex_workspace_file(
    session_id: Option<String>,
    relative_path: String,
    workspace_path: Option<String>,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    let root = codex_workspace_root(session_id.as_deref(), workspace_path.as_deref(), &acp_pool)?;
    let path = workspace::resolve_workspace_file(&root, &relative_path)
        .map_err(|error| format!("定位 Codex 工作区文件失败: {error:#}"))?;
    let directory = path
        .parent()
        .ok_or_else(|| format!("文件没有父目录: {}", path.display()))?;
    crate::platform::os::open_target(
        crate::platform::os::external_application_path(directory),
        "Codex 工作区目录",
    )
}

// 代码弹窗「新窗口打开」：校验工作区可读且目标文件存在，再交给单例阅读器窗口（tab 复用见 reader_window）。
// `kind="diff"` 时打开工作区变更差异（依赖会话基线，需 sessionId；文件可能已删除，放宽存在性校验）。
#[tauri::command]
pub async fn open_code_reader(
    session_id: Option<String>,
    workspace_path: Option<String>,
    relative_path: String,
    kind: Option<String>,
    app: tauri::AppHandle,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    let root = codex_workspace_root(session_id.as_deref(), workspace_path.as_deref(), &acp_pool)?;
    if kind.as_deref() == Some("diff") {
        if session_id.is_none() {
            return Err("打开代码阅读器失败: 差异预览需要会话。".to_string());
        }
        workspace::validate_workspace_relative_path(&root, &relative_path)
            .map_err(|error| format!("打开代码阅读器失败: {error:#}"))?;
    } else {
        workspace::resolve_workspace_file(&root, &relative_path)
            .map_err(|error| format!("打开代码阅读器失败: {error:#}"))?;
    }
    reader_window::open_code_reader(
        &app,
        ReaderOpenRequest {
            session_id,
            workspace_path,
            relative_path,
            kind,
        },
    )
}

// ReaderApp 挂载后拉取建窗前排队的打开请求（拉模式，规避窗口加载时序竞态）。
#[tauri::command]
pub async fn take_code_reader_pending() -> Result<Vec<ReaderOpenRequest>, String> {
    Ok(reader_window::take_pending_open())
}

#[tauri::command]
pub async fn set_codex_acp_mode(
    session_id: String,
    mode_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpSessionInfo, String> {
    acp_pool
        .set_mode(&session_id, &mode_id)
        .await
        .map_err(|error| format!("切换 Codex 权限模式失败: {error:#}"))
}

#[tauri::command]
pub async fn set_codex_acp_config_option(
    session_id: String,
    config_id: String,
    value_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpSessionInfo, String> {
    acp_pool
        .set_config_option(&session_id, &config_id, &value_id)
        .await
        .map_err(|error| format!("切换 Codex 配置失败: {error:#}"))
}

#[tauri::command]
pub async fn cancel_codex_acp(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    if !acp_pool.is_acp(&session_id) {
        return Err("当前会话不是 Codex ACP 会话".to_string());
    }
    acp_pool.cancel(&session_id).await;
    Ok(())
}

#[tauri::command]
pub async fn get_codex_acp_timeline(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<AcpEventEnvelope>, String> {
    acp_pool
        .timeline(&session_id)
        .await
        .map_err(|error| format!("读取 Codex ACP timeline 失败: {error:#}"))
}

#[tauri::command]
pub async fn get_codex_acp_pending_permissions(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<CodexAcpPendingPermission>, String> {
    if !acp_pool.is_acp(&session_id) {
        return Err("当前会话不是 Codex ACP 会话".to_string());
    }
    Ok(acp_pool.pending_permissions_for(&session_id).await)
}

#[tauri::command]
pub async fn respond_codex_acp_permission(
    session_id: String,
    tool_call_id: String,
    option_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    acp_pool
        .respond_permission(&session_id, &tool_call_id, &option_id)
        .await
        .map_err(|error| format!("回复 Codex ACP 权限失败: {error:#}"))
}

#[tauri::command]
pub async fn get_codex_acp_pending_elicitations(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<CodexAcpPendingElicitation>, String> {
    if !acp_pool.is_acp(&session_id) {
        return Err("当前会话不是 Codex ACP 会话".to_string());
    }
    Ok(acp_pool.pending_elicitations_for(&session_id).await)
}

#[tauri::command]
pub async fn respond_codex_acp_elicitation(
    session_id: String,
    elicitation_id: String,
    action: String,
    content: serde_json::Value,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    acp_pool
        .respond_elicitation(&session_id, &elicitation_id, &action, content)
        .await
        .map_err(|error| format!("回复 Codex ACP 输入请求失败: {error:#}"))
}

/// 列表项的 agent_id：ACP 后端使用各自 id；原生（品悟）代码会话固定为 "pinvou"。
fn code_session_agent_id(backend: AgentBackend) -> String {
    backend.agent_id().unwrap_or("pinvou").to_string()
}

/// 返回 Codex 会话，供主页左侧统一会话列表与代码模式共同消费。
#[tauri::command]
pub async fn list_codex_acp_sessions(
    store: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<CodexAcpSessionListItem>, String> {
    let mut metas = store
        .list()
        .map_err(|error| format!("list_codex_acp_sessions: {error:#}"))?;
    metas.retain(|metadata| {
        matches!(store.session_kind(&metadata.id), Ok(SessionKind::Chat))
            && (acp_pool.is_acp_metadata(metadata)
                || acp_pool.agents().is_code_session(&metadata.id))
            && !store.is_hidden(&metadata.id)
    });
    metas
        .into_iter()
        .map(|metadata| {
            let backend = acp_pool.backend(&metadata.id);
            let workspace = acp_pool
                .workspace_info(&metadata.id)
                .map_err(|error| format!("读取代码会话 {} 工作目录失败: {error:#}", metadata.id))?;
            let workspace_roots = project_keychain_roots(acp_pool.agents(), &store, &metadata.id);
            Ok(CodexAcpSessionListItem {
                pinned: store.is_pinned(&metadata.id),
                pinned_at: store.pinned_at(&metadata.id),
                metadata,
                workspace,
                workspace_roots,
                agent_id: code_session_agent_id(backend),
                agent_name: backend.display_name().to_string(),
            })
        })
        .collect()
}

/// 钥匙串投影(§6):codex-acp 权威存储优先(session-agents 记录,原生代码
/// 会话与 ACP 会话都经 `bind_code_native_session` / `set_acp_workspace` 写入),
/// 普通 SessionStore 的 workspace-binding.json sidecar 兜底——与 lib.rs 注入
/// 引擎的 workspace_roots resolver 同一取数顺序,保证列表投影与引擎实际生效
/// 的钥匙串一致(评审 #484 round-7 M1:此前只读普通 store,真实代码会话的
/// workspace_roots 恒为空,前端钥匙串 chip 与 Web 脱敏全是死代码)。
/// 空 = 单根语义(临时会话/无快照 → 空 vec,调用方按 cwd 归一)。
/// 单独成函数,让 store→列表项的接线可被变异测试钉住(评审 #484 round-4
/// minor 5:手工构造 roots 的 Web 脱敏测试测不出「投影被清空」这类回归)。
fn project_keychain_roots(
    agents: &SessionAgentStore,
    store: &SessionStore,
    session_id: &str,
) -> Vec<String> {
    let roots = agents.session_workspace_roots(session_id);
    let roots = if roots.is_empty() {
        store.session_workspace_roots(session_id)
    } else {
        roots
    };
    roots
        .iter()
        .map(|path| path.display().to_string())
        .collect()
}

/// 把主机工作区路径降级为最末一级目录名，避免向 WebUI 泄漏绝对路径。
/// 桌面端需要完整路径（历史记录、系统打开等），故投影只在 Web 入口应用。
pub(crate) fn redact_workspace_path_for_web(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty() && !name.ends_with(':'))
        .unwrap_or("workspace")
        .to_string()
}

/// Remove host-specific path components from Session metadata before it
/// crosses the Web/Relay boundary. Native callers keep the original metadata.
pub(crate) fn redact_session_metadata_for_web(mut metadata: SessionMetadata) -> SessionMetadata {
    redact_session_metadata_for_web_in_place(&mut metadata);
    metadata
}

fn redact_session_metadata_for_web_in_place(metadata: &mut SessionMetadata) {
    metadata.workspace = std::path::PathBuf::from(redact_workspace_path_for_web(
        &metadata.workspace.to_string_lossy(),
    ));
    // The foundation's attached-roots field (workspace_roots gitlink) is
    // host-absolute paths too: degrade each root at the same choke point, or
    // the web boundary leaks the host directory structure through the field
    // the redaction predated (review #484 round-8 m12).
    for root in metadata.workspace_roots.iter_mut() {
        *root = std::path::PathBuf::from(redact_workspace_path_for_web(&root.to_string_lossy()));
    }
}

fn redact_codex_session_list_item_for_web(item: &mut CodexAcpSessionListItem) {
    redact_session_metadata_for_web_in_place(&mut item.metadata);
    item.workspace.workspace_path = redact_workspace_path_for_web(&item.workspace.workspace_path);
    // The keychain snapshot is host-absolute paths too: degrade each root to
    // its last directory name before the web boundary (same discipline as
    // workspace.workspace_path; the web lane does not group by it).
    for root in item.workspace_roots.iter_mut() {
        *root = redact_workspace_path_for_web(root);
    }
}

/// Web 版代码会话列表：复用桌面端列表逻辑，但把工作区路径投影为目录名，
/// 避免向浏览器暴露主机绝对路径。前端通过 `web_access_list_codex_acp_sessions`
/// 调用，桌面端继续使用返回完整路径的 `list_codex_acp_sessions`。
pub async fn list_codex_acp_sessions_for_web(
    store: &SessionStore,
    acp_pool: &AcpPool,
) -> Result<Vec<CodexAcpSessionListItem>, String> {
    let mut items: Vec<CodexAcpSessionListItem> = store
        .list()
        .map_err(|error| format!("list_codex_acp_sessions: {error:?}"))?
        .into_iter()
        .filter(|metadata| {
            matches!(store.session_kind(&metadata.id), Ok(SessionKind::Chat))
                && acp_pool.is_acp_metadata(metadata)
                && !store.is_hidden(&metadata.id)
        })
        .map(|metadata| {
            let backend = acp_pool.backend(&metadata.id);
            let workspace = acp_pool
                .workspace_info(&metadata.id)
                .map_err(|error| format!("读取代码会话 {} 工作目录失败: {error:#}", metadata.id))?;
            let workspace_roots = project_keychain_roots(acp_pool.agents(), &store, &metadata.id);
            Ok(CodexAcpSessionListItem {
                pinned: store.is_pinned(&metadata.id),
                pinned_at: store.pinned_at(&metadata.id),
                metadata,
                workspace,
                workspace_roots,
                agent_id: code_session_agent_id(backend),
                agent_name: backend.display_name().to_string(),
            })
        })
        .collect::<Result<Vec<CodexAcpSessionListItem>, String>>()?;
    for item in &mut items {
        redact_codex_session_list_item_for_web(item);
    }
    Ok(items)
}

#[tauri::command]
pub async fn create_codex_acp_session(
    workspace_path: Option<String>,
    agent_id: Option<String>,
    workspace_roots: Option<Vec<String>>,
    project_id: Option<String>,
    app: tauri::AppHandle,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    acp_pool: State<'_, AcpPool>,
    projects: State<'_, ProjectStore>,
) -> Result<SessionMetadata, String> {
    // record_project_choice 写 tier-1 归属发生在内部实现里且失败只记日志;
    // 创建成功且携带项目归属时广播列表变更(罕见的写失败会让这次广播成为
    // 一次无害的额外刷新),否则前端 assignments 快照滞留,侧栏按 tier-2 把
    // 新会话归进宽项目(与 chat 车道 create_session 同一口径)。
    let expects_assignment = workspace_path.is_some() && project_id.is_some();
    let metadata = create_codex_acp_session_with_workspace_binding(
        workspace_path.map(PathBuf::from),
        agent_id,
        workspace_roots,
        project_id,
        store,
        pool,
        acp_pool,
        projects,
        None,
    )
    .await?;
    if expects_assignment {
        super::projects::emit_create_channel_assignment_event(&app);
    }
    Ok(metadata)
}

/// Internal code-Session creation entry point used by Web workspace grants.
/// The public desktop command remains path-based; only the Web bridge injects
/// an identity verifier. The verifier is deliberately lock-free and is run
/// before persistence, after Agent/workspace binding, and after baseline
/// capture so a replaced directory causes the newly-created Session to be
/// rolled back instead of becoming durable.
pub(crate) async fn create_codex_acp_session_with_workspace_binding(
    workspace_path: Option<PathBuf>,
    agent_id: Option<String>,
    workspace_roots: Option<Vec<String>>,
    project_id: Option<String>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    acp_pool: State<'_, AcpPool>,
    projects: State<'_, ProjectStore>,
    workspace_verifier: Option<&WorkspaceBindingVerifier<'_>>,
) -> Result<SessionMetadata, String> {
    let backend = AgentBackend::parse(agent_id.as_deref().or(Some("pinvou")))
        .map_err(|error| format!("{error:#}"))?;
    let project_workspace = workspace_path
        .as_deref()
        .map(validate_codex_project_workspace)
        .transpose()
        .map_err(|error| format!("{error:#}"))?;
    // 钥匙串快照(§6):绝对路径硬拒,不存在的附加根软警告保留(与
    // sessions::create_session 同一条校验)。
    let keychain =
        crate::features::sessions::validate_workspace_roots(workspace_roots.unwrap_or_default())
            .map_err(|error| {
                format!("create_codex_acp_session: invalid workspace_roots: {error:#}")
            })?;
    // 临时会话(未提供 workspace_path)没有绑定工作区,钥匙串快照在 store 层
    // 会被强制清空(§9.1 单根语义)。携带非空 roots 是调用方契约错误——前端
    // 只在项目通道把 roots 与 path 一起下发——显式拒绝而不是静默丢弃
    // (评审 #484 round-7 minor;报错风格对齐 sessions::create_session 的
    // workspace_path/workspace_roots 校验)。
    if project_workspace.is_none() && !keychain.is_empty() {
        return Err(
            "create_codex_acp_session: invalid workspace_roots: workspace_roots 必须随 workspace_path 一起提供(临时会话没有钥匙串快照)"
                .to_string(),
        );
    }
    verify_workspace_binding(project_workspace.as_deref(), workspace_verifier)?;
    if !backend.is_acp() {
        let metadata = create_code_native_session(
            project_workspace.clone(),
            keychain,
            &pool,
            &store,
            &acp_pool,
            workspace_verifier,
        )
        .await?;
        // 项目通道(§9.3):创建即更新项目记忆主文件夹(后端同命令内写,免二次
        // RPC;失败只记日志,不影响创建)。写入点在最后一个回滚点之后——失败的
        // 创建不得留下无会话的项目记忆(评审 #484 MINOR)。
        super::projects::record_project_choice(
            projects.inner(),
            &metadata.id,
            project_id.as_deref(),
            project_workspace.as_deref(),
        );
        return Ok(metadata);
    }
    let metadata_workspace = project_workspace
        .clone()
        .unwrap_or_else(|| pool.bridge.workspace.clone());
    let session = store
        .create_new(
            format!("{} (ACP)", backend.display_name()),
            None,
            metadata_workspace,
        )
        .map_err(|error| format!("create_codex_acp_session: {error:#}"))?;
    let kind = if project_workspace.is_some() {
        CodexWorkspaceKind::Project
    } else {
        CodexWorkspaceKind::Temporary
    };
    if kind == CodexWorkspaceKind::Temporary {
        let temporary_workspace = store
            .session_roots(&session.metadata.id)
            .map(|roots| roots.execution)
            .map_err(|error| format!("解析 Codex 临时工作目录失败: {error:#}"))?;
        if let Err(error) = ensure_codex_workspace_root(kind, &temporary_workspace) {
            let _ = store.delete(&session.metadata.id);
            return Err(format!("{error:#}"));
        }
    }
    if let Err(error) = acp_pool.agents().set_acp_workspace(
        &session.metadata.id,
        backend,
        kind,
        project_workspace.clone(),
        keychain.clone(),
    ) {
        rollback_created_code_session(&session.metadata.id, &store, &acp_pool);
        return Err(format!("保存 Codex ACP 会话工作目录失败: {error:#}"));
    }
    if let Err(error) = verify_workspace_binding(project_workspace.as_deref(), workspace_verifier) {
        rollback_created_code_session(&session.metadata.id, &store, &acp_pool);
        return Err(error);
    }
    let baseline_workspace = acp_pool.workspace_info(&session.metadata.id);
    if baseline_workspace.is_err() {
        rollback_created_code_session(&session.metadata.id, &store, &acp_pool);
    }
    let baseline_root = baseline_workspace
        .map_err(|error| format!("读取 Codex ACP 会话工作目录失败: {error:#}"))?
        .workspace_path;
    let baseline_session_id = session.metadata.id.clone();
    if let Err(error) = tauri::async_runtime::spawn_blocking(move || {
        workspace::capture_baseline(&baseline_session_id, std::path::Path::new(&baseline_root))
    })
    .await
    .map_err(|error| anyhow::anyhow!("工作区基线任务失败: {error}"))
    .and_then(|result| result)
    {
        rollback_created_code_session(&session.metadata.id, &store, &acp_pool);
        return Err(format!("创建 Codex 工作区基线失败: {error:#}"));
    }
    if let Err(error) = verify_workspace_binding(project_workspace.as_deref(), workspace_verifier) {
        rollback_created_code_session(&session.metadata.id, &store, &acp_pool);
        return Err(error);
    }
    // 项目通道(§9.3):同上——最后一个回滚点之后才写项目记忆。
    super::projects::record_project_choice(
        projects.inner(),
        &session.metadata.id,
        project_id.as_deref(),
        project_workspace.as_deref(),
    );
    Ok(session.metadata)
}

/// 项目通道记忆(§9.2/§9.3):共享实现见 `super::projects::record_project_choice`
/// (记忆主文件夹 + tier-1 显式归属)。只在会话创建完全落定后调用。

/// 创建“代码”模块原生（品悟 Engine）会话。
///
/// 临时会话执行目录与 ACP 临时会话共用 `SessionStore::session_roots` 推导
/// （两根一致，均为会话私有目录）；项目会话绑定调用方选定的目录（经
/// `validate_codex_project_workspace` 校验），执行根由 resolver 在
/// engine/shell 启动时解析，发消息走现有 `chat` 命令。
async fn create_code_native_session(
    project_workspace: Option<PathBuf>,
    workspace_roots: Vec<PathBuf>,
    pool: &EnginePool,
    store: &SessionStore,
    acp_pool: &AcpPool,
    workspace_verifier: Option<&WorkspaceBindingVerifier<'_>>,
) -> Result<SessionMetadata, String> {
    let kind = if project_workspace.is_some() {
        CodexWorkspaceKind::Project
    } else {
        CodexWorkspaceKind::Temporary
    };
    let metadata_workspace = project_workspace
        .clone()
        .unwrap_or_else(|| pool.bridge.workspace.clone());
    let session = store
        .create_new(
            format!("{} (代码)", AgentBackend::Deepseek.display_name()),
            None,
            metadata_workspace,
        )
        .map_err(|error| format!("create_codex_acp_session: {error:#}"))?;
    if let Err(error) = acp_pool.agents().bind_code_native_session(
        &session.metadata.id,
        kind,
        project_workspace.clone(),
        workspace_roots,
    ) {
        rollback_created_code_session(&session.metadata.id, store, acp_pool);
        return Err(format!("保存原生代码会话标记失败: {error:#}"));
    }
    // 基线根：项目会话用项目目录（capture_baseline 对 git 仓库只指纹 dirty 文件，
    // 与 ACP 项目会话一致）；临时会话先确保私有目录存在。
    if let Err(error) = verify_workspace_binding(project_workspace.as_deref(), workspace_verifier) {
        rollback_created_code_session(&session.metadata.id, store, acp_pool);
        return Err(error);
    }
    // Dispatch on workspace presence consistently with kind: Some is the
    // project session baseline root; None goes to the temporary directory.
    let baseline_root = match &project_workspace {
        Some(root) => root.clone(),
        None => {
            let temporary_workspace = match store
                .session_roots(&session.metadata.id)
                .map(|roots| roots.execution)
            {
                Ok(path) => path,
                Err(error) => {
                    let _ = acp_pool.agents().remove(&session.metadata.id);
                    let _ = store.delete(&session.metadata.id);
                    return Err(format!("解析原生代码会话临时工作目录失败: {error:#}"));
                }
            };
            if let Err(error) =
                ensure_codex_workspace_root(CodexWorkspaceKind::Temporary, &temporary_workspace)
            {
                let _ = acp_pool.agents().remove(&session.metadata.id);
                let _ = store.delete(&session.metadata.id);
                return Err(format!("{error:#}"));
            }
            temporary_workspace
        }
    };
    let baseline_session_id = session.metadata.id.clone();
    if let Err(error) = tauri::async_runtime::spawn_blocking(move || {
        workspace::capture_baseline(&baseline_session_id, &baseline_root)
    })
    .await
    .map_err(|error| anyhow::anyhow!("工作区基线任务失败: {error}"))
    .and_then(|result| result)
    {
        rollback_created_code_session(&session.metadata.id, store, acp_pool);
        return Err(format!("创建代码工作区基线失败: {error:#}"));
    }
    if let Err(error) = verify_workspace_binding(project_workspace.as_deref(), workspace_verifier) {
        rollback_created_code_session(&session.metadata.id, store, acp_pool);
        return Err(error);
    }
    Ok(session.metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_binding_is_fail_closed_and_repeatable() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let checks = AtomicUsize::new(0);
        let verifier = |path: &Path| {
            assert_eq!(path, Path::new("workspace"));
            let attempt = checks.fetch_add(1, Ordering::SeqCst);
            if attempt < 2 {
                Ok(())
            } else {
                Err("workspace identity changed".to_string())
            }
        };
        assert!(verify_workspace_binding(Some(Path::new("workspace")), Some(&verifier)).is_ok());
        assert!(verify_workspace_binding(Some(Path::new("workspace")), Some(&verifier)).is_ok());
        assert_eq!(
            verify_workspace_binding(Some(Path::new("workspace")), Some(&verifier)).unwrap_err(),
            "workspace identity changed"
        );
        assert_eq!(checks.load(Ordering::SeqCst), 3);
        assert!(verify_workspace_binding(None, Some(&verifier)).is_err());
        assert!(verify_workspace_binding(None, None).is_ok());
    }

    #[test]
    fn code_session_agent_id_maps_native_backend_to_pinvou() {
        assert_eq!(code_session_agent_id(AgentBackend::Deepseek), "pinvou");
        assert_eq!(code_session_agent_id(AgentBackend::CodexAcp), "codex");
        assert_eq!(code_session_agent_id(AgentBackend::ClaudeAcp), "claude");
        assert_eq!(code_session_agent_id(AgentBackend::KimiAcp), "kimi");
    }

    #[test]
    fn temporary_workspace_exists_before_baseline_capture() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-workspace-test-{}-temporary",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let workspace = root.join("session").join("workspace");
        ensure_codex_workspace_root(CodexWorkspaceKind::Temporary, &workspace)
            .expect("create temporary Codex workspace");
        assert!(workspace.is_dir());
        std::fs::remove_dir_all(root).expect("cleanup temporary Codex workspace");
    }

    #[test]
    fn redact_workspace_path_for_web_keeps_only_directory_name() {
        // 绝对路径只保留末级目录名，避免向 WebUI 暴露主机目录结构。
        assert_eq!(
            redact_workspace_path_for_web("/Users/asto/Documents/secret-project"),
            "secret-project"
        );
        assert_eq!(
            redact_workspace_path_for_web(r#"C:\Users\asto\Documents\secret-project"#),
            "secret-project"
        );
        // 已是末级名称时原样返回；空串兜底为自身。
        assert_eq!(redact_workspace_path_for_web("repo"), "repo");
        assert_eq!(redact_workspace_path_for_web(""), "");
        assert_eq!(redact_workspace_path_for_web("/"), "workspace");
        assert_eq!(redact_workspace_path_for_web(r#"C:\"#), "workspace");
    }

    fn metadata_with_workspace(workspace: &str) -> SessionMetadata {
        serde_json::from_value(serde_json::json!({
            "id": "session-web-projection",
            "title": "Web projection",
            "created_at": "2026-08-17T00:00:00Z",
            "updated_at": "2026-08-17T00:00:00Z",
            "message_count": 0,
            "total_tokens": 0,
            "model": "test-model",
            "workspace": workspace
        }))
        .expect("valid SessionMetadata fixture")
    }

    #[test]
    fn web_session_projection_removes_absolute_paths_from_serialized_payloads() {
        const PRIVATE_WORKSPACE: &str = "/Users/asto/Documents/secret-project";
        let metadata = redact_session_metadata_for_web(metadata_with_workspace(PRIVATE_WORKSPACE));
        let metadata_json = serde_json::to_value(&metadata).expect("serialize projected metadata");
        assert_eq!(metadata_json["workspace"], "secret-project");
        assert!(!metadata_json.to_string().contains(PRIVATE_WORKSPACE));

        let mut item = CodexAcpSessionListItem {
            metadata: metadata_with_workspace(PRIVATE_WORKSPACE),
            pinned: false,
            pinned_at: None,
            workspace: CodexAcpWorkspaceInfo {
                workspace_kind: CodexWorkspaceKind::Project,
                workspace_path: PRIVATE_WORKSPACE.to_string(),
                workspace_available: true,
            },
            workspace_roots: vec![
                "/Users/asto/Documents/secret-project".to_string(),
                "/Users/asto/Documents/secret-extra".to_string(),
            ],
            agent_id: "codex".to_string(),
            agent_name: "Codex".to_string(),
        };
        redact_codex_session_list_item_for_web(&mut item);
        let item_json = serde_json::to_value(&item).expect("serialize projected list item");
        assert_eq!(item_json["workspace"], "secret-project");
        assert_eq!(item_json["workspace_path"], "secret-project");
        assert!(!item_json.to_string().contains(PRIVATE_WORKSPACE));
        // The keychain snapshot rides the same discipline (review #484 M2):
        // each root degrades to its last directory name.
        assert_eq!(
            item_json["workspace_roots"],
            serde_json::json!(["secret-project", "secret-extra"])
        );
        assert!(!item_json.to_string().contains("secret-extra/.."));
        assert!(!item_json.to_string().contains("/Users/asto"));
    }

    #[test]
    fn code_list_projects_the_keychain_from_the_codex_acp_store() {
        // 变异锁定(评审 #484 round-4 minor 5 / round-7 M1):Web 脱敏测试用的是
        // 手工构造的 roots,若 store→列表项的投影被清空(直接 mutate 成
        // Vec::new())或退回只读普通 SessionStore 的 workspace-binding.json,
        // 那条测试依然全绿。这里走真实代码会话写入路径钉住投影接线:钥匙串
        // 快照只写进 codex-acp 权威存储(set_acp_workspace / bind_code_native_session,
        // 与 create_codex_acp_session_with_workspace_binding 同源),普通
        // SessionStore 里没有这份数据——投影若读错 store,断言立刻红。
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-codex-keychain-projection-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).expect("create test root");
        let store = SessionStore::boot_with_scheduled_root(tmp.join("scheduled"))
            .expect("boot session store");
        let agents = SessionAgentStore::for_test(tmp.join("session-agents.json"));
        let primary = tmp.join("primary");
        let extra = tmp.join("extra");
        std::fs::create_dir_all(&primary).expect("create primary");
        std::fs::create_dir_all(&extra).expect("create extra");
        let keychain = vec![primary.clone(), extra.clone()];
        let new_session = |title: &str| {
            store
                .create_new(title.to_string(), None, std::env::temp_dir())
                .expect("create session")
                .metadata
                .id
        };
        let assert_keychain = |id: &str| {
            let projected = project_keychain_roots(&agents, &store, id);
            assert_eq!(
                projected.len(),
                2,
                "the list item must project the codex-acp store's keychain"
            );
            assert!(
                projected.iter().any(|root| root.ends_with("extra")),
                "{projected:?}"
            );
        };

        // 无记录(纯 chat 会话)= 空。
        let plain = new_session("plain");
        assert!(
            project_keychain_roots(&agents, &store, &plain).is_empty(),
            "no record = empty"
        );

        // ACP 车道:生产写入路径 set_acp_workspace。
        let acp = new_session("acp");
        agents
            .set_acp_workspace(
                &acp,
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Project,
                Some(primary.clone()),
                keychain.clone(),
            )
            .expect("bind ACP workspace");
        assert_keychain(&acp);

        // 原生代码车道:生产写入路径 bind_code_native_session(快照同时落
        // 权威 sidecar,投影经同一条记录读取)。
        let native = new_session("native");
        agents
            .bind_code_native_session(
                &native,
                CodexWorkspaceKind::Project,
                Some(primary.clone()),
                keychain.clone(),
            )
            .expect("bind native code session");
        assert_keychain(&native);

        // 临时会话:写路径强制空快照(§9.1),投影为空。
        let temporary = new_session("temporary");
        agents
            .bind_code_native_session(&temporary, CodexWorkspaceKind::Temporary, None, Vec::new())
            .expect("bind temporary native code session");
        assert!(
            project_keychain_roots(&agents, &store, &temporary).is_empty(),
            "temporary session = empty"
        );

        // 兜底顺序与 lib.rs 引擎 resolver 一致:agents 记录为空时回落到普通
        // SessionStore 的 workspace-binding sidecar。
        store
            .bind_session_workspace_with_roots(&plain, primary.clone(), keychain.clone())
            .expect("bind plain session via sidecar");
        assert_keychain(&plain);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn project_workspace_is_not_created_implicitly() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-codex-workspace-test-{}-project",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let workspace = root.join("missing-project");
        ensure_codex_workspace_root(CodexWorkspaceKind::Project, &workspace)
            .expect("project workspace is caller-validated");
        assert!(!workspace.exists());
    }
}
