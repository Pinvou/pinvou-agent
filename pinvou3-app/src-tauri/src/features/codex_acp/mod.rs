//! Codex ACP 运行时。
//!
//! pinvou3 只做 ACP client、进程托管、权限路由、事件持久化和 `acp:event` 投影；
//! Codex 的模型调用、工具循环、会话与权限协议都由 `codex-acp` Agent 提供。

mod attachments;
mod diagnostics;
mod events;
mod runtime;
mod store;
pub(crate) mod workspace;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_client_protocol::schema::v1::{
    CancelNotification, ClientCapabilities, ContentBlock, CreateElicitationRequest,
    CreateElicitationResponse, ElicitationAcceptAction, ElicitationAction, ElicitationCapabilities,
    ElicitationContentValue, ElicitationFormCapabilities, Implementation, InitializeRequest,
    LoadSessionRequest, NewSessionRequest, PromptCapabilities, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption, SessionConfigSelectOptions,
    SessionModeState, SessionNotification, SetSessionConfigOptionRequest, SetSessionModeRequest,
    StopReason,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::features::sessions::SessionStore;
use attachments::{prepare_codex_prompt, CodexDisplayAttachment};
pub use events::AcpEventEnvelope;
use events::{load_timeline, patch_acp_state, persist_acp_state, EventBridge};
use runtime::{
    install_managed_codex, is_managed_newer_than, resolve_codex_path, ResolvedCodex,
    MANAGED_CODEX_VERSION,
};
pub use store::{
    validate_codex_project_workspace, AgentBackend, CodexWorkspaceKind, SessionAgentStore,
};

pub const CODEX_ACP_VERSION: &str = "1.1.5";
const CODEX_ACP_PACKAGE: &str = "@agentclientprotocol/codex-acp";

#[derive(Debug, Clone, Serialize)]
pub struct CodexAcpStatus {
    pub version: &'static str,
    pub installed: bool,
    pub bridge_ready: bool,
    pub adapter_path: Option<String>,
    pub node_available: bool,
    pub node_version: Option<String>,
    pub node_supported: bool,
    pub npm_available: bool,
    pub codex_available: bool,
    pub codex_path: Option<String>,
    pub codex_version: Option<String>,
    pub runtime_source: Option<&'static str>,
    pub managed_codex_version: &'static str,
    pub download_required: bool,
    pub downloaded_bytes: u64,
    pub download_total_bytes: u64,
    pub download_progress: Option<u8>,
    pub authenticated: bool,
    pub login_in_progress: bool,
    pub login_url: Option<String>,
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
    pub pending_elicitations: Vec<CodexAcpPendingElicitation>,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAcpPendingElicitation {
    pub session_id: String,
    pub elicitation_id: String,
    pub request: serde_json::Value,
}

struct PendingPermission {
    view: CodexAcpPendingPermission,
    option_ids: Vec<String>,
    response_tx: oneshot::Sender<RequestPermissionResponse>,
}

struct PendingElicitation {
    view: CodexAcpPendingElicitation,
    response_tx: oneshot::Sender<CreateElicitationResponse>,
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
    prompt_capabilities: PromptCapabilities,
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

    async fn prompt(
        self: Arc<Self>,
        content: String,
        blocks: Vec<ContentBlock>,
        attachments: Vec<CodexDisplayAttachment>,
    ) -> bool {
        let turn_id = self.bridge.begin_turn(&content, &attachments);
        let result = self
            .connection
            .send_request(PromptRequest::new(self.acp_session_id.clone(), blocks))
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
                crate::features::assistant::timing::finish_turn(
                    &self.bridge_session_id(),
                    status,
                    None,
                );
                self.bridge.finish_turn(&turn_id, status, None);
                false
            }
            Err(error) => {
                let message = format!("Codex ACP: {error}");
                let upgrade_required = codex_upgrade_required(&message);
                crate::features::assistant::timing::finish_turn(
                    &self.bridge_session_id(),
                    "Failed",
                    Some(&message),
                );
                self.bridge.finish_turn(&turn_id, "Failed", Some(&message));
                upgrade_required
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

    fn info(
        &self,
        pending_permissions: Vec<CodexAcpPendingPermission>,
        pending_elicitations: Vec<CodexAcpPendingElicitation>,
    ) -> CodexAcpSessionInfo {
        CodexAcpSessionInfo {
            session_id: self.acp_session_id.clone(),
            current_model_id: self.current_model.read().clone(),
            models: self.models.clone(),
            modes: self.modes.read().clone(),
            config_options: self.config_options.read().clone(),
            pending_permissions,
            pending_elicitations,
        }
    }

    async fn set_model(&self, model_id: &str) -> Result<()> {
        let mut options = self.config_options.read().clone();
        apply_config_option(
            &self.connection,
            &self.acp_session_id,
            &mut options,
            "model",
            model_id,
        )
        .await?;
        *self.config_options.write() = options;
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
    pending_elicitations: Arc<Mutex<HashMap<String, PendingElicitation>>>,
    agents: SessionAgentStore,
    session_store: SessionStore,
    installing: Arc<AtomicBool>,
    login_in_progress: Arc<AtomicBool>,
    login_url: Arc<parking_lot::RwLock<Option<String>>>,
    downloaded_bytes: Arc<AtomicU64>,
    download_total_bytes: Arc<AtomicU64>,
    last_error: Arc<parking_lot::RwLock<Option<String>>>,
    runtime_probe: Arc<parking_lot::RwLock<RuntimeProbeCache>>,
    runtime_probe_gate: Arc<Mutex<()>>,
    bundled_adapter: Option<PathBuf>,
    bundled_node: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
struct RuntimeProbeCache {
    initialized: bool,
    node_version: Option<String>,
    codex: Option<ResolvedCodex>,
}

impl AcpPool {
    pub fn new(app: AppHandle, session_store: SessionStore) -> Result<Self> {
        let resource_root = app.path().resource_dir().ok();
        let development_bridge = if crate::platform::capabilities::is_windows() {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("windows-runtime")
                .join("codex-bridge")
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join("platforms")
                .join("linux")
                .join("codex-bridge")
        };
        let bundled_adapter = resource_root.as_ref().and_then(|root| {
            [
                root.join("runtime")
                    .join("codex-bridge")
                    .join("acp")
                    .join("node_modules")
                    .join("@agentclientprotocol")
                    .join("codex-acp")
                    .join("dist")
                    .join("index.js"),
                root.join("codex-bridge")
                    .join("acp")
                    .join("node_modules")
                    .join("@agentclientprotocol")
                    .join("codex-acp")
                    .join("dist")
                    .join("index.js"),
                root.join("resources")
                    .join("codex-bridge")
                    .join("acp")
                    .join("node_modules")
                    .join("@agentclientprotocol")
                    .join("codex-acp")
                    .join("dist")
                    .join("index.js"),
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
                development_bridge
                    .join("acp")
                    .join("node_modules")
                    .join("@agentclientprotocol")
                    .join("codex-acp")
                    .join("dist")
                    .join("index.js"),
            ]
            .into_iter()
            .find(|candidate| candidate.is_file())
        });
        let bundled_node = resource_root.as_ref().and_then(|root| {
            let node_name = if crate::platform::capabilities::is_windows() {
                "node.exe"
            } else {
                "node"
            };
            [
                root.join("runtime").join("node").join(node_name),
                root.join("runtime")
                    .join("codex-bridge")
                    .join("node")
                    .join("bin")
                    .join(node_name),
                root.join("codex-bridge")
                    .join("node")
                    .join("bin")
                    .join(node_name),
                root.join("resources")
                    .join("codex-bridge")
                    .join("node")
                    .join("bin")
                    .join(node_name),
                development_bridge.join("node").join("bin").join(node_name),
            ]
            .into_iter()
            .find(|candidate| candidate.is_file())
        });
        Ok(Self {
            app,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            pending_elicitations: Arc::new(Mutex::new(HashMap::new())),
            agents: SessionAgentStore::load_or_empty(),
            session_store,
            installing: Arc::new(AtomicBool::new(false)),
            login_in_progress: Arc::new(AtomicBool::new(false)),
            login_url: Arc::new(parking_lot::RwLock::new(None)),
            downloaded_bytes: Arc::new(AtomicU64::new(0)),
            download_total_bytes: Arc::new(AtomicU64::new(0)),
            last_error: Arc::new(parking_lot::RwLock::new(None)),
            runtime_probe: Arc::new(parking_lot::RwLock::new(RuntimeProbeCache::default())),
            runtime_probe_gate: Arc::new(Mutex::new(())),
            bundled_adapter,
            bundled_node,
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
        let probe = self.runtime_probe.read().clone();
        let node_version = probe.node_version;
        let node_supported = node_version
            .as_deref()
            .and_then(node_major_version)
            .is_some_and(|major| major >= 20);
        let codex = probe.codex;
        let codex_version = codex.as_ref().map(|resolved| resolved.version.clone());
        let downloaded_bytes = self.downloaded_bytes.load(Ordering::Acquire);
        let download_total_bytes = self.download_total_bytes.load(Ordering::Acquire);
        let download_progress = (download_total_bytes > 0).then(|| {
            ((downloaded_bytes.saturating_mul(100) / download_total_bytes).min(100)) as u8
        });
        let bridge_ready = adapter.is_some() && node_supported;
        let codex_available = codex.is_some();
        CodexAcpStatus {
            version: CODEX_ACP_VERSION,
            installed: bridge_ready && codex_available,
            bridge_ready,
            adapter_path: adapter.map(|path| path.to_string_lossy().into_owned()),
            node_available: node_version.is_some(),
            node_version,
            node_supported,
            npm_available: find_in_path("npm").is_some(),
            codex_available,
            codex_path: codex
                .as_ref()
                .map(|resolved| resolved.path.to_string_lossy().into_owned()),
            codex_version,
            runtime_source: codex.as_ref().map(|resolved| resolved.source.as_str()),
            managed_codex_version: MANAGED_CODEX_VERSION,
            download_required: bridge_ready && !codex_available,
            downloaded_bytes,
            download_total_bytes,
            download_progress,
            authenticated: codex_authenticated(),
            login_in_progress: self.login_in_progress.load(Ordering::Acquire),
            login_url: self.login_url.read().clone(),
            installing: self.installing.load(Ordering::Acquire),
            error: self.last_error.read().clone(),
        }
    }

    pub async fn refresh_status(&self) -> CodexAcpStatus {
        self.refresh_runtime_probe(false).await;
        self.status()
    }

    async fn refresh_runtime_probe(&self, force: bool) {
        if !force && self.runtime_probe.read().initialized {
            return;
        }
        let _gate = self.runtime_probe_gate.lock().await;
        if !force && self.runtime_probe.read().initialized {
            return;
        }

        let operation_id = diagnostics::operation_id("probe");
        let started = Instant::now();
        let adapter = self.resolve_adapter();
        let node = adapter
            .as_deref()
            .and_then(|adapter| self.resolve_node(adapter));
        let system_codex = find_in_path(if crate::platform::capabilities::is_windows() {
            "codex.cmd"
        } else {
            "codex"
        });
        let legacy_codex = adapter.as_deref().and_then(codex_path_for_adapter);
        diagnostics::write(
            &operation_id,
            "probe:start",
            format!(
                "force={force} node_path={} system_codex_path={} managed_version={MANAGED_CODEX_VERSION}",
                node.as_deref()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "none".to_string()),
                system_codex
                    .as_deref()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "none".to_string())
            ),
        );
        let detected = tokio::task::spawn_blocking(move || RuntimeProbeCache {
            initialized: true,
            node_version: node.as_deref().and_then(installed_node_version),
            codex: resolve_codex_path(system_codex, legacy_codex),
        })
        .await;

        match detected {
            Ok(probe) => {
                diagnostics::write(
                    &operation_id,
                    "probe:complete",
                    format!(
                        "elapsed_ms={} node_version={} codex_path={} codex_version={} runtime_source={}",
                        started.elapsed().as_millis(),
                        probe.node_version.as_deref().unwrap_or("none"),
                        probe
                            .codex
                            .as_ref()
                            .map(|resolved| resolved.path.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "none".to_string()),
                        probe
                            .codex
                            .as_ref()
                            .map(|resolved| resolved.version.as_str())
                            .unwrap_or("none"),
                        probe
                            .codex
                            .as_ref()
                            .map(|resolved| resolved.source.as_str())
                            .unwrap_or("none")
                    ),
                );
                *self.runtime_probe.write() = probe;
            }
            Err(error) => {
                diagnostics::write(
                    &operation_id,
                    "probe:failed",
                    format!(
                        "elapsed_ms={} error={error:#}",
                        started.elapsed().as_millis()
                    ),
                );
                *self.runtime_probe.write() = RuntimeProbeCache {
                    initialized: true,
                    ..RuntimeProbeCache::default()
                };
            }
        }
    }

    pub async fn ensure_installed(&self) -> Result<CodexAcpStatus> {
        let operation_id = diagnostics::operation_id("prepare");
        self.refresh_runtime_probe(false).await;
        let status = self.status();
        diagnostics::write(
            &operation_id,
            "prepare:start",
            format!(
                "bridge_ready={} codex_available={} installing={} runtime_source={} log_path={}",
                status.bridge_ready,
                status.codex_available,
                status.installing,
                status.runtime_source.unwrap_or("none"),
                diagnostics::log_path().display()
            ),
        );
        if !status.bridge_ready {
            diagnostics::write(
                &operation_id,
                "prepare:bridge_unavailable",
                format!(
                    "adapter_path={} node_available={} node_supported={} node_version={}",
                    status.adapter_path.as_deref().unwrap_or("none"),
                    status.node_available,
                    status.node_supported,
                    status.node_version.as_deref().unwrap_or("none")
                ),
            );
            bail!("Pinvou 安装包缺少可用的 Codex ACP Bridge，请重新安装或重新生成 Bridge Runtime");
        }
        if status.codex_available {
            diagnostics::write(&operation_id, "prepare:already_available", "result=success");
            return Ok(status);
        }
        if self.installing.swap(true, Ordering::AcqRel) {
            diagnostics::write(
                &operation_id,
                "prepare:already_installing",
                "result=rejected",
            );
            bail!("托管 Codex 正在下载，请稍候");
        }
        let result = install_managed_codex(
            self.downloaded_bytes.clone(),
            self.download_total_bytes.clone(),
            &operation_id,
        )
        .await;
        self.installing.store(false, Ordering::Release);
        match result {
            Ok(_) => {
                self.refresh_runtime_probe(true).await;
                *self.last_error.write() = None;
                diagnostics::write(&operation_id, "prepare:complete", "result=success");
                Ok(self.status())
            }
            Err(error) => {
                let detail = format!("{error:#}");
                diagnostics::write(&operation_id, "prepare:failed", &detail);
                *self.last_error.write() = Some(format!(
                    "{detail}（诊断编号：{operation_id}；日志：{}）",
                    diagnostics::log_path().display()
                ));
                Err(error)
            }
        }
    }

    pub async fn login(&self) -> Result<CodexAcpStatus> {
        self.ensure_installed().await?;
        let operation_id = diagnostics::operation_id("login");
        diagnostics::write(&operation_id, "login:start", "request=login");
        if codex_authenticated() {
            diagnostics::write(
                &operation_id,
                "login:already_authenticated",
                "result=success",
            );
            return Ok(self.status());
        }
        let adapter = self.resolve_adapter().context("Codex ACP 尚未安装")?;
        let codex = self
            .resolve_codex(&adapter)
            .context("未检测到可用 Codex；请下载托管 Codex")?;
        if self.login_in_progress.swap(true, Ordering::AcqRel) {
            diagnostics::write(
                &operation_id,
                "login:already_in_progress",
                "result=accepted",
            );
            return Ok(self.status());
        }
        *self.login_url.write() = None;
        *self.last_error.write() = None;
        let pool = self.clone();
        tokio::spawn(async move {
            if let Err(error) = pool.run_login(codex.path, &operation_id).await {
                let detail = format!("Codex 授权登录失败: {error:#}");
                diagnostics::write(&operation_id, "login:failed", &detail);
                *pool.last_error.write() = Some(format!(
                    "{detail}（诊断编号：{operation_id}；日志：{}）",
                    diagnostics::log_path().display()
                ));
            } else {
                diagnostics::write(&operation_id, "login:complete", "result=success");
            }
            pool.login_in_progress.store(false, Ordering::Release);
        });
        Ok(self.status())
    }

    pub fn open_login_url(&self) -> Result<()> {
        let url = self
            .login_url
            .read()
            .clone()
            .context("Codex 授权链接尚未生成，请稍候")?;
        if let Some(browser) = [
            "firefox",
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "brave-browser",
            "brave",
        ]
        .into_iter()
        .find_map(find_in_path)
        {
            std::process::Command::new(&browser)
                .arg("--new-window")
                .arg(&url)
                .spawn()
                .with_context(|| format!("启动浏览器失败: {}", browser.display()))?;
            eprintln!(
                "[pinvou3-app] Codex authorization page requested via {}",
                browser.display()
            );
            return Ok(());
        }
        crate::platform::os::open_target(&url, "Codex 授权页面").map_err(anyhow::Error::msg)
    }

    async fn run_login(&self, codex_path: PathBuf, operation_id: &str) -> Result<()> {
        diagnostics::write(
            operation_id,
            "login:spawn",
            format!("codex_path={}", codex_path.display()),
        );
        let mut command = codex_login_command(&codex_path);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().context("启动 Codex CLI 登录失败")?;
        let stdout = child.stdout.take().context("读取 Codex 登录标准输出失败")?;
        let stderr = child.stderr.take().context("读取 Codex 登录错误输出失败")?;
        let stdout_reader = tokio::spawn(capture_login_output(stdout, self.login_url.clone()));
        let stderr_reader = tokio::spawn(capture_login_output(stderr, self.login_url.clone()));

        let status = match tokio::time::timeout(Duration::from_secs(600), child.wait()).await {
            Ok(result) => result.context("等待 Codex 登录进程失败")?,
            Err(_) => {
                diagnostics::write(operation_id, "login:timeout", "timeout_seconds=600");
                let _ = child.kill().await;
                let _ = child.wait().await;
                bail!("授权等待超过 10 分钟，请重新登录");
            }
        };
        let _ = stdout_reader.await;
        let _ = stderr_reader.await;

        diagnostics::write(
            operation_id,
            "login:process_exit",
            format!("status={status}"),
        );

        if !status.success() {
            bail!("Codex 登录进程退出: {status}");
        }
        if !codex_authenticated() {
            bail!("Codex 登录进程已结束，但未检测到授权信息");
        }
        *self.login_url.write() = None;
        *self.last_error.write() = None;
        Ok(())
    }

    pub async fn send_message(
        &self,
        session_id: &str,
        content: String,
        attachments: Vec<crate::features::files::file_ingest::IngestResult>,
        workspace_references: Vec<String>,
    ) -> Result<()> {
        let workspace = self.execution_workspace(session_id)?;
        let workspace_references =
            workspace::resolve_workspace_references(&workspace, &workspace_references)?;
        let runtime = self.get_or_spawn(session_id).await?;
        if runtime.configuring.load(Ordering::Acquire) {
            bail!("Codex 会话配置仍在同步，请稍候再发送");
        }
        let prepared = prepare_codex_prompt(
            &content,
            &attachments,
            &workspace_references,
            &runtime.prompt_capabilities,
        )?;
        if runtime.busy.swap(true, Ordering::AcqRel) {
            bail!("Codex ACP 会话仍在生成");
        }
        let pool = self.clone();
        let session_id = session_id.to_string();
        tokio::spawn(async move {
            if runtime
                .prompt(content, prepared.blocks, prepared.display_attachments)
                .await
            {
                pool.handle_outdated_codex_runtime(&session_id).await;
            }
        });
        Ok(())
    }

    async fn handle_outdated_codex_runtime(&self, session_id: &str) {
        let operation_id = diagnostics::operation_id("runtime-upgrade");
        let current = self.runtime_probe.read().codex.clone();
        diagnostics::write(
            &operation_id,
            "upgrade_required:detected",
            format!(
                "session_id={session_id} current_source={} current_version={} managed_version={MANAGED_CODEX_VERSION}",
                current
                    .as_ref()
                    .map(|resolved| resolved.source.as_str())
                    .unwrap_or("none"),
                current
                    .as_ref()
                    .map(|resolved| resolved.version.as_str())
                    .unwrap_or("none")
            ),
        );

        let can_switch_to_managed = current
            .as_ref()
            .is_some_and(|resolved| is_managed_newer_than(&resolved.version));
        if !can_switch_to_managed {
            *self.last_error.write() = Some(format!(
                "当前 Codex {} 已无法支持所选模型，且内置托管版本 {MANAGED_CODEX_VERSION} 不更新。请升级 Pinvou 后重试。",
                current
                    .as_ref()
                    .map(|resolved| resolved.version.as_str())
                    .unwrap_or("未知版本")
            ));
            diagnostics::write(
                &operation_id,
                "upgrade_required:app_update_needed",
                "managed_runtime_not_newer",
            );
            return;
        }

        *self.last_error.write() = Some(format!(
            "当前系统 Codex 版本过旧，正在切换到托管 Codex {MANAGED_CODEX_VERSION}；完成后请重试。"
        ));
        self.evict(session_id).await;
        *self.runtime_probe.write() = RuntimeProbeCache::default();
        match self.ensure_installed().await {
            Ok(status) => {
                *self.last_error.write() = Some(format!(
                    "已切换到 Codex {}，请重新发送刚才的消息。",
                    status
                        .codex_version
                        .as_deref()
                        .unwrap_or(MANAGED_CODEX_VERSION)
                ));
                diagnostics::write(
                    &operation_id,
                    "upgrade_required:managed_ready",
                    format!(
                        "runtime_source={} version={}",
                        status.runtime_source.unwrap_or("none"),
                        status.codex_version.as_deref().unwrap_or("none")
                    ),
                );
            }
            Err(error) => {
                let detail = format!("切换托管 Codex 失败: {error:#}");
                *self.last_error.write() = Some(detail.clone());
                diagnostics::write(&operation_id, "upgrade_required:managed_failed", detail);
            }
        }
    }

    pub async fn cancel(&self, session_id: &str) {
        self.cancel_pending_permissions(session_id).await;
        self.cancel_pending_elicitations(session_id).await;
        if let Some(runtime) = self.sessions.lock().await.get(session_id).cloned() {
            runtime.cancel();
            runtime
                .bridge
                .emit("cancel_requested", json!({ "status": "cancelling" }));
        }
    }

    pub async fn evict(&self, session_id: &str) {
        self.cancel_pending_permissions(session_id).await;
        self.cancel_pending_elicitations(session_id).await;
        if let Some(runtime) = self.sessions.lock().await.remove(session_id) {
            runtime.shutdown().await;
        }
    }

    pub async fn session_info(&self, session_id: &str) -> Result<CodexAcpSessionInfo> {
        if !self.is_codex(session_id) {
            bail!("当前会话不是 Codex ACP 会话");
        }
        let pending_permissions = self.pending_permissions_for(session_id).await;
        let pending_elicitations = self.pending_elicitations_for(session_id).await;
        Ok(self
            .get_or_spawn(session_id)
            .await?
            .info(pending_permissions, pending_elicitations))
    }

    pub async fn set_model(&self, session_id: &str, model_id: &str) -> Result<CodexAcpSessionInfo> {
        let runtime = self.get_or_spawn(session_id).await?;
        runtime.set_model(model_id).await?;
        self.agents
            .set_acp_model(session_id, Some(model_id.to_string()))?;
        let info = runtime.info(
            self.pending_permissions_for(session_id).await,
            self.pending_elicitations_for(session_id).await,
        );
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
        let info = runtime.info(
            self.pending_permissions_for(session_id).await,
            self.pending_elicitations_for(session_id).await,
        );
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
        let info = runtime.info(
            self.pending_permissions_for(session_id).await,
            self.pending_elicitations_for(session_id).await,
        );
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

    pub async fn pending_elicitations_for(
        &self,
        session_id: &str,
    ) -> Vec<CodexAcpPendingElicitation> {
        self.pending_elicitations
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

    pub async fn respond_elicitation(
        &self,
        session_id: &str,
        elicitation_id: &str,
        action: &str,
        content: serde_json::Value,
    ) -> Result<()> {
        let response = match action {
            "accept" => {
                let content =
                    serde_json::from_value::<BTreeMap<String, ElicitationContentValue>>(content)
                        .context("输入答案格式不符合 ACP elicitation schema")?;
                CreateElicitationResponse::new(ElicitationAcceptAction::new().content(content))
            }
            "decline" => CreateElicitationResponse::new(ElicitationAction::Decline),
            "cancel" => CreateElicitationResponse::new(ElicitationAction::Cancel),
            _ => bail!("不支持的输入请求操作: {action}"),
        };
        let key = elicitation_key(session_id, elicitation_id);
        let request = self
            .pending_elicitations
            .lock()
            .await
            .remove(&key)
            .context("输入请求已过期、已回复或不属于当前会话")?;
        request
            .response_tx
            .send(response)
            .map_err(|_| anyhow::anyhow!("Codex ACP 输入请求已关闭"))?;
        if let Some(runtime) = self.sessions.lock().await.get(session_id).cloned() {
            runtime.bridge.emit(
                "elicitation_resolved",
                json!({
                    "elicitationId": elicitation_id,
                    "action": action,
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

    async fn cancel_pending_elicitations(&self, session_id: &str) {
        let mut pending = self.pending_elicitations.lock().await;
        let keys = pending
            .iter()
            .filter(|(_, request)| request.view.session_id == session_id)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let mut cancelled = Vec::new();
        for key in keys {
            if let Some(request) = pending.remove(&key) {
                cancelled.push(request.view.elicitation_id.clone());
                let _ = request
                    .response_tx
                    .send(CreateElicitationResponse::new(ElicitationAction::Cancel));
            }
        }
        drop(pending);
        if let Some(runtime) = self.sessions.lock().await.get(session_id).cloned() {
            for elicitation_id in cancelled {
                runtime.bridge.emit(
                    "elicitation_resolved",
                    json!({
                        "elicitationId": elicitation_id,
                        "action": "cancel",
                    }),
                );
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

        let mut command = self.adapter_command(&adapter)?;
        self.configure_codex_path(&mut command, &adapter)?;
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
        let bridge_for_elicitation = event_bridge.clone();
        let replay_for_notification = replay_suppressed.clone();
        let pending_for_permission = self.pending_permissions.clone();
        let pending_for_elicitation = self.pending_elicitations.clone();
        let pinvou_id_for_permission = pinvou_session_id.to_string();
        let pinvou_id_for_elicitation = pinvou_session_id.to_string();

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
                .on_receive_request(
                    async move |request: CreateElicitationRequest, responder, _cx| {
                        let request_value =
                            serde_json::to_value(&request).unwrap_or(serde_json::Value::Null);
                        let elicitation_id = elicitation_id_for(&request_value);
                        let key = elicitation_key(&pinvou_id_for_elicitation, &elicitation_id);
                        let cancellation = responder.cancellation();
                        let view = CodexAcpPendingElicitation {
                            session_id: pinvou_id_for_elicitation.clone(),
                            elicitation_id: elicitation_id.clone(),
                            request: request_value.clone(),
                        };
                        let (response_tx, response_rx) = oneshot::channel();
                        pending_for_elicitation
                            .lock()
                            .await
                            .insert(key.clone(), PendingElicitation { view, response_tx });
                        bridge_for_elicitation.emit(
                            "elicitation_requested",
                            json!({
                                "elicitationId": elicitation_id,
                                "request": request_value,
                            }),
                        );
                        let (response, cancelled_by_agent) = tokio::select! {
                            response = response_rx => (
                                response.unwrap_or_else(|_| {
                                    CreateElicitationResponse::new(ElicitationAction::Cancel)
                                }),
                                false,
                            ),
                            _ = cancellation.cancelled() => (
                                CreateElicitationResponse::new(ElicitationAction::Cancel),
                                true,
                            ),
                        };
                        pending_for_elicitation.lock().await.remove(&key);
                        if cancelled_by_agent {
                            bridge_for_elicitation.emit(
                                "elicitation_resolved",
                                json!({
                                    "elicitationId": elicitation_id,
                                    "action": "cancel",
                                    "reason": "agent_cancelled",
                                }),
                            );
                        }
                        responder.respond(response)
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(transport, async move |connection: ConnectionTo<Agent>| {
                    let client_capabilities = codex_client_capabilities();
                    let initialized = connection
                        .send_request(
                            InitializeRequest::new(ProtocolVersion::LATEST)
                                .client_capabilities(client_capabilities)
                                .client_info(Implementation::new(
                                    "pinvou3",
                                    env!("CARGO_PKG_VERSION"),
                                )),
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
        let (acp_session_id, mut mode_state, mut config_options) =
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
        let current_model_id = current_config_value(&config_options, "model");
        let models = codex_models(&config_options);
        let prompt_capabilities = initialized.agent_capabilities.prompt_capabilities.clone();
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
            prompt_capabilities,
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
            child: Mutex::new(child),
        })
    }

    fn resolve_adapter(&self) -> Option<PathBuf> {
        resolve_adapter_from(self.bundled_adapter.as_deref())
    }

    fn resolve_node(&self, adapter: &Path) -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("PINVOU3_CODEX_NODE_PATH").map(PathBuf::from) {
            if path.is_file() {
                return Some(path);
            }
        }
        if let Some(path) = self.bundled_node.as_ref().filter(|path| path.is_file()) {
            return Some(path.clone());
        }
        if adapter_needs_path_node_lookup(adapter, crate::platform::capabilities::is_windows()) {
            return find_in_path(if crate::platform::capabilities::is_windows() {
                "node.exe"
            } else {
                "node"
            });
        }
        None
    }

    fn resolve_codex(&self, _adapter: &Path) -> Option<ResolvedCodex> {
        self.runtime_probe.read().codex.clone()
    }

    fn adapter_command(&self, adapter: &Path) -> Result<Command> {
        adapter_command(adapter, self.resolve_node(adapter).as_deref())
    }

    fn configure_codex_path(&self, command: &mut Command, adapter: &Path) -> Result<()> {
        let codex = self
            .resolve_codex(adapter)
            .context("未检测到可用 Codex；请下载托管 Codex")?;
        command.env("CODEX_PATH", &codex.path);
        Ok(())
    }
}

async fn new_acp_session(
    connection: &ConnectionTo<Agent>,
    workspace: &Path,
) -> Result<(String, Option<SessionModeState>, Vec<SessionConfigOption>)> {
    let response = connection
        .send_request(NewSessionRequest::new(workspace))
        .block_task()
        .await
        .context("Codex ACP session/new 失败")?;
    Ok((
        response.session_id.to_string(),
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
            value_id,
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

fn current_config_value(options: &[SessionConfigOption], config_id: &str) -> Option<String> {
    options.iter().find_map(|option| {
        if option.id.to_string() != config_id {
            return None;
        }
        match &option.kind {
            SessionConfigKind::Select(select) => Some(select.current_value.to_string()),
            _ => None,
        }
    })
}

fn codex_models(options: &[SessionConfigOption]) -> Vec<CodexAcpModel> {
    let Some(model_option) = options
        .iter()
        .find(|option| option.id.to_string() == "model")
    else {
        return Vec::new();
    };
    let SessionConfigKind::Select(select) = &model_option.kind else {
        return Vec::new();
    };
    match &select.options {
        SessionConfigSelectOptions::Ungrouped(options) => options
            .iter()
            .map(|model| CodexAcpModel {
                id: model.value.to_string(),
                name: model.name.clone(),
                description: model.description.clone(),
            })
            .collect(),
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| group.options.iter())
            .map(|model| CodexAcpModel {
                id: model.value.to_string(),
                name: model.name.clone(),
                description: model.description.clone(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn managed_runtime_dir() -> PathBuf {
    crate::platform::paths::pinvou3_home()
        .join("runtimes")
        .join(format!("codex-acp-{CODEX_ACP_VERSION}"))
}

fn managed_adapter_path() -> PathBuf {
    let name = if crate::platform::capabilities::is_windows() {
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
    if crate::platform::capabilities::is_windows() {
        "codex-acp.exe"
    } else {
        "codex-acp"
    }
}

fn adapter_command(adapter: &Path, node: Option<&Path>) -> Result<Command> {
    adapter_command_for_platform(adapter, node, crate::platform::capabilities::is_windows())
}

fn adapter_command_for_platform(
    adapter: &Path,
    node: Option<&Path>,
    is_windows: bool,
) -> Result<Command> {
    if adapter.extension().and_then(|value| value.to_str()) == Some("js") {
        let node = node.context("Codex ACP Bridge 缺少可用 Node")?;
        let mut command = Command::new(node);
        command.arg(adapter);
        Ok(command)
    } else if is_windows_cmd_for_platform(adapter, is_windows) {
        let mut command = Command::new("cmd");
        command.args(["/D", "/S", "/C"]).arg(adapter);
        Ok(command)
    } else {
        Ok(Command::new(adapter))
    }
}

fn codex_login_command(codex: &Path) -> Command {
    if is_windows_cmd(codex) {
        let mut command = Command::new("cmd");
        command.args(["/D", "/S", "/C"]).arg(codex).arg("login");
        command
    } else {
        let mut command = Command::new(codex);
        command.arg("login");
        command
    }
}

fn adapter_needs_path_node_lookup(adapter: &Path, is_windows: bool) -> bool {
    is_windows || adapter.extension().and_then(|value| value.to_str()) == Some("js")
}

fn is_windows_cmd(path: &Path) -> bool {
    is_windows_cmd_for_platform(path, crate::platform::capabilities::is_windows())
}

fn is_windows_cmd_for_platform(path: &Path, is_windows: bool) -> bool {
    is_windows && path.extension().and_then(|value| value.to_str()) == Some("cmd")
}

async fn capture_login_output<R>(reader: R, login_url: Arc<parking_lot::RwLock<Option<String>>>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(url) = extract_codex_login_url(&line) {
            *login_url.write() = Some(url.to_string());
        }
    }
}

fn extract_codex_login_url(line: &str) -> Option<&str> {
    line.split_whitespace().find(|token| {
        token.starts_with("https://auth.openai.com/")
            || token.starts_with("https://platform.openai.com/")
    })
}

fn codex_path_for_adapter(adapter: &Path) -> Option<PathBuf> {
    let name = if crate::platform::capabilities::is_windows() {
        "codex.cmd"
    } else {
        "codex"
    };
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
        if nonempty_file(&path) {
            return Some(path);
        }
    }
    if let Some(path) = bundled {
        if nonempty_file(path) {
            return Some(path.to_path_buf());
        }
    }
    let managed = managed_adapter_path();
    if nonempty_file(&managed) {
        return Some(managed);
    }
    find_in_path(if crate::platform::capabilities::is_windows() {
        "codex-acp.cmd"
    } else {
        "codex-acp"
    })
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| nonempty_file(candidate))
}

fn nonempty_file(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

fn codex_upgrade_required(message: &str) -> bool {
    message
        .to_ascii_lowercase()
        .contains("requires a newer version of codex")
}

fn installed_node_version(node: &Path) -> Option<String> {
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

fn elicitation_key(session_id: &str, elicitation_id: &str) -> String {
    format!("{session_id}\u{1f}{elicitation_id}")
}

fn elicitation_id_for(request: &serde_json::Value) -> String {
    static NEXT_ELICITATION_ID: AtomicU64 = AtomicU64::new(1);
    request
        .get("toolCallId")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            format!(
                "elicitation-{}",
                NEXT_ELICITATION_ID.fetch_add(1, Ordering::Relaxed)
            )
        })
}

fn codex_client_capabilities() -> ClientCapabilities {
    ClientCapabilities::new()
        .elicitation(ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()))
}

fn codex_authenticated() -> bool {
    if std::env::var_os("OPENAI_API_KEY").is_some() {
        return true;
    }
    let home = crate::platform::os::user_home_dir();
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
    fn empty_adapter_file_is_not_treated_as_installed() {
        let root =
            std::env::temp_dir().join(format!("pinvou3-codex-adapter-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create adapter test directory");
        let adapter = root.join("codex-acp.js");
        std::fs::File::create(&adapter).expect("empty adapter");
        assert!(!nonempty_file(&adapter));
        std::fs::write(&adapter, "console.log('ok');").expect("write adapter");
        assert!(nonempty_file(&adapter));
        std::fs::remove_dir_all(root).expect("cleanup adapter test directory");
    }

    #[test]
    fn javascript_adapter_uses_node_runtime() {
        let adapter = Path::new("C:\\runtime\\codex-acp.js");
        let command =
            adapter_command_for_platform(adapter, Some(Path::new("C:\\runtime\\node.exe")), true)
                .expect("build JavaScript adapter command");

        assert_eq!(command.as_std().get_program(), "C:\\runtime\\node.exe");
        assert_eq!(
            command
                .as_std()
                .get_args()
                .map(|value| value.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["C:\\runtime\\codex-acp.js"]
        );
    }

    #[test]
    fn windows_cmd_adapter_uses_command_interpreter() {
        let adapter = Path::new("C:\\runtime\\codex-acp.cmd");
        let command = adapter_command_for_platform(adapter, None, true)
            .expect("build Windows command-shim adapter command");

        assert_eq!(command.as_std().get_program(), "cmd");
        assert_eq!(
            command
                .as_std()
                .get_args()
                .map(|value| value.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["/D", "/S", "/C", "C:\\runtime\\codex-acp.cmd"]
        );
    }

    #[test]
    fn windows_adapter_status_always_checks_node_runtime() {
        assert!(adapter_needs_path_node_lookup(
            Path::new("C:\\runtime\\codex-acp.cmd"),
            true
        ));
        assert!(adapter_needs_path_node_lookup(
            Path::new("/runtime/codex-acp.js"),
            false
        ));
        assert!(!adapter_needs_path_node_lookup(
            Path::new("/runtime/codex-acp"),
            false
        ));
    }

    #[test]
    fn permission_key_is_scoped_to_session() {
        assert_ne!(
            permission_key("session-a", "tool-1"),
            permission_key("session-b", "tool-1")
        );
    }

    #[test]
    fn elicitation_key_is_scoped_and_prefers_tool_call_id() {
        assert_ne!(
            elicitation_key("session-a", "input-1"),
            elicitation_key("session-b", "input-1")
        );
        assert_eq!(
            elicitation_id_for(&json!({ "toolCallId": "request-user-input-1" })),
            "request-user-input-1"
        );
        assert!(elicitation_id_for(&json!({})).starts_with("elicitation-"));
    }

    #[test]
    fn advertises_form_elicitation_to_codex_acp() {
        let value = serde_json::to_value(codex_client_capabilities()).unwrap();
        assert_eq!(value["elicitation"]["form"], json!({}));
        assert!(value["elicitation"].get("url").is_none());
    }

    #[test]
    fn extracts_only_codex_authorization_urls() {
        assert_eq!(
            extract_codex_login_url(
                "https://auth.openai.com/oauth/authorize?response_type=code&state=test"
            ),
            Some("https://auth.openai.com/oauth/authorize?response_type=code&state=test")
        );
        assert_eq!(
            extract_codex_login_url("open https://platform.openai.com/codex/auth now"),
            Some("https://platform.openai.com/codex/auth")
        );
        assert_eq!(
            extract_codex_login_url("https://example.com/not-codex"),
            None
        );
    }

    #[test]
    fn detects_server_request_for_newer_codex_runtime() {
        assert!(codex_upgrade_required(
            "The 'gpt-5.6-sol' model requires a newer version of Codex."
        ));
        assert!(!codex_upgrade_required("Codex ACP connection closed"));
    }
}
