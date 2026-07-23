//! Codex ACP 运行时。
//!
//! pinvou3 只做 ACP client、进程托管、权限路由、事件持久化和 `acp:event` 投影；
//! Codex 的模型调用、工具循环、会话与权限协议都由 `codex-acp` Agent 提供。

mod events;
mod store;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::{
    CancelNotification, ContentBlock, Implementation, InitializeRequest, LoadSessionRequest,
    NewSessionRequest, PromptRequest, ProtocolVersion, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionConfigKind, SessionConfigOption, SessionConfigSelectOptions, SessionModeState,
    SessionModelState, SessionNotification, SetSessionConfigOptionRequest, SetSessionModeRequest,
    SetSessionModelRequest, StopReason, TextContent,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::bridge::sessions::SessionStore;
pub use events::AcpEventEnvelope;
use events::{load_timeline, patch_acp_state, persist_acp_state, EventBridge};
pub use store::{
    validate_codex_project_workspace, AgentBackend, CodexWorkspaceKind, SessionAgentRecord,
    SessionAgentStore,
};

pub const CODEX_ACP_VERSION: &str = "1.1.5";
const CODEX_ACP_PACKAGE: &str = "@agentclientprotocol/codex-acp";

#[derive(Debug, Clone, Serialize)]
pub struct CodexAcpStatus {
    pub version: &'static str,
    pub installed: bool,
    pub adapter_path: Option<String>,
    pub node_available: bool,
    pub node_version: Option<String>,
    pub node_supported: bool,
    pub npm_available: bool,
    pub authenticated: bool,
    pub installing: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodexAcpModel {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodexAcpSessionInfo {
    pub session_id: String,
    pub current_model_id: Option<String>,
    pub models: Vec<CodexAcpModel>,
    pub modes: Option<SessionModeState>,
    pub config_options: Vec<SessionConfigOption>,
    pub pending_permissions: Vec<CodexAcpPendingPermission>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodexAcpWorkspaceInfo {
    pub workspace_kind: CodexWorkspaceKind,
    pub workspace_path: String,
    pub workspace_available: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAcpPendingPermission {
    pub session_id: String,
    pub tool_call_id: String,
    pub request: serde_json::Value,
}

struct PendingPermission {
    view: CodexAcpPendingPermission,
    option_ids: Vec<String>,
    response_tx: oneshot::Sender<RequestPermissionResponse>,
}

struct AcpSession {
    connection: ConnectionTo<Agent>,
    acp_session_id: String,
    bridge: EventBridge,
    busy: AtomicBool,
    configuring: AtomicBool,
    models: Vec<CodexAcpModel>,
    current_model: parking_lot::RwLock<Option<String>>,
    modes: parking_lot::RwLock<Option<SessionModeState>>,
    config_options: parking_lot::RwLock<Vec<SessionConfigOption>>,
    shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
    child: Mutex<Child>,
}

impl AcpSession {
    async fn set_mode(&self, mode_id: &str) -> Result<()> {
        let supported = self.modes.read().as_ref().is_some_and(|modes| {
            modes
                .available_modes
                .iter()
                .any(|mode| mode.id.to_string() == mode_id)
        });
        if !supported {
            bail!("Codex ACP 未上报会话模式: {mode_id}");
        }
        self.connection
            .send_request(SetSessionModeRequest::new(
                self.acp_session_id.clone(),
                mode_id.to_string(),
            ))
            .block_task()
            .await
            .context("Codex ACP session/set_mode 失败")?;
        if let Some(modes) = self.modes.write().as_mut() {
            modes.current_mode_id = mode_id.to_string().into();
        }
        Ok(())
    }

    async fn prompt(self: Arc<Self>, content: String) {
        let turn_id = self.bridge.begin_turn(&content);
        let result = self
            .connection
            .send_request(PromptRequest::new(
                self.acp_session_id.clone(),
                vec![ContentBlock::Text(TextContent::new(content))],
            ))
            .block_task()
            .await;
        self.busy.store(false, Ordering::Release);
        match result {
            Ok(response) => {
                let status = match response.stop_reason {
                    StopReason::EndTurn => "Completed",
                    StopReason::Cancelled => "Interrupted",
                    StopReason::MaxTokens | StopReason::MaxTurnRequests => "LimitReached",
                    StopReason::Refusal => "Refused",
                    _ => "Completed",
                };
                crate::timing::finish_turn(&self.bridge_session_id(), status, None);
                self.bridge.finish_turn(&turn_id, status, None);
            }
            Err(error) => {
                let message = format!("Codex ACP: {error}");
                crate::timing::finish_turn(&self.bridge_session_id(), "Failed", Some(&message));
                self.bridge.finish_turn(&turn_id, "Failed", Some(&message));
            }
        }
    }

    fn bridge_session_id(&self) -> String {
        self.bridge.pinvou_session_id().to_string()
    }

    fn cancel(&self) {
        let _ = self
            .connection
            .send_notification(CancelNotification::new(self.acp_session_id.clone()));
    }

    async fn shutdown(&self) {
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(());
        }
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
    }

    fn info(&self, pending_permissions: Vec<CodexAcpPendingPermission>) -> CodexAcpSessionInfo {
        CodexAcpSessionInfo {
            session_id: self.acp_session_id.clone(),
            current_model_id: self.current_model.read().clone(),
            models: self.models.clone(),
            modes: self.modes.read().clone(),
            config_options: self.config_options.read().clone(),
            pending_permissions,
        }
    }

    async fn set_model(&self, model_id: &str) -> Result<()> {
        if !self.models.iter().any(|model| model.id == model_id) {
            bail!("Codex ACP 模型不存在: {model_id}");
        }
        self.connection
            .send_request(SetSessionModelRequest::new(
                self.acp_session_id.clone(),
                model_id.to_string(),
            ))
            .block_task()
            .await
            .context("Codex ACP session/set_model 失败")?;
        *self.current_model.write() = Some(model_id.to_string());
        Ok(())
    }

    async fn set_config_option(&self, config_id: &str, value_id: &str) -> Result<()> {
        let mut options = self.config_options.read().clone();
        apply_config_option(
            &self.connection,
            &self.acp_session_id,
            &mut options,
            config_id,
            value_id,
        )
        .await?;
        *self.config_options.write() = options;
        Ok(())
    }
}

#[derive(Clone)]
pub struct AcpPool {
    app: AppHandle,
    sessions: Arc<Mutex<HashMap<String, Arc<AcpSession>>>>,
    pending_permissions: Arc<Mutex<HashMap<String, PendingPermission>>>,
    agents: SessionAgentStore,
    session_store: SessionStore,
    installing: Arc<AtomicBool>,
    last_error: Arc<parking_lot::RwLock<Option<String>>>,
    bundled_adapter: Option<PathBuf>,
}

impl AcpPool {
    pub fn new(app: AppHandle, session_store: SessionStore) -> Result<Self> {
        let bundled_adapter = app.path().resource_dir().ok().and_then(|root| {
            [
                root.join("codex-acp")
                    .join("node_modules")
                    .join("@agentclientprotocol")
                    .join("codex-acp")
                    .join("dist")
                    .join("index.js"),
                root.join("codex-acp").join(adapter_filename()),
                root.join("resources")
                    .join("codex-acp")
                    .join("node_modules")
                    .join("@agentclientprotocol")
                    .join("codex-acp")
                    .join("dist")
                    .join("index.js"),
                root.join("resources")
                    .join("codex-acp")
                    .join(adapter_filename()),
            ]
            .into_iter()
            .find(|candidate| candidate.is_file())
        });
        Ok(Self {
            app,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            agents: SessionAgentStore::load_or_empty(),
            session_store,
            installing: Arc::new(AtomicBool::new(false)),
            last_error: Arc::new(parking_lot::RwLock::new(None)),
            bundled_adapter,
        })
    }

    pub fn agents(&self) -> &SessionAgentStore {
        &self.agents
    }

    pub fn is_codex(&self, session_id: &str) -> bool {
        self.agents.backend(session_id) == AgentBackend::CodexAcp
    }

    pub fn workspace_info(&self, session_id: &str) -> Result<CodexAcpWorkspaceInfo> {
        let record = self.agents.get(session_id);
        let path = match record.workspace_kind {
            CodexWorkspaceKind::Project => record
                .workspace_path
                .context("Codex 项目会话缺少工作目录记录")?,
            CodexWorkspaceKind::Temporary => self
                .session_store
                .execution_workspace(session_id)
                .with_context(|| format!("解析会话 {session_id} 临时工作目录失败"))?,
        };
        let available = match record.workspace_kind {
            CodexWorkspaceKind::Project => path.is_dir(),
            CodexWorkspaceKind::Temporary => true,
        };
        Ok(CodexAcpWorkspaceInfo {
            workspace_kind: record.workspace_kind,
            workspace_path: path.to_string_lossy().into_owned(),
            workspace_available: available,
        })
    }

    fn execution_workspace(&self, session_id: &str) -> Result<PathBuf> {
        let record = self.agents.get(session_id);
        match record.workspace_kind {
            CodexWorkspaceKind::Temporary => self
                .session_store
                .execution_workspace(session_id)
                .with_context(|| format!("解析会话 {session_id} 临时工作目录失败")),
            CodexWorkspaceKind::Project => {
                let path = record
                    .workspace_path
                    .context("Codex 项目会话缺少工作目录记录")?;
                validate_codex_project_workspace(&path).with_context(|| {
                    format!(
                        "Codex 会话绑定的项目目录已不可用: {}。请恢复该目录，或新建会话选择其他项目",
                        path.display()
                    )
                })
            }
        }
    }

    pub fn status(&self) -> CodexAcpStatus {
        let adapter = self.resolve_adapter();
        let node_version = installed_node_version();
        let node_supported = node_version
            .as_deref()
            .and_then(node_major_version)
            .is_some_and(|major| major >= 20);
        CodexAcpStatus {
            version: CODEX_ACP_VERSION,
            installed: adapter.is_some(),
            adapter_path: adapter.map(|path| path.to_string_lossy().into_owned()),
            node_available: node_version.is_some(),
            node_version,
            node_supported,
            npm_available: find_in_path("npm").is_some(),
            authenticated: codex_authenticated(),
            installing: self.installing.load(Ordering::Acquire),
            error: self.last_error.read().clone(),
        }
    }

    pub async fn ensure_installed(&self) -> Result<CodexAcpStatus> {
        let status = self.status();
        if !status.node_supported {
            bail!(
                "Codex ACP MVP 需要 Node.js 20+；当前版本: {}",
                status.node_version.as_deref().unwrap_or("未安装")
            );
        }
        if self.resolve_adapter().is_some() {
            return Ok(status);
        }
        if self.installing.swap(true, Ordering::AcqRel) {
            bail!("Codex ACP 正在安装，请稍候");
        }
        let result = self.install_adapter().await;
        self.installing.store(false, Ordering::Release);
        match result {
            Ok(()) => {
                *self.last_error.write() = None;
                Ok(self.status())
            }
            Err(error) => {
                *self.last_error.write() = Some(format!("{error:#}"));
                Err(error)
            }
        }
    }

    pub async fn login(&self) -> Result<CodexAcpStatus> {
        self.ensure_installed().await?;
        let adapter = self.resolve_adapter().context("Codex ACP 尚未安装")?;
        let mut command = adapter_command(&adapter)?;
        command
            .arg("login")
            .arg("--client-name")
            .arg("pinvou3")
            .arg("--client-title")
            .arg("品悟")
            .arg("--client-version")
            .arg(env!("CARGO_PKG_VERSION"));
        configure_codex_path(&mut command, &adapter);
        let status = command.status().await.context("启动 Codex 登录失败")?;
        if !status.success() {
            bail!("Codex 登录失败: {status}");
        }
        Ok(self.status())
    }

    async fn install_adapter(&self) -> Result<()> {
        let npm = find_in_path("npm").context("未找到 npm；请先安装 Node.js 20+")?;
        let root = managed_runtime_dir();
        tokio::fs::create_dir_all(&root).await?;
        let status = Command::new(npm)
            .arg("install")
            .arg("--prefix")
            .arg(&root)
            .arg("--no-audit")
            .arg("--no-fund")
            .arg(format!("{CODEX_ACP_PACKAGE}@{CODEX_ACP_VERSION}"))
            .status()
            .await
            .context("启动 npm install 失败")?;
        if !status.success() {
            bail!("npm 安装 Codex ACP 失败: {status}");
        }
        if self.resolve_adapter().is_none() {
            bail!("npm 安装完成，但未找到 codex-acp 可执行文件");
        }
        Ok(())
    }

    pub async fn send_message(&self, session_id: &str, content: String) -> Result<()> {
        let runtime = self.get_or_spawn(session_id).await?;
        if runtime.configuring.load(Ordering::Acquire) {
            bail!("Codex 会话配置仍在同步，请稍候再发送");
        }
        if runtime.busy.swap(true, Ordering::AcqRel) {
            bail!("Codex ACP 会话仍在生成");
        }
        tokio::spawn(runtime.prompt(content));
        Ok(())
    }

    pub async fn cancel(&self, session_id: &str) {
        self.cancel_pending_permissions(session_id).await;
        if let Some(runtime) = self.sessions.lock().await.get(session_id).cloned() {
            runtime.cancel();
            runtime
                .bridge
                .emit("cancel_requested", json!({ "status": "cancelling" }));
        }
    }

    pub async fn evict(&self, session_id: &str) {
        self.cancel_pending_permissions(session_id).await;
        if let Some(runtime) = self.sessions.lock().await.remove(session_id) {
            runtime.shutdown().await;
        }
    }

    pub async fn session_info(&self, session_id: &str) -> Result<CodexAcpSessionInfo> {
        if !self.is_codex(session_id) {
            bail!("当前会话不是 Codex ACP 会话");
        }
        let pending = self.pending_permissions_for(session_id).await;
        Ok(self.get_or_spawn(session_id).await?.info(pending))
    }

    pub async fn set_model(&self, session_id: &str, model_id: &str) -> Result<CodexAcpSessionInfo> {
        let runtime = self.get_or_spawn(session_id).await?;
        runtime.set_model(model_id).await?;
        self.agents
            .set_acp_model(session_id, Some(model_id.to_string()))?;
        let info = runtime.info(self.pending_permissions_for(session_id).await);
        patch_acp_state(session_id, json!({ "session": &info }))?;
        Ok(info)
    }

    pub async fn set_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value_id: &str,
    ) -> Result<CodexAcpSessionInfo> {
        let runtime = self.get_or_spawn(session_id).await?;
        if runtime.busy.load(Ordering::Acquire) {
            bail!("Codex 正在处理当前任务，配置将在本轮结束后才能修改");
        }
        if runtime.configuring.swap(true, Ordering::AcqRel) {
            bail!("Codex 会话已有配置正在同步");
        }
        if runtime.busy.load(Ordering::Acquire) {
            runtime.configuring.store(false, Ordering::Release);
            bail!("Codex 正在处理当前任务，配置将在本轮结束后才能修改");
        }
        runtime.bridge.emit(
            "config_change_requested",
            json!({ "configId": config_id, "valueId": value_id }),
        );
        let apply_result = runtime.set_config_option(config_id, value_id).await;
        runtime.configuring.store(false, Ordering::Release);
        if let Err(error) = apply_result {
            runtime.bridge.emit(
                "config_change_failed",
                json!({
                    "configId": config_id,
                    "valueId": value_id,
                    "message": format!("{error:#}"),
                }),
            );
            return Err(error);
        }
        if config_id == "mode" {
            self.agents
                .set_acp_mode(session_id, Some(value_id.to_string()))?;
        }
        runtime.bridge.emit(
            "config_change_applied",
            json!({ "configId": config_id, "valueId": value_id }),
        );
        let info = runtime.info(self.pending_permissions_for(session_id).await);
        patch_acp_state(session_id, json!({ "session": &info }))?;
        Ok(info)
    }

    pub async fn set_mode(&self, session_id: &str, mode_id: &str) -> Result<CodexAcpSessionInfo> {
        let runtime = self.get_or_spawn(session_id).await?;
        if runtime.busy.load(Ordering::Acquire) {
            bail!("Codex 正在处理当前任务，权限模式将在本轮结束后才能修改");
        }
        if runtime.configuring.swap(true, Ordering::AcqRel) {
            bail!("Codex 会话已有配置正在同步");
        }
        if runtime.busy.load(Ordering::Acquire) {
            runtime.configuring.store(false, Ordering::Release);
            bail!("Codex 正在处理当前任务，权限模式将在本轮结束后才能修改");
        }
        runtime.bridge.emit(
            "config_change_requested",
            json!({ "configId": "mode", "valueId": mode_id }),
        );
        let apply_result = runtime.set_mode(mode_id).await;
        runtime.configuring.store(false, Ordering::Release);
        if let Err(error) = apply_result {
            runtime.bridge.emit(
                "config_change_failed",
                json!({
                    "configId": "mode",
                    "valueId": mode_id,
                    "message": format!("{error:#}"),
                }),
            );
            return Err(error);
        }
        self.agents
            .set_acp_mode(session_id, Some(mode_id.to_string()))?;
        runtime.bridge.emit(
            "config_change_applied",
            json!({ "configId": "mode", "valueId": mode_id }),
        );
        let info = runtime.info(self.pending_permissions_for(session_id).await);
        patch_acp_state(session_id, json!({ "session": &info }))?;
        Ok(info)
    }

    pub fn timeline(&self, session_id: &str) -> Result<Vec<AcpEventEnvelope>> {
        if !self.is_codex(session_id) {
            bail!("当前会话不是 Codex ACP 会话");
        }
        load_timeline(session_id)
    }

    pub async fn pending_permissions_for(
        &self,
        session_id: &str,
    ) -> Vec<CodexAcpPendingPermission> {
        self.pending_permissions
            .lock()
            .await
            .values()
            .filter(|pending| pending.view.session_id == session_id)
            .map(|pending| pending.view.clone())
            .collect()
    }

    pub async fn respond_permission(
        &self,
        session_id: &str,
        tool_call_id: &str,
        option_id: &str,
    ) -> Result<()> {
        let key = permission_key(session_id, tool_call_id);
        let mut pending = self.pending_permissions.lock().await;
        let request = pending
            .remove(&key)
            .context("权限请求已过期、已回复或不属于当前会话")?;
        if !request
            .option_ids
            .iter()
            .any(|candidate| candidate == option_id)
        {
            pending.insert(key, request);
            bail!("权限选项不属于该请求");
        }
        let response = RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(option_id.to_string()),
        ));
        request
            .response_tx
            .send(response)
            .map_err(|_| anyhow::anyhow!("Codex ACP 权限请求已关闭"))?;
        if let Some(runtime) = self.sessions.lock().await.get(session_id).cloned() {
            runtime.bridge.emit(
                "permission_resolved",
                json!({
                    "toolCallId": tool_call_id,
                    "optionId": option_id,
                    "outcome": "selected",
                }),
            );
        }
        Ok(())
    }

    async fn cancel_pending_permissions(&self, session_id: &str) {
        let mut pending = self.pending_permissions.lock().await;
        let keys = pending
            .iter()
            .filter(|(_, request)| request.view.session_id == session_id)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(request) = pending.remove(&key) {
                let _ = request.response_tx.send(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ));
            }
        }
    }

    async fn get_or_spawn(&self, session_id: &str) -> Result<Arc<AcpSession>> {
        let mut sessions = self.sessions.lock().await;
        if let Some(runtime) = sessions.get(session_id) {
            return Ok(runtime.clone());
        }
        self.ensure_installed().await?;
        let runtime = Arc::new(self.spawn_session(session_id).await?);
        sessions.insert(session_id.to_string(), runtime.clone());
        Ok(runtime)
    }

    async fn spawn_session(&self, pinvou_session_id: &str) -> Result<AcpSession> {
        let adapter = self.resolve_adapter().context("Codex ACP 尚未安装")?;
        let workspace = self.execution_workspace(pinvou_session_id)?;
        if self.agents.get(pinvou_session_id).workspace_kind == CodexWorkspaceKind::Temporary {
            tokio::fs::create_dir_all(&workspace).await?;
        }

        let mut command = adapter_command(&adapter)?;
        configure_codex_path(&mut command, &adapter);
        command
            .current_dir(&workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .with_context(|| format!("启动 {} 失败", adapter.display()))?;
        let stdin = child.stdin.take().context("Codex ACP stdin 不可用")?;
        let stdout = child.stdout.take().context("Codex ACP stdout 不可用")?;
        if let Some(stderr) = child.stderr.take() {
            let sid = pinvou_session_id.to_string();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    eprintln!("[codex-acp:{sid}] {line}");
                }
            });
        }

        let event_bridge = EventBridge::new(self.app.clone(), pinvou_session_id.to_string());
        let replay_suppressed = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = oneshot::channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let bridge_for_notification = event_bridge.clone();
        let bridge_for_permission = event_bridge.clone();
        let replay_for_notification = replay_suppressed.clone();
        let pending_for_permission = self.pending_permissions.clone();
        let pinvou_id_for_permission = pinvou_session_id.to_string();

        tokio::spawn(async move {
            let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
            let mut ready_tx = Some(ready_tx);
            let mut shutdown_rx = Some(shutdown_rx);
            let result = Client
                .builder()
                .on_receive_notification(
                    async move |notification: SessionNotification, _cx| {
                        if !replay_for_notification.load(Ordering::Acquire) {
                            bridge_for_notification.handle(notification);
                        }
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .on_receive_request(
                    async move |request: RequestPermissionRequest, responder, _cx| {
                        let tool_call_id = request.tool_call.tool_call_id.to_string();
                        let key = permission_key(&pinvou_id_for_permission, &tool_call_id);
                        let option_ids = request
                            .options
                            .iter()
                            .map(|option| option.option_id.to_string())
                            .collect::<Vec<_>>();
                        let request_value =
                            serde_json::to_value(&request).unwrap_or(serde_json::Value::Null);
                        let view = CodexAcpPendingPermission {
                            session_id: pinvou_id_for_permission.clone(),
                            tool_call_id: tool_call_id.clone(),
                            request: request_value.clone(),
                        };
                        let (response_tx, response_rx) = oneshot::channel();
                        pending_for_permission.lock().await.insert(
                            key.clone(),
                            PendingPermission {
                                view,
                                option_ids,
                                response_tx,
                            },
                        );
                        bridge_for_permission.emit(
                            "permission_requested",
                            json!({
                                "toolCallId": tool_call_id,
                                "request": request_value,
                            }),
                        );
                        let response = response_rx.await.unwrap_or_else(|_| {
                            RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
                        });
                        pending_for_permission.lock().await.remove(&key);
                        responder.respond(response)
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(transport, async move |connection: ConnectionTo<Agent>| {
                    let initialized = connection
                        .send_request(
                            InitializeRequest::new(ProtocolVersion::LATEST).client_info(
                                Implementation::new("pinvou3", env!("CARGO_PKG_VERSION")),
                            ),
                        )
                        .block_task()
                        .await;
                    if let Some(tx) = ready_tx.take() {
                        let _ = tx.send(initialized.map(|response| (connection.clone(), response)));
                    }
                    if let Some(rx) = shutdown_rx.take() {
                        let _ = rx.await;
                    }
                    Ok(())
                })
                .await;
            if let Err(error) = result {
                eprintln!("[pinvou3-app] Codex ACP 协议连接结束: {error}");
            }
        });

        let (connection, initialized) = tokio::time::timeout(Duration::from_secs(30), ready_rx)
            .await
            .context("Codex ACP initialize 超时")?
            .context("Codex ACP initialize 通道中断")?
            .context("Codex ACP initialize 失败")?;

        let saved = self.agents.get(pinvou_session_id);
        let (acp_session_id, model_state, mut mode_state, mut config_options) =
            if initialized.agent_capabilities.load_session {
                if let Some(saved_id) = saved.acp_session_id.clone() {
                    replay_suppressed.store(true, Ordering::Release);
                    let loaded = connection
                        .send_request(LoadSessionRequest::new(saved_id.clone(), workspace.clone()))
                        .block_task()
                        .await;
                    replay_suppressed.store(false, Ordering::Release);
                    match loaded {
                        Ok(response) => (
                            saved_id,
                            response.models,
                            response.modes,
                            response.config_options.unwrap_or_default(),
                        ),
                        Err(error) => {
                            eprintln!("[pinvou3-app] Codex ACP 恢复会话失败，改建新会话: {error}");
                            new_acp_session(&connection, &workspace).await?
                        }
                    }
                } else {
                    new_acp_session(&connection, &workspace).await?
                }
            } else {
                new_acp_session(&connection, &workspace).await?
            };
        if let Some(mode_id) = saved.acp_mode_id.as_deref() {
            apply_saved_mode(
                &connection,
                &acp_session_id,
                &mut mode_state,
                &mut config_options,
                mode_id,
            )
            .await
            .with_context(|| format!("恢复 Codex 权限模式 {mode_id} 失败"))?;
        }
        let current_model_id = model_state
            .as_ref()
            .map(|state| state.current_model_id.to_string());
        let models = codex_models(model_state.as_ref());
        self.agents.set_acp_session(
            pinvou_session_id,
            acp_session_id.clone(),
            current_model_id.clone(),
        )?;
        persist_acp_state(
            pinvou_session_id,
            json!({
                "adapter": {
                    "package": CODEX_ACP_PACKAGE,
                    "version": CODEX_ACP_VERSION,
                    "path": adapter,
                },
                "agent": &initialized.agent_info,
                "capabilities": &initialized.agent_capabilities,
                "session": {
                    "session_id": &acp_session_id,
                    "current_model_id": &current_model_id,
                    "models": &models,
                    "modes": &mode_state,
                    "config_options": &config_options,
                },
                "lastStatus": "ready",
            }),
        )?;
        event_bridge.emit(
            "runtime_ready",
            json!({
                "agent": initialized.agent_info,
                "capabilities": initialized.agent_capabilities,
            }),
        );

        Ok(AcpSession {
            connection,
            acp_session_id,
            bridge: event_bridge,
            busy: AtomicBool::new(false),
            configuring: AtomicBool::new(false),
            models,
            current_model: parking_lot::RwLock::new(current_model_id),
            modes: parking_lot::RwLock::new(mode_state),
            config_options: parking_lot::RwLock::new(config_options),
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
            child: Mutex::new(child),
        })
    }

    fn resolve_adapter(&self) -> Option<PathBuf> {
        resolve_adapter_from(self.bundled_adapter.as_deref())
    }
}

async fn new_acp_session(
    connection: &ConnectionTo<Agent>,
    workspace: &Path,
) -> Result<(
    String,
    Option<SessionModelState>,
    Option<SessionModeState>,
    Vec<SessionConfigOption>,
)> {
    let response = connection
        .send_request(NewSessionRequest::new(workspace))
        .block_task()
        .await
        .context("Codex ACP session/new 失败")?;
    Ok((
        response.session_id.to_string(),
        response.models,
        response.modes,
        response.config_options.unwrap_or_default(),
    ))
}

fn config_option_supports(
    options: &[SessionConfigOption],
    config_id: &str,
    value_id: &str,
) -> bool {
    options.iter().any(|option| {
        option.id.to_string() == config_id
            && match &option.kind {
                SessionConfigKind::Select(select) => match &select.options {
                    SessionConfigSelectOptions::Ungrouped(options) => options
                        .iter()
                        .any(|candidate| candidate.value.to_string() == value_id),
                    SessionConfigSelectOptions::Grouped(groups) => groups.iter().any(|group| {
                        group
                            .options
                            .iter()
                            .any(|candidate| candidate.value.to_string() == value_id)
                    }),
                    _ => false,
                },
                _ => false,
            }
    })
}

async fn apply_config_option(
    connection: &ConnectionTo<Agent>,
    acp_session_id: &str,
    options: &mut [SessionConfigOption],
    config_id: &str,
    value_id: &str,
) -> Result<()> {
    if !config_option_supports(options, config_id, value_id) {
        bail!("Codex ACP 配置项或取值不存在: {config_id}={value_id}");
    }
    connection
        .send_request(SetSessionConfigOptionRequest::new(
            acp_session_id.to_string(),
            config_id.to_string(),
            value_id.to_string(),
        ))
        .block_task()
        .await
        .context("Codex ACP session/set_config_option 失败")?;
    for option in options {
        if option.id.to_string() != config_id {
            continue;
        }
        if let SessionConfigKind::Select(select) = &mut option.kind {
            select.current_value = value_id.to_string().into();
        }
    }
    Ok(())
}

async fn apply_saved_mode(
    connection: &ConnectionTo<Agent>,
    acp_session_id: &str,
    modes: &mut Option<SessionModeState>,
    config_options: &mut [SessionConfigOption],
    mode_id: &str,
) -> Result<()> {
    if config_options
        .iter()
        .any(|option| option.id.to_string() == "mode")
    {
        return apply_config_option(connection, acp_session_id, config_options, "mode", mode_id)
            .await;
    }
    let supported = modes.as_ref().is_some_and(|state| {
        state
            .available_modes
            .iter()
            .any(|mode| mode.id.to_string() == mode_id)
    });
    if !supported {
        bail!("Codex ACP 未上报会话模式: {mode_id}");
    }
    connection
        .send_request(SetSessionModeRequest::new(
            acp_session_id.to_string(),
            mode_id.to_string(),
        ))
        .block_task()
        .await
        .context("Codex ACP session/set_mode 失败")?;
    if let Some(state) = modes.as_mut() {
        state.current_mode_id = mode_id.to_string().into();
    }
    Ok(())
}

fn codex_models(state: Option<&SessionModelState>) -> Vec<CodexAcpModel> {
    state
        .map(|state| {
            state
                .available_models
                .iter()
                .map(|model| CodexAcpModel {
                    id: model.model_id.to_string(),
                    name: model.name.clone(),
                    description: model.description.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn managed_runtime_dir() -> PathBuf {
    crate::bridge::paths::pinvou3_home()
        .join("runtimes")
        .join(format!("codex-acp-{CODEX_ACP_VERSION}"))
}

fn managed_adapter_path() -> PathBuf {
    let name = if cfg!(windows) {
        "codex-acp.cmd"
    } else {
        "codex-acp"
    };
    managed_runtime_dir()
        .join("node_modules")
        .join(".bin")
        .join(name)
}

fn adapter_filename() -> &'static str {
    if cfg!(windows) {
        "codex-acp.exe"
    } else {
        "codex-acp"
    }
}

fn adapter_command(adapter: &Path) -> Result<Command> {
    if adapter.extension().and_then(|value| value.to_str()) == Some("js") {
        let node = find_in_path("node").context("未找到 node")?;
        let mut command = Command::new(node);
        command.arg(adapter);
        Ok(command)
    } else {
        Ok(Command::new(adapter))
    }
}

fn configure_codex_path(command: &mut Command, adapter: &Path) {
    if let Some(codex) = codex_path_for_adapter(adapter)
        .or_else(|| find_in_path(if cfg!(windows) { "codex.cmd" } else { "codex" }))
    {
        command.env("CODEX_PATH", codex);
    }
}

fn codex_path_for_adapter(adapter: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) { "codex.cmd" } else { "codex" };
    if adapter
        .parent()?
        .file_name()
        .and_then(|value| value.to_str())
        == Some(".bin")
    {
        let candidate = adapter.parent()?.join(name);
        return candidate.is_file().then_some(candidate);
    }
    adapter.ancestors().find_map(|ancestor| {
        (ancestor.file_name().and_then(|value| value.to_str()) == Some("node_modules"))
            .then(|| ancestor.join(".bin").join(name))
            .filter(|candidate| candidate.is_file())
    })
}

fn resolve_adapter_from(bundled: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PINVOU3_CODEX_ACP_BIN").map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(path) = bundled {
        if path.is_file() {
            return Some(path.to_path_buf());
        }
    }
    let managed = managed_adapter_path();
    if managed.is_file() {
        return Some(managed);
    }
    find_in_path(if cfg!(windows) {
        "codex-acp.cmd"
    } else {
        "codex-acp"
    })
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn installed_node_version() -> Option<String> {
    let node = find_in_path("node")?;
    let output = std::process::Command::new(node)
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?;
    Some(version.trim().trim_start_matches('v').to_string())
}

fn node_major_version(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

fn permission_key(session_id: &str, tool_call_id: &str) -> String {
    format!("{session_id}\u{1f}{tool_call_id}")
}

fn codex_authenticated() -> bool {
    if std::env::var_os("OPENAI_API_KEY").is_some() {
        return true;
    }
    let home = crate::os::user_home_dir();
    home.join(".codex").join("auth.json").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_path_is_versioned() {
        let path = managed_adapter_path().to_string_lossy().into_owned();
        assert!(path.contains(CODEX_ACP_VERSION));
        assert!(path.contains("codex-acp"));
    }

    #[test]
    fn node_version_parser_requires_a_major() {
        assert_eq!(node_major_version("20.18.1"), Some(20));
        assert_eq!(node_major_version("v20.18.1"), None);
        assert_eq!(node_major_version("unknown"), None);
    }

    #[test]
    fn permission_key_is_scoped_to_session() {
        assert_ne!(
            permission_key("session-a", "tool-1"),
            permission_key("session-b", "tool-1")
        );
    }
}
