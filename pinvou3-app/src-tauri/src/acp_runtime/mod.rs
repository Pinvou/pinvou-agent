//! Codex ACP 运行时。
//!
//! pinvou3 只做 ACP client、进程托管和现有 `chat:*` 事件的适配；Codex 的模型调用、
//! 工具循环、会话与权限协议都由官方 `@agentclientprotocol/codex-acp` Agent 提供。

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
    SessionModelState, SessionNotification, SetSessionConfigOptionRequest, SetSessionModeRequest,
    SetSessionModelRequest, StopReason, TextContent,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::bridge::mode_state::SerializableMode;
use crate::bridge::sessions::SessionStore;
use events::EventBridge;
pub use store::{AgentBackend, SessionAgentRecord, SessionAgentStore};

pub const CODEX_ACP_VERSION: &str = "1.1.5";
const CODEX_ACP_PACKAGE: &str = "@agentclientprotocol/codex-acp";

#[derive(Debug, Clone, Serialize)]
pub struct CodexAcpStatus {
    pub version: &'static str,
    pub installed: bool,
    pub adapter_path: Option<String>,
    pub node_available: bool,
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
}

struct AcpSession {
    connection: ConnectionTo<Agent>,
    acp_session_id: String,
    bridge: EventBridge,
    busy: AtomicBool,
    allow_permissions: Arc<AtomicBool>,
    models: Vec<CodexAcpModel>,
    current_model: parking_lot::RwLock<Option<String>>,
    shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
    child: Mutex<Child>,
}

impl AcpSession {
    async fn set_mode(&self, mode: SerializableMode) {
        let (agent_mode, collaboration_mode) = match mode {
            SerializableMode::Plan => ("read-only", "plan"),
            SerializableMode::Yolo => ("agent-full-access", "default"),
        };
        self.allow_permissions
            .store(matches!(mode, SerializableMode::Yolo), Ordering::Release);
        let _ = self
            .connection
            .send_request(SetSessionModeRequest::new(
                self.acp_session_id.clone(),
                agent_mode,
            ))
            .block_task()
            .await;
        let _ = self
            .connection
            .send_request(SetSessionConfigOptionRequest::new(
                self.acp_session_id.clone(),
                "collaboration_mode",
                collaboration_mode,
            ))
            .block_task()
            .await;
    }

    async fn prompt(self: Arc<Self>, content: String, mode: SerializableMode) {
        if self.busy.swap(true, Ordering::AcqRel) {
            self.bridge.emit(
                "chat:done",
                json!({ "status": "Failed", "error": "Codex ACP 会话仍在生成" }),
            );
            return;
        }
        self.set_mode(mode).await;
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
                if matches!(mode, SerializableMode::Plan) {
                    self.bridge.emit_plan_ready();
                }
                crate::timing::finish_turn(&self.bridge_session_id(), status, None);
                self.bridge
                    .emit("chat:done", json!({ "status": status, "error": null }));
            }
            Err(error) => {
                let message = format!("Codex ACP: {error}");
                crate::timing::finish_turn(&self.bridge_session_id(), "Failed", Some(&message));
                self.bridge
                    .emit("chat:done", json!({ "status": "Failed", "error": message }));
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

    fn info(&self) -> CodexAcpSessionInfo {
        CodexAcpSessionInfo {
            session_id: self.acp_session_id.clone(),
            current_model_id: self.current_model.read().clone(),
            models: self.models.clone(),
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
}

#[derive(Clone)]
pub struct AcpPool {
    app: AppHandle,
    sessions: Arc<Mutex<HashMap<String, Arc<AcpSession>>>>,
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
            agents: SessionAgentStore::load()?,
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

    pub fn status(&self) -> CodexAcpStatus {
        let adapter = self.resolve_adapter();
        CodexAcpStatus {
            version: CODEX_ACP_VERSION,
            installed: adapter.is_some(),
            adapter_path: adapter.map(|path| path.to_string_lossy().into_owned()),
            node_available: find_in_path("node").is_some(),
            npm_available: find_in_path("npm").is_some(),
            authenticated: codex_authenticated(),
            installing: self.installing.load(Ordering::Acquire),
            error: self.last_error.read().clone(),
        }
    }

    pub async fn ensure_installed(&self) -> Result<CodexAcpStatus> {
        if self.resolve_adapter().is_some() {
            return Ok(self.status());
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

    pub async fn send_message(
        &self,
        session_id: &str,
        content: String,
        mode: SerializableMode,
    ) -> Result<()> {
        let runtime = self.get_or_spawn(session_id).await?;
        tokio::spawn(runtime.prompt(content, mode));
        Ok(())
    }

    pub async fn cancel(&self, session_id: &str) {
        if let Some(runtime) = self.sessions.lock().await.get(session_id).cloned() {
            runtime.cancel();
        }
    }

    pub async fn evict(&self, session_id: &str) {
        if let Some(runtime) = self.sessions.lock().await.remove(session_id) {
            runtime.shutdown().await;
        }
    }

    pub async fn session_info(&self, session_id: &str) -> Result<CodexAcpSessionInfo> {
        if !self.is_codex(session_id) {
            bail!("当前会话不是 Codex ACP 会话");
        }
        Ok(self.get_or_spawn(session_id).await?.info())
    }

    pub async fn set_model(&self, session_id: &str, model_id: &str) -> Result<CodexAcpSessionInfo> {
        let runtime = self.get_or_spawn(session_id).await?;
        runtime.set_model(model_id).await?;
        self.agents
            .set_acp_model(session_id, Some(model_id.to_string()))?;
        Ok(runtime.info())
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
        let workspace = self
            .session_store
            .execution_workspace(pinvou_session_id)
            .with_context(|| format!("解析会话 {pinvou_session_id} 工作目录失败"))?;
        tokio::fs::create_dir_all(&workspace).await?;

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
        let allow_permissions = Arc::new(AtomicBool::new(true));
        let (ready_tx, ready_rx) = oneshot::channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let bridge_for_notification = event_bridge.clone();
        let bridge_for_permission = event_bridge.clone();
        let replay_for_notification = replay_suppressed.clone();
        let allow_for_permission = allow_permissions.clone();

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
                        use agent_client_protocol::schema::PermissionOptionKind;
                        let permits = allow_for_permission.load(Ordering::Acquire);
                        let selected = request.options.iter().find(|option| {
                            if permits {
                                matches!(option.kind, PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways)
                            } else {
                                matches!(option.kind, PermissionOptionKind::RejectOnce | PermissionOptionKind::RejectAlways)
                            }
                        });
                        bridge_for_permission.emit(
                            "chat:acp_permission",
                            json!({
                                "tool_call_id": request.tool_call.tool_call_id.to_string(),
                                "auto_approved": permits,
                                "selected_option": selected.map(|option| option.option_id.to_string()),
                            }),
                        );
                        let response = selected
                            .map(|option| {
                                RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                                    SelectedPermissionOutcome::new(option.option_id.clone()),
                                ))
                            })
                            .unwrap_or_else(|| {
                                RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
                            });
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
        let (acp_session_id, model_state) = if initialized.agent_capabilities.load_session {
            if let Some(saved_id) = saved.acp_session_id {
                replay_suppressed.store(true, Ordering::Release);
                let loaded = connection
                    .send_request(LoadSessionRequest::new(saved_id.clone(), workspace.clone()))
                    .block_task()
                    .await;
                replay_suppressed.store(false, Ordering::Release);
                match loaded {
                    Ok(response) => (saved_id, response.models),
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
        let current_model_id = model_state
            .as_ref()
            .map(|state| state.current_model_id.to_string());
        let models = codex_models(model_state.as_ref());
        self.agents.set_acp_session(
            pinvou_session_id,
            acp_session_id.clone(),
            current_model_id.clone(),
        )?;
        let _ = self.app.emit(
            "chat:acp_ready",
            json!({
                "session_id": pinvou_session_id,
                "agent": initialized.agent_info,
                "capabilities": initialized.agent_capabilities,
            }),
        );

        Ok(AcpSession {
            connection,
            acp_session_id,
            bridge: event_bridge,
            busy: AtomicBool::new(false),
            allow_permissions,
            models,
            current_model: parking_lot::RwLock::new(current_model_id),
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
) -> Result<(String, Option<SessionModelState>)> {
    let response = connection
        .send_request(NewSessionRequest::new(workspace))
        .block_task()
        .await
        .context("Codex ACP session/new 失败")?;
    Ok((response.session_id.to_string(), response.models))
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
}
