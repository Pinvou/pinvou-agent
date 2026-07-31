//! 外部 Agent ACP 运行时。
//!
//! pinvou3 只做 ACP client、进程托管、权限路由、事件持久化和 `acp:event` 投影；
//! Codex、Claude Code 与 Kimi 的模型调用、工具循环、会话与权限协议都由各自
//! ACP Agent 提供。

mod attachments;
mod diagnostics;
mod events;
mod platform;
mod runtime;
mod store;
pub(crate) mod workspace;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::Read;
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
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use wait_timeout::ChildExt;

use crate::features::sessions::SessionStore;
use attachments::{prepare_codex_prompt, CodexDisplayAttachment};
use deepseek_tui::session_manager::SessionMetadata;
pub use events::AcpEventEnvelope;
use events::{load_timeline, patch_acp_state, persist_acp_state, EventBridge};
use runtime::{
    install_managed_codex, is_managed_newer_than, resolve_codex_path, system_codex_incompatible,
    ResolvedCodex, MANAGED_CODEX_VERSION, MIN_CODEX_VERSION,
};
pub use store::{
    validate_codex_project_workspace, AgentBackend, CodexWorkspaceKind, SessionAgentStore,
};
use store::{AcpConfigDefaultsStore, SessionAgentRecord};

pub const CODEX_ACP_VERSION: &str = "1.1.5";
pub const CODEX_ACP_SESSION_MODEL: &str = "Codex (ACP)";
const CODEX_ACP_PACKAGE: &str = "@agentclientprotocol/codex-acp";
pub const CLAUDE_ACP_VERSION: &str = "0.62.0";
const CLAUDE_ACP_PACKAGE: &str = "@agentclientprotocol/claude-agent-acp";
const CLAUDE_ACP_SESSION_MODEL: &str = "Claude Code (ACP)";
const KIMI_ACP_PACKAGE: &str = "kimi acp";
const KIMI_ACP_SESSION_MODEL: &str = "Kimi (ACP)";

fn backend_for_session_model(model: &str) -> Option<AgentBackend> {
    match model {
        CODEX_ACP_SESSION_MODEL => Some(AgentBackend::CodexAcp),
        CLAUDE_ACP_SESSION_MODEL => Some(AgentBackend::ClaudeAcp),
        KIMI_ACP_SESSION_MODEL => Some(AgentBackend::KimiAcp),
        _ => None,
    }
}

fn acp_session_backend(backend: AgentBackend, model: &str) -> Option<AgentBackend> {
    backend
        .is_acp()
        .then_some(backend)
        .or_else(|| backend_for_session_model(model))
}

fn same_workspace(left: &Path, right: &Path) -> bool {
    left == right
        || left
            .canonicalize()
            .ok()
            .zip(right.canonicalize().ok())
            .is_some_and(|(left, right)| left == right)
}

fn backend_for_acp_state(state: &Value) -> Result<AgentBackend> {
    let package_backend = match state["adapter"]["package"].as_str() {
        Some(CODEX_ACP_PACKAGE) => Some(AgentBackend::CodexAcp),
        Some(CLAUDE_ACP_PACKAGE) => Some(AgentBackend::ClaudeAcp),
        Some(KIMI_ACP_PACKAGE) => Some(AgentBackend::KimiAcp),
        Some(other) => bail!("acp-state.json 包含未知 ACP adapter package: {other}"),
        None => None,
    };
    let agent_backend = state["adapter"]["agentId"]
        .as_str()
        .map(|agent_id| AgentBackend::parse(Some(agent_id)))
        .transpose()?
        .filter(|backend| backend.is_acp());
    if let (Some(package), Some(agent)) = (package_backend, agent_backend) {
        if package != agent {
            bail!("acp-state.json 的 Agent 与 adapter package 不匹配");
        }
    }
    agent_backend
        .or(package_backend)
        .context("acp-state.json 缺少可识别的 ACP Agent")
}

fn acp_mode_from_state(state: &Value) -> Option<String> {
    acp_config_values_from_state(state)
        .remove("mode")
        .or_else(|| {
            state["session"]["modes"]["currentModeId"]
                .as_str()
                .map(str::to_string)
        })
}

fn acp_config_values_from_state(state: &Value) -> HashMap<String, String> {
    state["session"]["config_options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|option| {
            Some((
                option["id"].as_str()?.to_string(),
                option["currentValue"].as_str()?.to_string(),
            ))
        })
        .collect()
}

fn saved_config_values(record: &SessionAgentRecord) -> HashMap<String, String> {
    let mut values = record.acp_config_values.clone();
    if let Some(model_id) = &record.acp_model_id {
        values
            .entry("model".to_string())
            .or_insert_with(|| model_id.clone());
    }
    if let Some(mode_id) = &record.acp_mode_id {
        values
            .entry("mode".to_string())
            .or_insert_with(|| mode_id.clone());
    }
    values
}

fn load_acp_config_values_from_state(
    session_id: &str,
    expected_backend: AgentBackend,
) -> Result<HashMap<String, String>> {
    let state_path = crate::platform::paths::sessions_root()
        .join(session_id)
        .join("acp-state.json");
    let state: Value = serde_json::from_slice(
        &std::fs::read(&state_path)
            .with_context(|| format!("读取 {} 失败", state_path.display()))?,
    )
    .with_context(|| format!("解析 {} 失败", state_path.display()))?;
    if backend_for_acp_state(&state)? != expected_backend {
        bail!("acp-state.json 的 Agent 与会话元数据不匹配");
    }
    Ok(acp_config_values_from_state(&state))
}

fn acp_recovery_record(
    pinvou_session_id: &str,
    expected_backend: AgentBackend,
    state: &Value,
    workspace_path: PathBuf,
    temporary_workspace: &Path,
) -> Result<SessionAgentRecord> {
    if state["pinvouSessionId"].as_str() != Some(pinvou_session_id) {
        bail!("acp-state.json 的 Pinvou 会话 ID 不匹配");
    }
    if !expected_backend.is_acp() {
        bail!("会话元数据不是 ACP Agent");
    }
    let state_backend = backend_for_acp_state(state)?;
    if state_backend != expected_backend {
        bail!(
            "acp-state.json 的 Agent {} 与会话元数据 {} 不匹配",
            state_backend.display_name(),
            expected_backend.display_name()
        );
    }
    let acp_session_id = state["session"]["session_id"]
        .as_str()
        .filter(|value| !value.is_empty())
        .context("acp-state.json 缺少原 ACP session id")?
        .to_string();
    if !workspace_path.is_absolute() {
        bail!("ACP 工作目录记录不是绝对路径");
    }
    let workspace_kind = match state["workspace"]["kind"].as_str() {
        Some("temporary") => {
            if !same_workspace(&workspace_path, temporary_workspace) {
                bail!("acp-state.json 的临时工作目录与会话目录不匹配");
            }
            CodexWorkspaceKind::Temporary
        }
        Some("project") => CodexWorkspaceKind::Project,
        Some(other) => bail!("acp-state.json 包含未知工作目录类型: {other}"),
        None if same_workspace(&workspace_path, temporary_workspace) => {
            CodexWorkspaceKind::Temporary
        }
        None => CodexWorkspaceKind::Project,
    };
    Ok(SessionAgentRecord {
        backend: expected_backend,
        acp_session_id: Some(acp_session_id),
        acp_model_id: state["session"]["current_model_id"]
            .as_str()
            .map(str::to_string),
        acp_mode_id: acp_mode_from_state(state),
        acp_config_values: acp_config_values_from_state(state),
        workspace_kind,
        workspace_path: (workspace_kind == CodexWorkspaceKind::Project).then_some(workspace_path),
    })
}

fn load_acp_recovery_record(
    session_id: &str,
    expected_backend: AgentBackend,
    session_store: &SessionStore,
) -> Result<SessionAgentRecord> {
    let temporary_workspace = session_store.execution_workspace(session_id)?;
    let session_root = crate::platform::paths::sessions_root().join(session_id);
    let state_path = session_root.join("acp-state.json");
    let state: Value = serde_json::from_slice(
        &std::fs::read(&state_path)
            .with_context(|| format!("读取 {} 失败", state_path.display()))?,
    )
    .with_context(|| format!("解析 {} 失败", state_path.display()))?;
    let workspace_path = if let Some(path) = state["workspace"]["path"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        PathBuf::from(path)
    } else {
        let baseline_path = session_root.join("codex-workspace-baseline.json");
        let baseline: Value = serde_json::from_slice(
            &std::fs::read(&baseline_path)
                .with_context(|| format!("读取 {} 失败", baseline_path.display()))?,
        )
        .with_context(|| format!("解析 {} 失败", baseline_path.display()))?;
        baseline["workspace_path"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .context("Codex 工作区基线缺少 workspace_path")?
    };
    acp_recovery_record(
        session_id,
        expected_backend,
        &state,
        workspace_path,
        &temporary_workspace,
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct CodexAcpStatus {
    pub agent_id: &'static str,
    pub agent_name: &'static str,
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
    /// codex-acp 验证过的最低 Codex CLI 版本（所有运行时来源统一强制）。
    pub min_codex_version: &'static str,
    /// 当前平台的 Codex 安装方式：
    /// "managed_download"（linux/windows）/ "homebrew"（macOS）/ "manual"（其他）。
    pub install_method: &'static str,
    /// 仅 macOS 探测 Homebrew；其他平台恒 false。
    pub brew_available: bool,
    /// 系统 PATH 里找到了 codex 但版本低于 min_codex_version，
    /// 用于 UI 区分「版本过低」与「未安装」。
    pub system_codex_incompatible: bool,
    pub download_required: bool,
    pub downloaded_bytes: u64,
    pub download_total_bytes: u64,
    pub download_progress: Option<u8>,
    pub authenticated: bool,
    pub login_in_progress: bool,
    pub login_url: Option<String>,
    pub login_code: Option<String>,
    pub login_input_required: bool,
    pub installing: bool,
    pub error: Option<String>,
    /// 稳定的英文提示代码，前端映射 i18n 文案：
    /// "kimi_cli_missing" / "kimi_auth_required" / "claude_auth_required"。
    pub setup_hint: Option<&'static str>,
}

#[derive(Debug, Clone, Default)]
struct AgentLoginState {
    in_progress: bool,
    url: Option<String>,
    code: Option<String>,
    input_required: bool,
    error: Option<String>,
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

#[derive(Debug)]
struct KimiDiagnosticCursor {
    session_id: String,
    log_path: Option<PathBuf>,
    offset: u64,
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
    kimi_session_id: Option<String>,
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
            bail!("ACP Agent 未上报会话模式: {mode_id}");
        }
        self.connection
            .send_request(SetSessionModeRequest::new(
                self.acp_session_id.clone(),
                mode_id.to_string(),
            ))
            .block_task()
            .await
            .context("ACP session/set_mode 失败")?;
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
        let kimi_diagnostic_cursor = match self.kimi_session_id.as_deref() {
            Some(session_id) => Some(kimi_diagnostic_cursor(session_id).await),
            None => None,
        };
        let result = self
            .connection
            .send_request(PromptRequest::new(self.acp_session_id.clone(), blocks))
            .block_task()
            .await;
        self.busy.store(false, Ordering::Release);
        match result {
            Ok(response) => {
                // Kimi ACP 将普通 provider failure 映射成 end_turn，详细错误只写入
                // 会话日志。只读取本回合新增日志中的明确失败标记，避免把正常空回复
                // 或历史错误误判为本次失败。
                if let Some(error) = match kimi_diagnostic_cursor {
                    Some(cursor) => kimi_failure_after(&cursor).await,
                    None => None,
                } {
                    crate::features::assistant::timing::finish_turn(
                        &self.bridge_session_id(),
                        "Failed",
                        Some(&error),
                    );
                    self.bridge.finish_turn(&turn_id, "Failed", Some(&error));
                    return false;
                }
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
                let message = format!("ACP Agent: {error}");
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
        let current_model = current_config_value(&options, "model");
        *self.config_options.write() = options;
        *self.current_model.write() = current_model;
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
        let current_model = current_config_value(&options, "model");
        *self.config_options.write() = options;
        *self.current_model.write() = current_model;
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
    config_defaults: AcpConfigDefaultsStore,
    acp_metadata_backends: Arc<parking_lot::RwLock<HashMap<String, AgentBackend>>>,
    session_store: SessionStore,
    installing: Arc<AtomicBool>,
    login_states: Arc<parking_lot::RwLock<HashMap<AgentBackend, AgentLoginState>>>,
    login_inputs: Arc<Mutex<HashMap<AgentBackend, ChildStdin>>>,
    downloaded_bytes: Arc<AtomicU64>,
    download_total_bytes: Arc<AtomicU64>,
    last_error: Arc<parking_lot::RwLock<Option<String>>>,
    runtime_probe: Arc<parking_lot::RwLock<RuntimeProbeCache>>,
    runtime_probe_gate: Arc<Mutex<()>>,
    bundled_adapter: Option<PathBuf>,
    bundled_claude_adapter: Option<PathBuf>,
    bundled_node: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
struct RuntimeProbeCache {
    initialized: bool,
    node_version: Option<String>,
    codex: Option<ResolvedCodex>,
    brew_available: bool,
    system_codex_incompatible: bool,
}

impl AcpPool {
    pub fn new(app: AppHandle, session_store: SessionStore) -> Result<Self> {
        let resource_root = app.path().resource_dir().ok();
        let development_bridge =
            platform::development_bridge_root(Path::new(env!("CARGO_MANIFEST_DIR")));
        let bundled_adapter = resource_root.as_ref().and_then(|root| {
            bundled_adapter_candidates(root, &development_bridge, "codex-acp")
                .into_iter()
                .find(|candidate| candidate.is_file())
        });
        let bundled_claude_adapter = resource_root.as_ref().and_then(|root| {
            bundled_adapter_candidates(root, &development_bridge, "claude-agent-acp")
                .into_iter()
                .find(|candidate| candidate.is_file())
        });
        let bundled_node = resource_root.as_ref().and_then(|root| {
            let node_name = platform::node_executable_name();
            let bridge_node = platform::bridge_node_relative_path();
            [
                root.join("runtime").join("node").join(node_name),
                root.join("runtime").join("codex-bridge").join(&bridge_node),
                root.join("codex-bridge").join(&bridge_node),
                root.join("resources")
                    .join("codex-bridge")
                    .join(&bridge_node),
                development_bridge.join(&bridge_node),
            ]
            .into_iter()
            .find(|candidate| candidate.is_file())
        });
        let agents = SessionAgentStore::load_or_empty();
        let metadata = session_store.list().unwrap_or_else(|error| {
            eprintln!("[pinvou3-app] preload Codex session metadata failed: {error:#}");
            Vec::new()
        });
        let acp_metadata_backends = metadata
            .iter()
            .filter_map(|metadata| {
                acp_session_backend(agents.backend(&metadata.id), &metadata.model)
                    .map(|backend| (metadata.id.clone(), backend))
            })
            .collect::<HashMap<_, _>>();
        for (session_id, backend) in &acp_metadata_backends {
            if agents.backend(session_id).is_acp() {
                continue;
            }
            match load_acp_recovery_record(session_id, *backend, &session_store)
                .and_then(|record| agents.restore_missing_acp_record(session_id, record))
            {
                Ok(()) => eprintln!(
                    "[pinvou3-app] recovered {} ACP session index for {session_id}",
                    backend.display_name()
                ),
                Err(error) => eprintln!(
                    "[pinvou3-app] {} ACP session {session_id} remains read-only until its index can be recovered: {error:#}",
                    backend.display_name()
                ),
            }
        }
        let config_defaults = AcpConfigDefaultsStore::load_or_empty();
        // 旧版本只保存了 session 级状态。按更新时间从新到旧，为每个 Agent
        // 迁移最近一次成功配置，使升级后的第一个新会话也能继承用户选择。
        for item in &metadata {
            let Some(backend) = acp_metadata_backends.get(&item.id).copied() else {
                continue;
            };
            if config_defaults.has_backend(backend) {
                continue;
            }
            let values = load_acp_config_values_from_state(&item.id, backend)
                .unwrap_or_else(|_| saved_config_values(&agents.get(&item.id)));
            match config_defaults.set_all_if_absent(backend, values) {
                Ok(true) => eprintln!(
                    "[pinvou3-app] migrated {} ACP defaults from session {}",
                    backend.display_name(),
                    item.id
                ),
                Ok(false) => {}
                Err(error) => eprintln!(
                    "[pinvou3-app] failed to migrate {} ACP defaults: {error:#}",
                    backend.display_name()
                ),
            }
        }
        // 新进程无法继续持有上次进程里的 ACP prompt future。恢复原 Agent session
        // 只恢复对话上下文，不会重新挂接当时正在等待的 prompt；因此必须在任何
        // runtime lazy spawn 之前，把 timeline 中遗留的 running 回合收口。
        for session_id in acp_metadata_backends.keys() {
            let bridge = EventBridge::new(app.clone(), session_id.clone());
            let interrupted = bridge.interrupt_orphaned_turns("application_restarted");
            if interrupted > 0 {
                eprintln!(
                    "[pinvou3-app] interrupted {interrupted} orphaned ACP turn(s) for {session_id}"
                );
            }
        }
        Ok(Self {
            app,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            pending_elicitations: Arc::new(Mutex::new(HashMap::new())),
            agents,
            config_defaults,
            acp_metadata_backends: Arc::new(parking_lot::RwLock::new(acp_metadata_backends)),
            session_store,
            installing: Arc::new(AtomicBool::new(false)),
            login_states: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            login_inputs: Arc::new(Mutex::new(HashMap::new())),
            downloaded_bytes: Arc::new(AtomicU64::new(0)),
            download_total_bytes: Arc::new(AtomicU64::new(0)),
            last_error: Arc::new(parking_lot::RwLock::new(None)),
            runtime_probe: Arc::new(parking_lot::RwLock::new(RuntimeProbeCache::default())),
            runtime_probe_gate: Arc::new(Mutex::new(())),
            bundled_adapter,
            bundled_claude_adapter,
            bundled_node,
        })
    }

    pub fn agents(&self) -> &SessionAgentStore {
        &self.agents
    }

    /// 会话类型以 ACP 辅助索引为主，并用 SavedSession 中持久化的 Agent 模型类型兜底。
    ///
    /// `session-agents.json` 是可重建的辅助索引，缺失或损坏时不能让历史 Codex
    /// / Claude / Kimi 会话掉回普通聊天列表；创建会话时写入的 `* (ACP)` 元数据
    /// 是长期兼容依据。列表调用已经持有 metadata，应使用本方法避免重复读取 transcript。
    pub fn is_acp_metadata(&self, metadata: &SessionMetadata) -> bool {
        let backend = self.agents.backend(&metadata.id);
        let Some(backend) = acp_session_backend(backend, &metadata.model) else {
            return false;
        };
        if !self.acp_metadata_backends.read().contains_key(&metadata.id) {
            self.acp_metadata_backends
                .write()
                .insert(metadata.id.clone(), backend);
        }
        true
    }

    pub fn is_acp(&self, session_id: &str) -> bool {
        self.agents.backend(session_id).is_acp()
            || self.acp_metadata_backends.read().contains_key(session_id)
    }

    pub fn backend(&self, session_id: &str) -> AgentBackend {
        let backend = self.agents.backend(session_id);
        if backend.is_acp() {
            backend
        } else {
            self.acp_metadata_backends
                .read()
                .get(session_id)
                .copied()
                .unwrap_or(backend)
        }
    }

    fn acp_record(&self, session_id: &str) -> Result<SessionAgentRecord> {
        let record = self.agents.get(session_id);
        if record.backend.is_acp() {
            return Ok(record);
        }
        if let Some(backend) = self.acp_metadata_backends.read().get(session_id).copied() {
            bail!(
                "{} 会话辅助索引缺失，且无法从 acp-state.json 与工作区基线完整恢复；\
                 为避免在错误目录新建上下文，当前会话仅允许查看历史",
                backend.display_name()
            );
        }
        bail!("会话不是 ACP 会话")
    }

    pub fn workspace_info(&self, session_id: &str) -> Result<CodexAcpWorkspaceInfo> {
        let record = self.agents.get(session_id);
        if !record.backend.is_acp() {
            if self.acp_metadata_backends.read().contains_key(session_id) {
                return Ok(CodexAcpWorkspaceInfo {
                    workspace_kind: CodexWorkspaceKind::Temporary,
                    workspace_path: "辅助索引缺失，无法安全恢复原工作目录".to_string(),
                    workspace_available: false,
                });
            }
            bail!("会话不是 ACP 会话");
        }
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
        let record = self.acp_record(session_id)?;
        match record.workspace_kind {
            CodexWorkspaceKind::Temporary => self
                .session_store
                .execution_workspace(session_id)
                .with_context(|| format!("解析会话 {session_id} 临时工作目录失败")),
            CodexWorkspaceKind::Project => {
                let path = record.workspace_path.with_context(|| {
                    format!("{} 项目会话缺少工作目录记录", record.backend.display_name())
                })?;
                validate_codex_project_workspace(&path).with_context(|| {
                    format!(
                        "{} 会话绑定的项目目录已不可用: {}。请恢复该目录，或新建会话选择其他项目",
                        record.backend.display_name(),
                        path.display()
                    )
                })
            }
        }
    }

    pub fn status(&self) -> CodexAcpStatus {
        self.status_for(AgentBackend::CodexAcp)
    }

    /// status_for 会同步 spawn 子进程探测 CLI（--version / auth status），
    /// async 上下文必须经 spawn_blocking，避免阻塞 tokio worker。
    async fn status_for_async(&self, backend: AgentBackend) -> CodexAcpStatus {
        let pool = self.clone();
        tokio::task::spawn_blocking(move || pool.status_for(backend))
            .await
            .expect("ACP 状态探测任务异常退出")
    }

    async fn status_async(&self) -> CodexAcpStatus {
        self.status_for_async(AgentBackend::CodexAcp).await
    }

    async fn agent_authenticated_async(&self, backend: AgentBackend, executable: &Path) -> bool {
        let pool = self.clone();
        let executable = executable.to_path_buf();
        tokio::task::spawn_blocking(move || pool.agent_authenticated(backend, &executable))
            .await
            .unwrap_or(false)
    }

    pub fn status_for_agent(&self, agent_id: &str) -> Result<CodexAcpStatus> {
        let backend = AgentBackend::parse(Some(agent_id))?;
        if !backend.is_acp() {
            bail!("Agent 不是 ACP 后端: {agent_id}");
        }
        Ok(self.status_for(backend))
    }

    pub fn agent_statuses(&self) -> Vec<CodexAcpStatus> {
        [
            AgentBackend::CodexAcp,
            AgentBackend::ClaudeAcp,
            AgentBackend::KimiAcp,
        ]
        .into_iter()
        .map(|backend| self.status_for(backend))
        .collect()
    }

    fn status_for(&self, backend: AgentBackend) -> CodexAcpStatus {
        let login = self
            .login_states
            .read()
            .get(&backend)
            .cloned()
            .unwrap_or_default();
        if backend == AgentBackend::KimiAcp {
            let kimi = resolve_kimi_path();
            let kimi_version = kimi.as_deref().and_then(command_version);
            let authenticated =
                !login.in_progress && kimi.as_deref().is_some_and(kimi_authenticated);
            return CodexAcpStatus {
                agent_id: "kimi",
                agent_name: "Kimi",
                version: "native",
                installed: kimi.is_some(),
                bridge_ready: kimi.is_some(),
                adapter_path: kimi
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
                node_available: true,
                node_version: None,
                node_supported: true,
                npm_available: find_in_path("npm").is_some(),
                codex_available: kimi.is_some(),
                codex_path: kimi
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
                codex_version: kimi_version,
                runtime_source: kimi.as_ref().map(|_| "system"),
                managed_codex_version: "",
                min_codex_version: "",
                install_method: "manual",
                brew_available: false,
                system_codex_incompatible: false,
                download_required: false,
                downloaded_bytes: 0,
                download_total_bytes: 0,
                download_progress: None,
                authenticated,
                login_in_progress: login.in_progress,
                login_url: login.url,
                login_code: login.code,
                login_input_required: false,
                installing: false,
                error: (!authenticated).then_some(login.error).flatten(),
                setup_hint: if kimi.is_none() {
                    Some("kimi_cli_missing")
                } else if !authenticated {
                    Some("kimi_auth_required")
                } else {
                    None
                },
            };
        }

        let (agent_id, agent_name, version, adapter) = match backend {
            AgentBackend::CodexAcp => ("codex", "Codex", CODEX_ACP_VERSION, self.resolve_adapter()),
            AgentBackend::ClaudeAcp => (
                "claude",
                "Claude Code",
                CLAUDE_ACP_VERSION,
                self.resolve_claude_adapter(),
            ),
            AgentBackend::Deepseek | AgentBackend::KimiAcp => unreachable!(),
        };
        let probe = (backend == AgentBackend::CodexAcp).then(|| self.runtime_probe.read().clone());
        let node_version = if let Some(probe) = probe.as_ref() {
            probe.node_version.clone()
        } else {
            adapter
                .as_deref()
                .and_then(|adapter| self.resolve_node(adapter))
                .as_deref()
                .and_then(installed_node_version)
        };
        let node_supported = node_version
            .as_deref()
            .and_then(node_major_version)
            .is_some_and(|major| major >= 20);
        let codex = probe.as_ref().and_then(|probe| probe.codex.clone());
        let claude = (backend == AgentBackend::ClaudeAcp)
            .then(|| adapter.as_deref().and_then(resolve_claude_cli_from_adapter))
            .flatten();
        let provider_available = match backend {
            AgentBackend::CodexAcp => codex.is_some(),
            AgentBackend::ClaudeAcp => claude.is_some(),
            AgentBackend::Deepseek | AgentBackend::KimiAcp => unreachable!(),
        };
        let provider_version = if backend == AgentBackend::CodexAcp {
            codex.as_ref().map(|resolved| resolved.version.clone())
        } else {
            claude.as_deref().and_then(command_version)
        };
        let downloaded_bytes = self.downloaded_bytes.load(Ordering::Acquire);
        let download_total_bytes = self.download_total_bytes.load(Ordering::Acquire);
        let download_progress = (download_total_bytes > 0).then(|| {
            ((downloaded_bytes.saturating_mul(100) / download_total_bytes).min(100)) as u8
        });
        let bridge_ready = adapter.is_some() && node_supported;
        let codex_available = provider_available;
        let installed = bridge_ready && codex_available;
        // 登录命令运行期间不要再启动同一 CLI 的 auth status。部分 CLI 会让两条
        // 命令争用凭证锁，原来的 750ms 状态轮询因此可能拖住 Tauri 的 IPC/UI。
        let authenticated = !login.in_progress
            && match backend {
                AgentBackend::CodexAcp => codex
                    .as_ref()
                    .is_some_and(|resolved| codex_authenticated(&resolved.path)),
                AgentBackend::ClaudeAcp => claude.as_deref().is_some_and(claude_authenticated),
                AgentBackend::Deepseek | AgentBackend::KimiAcp => unreachable!(),
            };
        CodexAcpStatus {
            agent_id,
            agent_name,
            version,
            installed,
            bridge_ready,
            adapter_path: adapter
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            node_available: node_version.is_some(),
            node_version,
            node_supported,
            npm_available: find_in_path("npm").is_some(),
            codex_available,
            codex_path: if backend == AgentBackend::CodexAcp {
                codex
                    .as_ref()
                    .map(|resolved| resolved.path.to_string_lossy().into_owned())
            } else {
                claude
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
            },
            codex_version: provider_version,
            runtime_source: if backend == AgentBackend::CodexAcp {
                codex.as_ref().map(|resolved| resolved.source.as_str())
            } else {
                // Claude adapter 可能来自内置资源或 PATH，按实际来源上报。
                adapter.as_deref().map(|path| {
                    if self.bundled_claude_adapter.as_deref() == Some(path) {
                        "bundled"
                    } else {
                        "system"
                    }
                })
            },
            managed_codex_version: if backend == AgentBackend::CodexAcp {
                MANAGED_CODEX_VERSION
            } else {
                ""
            },
            min_codex_version: if backend == AgentBackend::CodexAcp {
                MIN_CODEX_VERSION
            } else {
                ""
            },
            install_method: if backend == AgentBackend::CodexAcp {
                platform::install_method()
            } else {
                "manual"
            },
            brew_available: probe.as_ref().is_some_and(|probe| probe.brew_available),
            system_codex_incompatible: probe
                .as_ref()
                .is_some_and(|probe| probe.system_codex_incompatible),
            download_required: backend == AgentBackend::CodexAcp
                && bridge_ready
                && !codex_available,
            downloaded_bytes,
            download_total_bytes,
            download_progress,
            authenticated,
            login_in_progress: login.in_progress,
            login_url: login.url,
            login_code: login.code,
            login_input_required: login.input_required,
            installing: backend == AgentBackend::CodexAcp
                && self.installing.load(Ordering::Acquire),
            error: if authenticated {
                None
            } else {
                login.error.or_else(|| {
                    (backend == AgentBackend::CodexAcp)
                        .then(|| self.last_error.read().clone())
                        .flatten()
                })
            },
            // installed=false 多为桥或 Node 缺失，不属于认证问题，不给认证类提示。
            setup_hint: if backend == AgentBackend::ClaudeAcp && installed && !authenticated {
                Some("claude_auth_required")
            } else {
                None
            },
        }
    }

    pub async fn refresh_status(&self) -> CodexAcpStatus {
        self.refresh_runtime_probe(false).await;
        self.status_async().await
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
        let system_codex = find_in_path(platform::system_codex_name());
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
            codex: resolve_codex_path(system_codex.clone(), legacy_codex),
            brew_available: platform::brew_available(),
            system_codex_incompatible: system_codex_incompatible(system_codex),
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
        let status = self.status_async().await;
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
        match platform::install_method() {
            "managed_download" => {}
            "homebrew" => {
                // macOS 不提供托管下载产物，引导用户走 Homebrew 安装系统 Codex。
                diagnostics::write(
                    &operation_id,
                    "prepare:managed_download_unsupported",
                    "install_method=homebrew",
                );
                bail!(
                    "macOS 暂不支持下载托管 Codex，请点击「使用 Homebrew 安装」自动安装，\
                     或先安装 Homebrew（https://brew.sh）后手动执行 brew install --cask codex"
                );
            }
            _ => {
                diagnostics::write(
                    &operation_id,
                    "prepare:managed_download_unsupported",
                    format!("install_method={}", platform::install_method()),
                );
                bail!("当前平台不支持下载托管 Codex，请手动安装 Codex CLI 后重试");
            }
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
                Ok(self.status_async().await)
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

    /// macOS 上通过 Homebrew 安装系统 Codex（cask 名 codex）。
    pub async fn install_via_homebrew(&self) -> Result<CodexAcpStatus> {
        let operation_id = diagnostics::operation_id("homebrew-install");
        if platform::install_method() != "homebrew" {
            bail!("Homebrew 安装仅支持 macOS");
        }
        if !platform::brew_available() {
            diagnostics::write(&operation_id, "homebrew:unavailable", "result=rejected");
            bail!(
                "未检测到 Homebrew。请先从 https://brew.sh 安装 Homebrew，\
                 再点击「使用 Homebrew 安装」安装 Codex，或手动执行 brew install --cask codex"
            );
        }
        if self.installing.swap(true, Ordering::AcqRel) {
            diagnostics::write(
                &operation_id,
                "homebrew:already_installing",
                "result=rejected",
            );
            bail!("Codex 正在通过 Homebrew 安装，请稍候");
        }
        diagnostics::write(&operation_id, "homebrew:start", "cask=codex");
        // brew install 是阻塞式子进程，放到 spawn_blocking 避免卡住 async runtime。
        let result = tokio::task::spawn_blocking(|| {
            let run_brew = |args: &[&str]| -> Result<std::process::Output> {
                std::process::Command::new(platform::brew_bin())
                    .args(args)
                    .output()
                    .context("启动 Homebrew 失败")
            };
            let already_installed = |output: &std::process::Output| {
                String::from_utf8_lossy(&output.stdout).contains("already installed")
                    || String::from_utf8_lossy(&output.stderr).contains("already installed")
            };
            let output = run_brew(&["install", "--cask", "codex"])?;
            // 已通过 brew 安装的 codex 会提示 already installed；此时可能是版本过低，
            // 改用 brew upgrade 升级到最新（已是最新时 upgrade 同样提示 already installed）。
            let (command, output) = if output.status.success() {
                return Ok(());
            } else if already_installed(&output) {
                ("upgrade", run_brew(&["upgrade", "--cask", "codex"])?)
            } else {
                ("install", output)
            };
            if output.status.success() || already_installed(&output) {
                return Ok(());
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
            bail!(
                "brew {command} --cask codex 失败 (exit {}): {}",
                output.status.code().unwrap_or(-1),
                tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
            );
        })
        .await;
        self.installing.store(false, Ordering::Release);
        match result.context("等待 Homebrew 安装任务失败")? {
            Ok(()) => {
                self.refresh_runtime_probe(true).await;
                *self.last_error.write() = None;
                diagnostics::write(&operation_id, "homebrew:complete", "result=success");
                Ok(self.status_async().await)
            }
            Err(error) => {
                let detail = format!("{error:#}");
                diagnostics::write(&operation_id, "homebrew:failed", &detail);
                *self.last_error.write() = Some(format!(
                    "{detail}（诊断编号：{operation_id}；日志：{}）",
                    diagnostics::log_path().display()
                ));
                Err(error)
            }
        }
    }

    pub async fn login(&self) -> Result<CodexAcpStatus> {
        self.login_agent("codex").await
    }

    pub fn open_login_url(&self) -> Result<()> {
        self.open_agent_login_url("codex")
    }

    pub async fn login_agent(&self, agent_id: &str) -> Result<CodexAcpStatus> {
        self.start_agent_login(agent_id, false).await
    }

    /// 无论当前凭证是否仍然有效，都重新进入 Agent 的官方登录流程。
    ///
    /// 登录成功后关闭该 Agent 的现有运行时；会话与时间线仍保留，下一次发送消息时
    /// 会用新账号重新拉起进程，避免旧进程继续持有切换前的凭证。
    pub async fn switch_agent_account(&self, agent_id: &str) -> Result<CodexAcpStatus> {
        self.start_agent_login(agent_id, true).await
    }

    async fn start_agent_login(&self, agent_id: &str, force: bool) -> Result<CodexAcpStatus> {
        let backend = AgentBackend::parse(Some(agent_id))?;
        if !backend.is_acp() {
            bail!("Agent 不是 ACP 后端: {agent_id}");
        }
        if backend == AgentBackend::CodexAcp {
            self.ensure_installed().await?;
        }
        let executable = self
            .login_executable(backend)
            .with_context(|| format!("未检测到可用的 {} CLI", backend.display_name()))?;
        if !force && self.agent_authenticated_async(backend, &executable).await {
            return Ok(self.status_for_async(backend).await);
        }
        let already_in_progress = {
            let mut states = self.login_states.write();
            let state = states.entry(backend).or_default();
            if state.in_progress {
                true
            } else {
                *state = AgentLoginState {
                    in_progress: true,
                    input_required: backend == AgentBackend::ClaudeAcp,
                    ..AgentLoginState::default()
                };
                false
            }
        };
        if already_in_progress {
            return Ok(self.status_for_async(backend).await);
        }
        if backend == AgentBackend::CodexAcp {
            *self.last_error.write() = None;
        }
        let pool = self.clone();
        tokio::spawn(async move {
            let result = pool.run_agent_login(backend, executable).await;
            pool.login_inputs.lock().await.remove(&backend);
            let login_succeeded = {
                let mut states = pool.login_states.write();
                let state = states.entry(backend).or_default();
                state.in_progress = false;
                state.input_required = false;
                match result {
                    Ok(()) => {
                        *state = AgentLoginState::default();
                        if backend == AgentBackend::CodexAcp {
                            *pool.last_error.write() = None;
                        }
                    }
                    Err(error) => {
                        state.error = Some(format!(
                            "{} 授权登录失败: {error:#}",
                            backend.display_name()
                        ));
                    }
                }
                state.error.is_none()
            };
            if login_succeeded {
                pool.restart_agent_sessions(backend).await;
            }
        });
        Ok(self.status_for_async(backend).await)
    }

    async fn restart_agent_sessions(&self, backend: AgentBackend) {
        let runtimes = {
            let mut sessions = self.sessions.lock().await;
            let session_ids = sessions
                .keys()
                .filter(|session_id| self.agents.backend(session_id) == backend)
                .cloned()
                .collect::<Vec<_>>();
            session_ids
                .iter()
                .filter_map(|session_id| {
                    sessions
                        .remove(session_id)
                        .map(|runtime| (session_id.clone(), runtime))
                })
                .collect::<Vec<_>>()
        };
        for (session_id, runtime) in runtimes {
            // runtime 已从共享表移除，避免新请求继续复用旧账号进程；显式使用旧
            // runtime 的 bridge 记录取消事件，让 timeline、acp-state 与前端 pending
            // 状态同时收口。
            self.cancel_pending_permissions_with_bridge(&session_id, Some(&runtime.bridge))
                .await;
            self.cancel_pending_elicitations_with_bridge(&session_id, Some(&runtime.bridge))
                .await;
            runtime.shutdown().await;
        }
    }

    pub fn open_agent_login_url(&self, agent_id: &str) -> Result<()> {
        let backend = AgentBackend::parse(Some(agent_id))?;
        let url = self
            .login_states
            .read()
            .get(&backend)
            .and_then(|state| state.url.clone())
            .with_context(|| format!("{} 授权链接尚未生成，请稍候", backend.display_name()))?;
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
                "[pinvou3-app] {} authorization page requested via {}",
                backend.display_name(),
                browser.display(),
            );
            return Ok(());
        }
        crate::platform::os::open_target(&url, &format!("{} 授权页面", backend.display_name()))
            .map_err(anyhow::Error::msg)
    }

    pub async fn submit_agent_login_code(&self, agent_id: &str, code: &str) -> Result<()> {
        let backend = AgentBackend::parse(Some(agent_id))?;
        if backend != AgentBackend::ClaudeAcp {
            bail!("{} 登录不需要回填授权码", backend.display_name());
        }
        let code = code.trim();
        if code.is_empty() || code.len() > 4096 || code.chars().any(char::is_control) {
            bail!("Claude 授权码格式无效");
        }
        let mut inputs = self.login_inputs.lock().await;
        let input = inputs
            .get_mut(&backend)
            .context("Claude 登录进程未等待授权码，请重新发起登录")?;
        input
            .write_all(format!("{code}\n").as_bytes())
            .await
            .context("向 Claude 登录进程提交授权码失败")?;
        input.flush().await.context("刷新 Claude 授权码失败")?;
        if let Some(state) = self.login_states.write().get_mut(&backend) {
            state.input_required = false;
            state.error = None;
        }
        Ok(())
    }

    fn login_executable(&self, backend: AgentBackend) -> Option<PathBuf> {
        match backend {
            AgentBackend::CodexAcp => {
                let adapter = self.resolve_adapter()?;
                self.resolve_codex(&adapter).map(|resolved| resolved.path)
            }
            AgentBackend::ClaudeAcp => {
                let adapter = self.resolve_claude_adapter()?;
                resolve_claude_cli_from_adapter(&adapter)
            }
            AgentBackend::KimiAcp => resolve_kimi_path(),
            AgentBackend::Deepseek => None,
        }
    }

    fn agent_authenticated(&self, backend: AgentBackend, executable: &Path) -> bool {
        match backend {
            AgentBackend::CodexAcp => codex_authenticated(executable),
            AgentBackend::ClaudeAcp => claude_authenticated(executable),
            AgentBackend::KimiAcp => kimi_authenticated(executable),
            AgentBackend::Deepseek => true,
        }
    }

    async fn run_agent_login(&self, backend: AgentBackend, executable: PathBuf) -> Result<()> {
        let operation_id = diagnostics::operation_id("login");
        diagnostics::write(
            &operation_id,
            "login:spawn",
            format!(
                "agent={} executable={}",
                backend.agent_id().unwrap_or("deepseek"),
                executable.display()
            ),
        );
        let mut command = agent_login_command(backend, &executable);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .with_context(|| format!("启动 {} CLI 登录失败", backend.display_name()))?;
        let stdin = child.stdin.take().context("读取 Agent 登录标准输入失败")?;
        if backend == AgentBackend::ClaudeAcp {
            self.login_inputs.lock().await.insert(backend, stdin);
        }
        let stdout = child.stdout.take().context("读取 Agent 登录标准输出失败")?;
        let stderr = child.stderr.take().context("读取 Agent 登录错误输出失败")?;
        let stdout_reader = tokio::spawn(capture_agent_login_output(
            stdout,
            backend,
            self.login_states.clone(),
        ));
        let stderr_reader = tokio::spawn(capture_agent_login_output(
            stderr,
            backend,
            self.login_states.clone(),
        ));
        let timeout = if backend == AgentBackend::KimiAcp {
            Duration::from_secs(1800)
        } else {
            Duration::from_secs(600)
        };
        let status = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(result) => result.context("等待 Agent 登录进程失败")?,
            Err(_) => {
                diagnostics::write(
                    &operation_id,
                    "login:timeout",
                    format!("timeout_seconds={}", timeout.as_secs()),
                );
                let _ = child.kill().await;
                let _ = child.wait().await;
                bail!("授权等待超时，请重新登录");
            }
        };
        let _ = stdout_reader.await;
        let _ = stderr_reader.await;

        diagnostics::write(
            &operation_id,
            "login:process_exit",
            format!("status={status}"),
        );

        if !status.success() {
            bail!("{} 登录进程退出: {status}", backend.display_name());
        }
        if !self.agent_authenticated_async(backend, &executable).await {
            bail!(
                "{} 登录进程已结束，但未检测到有效授权信息",
                backend.display_name()
            );
        }
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
            bail!("ACP 会话配置仍在同步，请稍候再发送");
        }
        let prepared = prepare_codex_prompt(
            &content,
            &attachments,
            &workspace_references,
            &runtime.prompt_capabilities,
        )?;
        if runtime.busy.swap(true, Ordering::AcqRel) {
            bail!("ACP 会话仍在生成");
        }
        if let Err(error) = self.session_store.touch_activity(session_id) {
            runtime.busy.store(false, Ordering::Release);
            return Err(error).context("更新 ACP 会话最近活跃时间失败");
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
            if runtime.busy.load(Ordering::Acquire) {
                runtime.cancel();
                runtime
                    .bridge
                    .emit("cancel_requested", json!({ "status": "cancelling" }));
            } else {
                // session 已恢复但当前进程没有活跃 prompt 时，session/cancel 无法
                // 命中旧进程的 turn。直接收口持久化孤儿回合，让停止操作可恢复且幂等。
                runtime
                    .bridge
                    .interrupt_orphaned_turns("cancel_without_active_prompt");
            }
        } else {
            // 正常 UI 加载会先 lazy spawn runtime；这里仍为启动失败或竞态保留兜底，
            // 避免“停止”在没有内存 runtime 时静默无效。
            EventBridge::new(self.app.clone(), session_id.to_string())
                .interrupt_orphaned_turns("cancel_without_runtime");
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
        if !self.is_acp(session_id) {
            bail!("当前会话不是 ACP 会话");
        }
        let pending_permissions = self.pending_permissions_for(session_id).await;
        let pending_elicitations = self.pending_elicitations_for(session_id).await;
        Ok(self
            .get_or_spawn(session_id)
            .await?
            .info(pending_permissions, pending_elicitations))
    }

    fn remember_config_choice(
        &self,
        session_id: &str,
        runtime: &AcpSession,
        config_id: &str,
        value_id: &str,
    ) {
        let backend = self.backend(session_id);
        let mut errors = Vec::new();
        if let Err(error) = self
            .agents
            .set_acp_config_value(session_id, config_id, value_id)
        {
            errors.push(format!("会话配置: {error:#}"));
        }
        if let Err(error) = self.config_defaults.set(backend, config_id, value_id) {
            errors.push(format!("新会话默认值: {error:#}"));
        }
        if !errors.is_empty() {
            let message = errors.join("；");
            eprintln!(
                "[pinvou3-app] failed to persist {} ACP config {}={}: {}",
                backend.display_name(),
                config_id,
                value_id,
                message
            );
            runtime.bridge.emit(
                "config_persistence_failed",
                json!({
                    "configId": config_id,
                    "valueId": value_id,
                    "message": message,
                }),
            );
        }
    }

    pub async fn set_model(&self, session_id: &str, model_id: &str) -> Result<CodexAcpSessionInfo> {
        let runtime = self.get_or_spawn(session_id).await?;
        runtime.set_model(model_id).await?;
        self.remember_config_choice(session_id, &runtime, "model", model_id);
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
            bail!("Agent 正在处理当前任务，配置将在本轮结束后才能修改");
        }
        if runtime.configuring.swap(true, Ordering::AcqRel) {
            bail!("ACP 会话已有配置正在同步");
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
        self.remember_config_choice(session_id, &runtime, config_id, value_id);
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
            bail!("Agent 正在处理当前任务，权限模式将在本轮结束后才能修改");
        }
        if runtime.configuring.swap(true, Ordering::AcqRel) {
            bail!("ACP 会话已有配置正在同步");
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
        self.remember_config_choice(session_id, &runtime, "mode", mode_id);
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
        if !self.is_acp(session_id) {
            bail!("当前会话不是 ACP 会话");
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
        let bridge = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .map(|runtime| runtime.bridge.clone());
        self.cancel_pending_permissions_with_bridge(session_id, bridge.as_ref())
            .await;
    }

    async fn cancel_pending_permissions_with_bridge(
        &self,
        session_id: &str,
        bridge: Option<&EventBridge>,
    ) {
        let mut pending = self.pending_permissions.lock().await;
        let keys = pending
            .iter()
            .filter(|(_, request)| request.view.session_id == session_id)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let mut cancelled = Vec::new();
        for key in keys {
            if let Some(request) = pending.remove(&key) {
                cancelled.push(request.view.tool_call_id.clone());
                let _ = request.response_tx.send(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ));
            }
        }
        drop(pending);
        if let Some(bridge) = bridge {
            for tool_call_id in cancelled {
                bridge.emit(
                    "permission_resolved",
                    json!({
                        "toolCallId": tool_call_id,
                        "outcome": "cancelled",
                    }),
                );
            }
        }
    }

    async fn cancel_pending_elicitations(&self, session_id: &str) {
        let bridge = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .map(|runtime| runtime.bridge.clone());
        self.cancel_pending_elicitations_with_bridge(session_id, bridge.as_ref())
            .await;
    }

    async fn cancel_pending_elicitations_with_bridge(
        &self,
        session_id: &str,
        bridge: Option<&EventBridge>,
    ) {
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
        if let Some(bridge) = bridge {
            for elicitation_id in cancelled {
                bridge.emit(
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
        let operation_id = diagnostics::operation_id("session");
        diagnostics::write(
            &operation_id,
            "session:resolve_start",
            format!("session_id={session_id}"),
        );
        let mut sessions = self.sessions.lock().await;
        if let Some(runtime) = sessions.get(session_id) {
            diagnostics::write(
                &operation_id,
                "session:reused",
                format!("session_id={session_id}"),
            );
            return Ok(runtime.clone());
        }
        // 与 spawn_session 一致走 self.backend()，让辅助索引缺失的会话在
        // acp_record 处得到明确报错，而不是误导性的「当前会话不是 ACP 会话」。
        let backend = self.backend(session_id);
        let readiness = match backend {
            AgentBackend::CodexAcp => self.ensure_installed().await.map(|_| ()),
            AgentBackend::ClaudeAcp | AgentBackend::KimiAcp => {
                let status = self.status_for_async(backend).await;
                if !status.installed {
                    Err(anyhow::anyhow!(
                        "{} ACP 尚未就绪。{}",
                        backend.display_name(),
                        setup_hint_message(status.setup_hint)
                    ))
                } else {
                    Ok(())
                }
            }
            AgentBackend::Deepseek => Err(anyhow::anyhow!("当前会话不是 ACP 会话")),
        };
        if let Err(error) = readiness {
            diagnostics::write(
                &operation_id,
                "session:runtime_failed",
                format!("session_id={session_id} error={error:#}"),
            );
            return Err(error);
        }
        diagnostics::write(
            &operation_id,
            "session:spawn_start",
            format!("session_id={session_id}"),
        );
        let runtime = match self.spawn_session(session_id, &operation_id).await {
            Ok(runtime) => Arc::new(runtime),
            Err(error) => {
                diagnostics::write(
                    &operation_id,
                    "session:spawn_failed",
                    format!("session_id={session_id} error={error:#}"),
                );
                return Err(error);
            }
        };
        sessions.insert(session_id.to_string(), runtime.clone());
        diagnostics::write(
            &operation_id,
            "session:ready",
            format!("session_id={session_id}"),
        );
        Ok(runtime)
    }

    async fn spawn_session(
        &self,
        pinvou_session_id: &str,
        operation_id: &str,
    ) -> Result<AcpSession> {
        let backend = self.backend(pinvou_session_id);
        let (mut command, adapter, package_name, package_version) = match backend {
            AgentBackend::CodexAcp => {
                let adapter = self.resolve_adapter().context("Codex ACP 尚未安装")?;
                let mut command = self.adapter_command(&adapter)?;
                self.configure_codex_path(&mut command, &adapter)?;
                (command, adapter, CODEX_ACP_PACKAGE, CODEX_ACP_VERSION)
            }
            AgentBackend::ClaudeAcp => {
                let adapter = self
                    .resolve_claude_adapter()
                    .context("Claude ACP Bridge 尚未安装")?;
                (
                    self.adapter_command(&adapter)?,
                    adapter,
                    CLAUDE_ACP_PACKAGE,
                    CLAUDE_ACP_VERSION,
                )
            }
            AgentBackend::KimiAcp => {
                let executable = resolve_kimi_path()
                    .context("未检测到 Kimi Code CLI；请先安装 Kimi，并确保 kimi 在 PATH 中")?;
                let mut command = crate::platform::process::HiddenTokioCommand::new(
                    crate::platform::os::external_application_path(&executable),
                );
                command.arg("acp");
                (command, executable, "kimi acp", "native")
            }
            AgentBackend::Deepseek => bail!("当前会话不是 ACP 会话"),
        };
        let workspace = self.execution_workspace(pinvou_session_id)?;
        if self.agents.get(pinvou_session_id).workspace_kind == CodexWorkspaceKind::Temporary {
            tokio::fs::create_dir_all(&workspace).await?;
        }

        command
            .current_dir(&workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .with_context(|| format!("启动 {} 失败", backend.display_name()))?;
        let stdin = child.stdin.take().context("ACP stdin 不可用")?;
        let stdout = child.stdout.take().context("ACP stdout 不可用")?;
        let stderr_tail = Arc::new(parking_lot::Mutex::new(VecDeque::<String>::new()));
        if let Some(stderr) = child.stderr.take() {
            let sid = pinvou_session_id.to_string();
            let operation_id = operation_id.to_string();
            let stderr_tail = stderr_tail.clone();
            let agent_id = backend.agent_id().unwrap_or("acp");
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    {
                        let mut tail = stderr_tail.lock();
                        if tail.len() >= 40 {
                            tail.pop_front();
                        }
                        tail.push_back(line.chars().take(2_000).collect());
                    }
                    diagnostics::write(
                        &operation_id,
                        "session:bridge_stderr",
                        format!("agent={agent_id} session_id={sid} stderr={line}"),
                    );
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
                eprintln!("[pinvou3-app] ACP 协议连接结束: {error}");
            }
        });

        let ready_result: Result<_> = async {
            let received = tokio::time::timeout(Duration::from_secs(30), ready_rx)
                .await
                .with_context(|| format!("{} ACP initialize 超时", backend.display_name()))?;
            let initialized = received.context("ACP initialize 通道中断")?;
            initialized.context("ACP initialize 失败")
        }
        .await;
        let (connection, initialized) = match ready_result {
            Ok(initialized) => initialized,
            Err(error) => {
                // Give the process and stderr reader a brief chance to publish the real failure.
                tokio::time::sleep(Duration::from_millis(100)).await;
                let exit_status = child
                    .try_wait()
                    .map(|status| {
                        status
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "running".to_string())
                    })
                    .unwrap_or_else(|wait_error| format!("unknown ({wait_error})"));
                let stderr = stderr_tail
                    .lock()
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" | ");
                diagnostics::write(
                    operation_id,
                    "session:initialize_failed",
                    format!(
                        "session_id={pinvou_session_id} exit_status={exit_status} stderr={stderr} error={error:#}"
                    ),
                );
                return Err(error);
            }
        };

        let saved = self.agents.get(pinvou_session_id);
        let desired_config_values = if saved.acp_session_id.is_some() {
            saved_config_values(&saved)
        } else {
            self.config_defaults.get(backend)
        };
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
                            eprintln!(
                                "[pinvou3-app] {} ACP 恢复会话失败，改建新会话: {error}",
                                backend.display_name()
                            );
                            new_acp_session(&connection, &workspace, backend).await?
                        }
                    }
                } else {
                    new_acp_session(&connection, &workspace, backend).await?
                }
            } else {
                new_acp_session(&connection, &workspace, backend).await?
            };
        restore_config_values(
            &connection,
            &acp_session_id,
            &mut mode_state,
            &mut config_options,
            &desired_config_values,
            backend,
        )
        .await;
        let current_model_id = current_config_value(&config_options, "model");
        let models = codex_models(&config_options);
        let config_values = config_values_from_options(&config_options, &mode_state);
        let prompt_capabilities = initialized.agent_capabilities.prompt_capabilities.clone();
        self.agents.set_acp_session(
            pinvou_session_id,
            acp_session_id.clone(),
            current_model_id.clone(),
            config_values,
        )?;
        persist_acp_state(
            pinvou_session_id,
            json!({
                "adapter": {
                    "agentId": backend.agent_id(),
                    "package": package_name,
                    "version": package_version,
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
                "workspace": {
                    "kind": saved.workspace_kind,
                    "path": &workspace,
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
            kimi_session_id: (backend == AgentBackend::KimiAcp).then(|| acp_session_id.clone()),
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

    fn resolve_claude_adapter(&self) -> Option<PathBuf> {
        resolve_claude_adapter_from(self.bundled_claude_adapter.as_deref())
    }

    fn resolve_node(&self, adapter: &Path) -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("PINVOU3_ACP_NODE_PATH")
            .or_else(|| std::env::var_os("PINVOU3_CODEX_NODE_PATH"))
            .map(PathBuf::from)
        {
            if path.is_file() {
                return Some(path);
            }
        }
        if let Some(path) = self.bundled_node.as_ref().filter(|path| path.is_file()) {
            return Some(path.clone());
        }
        if platform::adapter_needs_node(adapter) {
            return find_in_path(platform::node_executable_name());
        }
        None
    }

    fn resolve_codex(&self, _adapter: &Path) -> Option<ResolvedCodex> {
        self.runtime_probe.read().codex.clone()
    }

    fn adapter_command(&self, adapter: &Path) -> Result<Command> {
        platform::adapter_command(adapter, self.resolve_node(adapter).as_deref())
    }

    fn configure_codex_path(&self, command: &mut Command, adapter: &Path) -> Result<()> {
        let codex = self
            .resolve_codex(adapter)
            .context("未检测到可用 Codex；请下载托管 Codex")?;
        command.env(
            "CODEX_PATH",
            crate::platform::os::external_application_path(&codex.path),
        );
        Ok(())
    }
}

async fn new_acp_session(
    connection: &ConnectionTo<Agent>,
    workspace: &Path,
    backend: AgentBackend,
) -> Result<(String, Option<SessionModeState>, Vec<SessionConfigOption>)> {
    let response = connection
        .send_request(NewSessionRequest::new(workspace))
        .block_task()
        .await
        .with_context(|| format!("{} ACP session/new 失败", backend.display_name()))?;
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
    options: &mut Vec<SessionConfigOption>,
    config_id: &str,
    value_id: &str,
) -> Result<()> {
    if !config_option_supports(options, config_id, value_id) {
        bail!("ACP 配置项或取值不存在: {config_id}={value_id}");
    }
    let response = connection
        .send_request(SetSessionConfigOptionRequest::new(
            acp_session_id.to_string(),
            config_id.to_string(),
            value_id,
        ))
        .block_task()
        .await
        .context("ACP session/set_config_option 失败")?;
    // ACP 规定响应包含完整的最新配置集。配置项之间可能联动，必须以 Agent
    // 返回值整体替换，不能只在本地手工修改当前字段。
    *options = response.config_options;
    Ok(())
}

async fn apply_saved_mode(
    connection: &ConnectionTo<Agent>,
    acp_session_id: &str,
    modes: &mut Option<SessionModeState>,
    config_options: &mut Vec<SessionConfigOption>,
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
        bail!("ACP Agent 未上报会话模式: {mode_id}");
    }
    connection
        .send_request(SetSessionModeRequest::new(
            acp_session_id.to_string(),
            mode_id.to_string(),
        ))
        .block_task()
        .await
        .context("ACP session/set_mode 失败")?;
    if let Some(state) = modes.as_mut() {
        state.current_mode_id = mode_id.to_string().into();
    }
    Ok(())
}

async fn restore_config_values(
    connection: &ConnectionTo<Agent>,
    acp_session_id: &str,
    modes: &mut Option<SessionModeState>,
    config_options: &mut Vec<SessionConfigOption>,
    values: &HashMap<String, String>,
    backend: AgentBackend,
) {
    let mut desired = values.iter().collect::<Vec<_>>();
    desired.sort_by(|(left, _), (right, _)| {
        let priority = |config_id: &str| match config_id {
            "model" => 0,
            "mode" => 1,
            _ => 2,
        };
        priority(left)
            .cmp(&priority(right))
            .then_with(|| left.cmp(right))
    });
    let mut failed = HashSet::new();

    // 某些 Agent 会根据 model/mode 改变后续可选项。最多多跑两轮，让 Agent
    // 返回的完整配置集稳定下来，同时避免互斥配置导致无限来回设置。
    for _ in 0..3 {
        let mut progress = false;
        for (config_id, value_id) in &desired {
            if failed.contains(config_id.as_str())
                || current_config_value(config_options, config_id) == Some((*value_id).clone())
            {
                continue;
            }
            if !config_option_supports(config_options, config_id, value_id) {
                continue;
            }
            match apply_config_option(
                connection,
                acp_session_id,
                config_options,
                config_id,
                value_id,
            )
            .await
            {
                Ok(()) => progress = true,
                Err(error) => {
                    failed.insert((*config_id).clone());
                    eprintln!(
                        "[pinvou3-app] skipped {} saved ACP config {}={}: {error:#}",
                        backend.display_name(),
                        config_id,
                        value_id
                    );
                }
            }
        }
        if !progress {
            break;
        }
    }

    if let Some(mode_id) = values.get("mode") {
        let has_config_mode = config_options
            .iter()
            .any(|option| option.id.to_string() == "mode");
        if !has_config_mode
            && modes
                .as_ref()
                .is_some_and(|state| state.current_mode_id.to_string() != mode_id.as_str())
        {
            if let Err(error) =
                apply_saved_mode(connection, acp_session_id, modes, config_options, mode_id).await
            {
                eprintln!(
                    "[pinvou3-app] skipped {} saved ACP mode {}: {error:#}",
                    backend.display_name(),
                    mode_id
                );
            }
        }
    }

    for (config_id, value_id) in desired {
        if config_id == "mode"
            && !config_options
                .iter()
                .any(|option| option.id.to_string() == "mode")
        {
            continue;
        }
        if current_config_value(config_options, config_id) != Some(value_id.clone()) {
            eprintln!(
                "[pinvou3-app] {} ACP no longer supports saved config {}={}",
                backend.display_name(),
                config_id,
                value_id
            );
        }
    }
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

fn config_values_from_options(
    options: &[SessionConfigOption],
    modes: &Option<SessionModeState>,
) -> HashMap<String, String> {
    let mut values = options
        .iter()
        .filter_map(|option| {
            let SessionConfigKind::Select(select) = &option.kind else {
                return None;
            };
            Some((option.id.to_string(), select.current_value.to_string()))
        })
        .collect::<HashMap<_, _>>();
    if !values.contains_key("mode") {
        if let Some(mode_id) = modes
            .as_ref()
            .map(|state| state.current_mode_id.to_string())
        {
            values.insert("mode".to_string(), mode_id);
        }
    }
    values
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

fn bundled_adapter_candidates(
    resource_root: &Path,
    development_bridge: &Path,
    package: &str,
) -> Vec<PathBuf> {
    let node_entry = |root: PathBuf| {
        root.join("node_modules")
            .join("@agentclientprotocol")
            .join(package)
            .join("dist")
            .join("index.js")
    };
    let package_entry = |root: PathBuf| node_entry(root.join("acp"));
    let mut candidates = vec![
        package_entry(resource_root.join("runtime").join("codex-bridge")),
        package_entry(resource_root.join("codex-bridge")),
        package_entry(resource_root.join("resources").join("codex-bridge")),
        package_entry(development_bridge.to_path_buf()),
    ];
    if package == "codex-acp" {
        let legacy_binary = if crate::platform::capabilities::is_windows() {
            "codex-acp.exe"
        } else {
            "codex-acp"
        };
        candidates.extend([
            node_entry(resource_root.join("codex-acp")),
            resource_root.join("codex-acp").join(legacy_binary),
            node_entry(resource_root.join("resources").join("codex-acp")),
            resource_root
                .join("resources")
                .join("codex-acp")
                .join(legacy_binary),
        ]);
    }
    candidates
}

fn managed_adapter_path() -> PathBuf {
    managed_runtime_dir()
        .join("node_modules")
        .join(".bin")
        .join(platform::managed_adapter_name())
}

/// readiness 报错面向用户展示中文，把结构化 setup_hint 代码映射回中文文案。
fn setup_hint_message(hint: Option<&str>) -> &'static str {
    match hint {
        Some("kimi_cli_missing") => "请先安装 Kimi Code CLI",
        Some("kimi_auth_required") => "使用 Kimi 账号完成设备码授权",
        Some("claude_auth_required") => "使用 Claude 账号完成浏览器授权，或设置 ANTHROPIC_API_KEY",
        _ => "请检查 Agent 安装和 PATH",
    }
}

fn agent_login_command(backend: AgentBackend, executable: &Path) -> Command {
    if backend == AgentBackend::CodexAcp {
        return platform::codex_login_command(executable);
    }
    let args: &[&str] = match backend {
        AgentBackend::CodexAcp => unreachable!(),
        AgentBackend::ClaudeAcp => &["auth", "login"],
        AgentBackend::KimiAcp => &["login"],
        AgentBackend::Deepseek => &[],
    };
    let executable = crate::platform::os::external_application_path(executable);
    if crate::platform::capabilities::is_windows()
        && executable.extension().and_then(|value| value.to_str()) == Some("cmd")
    {
        let mut command = crate::platform::process::HiddenTokioCommand::new("cmd");
        command.args(["/D", "/S", "/C"]).arg(executable).args(args);
        command
    } else {
        let mut command = crate::platform::process::HiddenTokioCommand::new(executable);
        command.args(args);
        command
    }
}

async fn capture_agent_login_output<R>(
    mut reader: R,
    backend: AgentBackend,
    states: Arc<parking_lot::RwLock<HashMap<AgentBackend, AgentLoginState>>>,
) where
    R: AsyncRead + Unpin,
{
    let mut chunk = [0_u8; 2048];
    let mut output = String::new();
    loop {
        let read = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        output.push_str(&String::from_utf8_lossy(&chunk[..read]));
        if output.len() > 65_536 {
            output.drain(..output.len() - 65_536);
        }
        let url = extract_agent_login_url(backend, &output);
        let code = extract_device_code(&output, url.as_deref());
        if url.is_some() || code.is_some() {
            let mut states = states.write();
            let state = states.entry(backend).or_default();
            if url.is_some() {
                state.url = url;
            }
            if code.is_some() {
                state.code = code;
            }
        }
    }
}

fn extract_agent_login_url(backend: AgentBackend, output: &str) -> Option<String> {
    output
        .match_indices("https://")
        .filter_map(|(start, _)| {
            let tail = &output[start..];
            let end = tail
                .char_indices()
                .find_map(|(index, character)| {
                    (character.is_whitespace()
                        || character.is_control()
                        || matches!(character, '"' | '\'' | '<' | '>'))
                    .then_some(index)
                })
                .unwrap_or(tail.len());
            let candidate = tail[..end].trim_end_matches(['.', ',', ')', ']']);
            agent_login_url_allowed(backend, candidate).then(|| candidate.to_string())
        })
        .last()
}

fn agent_login_url_allowed(backend: AgentBackend, url: &str) -> bool {
    match backend {
        AgentBackend::CodexAcp => {
            url.starts_with("https://auth.openai.com/")
                || url.starts_with("https://platform.openai.com/")
        }
        AgentBackend::ClaudeAcp => {
            url.starts_with("https://claude.com/")
                || url.starts_with("https://claude.ai/")
                || url.starts_with("https://platform.claude.com/")
        }
        AgentBackend::KimiAcp => {
            url.starts_with("https://www.kimi.com/") || url.starts_with("https://kimi.com/")
        }
        AgentBackend::Deepseek => false,
    }
}

fn extract_device_code(output: &str, login_url: Option<&str>) -> Option<String> {
    if let Some(url) = login_url {
        if let Some(value) = url.split("user_code=").nth(1) {
            let code = value
                .split(|character: char| character == '&' || character.is_whitespace())
                .next()
                .unwrap_or_default();
            if valid_device_code(code) {
                return Some(code.to_string());
            }
        }
    }
    ["enter code:", "user code:"]
        .into_iter()
        .find_map(|marker| {
            let start = output.to_ascii_lowercase().rfind(marker)? + marker.len();
            let code = output[start..].split_whitespace().next()?;
            valid_device_code(code).then(|| code.to_string())
        })
}

fn valid_device_code(code: &str) -> bool {
    (4..=32).contains(&code.len())
        && code
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn codex_path_for_adapter(adapter: &Path) -> Option<PathBuf> {
    let name = platform::system_codex_name();
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
    find_in_path(platform::managed_adapter_name())
}

fn resolve_claude_adapter_from(bundled: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PINVOU3_CLAUDE_ACP_BIN").map(PathBuf::from) {
        if nonempty_file(&path) {
            return Some(path);
        }
    }
    if let Some(path) = bundled {
        if nonempty_file(path) {
            return Some(path.to_path_buf());
        }
    }
    find_in_path(if crate::platform::capabilities::is_windows() {
        "claude-agent-acp.cmd"
    } else {
        "claude-agent-acp"
    })
}

fn resolve_claude_cli_from_adapter(adapter: &Path) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PINVOU3_CLAUDE_CLI_PATH").map(PathBuf::from) {
        if nonempty_file(&path) {
            return Some(path);
        }
    }
    let (package, binary) = claude_native_runtime(
        std::env::consts::OS,
        std::env::consts::ARCH,
        crate::platform::capabilities::is_musl(),
    )?;
    if let Some(path) = adapter.ancestors().find_map(|ancestor| {
        (ancestor.file_name().and_then(|value| value.to_str()) == Some("node_modules"))
            .then(|| ancestor.join("@anthropic-ai").join(&package).join(binary))
            .filter(|candidate| nonempty_file(candidate))
    }) {
        return Some(path);
    }
    find_in_path(binary)
}

fn claude_native_runtime(os: &str, arch: &str, musl: bool) -> Option<(String, &'static str)> {
    let platform = match os {
        "windows" => "win32",
        "macos" => "darwin",
        "linux" => "linux",
        _ => return None,
    };
    let arch = match arch {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        _ => return None,
    };
    let libc = if os == "linux" && musl { "-musl" } else { "" };
    let binary = if os == "windows" {
        "claude.exe"
    } else {
        "claude"
    };
    Some((format!("claude-agent-sdk-{platform}-{arch}{libc}"), binary))
}

fn resolve_kimi_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PINVOU3_KIMI_ACP_BIN").map(PathBuf::from) {
        if nonempty_file(&path) {
            return Some(path);
        }
    }
    find_in_path(if crate::platform::capabilities::is_windows() {
        "kimi.exe"
    } else {
        "kimi"
    })
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| nonempty_file(candidate))
}

/// `--version` 探测与 cli_status_success 一致限制 3 秒，避免卡住的 CLI 拖住状态轮询。
fn command_version_output(executable: &Path) -> Option<String> {
    let mut child = crate::platform::process::HiddenCommand::new(
        crate::platform::os::external_application_path(executable),
    )
    .arg("--version")
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null())
    .spawn()
    .ok()?;
    match child.wait_timeout(Duration::from_secs(3)) {
        Ok(Some(status)) if status.success() => {}
        Ok(Some(_)) => return None,
        Ok(None) | Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    }
    let mut version = String::new();
    child.stdout.take()?.read_to_string(&mut version).ok()?;
    Some(version.trim().to_string())
}

fn command_version(command: &Path) -> Option<String> {
    let version = command_version_output(command)?;
    (!version.is_empty()).then_some(version)
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
    command_version_output(node).map(|version| version.trim_start_matches('v').to_string())
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

fn codex_authenticated(codex: &Path) -> bool {
    if nonempty_env("OPENAI_API_KEY") {
        return true;
    }
    cli_status_success(codex, &["login", "status"])
}

fn claude_authenticated(claude: &Path) -> bool {
    if [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ]
    .into_iter()
    .any(nonempty_env)
    {
        return true;
    }
    cli_status_success(claude, &["auth", "status"])
}

fn kimi_authenticated(_kimi: &Path) -> bool {
    if nonempty_env("KIMI_API_KEY") {
        return true;
    }
    let root = kimi_data_root();
    let Ok(raw) = std::fs::read_to_string(root.join("credentials").join("kimi-code.json")) else {
        return false;
    };
    kimi_credentials_valid(&raw)
}

fn kimi_data_root() -> PathBuf {
    std::env::var_os("KIMI_CODE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::platform::os::user_home_dir().join(".kimi-code"))
}

async fn kimi_diagnostic_cursor(session_id: &str) -> KimiDiagnosticCursor {
    let log_path = resolve_kimi_session_log_path(session_id).await;
    let offset = match log_path.as_ref() {
        Some(path) => tokio::fs::metadata(path)
            .await
            .map(|metadata| metadata.len())
            .unwrap_or(0),
        None => 0,
    };
    KimiDiagnosticCursor {
        session_id: session_id.to_string(),
        log_path,
        offset,
    }
}

async fn kimi_failure_after(cursor: &KimiDiagnosticCursor) -> Option<String> {
    // Kimi 在返回 ACP end_turn 前先写日志，但文件 sink 可能有极短刷新延迟。
    for delay_ms in [0, 25, 75] {
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        let log_path = match cursor.log_path.clone() {
            Some(path) => path,
            None => match resolve_kimi_session_log_path(&cursor.session_id).await {
                Some(path) => path,
                None => continue,
            },
        };
        // 按 offset seek 只增量读取本回合新增内容，不再每回合整读日志文件。
        let offset = if cursor.log_path.as_ref() == Some(&log_path) {
            cursor.offset
        } else {
            0
        };
        let Ok(mut file) = tokio::fs::File::open(&log_path).await else {
            continue;
        };
        if file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
            continue;
        }
        let mut raw = Vec::new();
        if file.read_to_end(&mut raw).await.is_err() {
            continue;
        }
        if let Some(error) = parse_kimi_acp_failure(&String::from_utf8_lossy(&raw)) {
            return Some(error);
        }
    }
    None
}

async fn resolve_kimi_session_log_path(session_id: &str) -> Option<PathBuf> {
    let root = kimi_data_root();
    let raw = tokio::fs::read_to_string(root.join("session_index.jsonl"))
        .await
        .ok()?;
    kimi_session_log_path_from_index(&raw, &root, session_id)
}

fn kimi_session_log_path_from_index(
    index: &str,
    data_root: &Path,
    session_id: &str,
) -> Option<PathBuf> {
    let sessions_root = data_root.join("sessions");
    index.lines().rev().find_map(|line| {
        let record = serde_json::from_str::<serde_json::Value>(line).ok()?;
        (record.get("sessionId")?.as_str()? == session_id).then_some(())?;
        let raw_dir = PathBuf::from(record.get("sessionDir")?.as_str()?);
        let session_dir = if raw_dir.is_absolute() {
            raw_dir
        } else {
            data_root.join(raw_dir)
        };
        if session_dir
            .components()
            .any(|component| component == std::path::Component::ParentDir)
            || !session_dir.starts_with(&sessions_root)
        {
            return None;
        }
        Some(session_dir.join("logs").join("kimi-code.log"))
    })
}

fn parse_kimi_acp_failure(log_tail: &str) -> Option<String> {
    const MARKER: &str = "acp: turn ended with failed reason";
    log_tail.lines().rev().find_map(|line| {
        let (_, details) = line.split_once(MARKER)?;
        let (_, raw_error) = details.split_once("error=")?;
        let raw_error = raw_error.trim();
        let decoded = if raw_error.starts_with('"') {
            serde_json::from_str::<String>(raw_error).ok()?
        } else {
            raw_error.to_string()
        };
        let error = serde_json::from_str::<serde_json::Value>(&decoded).ok()?;
        let code = error
            .get("code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("provider.error");
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Kimi Code 模型请求失败");
        Some(format_kimi_provider_error(code, message))
    })
}

fn format_kimi_provider_error(code: &str, message: &str) -> String {
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("402") && normalized.contains("membership benefits") {
        return "Kimi Code 会员权益校验失败（HTTP 402）：请确认当前登录账号已开通且会员仍有效"
            .to_string();
    }
    if code.contains("auth") || normalized.contains("authentication") || normalized.contains("401")
    {
        return "Kimi Code 登录已失效（HTTP 401），请重新登录".to_string();
    }
    if normalized.contains("429")
        || normalized.contains("rate limit")
        || normalized.contains("quota")
    {
        return "Kimi Code 请求受限或额度不足，请稍后重试或检查账号额度".to_string();
    }
    let message = message
        .chars()
        .filter(|character| !character.is_control())
        .take(1000)
        .collect::<String>();
    format!("Kimi Code 请求失败（{code}）：{message}")
}

fn kimi_credentials_valid(raw: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    let token_present = ["access_token", "refresh_token"].into_iter().all(|key| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|token| !token.trim().is_empty())
    });
    // Kimi 的 access_token 约 15 分钟即过期，Kimi CLI 运行时会用 refresh_token 自动续期，
    // 因此 expires_at（Unix 秒）过期不判未认证，否则登录 15 分钟后状态就会误报。
    // 这里仅要求 expires_at 是合法的正数时间戳，用于识别损坏的凭证文件。
    let expiry_valid = value
        .get("expires_at")
        .and_then(serde_json::Value::as_i64)
        .is_some_and(|expiry| expiry > 0);
    token_present && expiry_valid
}

fn nonempty_env(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

fn cli_status_success(executable: &Path, args: &[&str]) -> bool {
    let mut command = if crate::platform::capabilities::is_windows()
        && executable.extension().and_then(|value| value.to_str()) == Some("cmd")
    {
        let mut command = crate::platform::process::HiddenCommand::new("cmd");
        command
            .args(["/D", "/S", "/C"])
            .arg(crate::platform::os::external_application_path(executable))
            .args(args);
        command
    } else {
        let mut command = crate::platform::process::HiddenCommand::new(
            crate::platform::os::external_application_path(executable),
        );
        command.args(args);
        command
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    match child.wait_timeout(Duration::from_secs(3)) {
        Ok(Some(status)) => status.success(),
        Ok(None) | Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_session_classification_survives_missing_auxiliary_index() {
        assert_eq!(
            acp_session_backend(AgentBackend::Deepseek, CODEX_ACP_SESSION_MODEL),
            Some(AgentBackend::CodexAcp)
        );
        assert_eq!(
            acp_session_backend(AgentBackend::Deepseek, CLAUDE_ACP_SESSION_MODEL),
            Some(AgentBackend::ClaudeAcp)
        );
        assert_eq!(
            acp_session_backend(AgentBackend::Deepseek, KIMI_ACP_SESSION_MODEL),
            Some(AgentBackend::KimiAcp)
        );
        assert_eq!(
            acp_session_backend(AgentBackend::ClaudeAcp, "unexpected legacy model"),
            Some(AgentBackend::ClaudeAcp)
        );
        assert_eq!(
            acp_session_backend(AgentBackend::Deepseek, "deepseek-chat"),
            None
        );
    }

    #[test]
    fn acp_recovery_preserves_original_agent_session_mode_and_workspace() {
        let state = json!({
            "pinvouSessionId": "pinvou-session",
            "adapter": {
                "agentId": "kimi",
                "package": KIMI_ACP_PACKAGE,
            },
            "session": {
                "session_id": "acp-session",
                "current_model_id": "kimi-test",
                "modes": {
                    "currentModeId": "stale-agent-mode",
                    "availableModes": [],
                },
                "config_options": [
                    {
                        "id": "mode",
                        "currentValue": "auto",
                    },
                    {
                        "id": "reasoning_effort",
                        "currentValue": "high",
                    }
                ],
            },
        });
        let temporary = Path::new("/tmp/pinvou-session-workspace");
        let project = PathBuf::from("/tmp/pinvou-project");
        let recovered = acp_recovery_record(
            "pinvou-session",
            AgentBackend::KimiAcp,
            &state,
            project.clone(),
            temporary,
        )
        .unwrap();

        assert_eq!(recovered.backend, AgentBackend::KimiAcp);
        assert_eq!(recovered.acp_session_id.as_deref(), Some("acp-session"));
        assert_eq!(recovered.acp_model_id.as_deref(), Some("kimi-test"));
        assert_eq!(recovered.acp_mode_id.as_deref(), Some("auto"));
        assert_eq!(
            recovered.acp_config_values.get("reasoning_effort"),
            Some(&"high".to_string())
        );
        assert_eq!(recovered.workspace_kind, CodexWorkspaceKind::Project);
        assert_eq!(recovered.workspace_path, Some(project));

        let temporary_recovered = acp_recovery_record(
            "pinvou-session",
            AgentBackend::KimiAcp,
            &state,
            temporary.to_path_buf(),
            temporary,
        )
        .unwrap();
        assert_eq!(
            temporary_recovered.workspace_kind,
            CodexWorkspaceKind::Temporary
        );
        assert_eq!(temporary_recovered.workspace_path, None);
    }

    #[test]
    fn acp_recovery_rejects_incomplete_or_mismatched_state() {
        let state = json!({
            "pinvouSessionId": "pinvou-session",
            "adapter": {
                "agentId": "claude",
                "package": CLAUDE_ACP_PACKAGE,
            },
            "session": {},
        });
        assert!(acp_recovery_record(
            "pinvou-session",
            AgentBackend::ClaudeAcp,
            &state,
            PathBuf::from("/tmp/pinvou-project"),
            Path::new("/tmp/pinvou-session-workspace"),
        )
        .is_err());
        let mismatched = json!({
            "pinvouSessionId": "pinvou-session",
            "adapter": {
                "agentId": "claude",
                "package": CLAUDE_ACP_PACKAGE,
            },
            "session": { "session_id": "claude-session" },
        });
        assert!(acp_recovery_record(
            "pinvou-session",
            AgentBackend::KimiAcp,
            &mismatched,
            PathBuf::from("/tmp/pinvou-project"),
            Path::new("/tmp/pinvou-session-workspace"),
        )
        .is_err());
    }

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
    fn claude_native_runtime_is_explicit_for_supported_platforms() {
        assert_eq!(
            claude_native_runtime("macos", "aarch64", false),
            Some(("claude-agent-sdk-darwin-arm64".to_string(), "claude"))
        );
        assert_eq!(
            claude_native_runtime("macos", "x86_64", false),
            Some(("claude-agent-sdk-darwin-x64".to_string(), "claude"))
        );
        assert_eq!(
            claude_native_runtime("windows", "x86_64", false),
            Some(("claude-agent-sdk-win32-x64".to_string(), "claude.exe"))
        );
        assert_eq!(
            claude_native_runtime("linux", "aarch64", true),
            Some(("claude-agent-sdk-linux-arm64-musl".to_string(), "claude"))
        );
        assert_eq!(claude_native_runtime("freebsd", "x86_64", false), None);
        assert_eq!(claude_native_runtime("windows", "riscv64", false), None);
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
    fn extracts_only_allowed_agent_authorization_urls() {
        assert_eq!(
            extract_agent_login_url(
                AgentBackend::CodexAcp,
                "https://auth.openai.com/oauth/authorize?response_type=code&state=test",
            ),
            Some(
                "https://auth.openai.com/oauth/authorize?response_type=code&state=test".to_string()
            )
        );
        assert_eq!(
            extract_agent_login_url(
                AgentBackend::ClaudeAcp,
                "If the browser did not open, visit: \u{1b}]8;;https://claude.com/cai/oauth/authorize?state=test\u{7}https://claude.com/cai/oauth/authorize?state=test\u{1b}]8;;\u{7}",
            ),
            Some("https://claude.com/cai/oauth/authorize?state=test".to_string())
        );
        assert_eq!(
            extract_agent_login_url(
                AgentBackend::ClaudeAcp,
                "Legacy Claude login: https://claude.ai/oauth/authorize?state=test",
            ),
            Some("https://claude.ai/oauth/authorize?state=test".to_string())
        );
        assert_eq!(
            extract_agent_login_url(
                AgentBackend::KimiAcp,
                "Opening https://www.kimi.com/code/authorize_device?user_code=ABCD-1234",
            ),
            Some("https://www.kimi.com/code/authorize_device?user_code=ABCD-1234".to_string())
        );
        assert_eq!(
            extract_agent_login_url(AgentBackend::ClaudeAcp, "https://example.com/not-claude",),
            None
        );
    }

    #[test]
    fn extracts_kimi_device_code_without_accepting_arbitrary_text() {
        let url = "https://www.kimi.com/code/authorize_device?user_code=MO3M-6JFK";
        assert_eq!(
            extract_device_code("Opening browser", Some(url)),
            Some("MO3M-6JFK".to_string())
        );
        assert_eq!(extract_device_code("enter code: <script>", None), None);
    }

    #[test]
    fn kimi_credentials_require_tokens_and_nonzero_expiry() {
        assert!(kimi_credentials_valid(
            r#"{"access_token":"access","refresh_token":"refresh","expires_at":1}"#
        ));
        // access_token 过期但 refresh_token 仍在时不判未认证（Kimi CLI 会自动续期）。
        assert!(kimi_credentials_valid(
            r#"{"access_token":"access","refresh_token":"refresh","expires_at":1700000000}"#
        ));
        assert!(!kimi_credentials_valid(
            r#"{"access_token":"access","refresh_token":"refresh","expires_at":0}"#
        ));
        assert!(!kimi_credentials_valid(
            r#"{"access_token":"","refresh_token":"refresh","expires_at":1}"#
        ));
        assert!(!kimi_credentials_valid("not-json"));
    }

    #[test]
    fn parses_kimi_provider_failure_from_session_log() {
        let log = concat!(
            "2026-07-27T08:18:51Z INFO llm request\n",
            "2026-07-27T08:18:51Z WARN acp: turn ended with failed reason  ",
            "error=\"{\\\"code\\\":\\\"provider.api_error\\\",",
            "\\\"message\\\":\\\"402 We're unable to verify your membership benefits at this time.\\\"}\"\n",
        );
        assert_eq!(
            parse_kimi_acp_failure(log).as_deref(),
            Some("Kimi Code 会员权益校验失败（HTTP 402）：请确认当前登录账号已开通且会员仍有效")
        );
        assert!(parse_kimi_acp_failure("INFO turn completed").is_none());
    }

    #[test]
    fn maps_kimi_auth_and_quota_failures_to_actionable_messages() {
        assert_eq!(
            format_kimi_provider_error("provider.auth_failed", "401 unauthorized"),
            "Kimi Code 登录已失效（HTTP 401），请重新登录"
        );
        assert_eq!(
            format_kimi_provider_error("provider.api_error", "429 quota exceeded"),
            "Kimi Code 请求受限或额度不足，请稍后重试或检查账号额度"
        );
    }

    #[test]
    fn resolves_only_kimi_session_logs_under_the_data_root() {
        let root = Path::new("/tmp/kimi-home");
        let index = concat!(
            "{\"sessionId\":\"session-safe\",\"sessionDir\":\"/tmp/kimi-home/sessions/wd_project/session-safe\"}\n",
            "{\"sessionId\":\"session-escape\",\"sessionDir\":\"/tmp/kimi-home/sessions/../credentials\"}\n",
        );
        assert_eq!(
            kimi_session_log_path_from_index(index, root, "session-safe"),
            Some(PathBuf::from(
                "/tmp/kimi-home/sessions/wd_project/session-safe/logs/kimi-code.log"
            ))
        );
        assert_eq!(
            kimi_session_log_path_from_index(index, root, "session-escape"),
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

    #[test]
    fn status_serializes_install_contract_fields() {
        let status = CodexAcpStatus {
            agent_id: "codex",
            agent_name: "Codex",
            version: CODEX_ACP_VERSION,
            installed: false,
            bridge_ready: false,
            adapter_path: None,
            node_available: false,
            node_version: None,
            node_supported: false,
            npm_available: false,
            codex_available: false,
            codex_path: None,
            codex_version: None,
            runtime_source: None,
            managed_codex_version: MANAGED_CODEX_VERSION,
            min_codex_version: MIN_CODEX_VERSION,
            install_method: platform::install_method(),
            brew_available: false,
            system_codex_incompatible: true,
            download_required: false,
            downloaded_bytes: 0,
            download_total_bytes: 0,
            download_progress: None,
            authenticated: false,
            login_in_progress: false,
            login_url: None,
            login_code: None,
            login_input_required: false,
            installing: false,
            error: None,
            setup_hint: None,
        };
        let value = serde_json::to_value(&status).expect("serialize CodexAcpStatus");
        assert_eq!(value["min_codex_version"], json!(MIN_CODEX_VERSION));
        assert_eq!(value["install_method"], json!(platform::install_method()));
        assert_eq!(value["brew_available"], json!(false));
        assert_eq!(value["system_codex_incompatible"], json!(true));
    }

    #[test]
    fn install_method_matches_platform_contract() {
        // 具体取值由各平台适配实现保证（platform/ 下的 INSTALL_METHOD），
        // 这里只校验契约允许值，避免在适配层之外出现平台 cfg。
        assert!(matches!(
            platform::install_method(),
            "homebrew" | "managed_download" | "manual"
        ));
    }
}
