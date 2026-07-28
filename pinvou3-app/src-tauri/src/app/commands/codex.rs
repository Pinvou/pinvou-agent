//! 主页“代码”模式使用的 Codex ACP Tauri 命令。
//!
//! 这里只保留传输边界与会话元数据编排；Codex 进程、ACP 协议、权限和事件适配
//! 均由 `features::codex_acp` 领域模块负责。

use anyhow::Context;
use deepseek_tui::session_manager::SessionMetadata;
use serde::Serialize;
use tauri::State;

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::codex_acp::workspace::{
    self, WorkspaceArtifact, WorkspaceChanges, WorkspaceDiff, WorkspaceEntry, WorkspaceListing,
    WorkspacePreview,
};
use crate::features::codex_acp::{
    validate_codex_project_workspace, AcpEventEnvelope, AcpPool, CodexAcpPendingElicitation,
    CodexAcpPendingPermission, CodexAcpSessionInfo, CodexAcpStatus, CodexAcpWorkspaceInfo,
    CodexWorkspaceKind, CODEX_ACP_SESSION_MODEL,
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
    attachments: Option<Vec<crate::features::files::file_ingest::IngestResult>>,
    workspace_references: Option<Vec<String>>,
    store: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    let message = message.trim().to_string();
    let attachments = attachments.unwrap_or_default();
    let workspace_references = workspace_references.unwrap_or_default();
    if message.is_empty() && attachments.is_empty() && workspace_references.is_empty() {
        return Err("empty message".to_string());
    }
    if !acp_pool.is_codex(&session_id) {
        return Err("当前会话不是 Codex ACP 会话".to_string());
    }
    let session = store
        .load(&session_id)
        .map_err(|error| format!("读取 Codex 会话失败: {error:#}"))?;
    if session.metadata.title == "新对话" {
        let title_source = if message.is_empty() {
            attachments
                .first()
                .map(|attachment| attachment.basename.as_str())
                .or_else(|| workspace_references.first().map(String::as_str))
                .unwrap_or("附件")
        } else {
            &message
        };
        let title = title_source.chars().take(28).collect::<String>();
        store
            .set_title(&session_id, title)
            .map_err(|error| format!("更新 Codex 会话标题失败: {error:#}"))?;
    }
    crate::features::assistant::timing::start_turn(&session_id);
    acp_pool
        .send_message(&session_id, message, attachments, workspace_references)
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

fn codex_workspace_root(
    session_id: &str,
    acp_pool: &AcpPool,
) -> Result<std::path::PathBuf, String> {
    if !acp_pool.is_codex(session_id) {
        return Err("当前会话不是 Codex ACP 会话".to_string());
    }
    let info = acp_pool
        .workspace_info(session_id)
        .map_err(|error| format!("读取 Codex 工作目录失败: {error:#}"))?;
    if !info.workspace_available {
        return Err(format!("Codex 工作目录不可用: {}", info.workspace_path));
    }
    Ok(std::path::PathBuf::from(info.workspace_path))
}

#[tauri::command]
pub async fn list_codex_workspace(
    session_id: String,
    relative_path: Option<String>,
    acp_pool: State<'_, AcpPool>,
) -> Result<WorkspaceListing, String> {
    let root = codex_workspace_root(&session_id, &acp_pool)?;
    tauri::async_runtime::spawn_blocking(move || {
        workspace::list_workspace(&root, relative_path.as_deref())
            .map_err(|error| format!("读取 Codex 工作区失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("读取 Codex 工作区任务失败: {error}"))?
}

#[tauri::command]
pub async fn search_codex_workspace(
    session_id: String,
    query: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<WorkspaceEntry>, String> {
    let root = codex_workspace_root(&session_id, &acp_pool)?;
    tauri::async_runtime::spawn_blocking(move || {
        workspace::search_workspace(&root, &query)
            .map_err(|error| format!("搜索 Codex 工作区失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("搜索 Codex 工作区任务失败: {error}"))?
}

#[tauri::command]
pub async fn preview_codex_workspace_file(
    session_id: String,
    relative_path: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<WorkspacePreview, String> {
    let root = codex_workspace_root(&session_id, &acp_pool)?;
    tauri::async_runtime::spawn_blocking(move || {
        workspace::preview_workspace_file(&root, &relative_path)
            .map_err(|error| format!("预览 Codex 工作区文件失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("预览 Codex 工作区文件任务失败: {error}"))?
}

#[tauri::command]
pub async fn get_codex_workspace_changes(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<WorkspaceChanges, String> {
    let root = codex_workspace_root(&session_id, &acp_pool)?;
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
    let root = codex_workspace_root(&session_id, &acp_pool)?;
    tauri::async_runtime::spawn_blocking(move || {
        workspace::workspace_diff(&root, &relative_path)
            .map_err(|error| format!("读取 Codex 文件差异失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("读取 Codex 文件差异任务失败: {error}"))?
}

#[tauri::command]
pub async fn open_codex_workspace_file(
    session_id: String,
    relative_path: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    let root = codex_workspace_root(&session_id, &acp_pool)?;
    let path = workspace::resolve_workspace_file(&root, &relative_path)
        .map_err(|error| format!("打开 Codex 工作区文件失败: {error:#}"))?;
    crate::platform::os::open_target(
        crate::platform::os::external_application_path(&path),
        "Codex 工作区文件",
    )
}

#[tauri::command]
pub async fn reveal_codex_workspace_file(
    session_id: String,
    relative_path: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<(), String> {
    let root = codex_workspace_root(&session_id, &acp_pool)?;
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

/// Reconcile Codex outputs into Pinvou's shared Session artifact index.
///
/// ACP Code mode bypasses the regular chat tool-event bridge, so without this
/// explicit handoff its files can be browsed in the workspace but cannot be
/// previewed or managed as Pinvou artifacts.
#[tauri::command]
pub async fn reconcile_codex_acp_artifacts(
    session_id: String,
    store: State<'_, SessionStore>,
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<WorkspaceArtifact>, String> {
    let root = codex_workspace_root(&session_id, &acp_pool)?;
    let workspace_info = acp_pool
        .workspace_info(&session_id)
        .map_err(|error| format!("读取 Codex 工作目录失败: {error:#}"))?;
    let timeline = acp_pool
        .timeline(&session_id)
        .map_err(|error| format!("读取 Codex ACP timeline 失败: {error:#}"))?;
    let saved = store
        .load(&session_id)
        .map_err(|error| format!("读取 Codex 会话产出物失败: {error:#}"))?;
    let retained = saved
        .artifacts
        .iter()
        .map(|artifact| artifact.storage_path.clone())
        .collect::<Vec<_>>();
    let temporary = workspace_info.workspace_kind == CodexWorkspaceKind::Temporary;
    let discovery_session_id = session_id.clone();
    let artifacts = tauri::async_runtime::spawn_blocking(move || {
        workspace::discover_artifacts(
            &discovery_session_id,
            &root,
            temporary,
            &timeline,
            &retained,
        )
        .map_err(|error| format!("识别 Codex 会话产出物失败: {error:#}"))
    })
    .await
    .map_err(|error| format!("识别 Codex 会话产出物任务失败: {error}"))??;

    let next_paths = artifacts
        .iter()
        .map(|artifact| artifact.path.clone())
        .collect::<Vec<_>>();
    let current_paths = saved
        .artifacts
        .iter()
        .map(|artifact| artifact.storage_path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if current_paths != next_paths {
        store
            .update_artifacts(&session_id, next_paths)
            .map_err(|error| format!("保存 Codex 会话产出物失败: {error:#}"))?;
    }
    Ok(artifacts)
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

#[tauri::command]
pub async fn get_codex_acp_pending_elicitations(
    session_id: String,
    acp_pool: State<'_, AcpPool>,
) -> Result<Vec<CodexAcpPendingElicitation>, String> {
    if !acp_pool.is_codex(&session_id) {
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

/// 返回 Codex 会话，供主页左侧统一会话列表与代码模式共同消费。
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
            && acp_pool.is_codex_metadata(metadata)
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
        .create_new(
            CODEX_ACP_SESSION_MODEL.to_string(),
            None,
            metadata_workspace,
        )
        .map_err(|error| format!("create_codex_acp_session: {error:?}"))?;
    let kind = if project_workspace.is_some() {
        CodexWorkspaceKind::Project
    } else {
        CodexWorkspaceKind::Temporary
    };
    if kind == CodexWorkspaceKind::Temporary {
        let temporary_workspace = store
            .execution_workspace(&session.metadata.id)
            .map_err(|error| format!("解析 Codex 临时工作目录失败: {error:#}"))?;
        if let Err(error) = ensure_codex_workspace_root(kind, &temporary_workspace) {
            let _ = store.delete(&session.metadata.id);
            return Err(format!("{error:#}"));
        }
    }
    if let Err(error) =
        acp_pool
            .agents()
            .set_codex_workspace(&session.metadata.id, kind, project_workspace)
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

#[cfg(test)]
mod tests {
    use super::*;

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
