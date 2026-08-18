//! 主页“代码”模式使用的 Codex ACP Tauri 命令。
//!
//! 这里只保留传输边界与会话元数据编排；Codex 进程、ACP 协议、权限和事件适配
//! 均由 `features::codex_acp` 领域模块负责。

use anyhow::Context;
use deepseek_tui::session_manager::SessionMetadata;
use serde::Serialize;
use tauri::State;

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::codex_acp::reader_window::{self, ReaderOpenRequest};
use crate::features::codex_acp::workspace::{
    self, WorkspaceChanges, WorkspaceDiff, WorkspaceEntry, WorkspaceListing, WorkspacePreview,
};
use crate::features::codex_acp::{
    validate_codex_project_workspace, AcpAgentDescriptor, AcpEventEnvelope, AcpPool, AgentBackend,
    CodexAcpPendingElicitation, CodexAcpPendingPermission, CodexAcpSessionInfo, CodexAcpStatus,
    CodexAcpWorkspaceInfo, CodexWorkspaceKind,
};
use crate::features::sessions::{SessionKind, SessionStore};

#[derive(Debug, Clone, Serialize)]
pub struct CodexAcpSessionListItem {
    #[serde(flatten)]
    pub metadata: SessionMetadata,
    pub pinned: bool,
    pub pinned_at: Option<String>,
    #[serde(flatten)]
    pub workspace: CodexAcpWorkspaceInfo,
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

#[tauri::command]
pub async fn get_codex_acp_status(acp_pool: State<'_, AcpPool>) -> Result<CodexAcpStatus, String> {
    Ok(acp_pool.refresh_status().await)
}

#[tauri::command]
pub async fn list_acp_agents(
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<AcpAgentDescriptor>, String> {
    list_acp_agents_for_pool(&acp_pool).await
}

pub(crate) async fn list_acp_agents_for_pool(
    _acp_pool: &AcpPool,
) -> Result<Vec<AcpAgentDescriptor>, String> {
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

#[tauri::command]
pub async fn prepare_codex_acp(acp_pool: State<'_, AcpPool>) -> Result<CodexAcpStatus, String> {
    let status = acp_pool.refresh_status().await;
    if !status.bridge_ready {
        return Err(
            "准备 Codex 运行环境失败: Pinvou 安装包缺少可用的 Codex ACP Bridge".to_string(),
        );
    }
    acp_pool
        .install_agent("codex", None)
        .await
        .map_err(|error| format!("准备 Codex 运行环境失败: {error:#}"))
}

#[tauri::command]
pub async fn install_codex_homebrew(
    acp_pool: State<'_, AcpPool>,
) -> Result<CodexAcpStatus, String> {
    acp_pool
        .install_via_homebrew()
        .await
        .map_err(|error| format!("{error:#}"))
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
pub async fn login_codex_acp(acp_pool: State<'_, AcpPool>) -> Result<CodexAcpStatus, String> {
    acp_pool
        .login()
        .await
        .map_err(|error| format!("登录 Codex 失败: {error:#}"))
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
pub fn open_codex_login_url(acp_pool: State<'_, AcpPool>) -> Result<(), String> {
    acp_pool
        .open_login_url()
        .map_err(|error| format!("打开 Codex 授权页面失败: {error:#}"))
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
    super::sessions::apply_default_session_title(&store, &session_id, title_source)?;
    crate::features::assistant::timing::start_turn(&session_id);
    acp_pool
        .send_message(&session_id, message, attachments, workspace_references)
        .await
        .map_err(|error| {
            crate::features::assistant::timing::finish_turn(
                &session_id,
                "send_error",
                Some(&format!("{error:#}")),
            );
            format!("ACP Agent send failed: {error:#}")
        })
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
pub fn get_codex_acp_timeline(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<AcpEventEnvelope>, String> {
    acp_pool
        .timeline(&session_id)
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
            Ok(CodexAcpSessionListItem {
                pinned: store.is_pinned(&metadata.id),
                pinned_at: store.pinned_at(&metadata.id),
                metadata,
                workspace,
                agent_id: code_session_agent_id(backend),
                agent_name: backend.display_name().to_string(),
            })
        })
        .collect()
}

/// 把主机工作区路径降级为最末一级目录名，避免向 WebUI 泄漏绝对路径。
/// 桌面端需要完整路径（历史记录、系统打开等），故投影只在 Web 入口应用。
pub(crate) fn redact_workspace_path_for_web(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "workspace".to_string())
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
}

fn redact_codex_session_list_item_for_web(item: &mut CodexAcpSessionListItem) {
    redact_session_metadata_for_web_in_place(&mut item.metadata);
    item.workspace.workspace_path = redact_workspace_path_for_web(&item.workspace.workspace_path);
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
                && (acp_pool.is_acp_metadata(metadata)
                    || acp_pool.agents().is_code_session(&metadata.id))
                && !store.is_hidden(&metadata.id)
        })
        .map(|metadata| {
            let backend = acp_pool.backend(&metadata.id);
            let workspace = acp_pool
                .workspace_info(&metadata.id)
                .map_err(|error| format!("读取代码会话 {} 工作目录失败: {error:#}", metadata.id))?;
            Ok(CodexAcpSessionListItem {
                pinned: store.is_pinned(&metadata.id),
                pinned_at: store.pinned_at(&metadata.id),
                metadata,
                workspace,
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
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    acp_pool: State<'_, AcpPool>,
) -> Result<SessionMetadata, String> {
    let backend = AgentBackend::parse(agent_id.as_deref().or(Some("pinvou")))
        .map_err(|error| format!("{error:#}"))?;
    if !backend.is_acp() {
        return create_code_native_session(workspace_path, &pool, &store, &acp_pool).await;
    }
    let project_workspace = workspace_path
        .as_deref()
        .map(|path| validate_codex_project_workspace(std::path::Path::new(path)))
        .transpose()
        .map_err(|error| format!("{error:#}"))?;
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
    if let Err(error) =
        acp_pool
            .agents()
            .set_acp_workspace(&session.metadata.id, backend, kind, project_workspace)
    {
        let _ = store.delete(&session.metadata.id);
        return Err(format!("保存 Codex ACP 会话工作目录失败: {error:#}"));
    }
    let baseline_root = acp_pool
        .workspace_info(&session.metadata.id)
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
        let _ = acp_pool.agents().remove(&session.metadata.id);
        let _ = store.delete(&session.metadata.id);
        return Err(format!("创建 Codex 工作区基线失败: {error:#}"));
    }
    Ok(session.metadata)
}

/// 创建“代码”模块原生（品悟 Engine）会话。
///
/// 临时会话执行目录与 ACP 临时会话共用 `SessionStore::session_roots` 推导
/// （两根一致，均为会话私有目录）；项目会话绑定调用方选定的目录（经
/// `validate_codex_project_workspace` 校验），执行根由 resolver 在
/// engine/shell 启动时解析，发消息走现有 `chat` 命令。
async fn create_code_native_session(
    workspace_path: Option<String>,
    pool: &EnginePool,
    store: &SessionStore,
    acp_pool: &AcpPool,
) -> Result<SessionMetadata, String> {
    let project_workspace = workspace_path
        .as_deref()
        .map(|path| validate_codex_project_workspace(std::path::Path::new(path)))
        .transpose()
        .map_err(|error| format!("{error:#}"))?;
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
    ) {
        let _ = store.delete(&session.metadata.id);
        return Err(format!("保存原生代码会话标记失败: {error:#}"));
    }
    // 基线根：项目会话用项目目录（capture_baseline 对 git 仓库只指纹 dirty 文件，
    // 与 ACP 项目会话一致）；临时会话先确保私有目录存在。
    let baseline_root = match kind {
        CodexWorkspaceKind::Project => project_workspace.expect("项目会话必有工作目录"),
        CodexWorkspaceKind::Temporary => {
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
        let _ = acp_pool.agents().remove(&session.metadata.id);
        let _ = store.delete(&session.metadata.id);
        return Err(format!("创建代码工作区基线失败: {error:#}"));
    }
    Ok(session.metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // 已是末级名称时原样返回；空串兜底为自身。
        assert_eq!(redact_workspace_path_for_web("repo"), "repo");
        assert_eq!(redact_workspace_path_for_web(""), "");
        assert_eq!(redact_workspace_path_for_web("/"), "workspace");
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
            agent_id: "codex".to_string(),
            agent_name: "Codex".to_string(),
        };
        redact_codex_session_list_item_for_web(&mut item);
        let item_json = serde_json::to_value(&item).expect("serialize projected list item");
        assert_eq!(item_json["workspace"], "secret-project");
        assert_eq!(item_json["workspace_path"], "secret-project");
        assert!(!item_json.to_string().contains(PRIVATE_WORKSPACE));
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
