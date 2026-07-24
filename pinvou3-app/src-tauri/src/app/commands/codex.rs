//! Codex ACP 独立页面的 Tauri 命令。
//!
//! 这里只保留传输边界与会话元数据编排；Codex 进程、ACP 协议、权限和事件适配
//! 均由 `features::codex_acp` 领域模块负责。

use deepseek_tui::session_manager::SessionMetadata;
use serde::Serialize;
use tauri::State;

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::codex_acp::{
    validate_codex_project_workspace, AcpEventEnvelope, AcpPool, CodexAcpPendingPermission,
    CodexAcpSessionInfo, CodexAcpStatus, CodexAcpWorkspaceInfo, CodexWorkspaceKind,
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
}

#[tauri::command]
pub fn get_codex_acp_status(acp_pool: State<'_, AcpPool>) -> CodexAcpStatus {
    acp_pool.status()
}

#[tauri::command]
pub async fn prepare_codex_acp(acp_pool: State<'_, AcpPool>) -> Result<CodexAcpStatus, String> {
    acp_pool
        .ensure_installed()
        .await
        .map_err(|error| format!("准备 Codex 运行环境失败: {error:#}"))
}

#[tauri::command]
pub async fn login_codex_acp(acp_pool: State<'_, AcpPool>) -> Result<CodexAcpStatus, String> {
    acp_pool
        .login()
        .await
        .map_err(|error| format!("登录 Codex 失败: {error:#}"))
}

#[tauri::command]
pub fn open_codex_login_url(acp_pool: State<'_, AcpPool>) -> Result<(), String> {
    acp_pool
        .open_login_url()
        .map_err(|error| format!("打开 Codex 授权页面失败: {error:#}"))
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
    store: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    let message = message.trim().to_string();
    if message.is_empty() {
        return Err("empty message".to_string());
    }
    if !acp_pool.is_codex(&session_id) {
        return Err("当前会话不是 Codex ACP 会话".to_string());
    }
    let session = store
        .load(&session_id)
        .map_err(|error| format!("读取 Codex 会话失败: {error:#}"))?;
    if session.metadata.title == "新对话" {
        let title = message.chars().take(28).collect::<String>();
        store
            .set_title(&session_id, title)
            .map_err(|error| format!("更新 Codex 会话标题失败: {error:#}"))?;
    }
    crate::features::assistant::timing::start_turn(&session_id);
    acp_pool
        .send_message(&session_id, message)
        .await
        .map_err(|error| {
            crate::features::assistant::timing::finish_turn(
                &session_id,
                "send_error",
                Some(&format!("{error:?}")),
            );
            format!("Codex ACP send failed: {error:#}")
        })
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
    if !acp_pool.is_codex(&session_id) {
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
    if !acp_pool.is_codex(&session_id) {
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

/// Codex 页面拥有独立会话列表，DeepSeek 历史面板不会消费这些记录。
#[tauri::command]
pub async fn list_codex_acp_sessions(
    store: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<CodexAcpSessionListItem>, String> {
    let mut metas = store
        .list()
        .map_err(|error| format!("list_codex_acp_sessions: {error:?}"))?;
    metas.retain(|metadata| {
        matches!(store.session_kind(&metadata.id), Ok(SessionKind::Chat))
            && acp_pool.is_codex(&metadata.id)
            && !store.is_hidden(&metadata.id)
    });
    metas
        .into_iter()
        .map(|metadata| {
            let workspace = acp_pool.workspace_info(&metadata.id).map_err(|error| {
                format!("读取 Codex 会话 {} 工作目录失败: {error:#}", metadata.id)
            })?;
            Ok(CodexAcpSessionListItem {
                pinned: store.is_pinned(&metadata.id),
                pinned_at: store.pinned_at(&metadata.id),
                metadata,
                workspace,
            })
        })
        .collect()
}

#[tauri::command]
pub async fn create_codex_acp_session(
    workspace_path: Option<String>,
    store: State<'_, SessionStore>,
    pool: State<'_, EnginePool>,
    acp_pool: State<'_, AcpPool>,
) -> Result<SessionMetadata, String> {
    let project_workspace = workspace_path
        .as_deref()
        .map(|path| validate_codex_project_workspace(std::path::Path::new(path)))
        .transpose()
        .map_err(|error| format!("{error:#}"))?;
    let metadata_workspace = project_workspace
        .clone()
        .unwrap_or_else(|| pool.bridge.workspace.clone());
    let session = store
        .create_new("Codex (ACP)".to_string(), None, metadata_workspace)
        .map_err(|error| format!("create_codex_acp_session: {error:?}"))?;
    let kind = if project_workspace.is_some() {
        CodexWorkspaceKind::Project
    } else {
        CodexWorkspaceKind::Temporary
    };
    if let Err(error) =
        acp_pool
            .agents()
            .set_codex_workspace(&session.metadata.id, kind, project_workspace)
    {
        let _ = store.delete(&session.metadata.id);
        return Err(format!("保存 Codex ACP 会话工作目录失败: {error:#}"));
    }
    Ok(session.metadata)
}
