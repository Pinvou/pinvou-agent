//! 外部 Agent ACP 运行时。
//!
//! pinvou3 只做 ACP client、进程托管、权限路由、事件持久化和 `acp:event` 投影；
//! Codex、Claude Code 与 Kimi 的模型调用、工具循环、会话与权限协议都由各自
//! ACP Agent 提供。

mod attachments;
mod diagnostics;
mod events;
mod latest;
mod platform;
pub(crate) mod reader_window;
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
use latest::LatestVersionProbe;
use runtime::{
    resolve_codex_path, system_codex_incompatible, version_at_least, ResolvedCodex,
    MIN_CODEX_VERSION,
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
/// claude-agent-acp 要求的最低 claude CLI 版本（输出形如 `2.1.163 (Claude Code)`）。
const MIN_CLAUDE_VERSION: &str = "2.0.0";
/// Kimi ACP 要求的最低 kimi CLI 版本（裸 semver；旧 Python 版 kimi-cli 已废弃）。
const MIN_KIMI_VERSION: &str = "0.9.0";
const CODEX_INSTALL_SCRIPT_UNIX: &str = "https://chatgpt.com/codex/install.sh";
const CODEX_INSTALL_SCRIPT_WINDOWS: &str = "https://chatgpt.com/codex/install.ps1";
const CLAUDE_INSTALL_SCRIPT_UNIX: &str = "https://claude.ai/install.sh";
const CLAUDE_INSTALL_SCRIPT_WINDOWS: &str = "https://claude.ai/install.ps1";
const KIMI_INSTALL_SCRIPT_UNIX: &str = "https://code.kimi.com/kimi-code/install.sh";
const KIMI_INSTALL_SCRIPT_WINDOWS: &str = "https://code.kimi.com/kimi-code/install.ps1";

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
        code_session: false,
    })
}

fn load_acp_recovery_record(
    session_id: &str,
    expected_backend: AgentBackend,
    session_store: &SessionStore,
) -> Result<SessionAgentRecord> {
    let temporary_workspace = session_store.session_roots(session_id)?.execution;
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

/// 启动时原生代码会话 sidecar 恢复/回填/清理的统计，供真实恢复信号计数与测试断言。
#[derive(Debug, Default, PartialEq, Eq)]
struct SidecarRecoverySummary {
    /// 真实从 sidecar 恢复的索引记录数（索引完好不误报）。
    restored: usize,
    /// 索引在而 sidecar 缺失时按索引补写的 sidecar 数。
    backfilled: usize,
    /// 已被 ACP 会话占用、作为残留清理的 sidecar 数。
    cleaned: usize,
}

/// 扫描 session 私有目录中的原生代码会话权威 sidecar，恢复辅助索引缺失的
/// 原生代码会话记录，并回填索引在而 sidecar 缺失的记录。
///
/// sidecar（`code-session.json`）随会话存续，是跨进程的权威真相源；辅助索引
/// `session-agents.json` 可损坏/丢失后据此重建。扫描根与 sidecar 读取根同源
/// （均由辅助索引路径派生，见 `store::code_session_sidecar_root`），避免两处
/// 根定义漂移。对每个 sidecar：索引已持有 code_session 记录时跳过（不误报
/// 恢复）；会话已被 ACP 占用时 sidecar 属历史残留，直接清理而非反复重试；
/// 其余情况据 sidecar 恢复索引。
fn restore_code_native_sessions_from_sidecars(
    agents: &SessionAgentStore,
) -> SidecarRecoverySummary {
    let mut summary = SidecarRecoverySummary::default();
    let sessions_root = store::code_session_sidecar_root(agents.path());
    let entries = match std::fs::read_dir(&sessions_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // sessions 根不存在 = 没有任何会话，无需恢复。
            return summary;
        }
        Err(error) => {
            eprintln!(
                "[pinvou3-app] scan native code session sidecars failed ({}): {error:#}",
                sessions_root.display()
            );
            return summary;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!(
                    "[pinvou3-app] scan native code session sidecars failed to read an entry: {error:#}"
                );
                continue;
            }
        };
        let session_dir = entry.path();
        if !session_dir.is_dir() {
            continue;
        }
        let Some(session_id) = session_dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(sidecar) = store::read_code_session_sidecar(agents.path(), session_id) else {
            continue;
        };
        let record = agents.get(session_id);
        if record.code_session {
            // 索引完好无需恢复：不计入 restored，避免每次启动误报恢复信号。
            continue;
        }
        if record.backend.is_acp() {
            // 会话已被 ACP 绑定：sidecar 是历史残留，恢复必然被拒，直接清理并
            // 如实记日志，而不是每次启动重复报 degraded。
            store::remove_code_session_sidecar(agents.path(), session_id);
            summary.cleaned += 1;
            eprintln!(
                "[pinvou3-app] removed leftover native code session sidecar for ACP session {session_id}"
            );
            continue;
        }
        match agents.restore_missing_code_session_record(session_id, sidecar) {
            Ok(true) => {
                summary.restored += 1;
                eprintln!("[pinvou3-app] recovered native code session index for {session_id}");
            }
            Ok(false) => {}
            Err(error) => eprintln!(
                "[pinvou3-app] native code session {session_id} remains degraded: {error:#}"
            ),
        }
    }
    // 回填自愈：索引在而 sidecar 缺失（修复前构建的存量会话，或绑定时 sidecar
    // 写失败）时按索引补写 sidecar，写失败逐条记日志。
    summary.backfilled = agents.backfill_missing_code_session_sidecars();
    if summary.restored > 0 {
        eprintln!(
            "[pinvou3-app] recovered {} native code session index record(s) from sidecars",
            summary.restored
        );
    }
    if summary.backfilled > 0 {
        eprintln!(
            "[pinvou3-app] backfilled {} missing native code session sidecar(s) from index",
            summary.backfilled
        );
    }
    summary
}

#[derive(Debug, Clone, Serialize)]
pub struct CodexAcpStatus {
    pub agent_id: &'static str,
    pub agent_name: &'static str,
    /// 实际探测到的 CLI 版本（`--version` 原始输出）；未探测到时为 None。
    pub version: Option<String>,
    /// 检测到官方新版本时的升级目标；已是最新、离线或接口异常时为 None。
    pub latest_version: Option<String>,
    /// CLI 存在且满足最低兼容版本；官方 latest 仅通过 update_available 提醒。
    pub installed: bool,
    /// 当前 CLI 低于官方最新版；这是可暂缓的升级提醒，不阻止继续使用。
    pub update_available: bool,
    /// Agent 明确报告必须升级；这是不可暂缓的动态门禁。
    pub update_required: bool,
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
    /// codex-acp 验证过的最低 Codex CLI 版本（所有运行时来源统一强制）。
    pub min_codex_version: &'static str,
    /// 该 Agent CLI 的最低版本要求（"0.144.6" / "2.0.0" / "0.9.0"）。
    pub min_version: &'static str,
    /// 缺失、低于最低版本或发现官方新版本时前端应提供的安装/升级动作：
    /// "none"（已合规）/ "brew_upgrade"（macOS brew 升级）/
    /// "npm_upgrade"（npm 全局升级）/ "official_script"（官方脚本）/ "manual"。
    pub install_action: &'static str,
    /// 已探测到 CLI 的安装来源："brew" / "npm" / "script"（官方脚本目录）/
    /// null（无 CLI 或来源未知）。
    pub install_source: Option<String>,
    /// 仅 macOS 探测 Homebrew；其他平台恒 false。
    pub brew_available: bool,
    /// 系统 PATH 里找到了 codex 但版本低于 min_codex_version，
    /// 用于 UI 区分「版本过低」与「未安装」。
    pub system_codex_incompatible: bool,
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

/// 安装、升级和运行时错误必须与 Agent 绑定。共享单值会让 Claude/Kimi 的
/// Homebrew 错误显示到 Codex，或让其他 Agent 的成功操作清掉 Codex 升级门禁错误。
#[derive(Clone, Default)]
struct AgentRuntimeErrors {
    inner: Arc<parking_lot::RwLock<HashMap<AgentBackend, String>>>,
}

impl AgentRuntimeErrors {
    fn get(&self, backend: AgentBackend) -> Option<String> {
        self.inner.read().get(&backend).cloned()
    }

    fn set(&self, backend: AgentBackend, error: String) {
        self.inner.write().insert(backend, error);
    }

    fn clear(&self, backend: AgentBackend) {
        self.inner.write().remove(&backend);
    }
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

/// “代码”模块原生（品悟 Engine）会话的工作区信息。
///
/// 临时会话执行目录与 ACP 临时会话一样由 `SessionStore::session_roots` 推导；
/// 项目会话返回绑定的项目目录，available 语义与 ACP 项目分支一致（目录存在即可用）。
fn code_native_workspace_info(
    session_store: &SessionStore,
    session_id: &str,
    record: &SessionAgentRecord,
) -> Result<CodexAcpWorkspaceInfo> {
    if record.workspace_kind == CodexWorkspaceKind::Project {
        let path = record
            .workspace_path
            .clone()
            .context("原生项目会话缺少工作目录记录")?;
        return Ok(CodexAcpWorkspaceInfo {
            workspace_kind: CodexWorkspaceKind::Project,
            workspace_available: path.is_dir(),
            workspace_path: path.to_string_lossy().into_owned(),
        });
    }
    let path = session_store
        .session_roots(session_id)
        .map(|roots| roots.execution)
        .with_context(|| format!("解析会话 {session_id} 临时工作目录失败"))?;
    Ok(CodexAcpWorkspaceInfo {
        workspace_kind: CodexWorkspaceKind::Temporary,
        workspace_path: path.to_string_lossy().into_owned(),
        workspace_available: true,
    })
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
    installing_agents: Arc<parking_lot::RwLock<HashSet<AgentBackend>>>,
    login_states: Arc<parking_lot::RwLock<HashMap<AgentBackend, AgentLoginState>>>,
    login_inputs: Arc<Mutex<HashMap<AgentBackend, ChildStdin>>>,
    codex_upgrade_required: Arc<AtomicBool>,
    runtime_errors: AgentRuntimeErrors,
    runtime_probe: Arc<parking_lot::RwLock<RuntimeProbeCache>>,
    runtime_probe_gate: Arc<Mutex<()>>,
    cli_probe: Arc<parking_lot::RwLock<CliProbeCache>>,
    latest_version_probe: LatestVersionProbe,
    bundled_adapter: Option<PathBuf>,
    bundled_claude_adapter: Option<PathBuf>,
    bundled_node: Option<PathBuf>,
}

/// 同一 Agent 的安装/升级互斥，不同 Agent 的状态与任务彼此隔离。
/// guard 在外部命令完成后显式 drop，使随后返回的 status 已恢复 installing=false；
/// 提前返回或 panic 时 Drop 仍会兜底清理。
struct AgentInstallGuard {
    installing_agents: Arc<parking_lot::RwLock<HashSet<AgentBackend>>>,
    backend: AgentBackend,
}

impl AgentInstallGuard {
    fn try_start(
        installing_agents: &Arc<parking_lot::RwLock<HashSet<AgentBackend>>>,
        backend: AgentBackend,
    ) -> Option<Self> {
        if !installing_agents.write().insert(backend) {
            return None;
        }
        Some(Self {
            installing_agents: installing_agents.clone(),
            backend,
        })
    }
}

impl Drop for AgentInstallGuard {
    fn drop(&mut self) {
        self.installing_agents.write().remove(&self.backend);
    }
}

#[derive(Debug, Clone, Default)]
struct RuntimeProbeCache {
    initialized: bool,
    node_version: Option<String>,
    codex: Option<ResolvedCodex>,
    brew_available: bool,
    /// 已解析或 PATH/官方目录中版本过旧 codex 的安装来源（"brew"/"npm"/"script"），
    /// 供版本过旧时按来源分派 brew/npm 升级。
    codex_install_source: Option<&'static str>,
    system_codex_incompatible: bool,
}

/// claude / kimi CLI 探测缓存：前端按秒轮询状态，不能每次都 spawn `--version`。
/// 「重新检测」与安装完成后通过 invalidate_cli_probe 强制刷新。
#[derive(Debug, Clone, Default)]
struct CliProbeCache {
    initialized: bool,
    claude: Option<ResolvedCli>,
    kimi: Option<ResolvedCli>,
}

#[derive(Debug, Clone)]
struct ResolvedCli {
    path: PathBuf,
    /// `--version` 原始输出；版本门禁单独校验，解析失败一律不合规。
    version: Option<String>,
    /// 安装来源（"brew"/"npm"/"script"），供版本过旧时按来源分派升级。
    install_source: Option<&'static str>,
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
        // 原生代码会话的权威 sidecar 恢复：sidecar 随 session 私有目录存续，辅助
        // 索引（session-agents.json）损坏/丢失后，据 sidecar 恢复原生代码会话类型
        // 与项目目录绑定，避免会话静默掉回普通聊天、执行根退回私有目录。
        restore_code_native_sessions_from_sidecars(&agents);
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
            installing_agents: Arc::new(parking_lot::RwLock::new(HashSet::new())),
            login_states: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            login_inputs: Arc::new(Mutex::new(HashMap::new())),
            codex_upgrade_required: Arc::new(AtomicBool::new(false)),
            runtime_errors: AgentRuntimeErrors::default(),
            runtime_probe: Arc::new(parking_lot::RwLock::new(RuntimeProbeCache::default())),
            runtime_probe_gate: Arc::new(Mutex::new(())),
            cli_probe: Arc::new(parking_lot::RwLock::new(CliProbeCache::default())),
            latest_version_probe: LatestVersionProbe::new()?,
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
        if record.code_session {
            return code_native_workspace_info(&self.session_store, session_id, &record);
        }
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
                .session_roots(session_id)
                .map(|roots| roots.execution)
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
        // 防御性保留，当前不可达：两个调用方（prompt / ensure ACP runtime）都在
        // 上游通过 is_acp / acp_record 拒绝了原生代码会话；保留本分支是为了让
        // 未来直接使用本方法的路径对 code_session 也有正确语义。
        if record.code_session {
            if record.workspace_kind == CodexWorkspaceKind::Project {
                let path = record
                    .workspace_path
                    .clone()
                    .context("原生项目会话缺少工作目录记录")?;
                return validate_codex_project_workspace(&path).with_context(|| {
                    format!(
                        "品悟会话绑定的项目目录已不可用: {}。请恢复该目录，或新建会话选择其他项目",
                        path.display()
                    )
                });
            }
            return self
                .session_store
                .session_roots(session_id)
                .map(|roots| roots.execution)
                .with_context(|| format!("解析会话 {session_id} 临时工作目录失败"));
        }
        let record = self.acp_record(session_id)?;
        match record.workspace_kind {
            CodexWorkspaceKind::Temporary => self
                .session_store
                .session_roots(session_id)
                .map(|roots| roots.execution)
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

    /// 「重新检测」入口：忽略探测缓存强制重新探测后返回最新状态，
    /// 供用户在 App 外手动安装/升级 CLI 后刷新（安装/升级成功路径
    /// 已通过 refresh_agent_cli_probe 自动失效缓存）。
    pub async fn recheck_agent_status(&self, agent_id: &str) -> Result<CodexAcpStatus> {
        let backend = AgentBackend::parse(Some(agent_id))?;
        if !backend.is_acp() {
            bail!("Agent 不是 ACP 后端: {agent_id}");
        }
        let codex_upgrade_was_required = backend == AgentBackend::CodexAcp
            && self.codex_upgrade_required.load(Ordering::Acquire);
        let previous_codex_version = codex_upgrade_was_required
            .then(|| {
                self.runtime_probe
                    .read()
                    .codex
                    .as_ref()
                    .map(|resolved| resolved.version.clone())
            })
            .flatten();
        self.refresh_agent_cli_probe(backend).await;
        self.latest_version_probe.refresh(backend, true).await;
        let current_codex_version = self
            .runtime_probe
            .read()
            .codex
            .as_ref()
            .map(|resolved| resolved.version.clone());
        if codex_upgrade_was_required
            && codex_version_changed(
                previous_codex_version.as_deref(),
                current_codex_version.as_deref(),
            )
        {
            self.codex_upgrade_required.store(false, Ordering::Release);
            self.runtime_errors.clear(AgentBackend::CodexAcp);
        }
        Ok(self.status_for_async(backend).await)
    }

    fn status_for(&self, backend: AgentBackend) -> CodexAcpStatus {
        let login = self
            .login_states
            .read()
            .get(&backend)
            .cloned()
            .unwrap_or_default();
        if backend == AgentBackend::KimiAcp {
            let kimi = self.cli_probe().kimi;
            let kimi_path = kimi
                .as_ref()
                .map(|cli| cli.path.to_string_lossy().into_owned());
            let kimi_version = kimi.as_ref().and_then(|cli| cli.version.clone());
            let install_source = kimi
                .as_ref()
                .and_then(|cli| cli.install_source)
                .map(str::to_string);
            let version_supported = kimi_version.as_deref().is_some_and(kimi_version_supported);
            // Kimi 直接 spawn 原生 CLI（kimi acp）：先表达最低版本门禁；官方
            // latest 门禁由异步 status_for_async 在联网查询完成后统一叠加。
            let cli_ready = kimi.is_some() && version_supported;
            let installed = cli_ready;
            let authenticated = !login.in_progress
                && kimi
                    .as_ref()
                    .is_some_and(|cli| kimi_authenticated(&cli.path));
            return CodexAcpStatus {
                agent_id: "kimi",
                agent_name: "Kimi",
                version: kimi_version.clone(),
                latest_version: None,
                installed,
                update_available: false,
                update_required: false,
                // Kimi 直接运行原生 CLI，不依赖独立 Bridge。CLI 是否可用由
                // installed 单独表达，避免未安装时被前端 Bridge 错误分支截断。
                bridge_ready: true,
                adapter_path: kimi_path.clone(),
                // Kimi 不经 Node bridge，Node 字段不适用；node_supported 视为无门槛满足。
                node_available: false,
                node_version: None,
                node_supported: true,
                npm_available: npm_executable().is_some(),
                codex_available: cli_ready,
                codex_path: kimi_path,
                codex_version: kimi_version,
                runtime_source: kimi.as_ref().map(|_| "system"),
                min_codex_version: "",
                min_version: MIN_KIMI_VERSION,
                install_action: if installed {
                    "none"
                } else {
                    install_action_for(
                        backend,
                        kimi.as_ref().and_then(|cli| cli.install_source),
                        npm_executable().is_some(),
                        official_script_supported(backend),
                    )
                },
                install_source,
                brew_available: false,
                system_codex_incompatible: false,
                authenticated,
                login_in_progress: login.in_progress,
                login_url: login.url,
                login_code: login.code,
                login_input_required: false,
                installing: self.installing_agents.read().contains(&backend),
                error: login.error.or_else(|| self.runtime_errors.get(backend)),
                setup_hint: if !cli_ready {
                    Some("kimi_cli_missing")
                } else if !authenticated {
                    Some("kimi_auth_required")
                } else {
                    None
                },
            };
        }

        let (agent_id, agent_name, adapter) = match backend {
            AgentBackend::CodexAcp => ("codex", "Codex", self.resolve_adapter()),
            AgentBackend::ClaudeAcp => ("claude", "Claude Code", self.resolve_claude_adapter()),
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
            .then(|| self.cli_probe().claude)
            .flatten();
        let claude_supported = claude
            .as_ref()
            .and_then(|cli| cli.version.as_deref())
            .is_some_and(claude_version_supported);
        // 「可用」要求 CLI 存在且版本满足最低门禁；版本过旧与未安装一样进入安装流程。
        let provider_available = match backend {
            AgentBackend::CodexAcp => codex.is_some(),
            AgentBackend::ClaudeAcp => claude.is_some() && claude_supported,
            AgentBackend::Deepseek | AgentBackend::KimiAcp => unreachable!(),
        };
        let provider_version = match backend {
            AgentBackend::CodexAcp => codex.as_ref().map(|resolved| resolved.version.clone()),
            AgentBackend::ClaudeAcp => claude.as_ref().and_then(|cli| cli.version.clone()),
            AgentBackend::Deepseek | AgentBackend::KimiAcp => unreachable!(),
        };
        let bridge_ready = adapter.is_some() && node_supported;
        let codex_available = provider_available;
        let dynamic_codex_upgrade_required = backend == AgentBackend::CodexAcp
            && self.codex_upgrade_required.load(Ordering::Acquire);
        let installed = match backend {
            AgentBackend::CodexAcp => {
                bridge_ready && codex_available && !dynamic_codex_upgrade_required
            }
            AgentBackend::ClaudeAcp => provider_available,
            AgentBackend::Deepseek | AgentBackend::KimiAcp => unreachable!(),
        };
        let install_action = match backend {
            AgentBackend::CodexAcp => {
                if codex.is_some() && !dynamic_codex_upgrade_required {
                    "none"
                } else {
                    // 过旧 CLI 按安装来源分派 brew/npm 升级；无 CLI、脚本来源或
                    // 来源未知时统一运行官方安装脚本。
                    install_action_for(
                        backend,
                        probe.as_ref().and_then(|probe| probe.codex_install_source),
                        npm_executable().is_some(),
                        official_script_supported(backend),
                    )
                }
            }
            AgentBackend::ClaudeAcp => {
                if installed {
                    "none"
                } else {
                    install_action_for(
                        backend,
                        claude.as_ref().and_then(|cli| cli.install_source),
                        npm_executable().is_some(),
                        official_script_supported(backend),
                    )
                }
            }
            AgentBackend::Deepseek | AgentBackend::KimiAcp => unreachable!(),
        };
        // 登录命令运行期间不要再启动同一 CLI 的 auth status。部分 CLI 会让两条
        // 命令争用凭证锁，原来的 750ms 状态轮询因此可能拖住 Tauri 的 IPC/UI。
        let authenticated = !login.in_progress
            && match backend {
                AgentBackend::CodexAcp => codex
                    .as_ref()
                    .is_some_and(|resolved| codex_authenticated(&resolved.path)),
                AgentBackend::ClaudeAcp => claude
                    .as_ref()
                    .is_some_and(|cli| claude_authenticated(&cli.path)),
                AgentBackend::Deepseek | AgentBackend::KimiAcp => unreachable!(),
            };
        CodexAcpStatus {
            agent_id,
            agent_name,
            version: provider_version.clone(),
            latest_version: None,
            installed,
            update_available: false,
            update_required: dynamic_codex_upgrade_required,
            bridge_ready,
            adapter_path: adapter
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            node_available: node_version.is_some(),
            node_version,
            node_supported,
            npm_available: npm_executable().is_some(),
            codex_available,
            codex_path: if backend == AgentBackend::CodexAcp {
                codex
                    .as_ref()
                    .map(|resolved| resolved.path.to_string_lossy().into_owned())
            } else {
                claude
                    .as_ref()
                    .map(|cli| cli.path.to_string_lossy().into_owned())
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
            min_codex_version: if backend == AgentBackend::CodexAcp {
                MIN_CODEX_VERSION
            } else {
                ""
            },
            min_version: match backend {
                AgentBackend::CodexAcp => MIN_CODEX_VERSION,
                AgentBackend::ClaudeAcp => MIN_CLAUDE_VERSION,
                AgentBackend::Deepseek | AgentBackend::KimiAcp => unreachable!(),
            },
            install_action,
            install_source: match backend {
                AgentBackend::CodexAcp => probe
                    .as_ref()
                    .and_then(|probe| probe.codex_install_source)
                    .map(str::to_string),
                AgentBackend::ClaudeAcp => claude
                    .as_ref()
                    .and_then(|cli| cli.install_source)
                    .map(str::to_string),
                AgentBackend::Deepseek | AgentBackend::KimiAcp => unreachable!(),
            },
            brew_available: probe.as_ref().is_some_and(|probe| probe.brew_available),
            system_codex_incompatible: probe
                .as_ref()
                .is_some_and(|probe| probe.system_codex_incompatible),
            authenticated,
            login_in_progress: login.in_progress,
            login_url: login.url,
            login_code: login.code,
            login_input_required: login.input_required,
            // 安装状态按 Agent 隔离；Claude/Kimi 的任务不会污染 Codex 状态，反之亦然。
            installing: self.installing_agents.read().contains(&backend),
            error: login.error.or_else(|| self.runtime_errors.get(backend)),
            // installed=false 多为桥或 Node 缺失，不属于认证问题，不给认证类提示；
            // Claude Code 不再随包内置；仅在 CLI 缺失或低于最低版本时给 missing
            // 提示，低于 latest 的升级提示由 update_available 单独表达。
            setup_hint: if backend == AgentBackend::ClaudeAcp && !provider_available {
                Some("claude_cli_missing")
            } else if backend == AgentBackend::ClaudeAcp && provider_available && !authenticated {
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
        let system_codex = resolve_codex_cli();
        let legacy_codex = adapter.as_deref().and_then(codex_path_for_adapter);
        diagnostics::write(
            &operation_id,
            "probe:start",
            format!(
                "force={force} node_path={} system_codex_path={}",
                node.as_deref()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "none".to_string()),
                system_codex
                    .as_deref()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "none".to_string())
            ),
        );
        let detected = tokio::task::spawn_blocking(move || {
            let brew_available = platform::brew_available();
            let codex = resolve_codex_path(system_codex.clone(), legacy_codex);
            // 已解析（合规）或 PATH 中过旧的 codex 都判定安装来源，供升级分派。
            let codex_install_source = codex
                .as_ref()
                .map(|resolved| resolved.path.clone())
                .or_else(|| system_codex.clone())
                .and_then(|path| detect_install_source(AgentBackend::CodexAcp, &path));
            RuntimeProbeCache {
                initialized: true,
                node_version: node.as_deref().and_then(installed_node_version),
                codex,
                brew_available,
                codex_install_source,
                system_codex_incompatible: system_codex_incompatible(system_codex),
            }
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

    /// 验证 Codex Bridge 与 CLI 已就绪；不会隐式安装或执行外部脚本。
    async fn ensure_codex_ready(&self) -> Result<CodexAcpStatus> {
        self.refresh_runtime_probe(false).await;
        let status = self.status_async().await;
        if !status.bridge_ready {
            bail!("Pinvou 安装包缺少可用的 Codex ACP Bridge，请重新安装或重新生成 Bridge Runtime");
        }
        if status.update_required {
            bail!("当前 Codex CLI 已无法支持所选模型，请先升级到官方最新版后重试");
        }
        if !status.codex_available {
            bail!("未检测到兼容的 Codex CLI，请先通过 Pinvou 运行官方安装或升级后重试");
        }
        Ok(status)
    }

    fn codex_version_before_install(&self, backend: AgentBackend) -> Option<String> {
        (backend == AgentBackend::CodexAcp)
            .then(|| {
                self.runtime_probe
                    .read()
                    .codex
                    .as_ref()
                    .map(|resolved| resolved.version.clone())
            })
            .flatten()
    }

    /// 安装命令退出 0 不等于动态升级已经生效。若服务端已要求新版 Codex，
    /// 只有实际解析到的版本发生变化才能解除门禁；`brew already up-to-date`、
    /// 升级到另一份副本等情况必须继续保持不可用。
    async fn finalize_agent_install(
        &self,
        backend: AgentBackend,
        previous_codex_version: Option<String>,
    ) -> Result<CodexAcpStatus> {
        let mut status = self.status_for_async(backend).await;
        let clear_error = if backend == AgentBackend::CodexAcp {
            !self.codex_upgrade_required.load(Ordering::Acquire)
                || codex_version_changed(
                    previous_codex_version.as_deref(),
                    status.codex_version.as_deref(),
                )
        } else {
            true
        };
        if clear_error {
            self.runtime_errors.clear(backend);
            if backend == AgentBackend::CodexAcp && status.update_required {
                self.codex_upgrade_required.store(false, Ordering::Release);
                status = self.status_for_async(backend).await;
            }
        }
        if let Err(error) = latest::ensure_agent_cli_ready(backend, &status) {
            self.runtime_errors.set(backend, format!("{error:#}"));
            return Err(error);
        }
        Ok(status)
    }

    /// macOS 上通过 Homebrew 安装系统 Codex（cask 名 codex）。
    pub async fn install_via_homebrew(&self) -> Result<CodexAcpStatus> {
        self.upgrade_via_homebrew(AgentBackend::CodexAcp).await
    }

    /// 通过 Homebrew 升级 Agent CLI：codex=`brew upgrade --cask codex`（未安装时
    /// 回退 install）、claude=`brew upgrade --cask claude-code`、
    /// kimi=`brew upgrade kimi-code`（kimi-code 是 formula，无 --cask）。
    /// claude/kimi 只在探测到 brew 来源时进入本分支，一律 upgrade。
    async fn upgrade_via_homebrew(&self, backend: AgentBackend) -> Result<CodexAcpStatus> {
        let operation_id = diagnostics::operation_id("homebrew-install");
        let previous_codex_version = self.codex_version_before_install(backend);
        if !platform::brew_available() {
            diagnostics::write(&operation_id, "homebrew:unavailable", "result=rejected");
            bail!(
                "未检测到 Homebrew。请先从 https://brew.sh 安装 Homebrew 后重试，\
                 或按官方文档手动安装 {} CLI",
                backend.display_name()
            );
        }
        let Some(install_guard) = AgentInstallGuard::try_start(&self.installing_agents, backend)
        else {
            diagnostics::write(
                &operation_id,
                "homebrew:already_installing",
                "result=rejected",
            );
            bail!("{} 正在通过 Homebrew 安装，请稍候", backend.display_name());
        };
        diagnostics::write(
            &operation_id,
            "homebrew:start",
            format!("agent={}", backend.agent_id().unwrap_or("unknown")),
        );
        // brew install/upgrade 是阻塞式子进程，放到 spawn_blocking 避免卡住 async runtime。
        let result = tokio::task::spawn_blocking(move || {
            let run_brew = |args: &[&str]| -> Result<std::process::Output> {
                std::process::Command::new(platform::brew_bin())
                    .args(args)
                    .output()
                    .context("启动 Homebrew 失败")
            };
            // 幂等提示不算错误：install 报 already installed，upgrade 报 already up-to-date。
            let already_done = |output: &std::process::Output| {
                ["already installed", "already up-to-date"]
                    .iter()
                    .any(|marker| {
                        String::from_utf8_lossy(&output.stdout).contains(marker)
                            || String::from_utf8_lossy(&output.stderr).contains(marker)
                    })
            };
            let (command, args) = brew_install_args(backend, brew_package_installed(backend))
                .with_context(|| format!("{} 不支持 Homebrew 升级", backend.display_name()))?;
            let output = run_brew(&args)?;
            if output.status.success() || already_done(&output) {
                return Ok(());
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
            bail!(
                "{command} 失败 (exit {}): {}",
                output.status.code().unwrap_or(-1),
                tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
            );
        })
        .await;
        drop(install_guard);
        match result.context("等待 Homebrew 安装任务失败")? {
            Ok(()) => {
                self.refresh_agent_cli_probe(backend).await;
                diagnostics::write(&operation_id, "homebrew:complete", "result=success");
                self.finalize_agent_install(backend, previous_codex_version)
                    .await
            }
            Err(error) => {
                let detail = format!("{error:#}");
                diagnostics::write(&operation_id, "homebrew:failed", &detail);
                self.runtime_errors.set(
                    backend,
                    format!(
                        "{detail}（诊断编号：{operation_id}；日志：{}）",
                        diagnostics::log_path().display()
                    ),
                );
                Err(error)
            }
        }
    }

    /// `install_acp_agent` 命令入口：按 status.install_action 分派安装，
    /// 完成后强制重新探测并返回最新状态。action 提供时经校验后优先。
    pub async fn install_agent(
        &self,
        agent_id: &str,
        action: Option<&str>,
    ) -> Result<CodexAcpStatus> {
        let backend = AgentBackend::parse(Some(agent_id))?;
        if !backend.is_acp() {
            bail!("Agent 不是 ACP 后端: {agent_id}");
        }
        // 分派前强制刷新探测，确保 install_action 基于当前真实环境。
        self.refresh_agent_cli_probe(backend).await;
        let status = self.status_for_async(backend).await;
        let action = match action {
            Some(action) => parse_install_action(action)?,
            None => status.install_action,
        };
        match action {
            "none" => Ok(status),
            "brew_upgrade" => self.upgrade_via_homebrew(backend).await,
            "npm_upgrade" => self.upgrade_via_npm(backend).await,
            "official_script" => self.install_via_official_script(backend).await,
            "manual" => bail!(
                "当前平台不支持自动安装 {} CLI，请按官方文档手动安装后重试",
                backend.display_name()
            ),
            other => bail!("未知安装方式: {other}"),
        }
    }

    /// 安装/升级后按 Agent 强制重探测：codex 走 runtime probe，claude/kimi 走 CLI 缓存。
    async fn refresh_agent_cli_probe(&self, backend: AgentBackend) {
        match backend {
            AgentBackend::CodexAcp => self.refresh_runtime_probe(true).await,
            _ => self.invalidate_cli_probe(),
        }
    }

    /// 通过 npm 全局升级 Agent CLI（`npm install -g <pkg>@latest`），输出写诊断日志。
    async fn upgrade_via_npm(&self, backend: AgentBackend) -> Result<CodexAcpStatus> {
        let operation_id = diagnostics::operation_id("npm-upgrade");
        let previous_codex_version = self.codex_version_before_install(backend);
        if npm_package(backend).is_none() {
            diagnostics::write(&operation_id, "npm:unsupported", "result=rejected");
            bail!("{} 不支持 npm 全局升级", backend.display_name());
        }
        let Some(install_guard) = AgentInstallGuard::try_start(&self.installing_agents, backend)
        else {
            diagnostics::write(&operation_id, "npm:already_installing", "result=rejected");
            bail!("{} 正在升级，请稍候", backend.display_name());
        };
        diagnostics::write(
            &operation_id,
            "npm:start",
            format!("agent={}", backend.agent_id().unwrap_or("unknown")),
        );
        let result = run_npm_global_upgrade(backend, &operation_id).await;
        drop(install_guard);
        // 无论成败都强制重新探测：npm 可能部分完成（已写入二进制但链接失败）。
        self.refresh_agent_cli_probe(backend).await;
        match result {
            Ok(()) => {
                diagnostics::write(&operation_id, "npm:complete", "result=success");
                self.finalize_agent_install(backend, previous_codex_version)
                    .await
            }
            Err(error) => {
                let detail = format!("{error:#}");
                diagnostics::write(&operation_id, "npm:failed", &detail);
                self.runtime_errors.set(backend, detail);
                Err(error)
            }
        }
    }

    /// 通过各 Agent 官方安装脚本安装 CLI（免管理员）。
    /// 脚本无进度输出约定，前端以进行中 spinner 呈现；输出写入诊断日志。
    async fn install_via_official_script(&self, backend: AgentBackend) -> Result<CodexAcpStatus> {
        let operation_id = diagnostics::operation_id("script-install");
        let previous_codex_version = self.codex_version_before_install(backend);
        if !official_script_supported(backend) {
            diagnostics::write(&operation_id, "script:unsupported", "result=rejected");
            bail!(
                "当前平台不支持 {} 官方安装脚本，请手动安装",
                backend.display_name()
            );
        }
        let Some(install_guard) = AgentInstallGuard::try_start(&self.installing_agents, backend)
        else {
            diagnostics::write(
                &operation_id,
                "script:already_installing",
                "result=rejected",
            );
            bail!("{} 正在安装，请稍候", backend.display_name());
        };
        diagnostics::write(
            &operation_id,
            "script:start",
            format!("agent={}", backend.agent_id().unwrap_or("unknown")),
        );
        let result = run_official_install_script(backend, &operation_id).await;
        drop(install_guard);
        // 无论成败都按 Agent 强制重新探测：脚本可能部分完成（如已写入二进制但
        // PATH 更新失败）；Codex 还需立即检查 ~/.local/bin 的官方安装绝对路径。
        self.refresh_agent_cli_probe(backend).await;
        match result {
            Ok(()) => {
                diagnostics::write(&operation_id, "script:complete", "result=success");
                self.finalize_agent_install(backend, previous_codex_version)
                    .await
            }
            Err(error) => {
                let detail = format!("{error:#}");
                diagnostics::write(&operation_id, "script:failed", &detail);
                self.runtime_errors.set(backend, detail);
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
            self.ensure_codex_ready().await?;
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
        self.runtime_errors.clear(backend);
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
                        pool.runtime_errors.clear(backend);
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
                resolve_claude_cli(Some(&adapter))
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
        let stdout_output = stdout_reader.await.unwrap_or_default();
        let stderr_output = stderr_reader.await.unwrap_or_default();
        let failure_detail = (backend == AgentBackend::KimiAcp && !status.success())
            .then(|| kimi_login_failure_detail(&stdout_output, &stderr_output))
            .flatten();

        diagnostics::write(
            &operation_id,
            "login:process_exit",
            match failure_detail.as_deref() {
                Some(detail) => format!("status={status} detail={detail}"),
                None => format!("status={status}"),
            },
        );

        if !status.success() {
            if let Some(detail) = failure_detail {
                bail!("{} 登录进程失败：{detail}", backend.display_name());
            }
            bail!("{} 登录进程退出: {status}", backend.display_name());
        }
        if !self.agent_authenticated_async(backend, &executable).await {
            if backend == AgentBackend::KimiAcp {
                bail!("Kimi 登录授权已完成，但未获取到可用模型，请重新登录并检查账号权益或网络");
            }
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
                "session_id={session_id} current_source={} current_version={}",
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
        self.evict(session_id).await;
        self.codex_upgrade_required.store(true, Ordering::Release);
        self.runtime_errors.set(
            AgentBackend::CodexAcp,
            format!(
                "当前 Codex {} 已无法支持所选模型，请通过官方安装方式升级到最新版后重试。",
                current
                    .as_ref()
                    .map(|resolved| resolved.version.as_str())
                    .unwrap_or("未知版本")
            ),
        );
        diagnostics::write(
            &operation_id,
            "upgrade_required:user_confirmation_needed",
            "official_upgrade_required",
        );
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
            AgentBackend::CodexAcp => self.ensure_codex_ready().await.map(|_| ()),
            AgentBackend::ClaudeAcp | AgentBackend::KimiAcp => {
                let status = self.status_for_async(backend).await;
                // update_required 仅 Codex 动态门禁会置真；此处的 !installed 一律表示
                // 低于最低兼容版本，按 setup_hint 引导即可。
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
                let mut command = self.adapter_command(&adapter)?;
                self.configure_claude_executable(&mut command, Some(&adapter))?;
                (command, adapter, CLAUDE_ACP_PACKAGE, CLAUDE_ACP_VERSION)
            }
            AgentBackend::KimiAcp => {
                let executable = resolve_kimi_path()
                    .context("未检测到 Kimi Code CLI；请先安装 Kimi，并确保 kimi 在 PATH 中")?;
                let mut command = crate::platform::process::external_tokio_command(&executable);
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

    /// claude / kimi CLI 探测缓存（仿 RuntimeProbeCache）：status_for 已在
    /// spawn_blocking 中运行，首次访问同步探测一次，之后轮询只读缓存。
    fn cli_probe(&self) -> CliProbeCache {
        {
            let probe = self.cli_probe.read();
            if probe.initialized {
                return probe.clone();
            }
        }
        let probe = CliProbeCache {
            initialized: true,
            claude: probe_cli(
                AgentBackend::ClaudeAcp,
                resolve_claude_cli(self.resolve_claude_adapter().as_deref()),
            ),
            kimi: probe_cli(AgentBackend::KimiAcp, resolve_kimi_path()),
        };
        *self.cli_probe.write() = probe.clone();
        probe
    }

    fn invalidate_cli_probe(&self) {
        self.cli_probe.write().initialized = false;
    }

    fn adapter_command(&self, adapter: &Path) -> Result<Command> {
        platform::adapter_command(adapter, self.resolve_node(adapter).as_deref())
    }

    fn configure_codex_path(&self, command: &mut Command, adapter: &Path) -> Result<()> {
        let codex = self
            .resolve_codex(adapter)
            .context("未检测到可用 Codex；请通过官方方式安装或升级 Codex")?;
        command.env(
            "CODEX_PATH",
            crate::platform::os::external_application_path(&codex.path),
        );
        Ok(())
    }

    fn configure_claude_executable(
        &self,
        command: &mut Command,
        adapter: Option<&Path>,
    ) -> Result<()> {
        let claude = resolve_claude_cli(adapter)
            .context("未检测到 Claude Code CLI；请先安装 Claude Code，并确保 claude 在 PATH 中")?;
        command.env(
            "CLAUDE_CODE_EXECUTABLE",
            crate::platform::os::external_application_path(&claude),
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
    let mut command = crate::platform::process::external_tokio_command(executable);
    command.args(args);
    command
}

async fn capture_agent_login_output<R>(
    mut reader: R,
    backend: AgentBackend,
    states: Arc<parking_lot::RwLock<HashMap<AgentBackend, AgentLoginState>>>,
) -> String
where
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
    output
}

/// Kimi 的登录流程会先落 OAuth 凭证，再请求模型列表并写入 config.toml。
/// 后半段失败时 CLI 会以 `Login failed:` 输出可操作原因；只提取该明确前缀，
/// 同时隐藏 URL 并拒绝可能包含凭证的内容，避免把设备码或令牌写入诊断日志/UI。
fn kimi_login_failure_detail(stdout: &str, stderr: &str) -> Option<String> {
    [stderr, stdout].into_iter().find_map(|output| {
        output.lines().rev().find_map(|line| {
            let (_, detail) = line.rsplit_once("Login failed:")?;
            sanitize_kimi_login_failure_detail(detail)
        })
    })
}

fn sanitize_kimi_login_failure_detail(detail: &str) -> Option<String> {
    let normalized = detail.to_ascii_lowercase();
    if [
        "access_token",
        "refresh_token",
        "authorization:",
        "bearer ",
        "api_key",
        "api-key",
        "secret=",
        "cookie:",
        "user_code=",
        "device_code=",
    ]
    .into_iter()
    .any(|marker| normalized.contains(marker))
    {
        return None;
    }

    let mut sanitized = String::new();
    for word in detail.split_whitespace() {
        if !sanitized.is_empty() {
            sanitized.push(' ');
        }
        if word.contains("https://") || word.contains("http://") || looks_like_device_code(word) {
            sanitized.push_str("[敏感信息已隐藏]");
        } else {
            sanitized.extend(word.chars().filter(|character| !character.is_control()));
        }
        if sanitized.chars().count() >= 500 {
            break;
        }
    }
    let sanitized = sanitized.chars().take(500).collect::<String>();
    (!sanitized.is_empty()).then_some(sanitized)
}

fn looks_like_device_code(word: &str) -> bool {
    let candidate =
        word.trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '-');
    let Some((left, right)) = candidate.split_once('-') else {
        return false;
    };
    [left, right].into_iter().all(|part| {
        part.len() == 4
            && part
                .chars()
                .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
    })
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

fn resolve_codex_cli() -> Option<PathBuf> {
    // OpenAI Unix 安装器默认写入 ~/.local/bin；Windows 安装器写入
    // %LOCALAPPDATA%\Programs\OpenAI\Codex\bin。优先检查平台绝对路径，使脚本
    // 安装完成后无需重启桌面应用或依赖其启动时继承的 PATH。
    let script_installed = platform::codex_official_install_path();
    if nonempty_file(&script_installed) {
        return Some(script_installed);
    }
    find_agent_cli_in_path(AgentBackend::CodexAcp)
}

fn resolve_claude_cli(adapter: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PINVOU3_CLAUDE_CLI_PATH").map(PathBuf::from) {
        if nonempty_file(&path) {
            return Some(path);
        }
    }
    let binary = if crate::platform::capabilities::is_windows() {
        "claude.exe"
    } else {
        "claude"
    };
    if let Some((package, binary)) = claude_native_runtime(
        std::env::consts::OS,
        std::env::consts::ARCH,
        crate::platform::capabilities::is_musl(),
    ) {
        if let Some(path) = adapter.and_then(|adapter| {
            adapter.ancestors().find_map(|ancestor| {
                (ancestor.file_name().and_then(|value| value.to_str()) == Some("node_modules"))
                    .then(|| ancestor.join("@anthropic-ai").join(&package).join(&binary))
                    .filter(|candidate| nonempty_file(candidate))
            })
        }) {
            return Some(path);
        }
    }
    // 官方安装脚本默认目录（unix ~/.local/bin，Windows %USERPROFILE%\.local\bin），
    // 使脚本装完后无需重启 App 即可探测到。
    let script_installed = crate::platform::os::user_home_dir()
        .join(".local")
        .join("bin")
        .join(binary);
    if nonempty_file(&script_installed) {
        return Some(script_installed);
    }
    find_agent_cli_in_path(AgentBackend::ClaudeAcp)
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
    let binary = if crate::platform::capabilities::is_windows() {
        "kimi.exe"
    } else {
        "kimi"
    };
    // 官方安装脚本默认目录（unix ~/.kimi-code/bin，Windows %USERPROFILE%\.kimi-code\bin）。
    // 优先于 PATH：脚本装完无需重启即可探测，且避开 PATH 中可能残留的废弃
    // Python 版 kimi-cli。
    let script_installed = crate::platform::os::user_home_dir()
        .join(".kimi-code")
        .join("bin")
        .join(binary);
    if nonempty_file(&script_installed) {
        return Some(script_installed);
    }
    find_agent_cli_in_path(AgentBackend::KimiAcp)
}

fn agent_cli_names(backend: AgentBackend, windows: bool) -> &'static [&'static str] {
    match (backend, windows) {
        (AgentBackend::CodexAcp, true) => &["codex.cmd", "codex.exe"],
        (AgentBackend::ClaudeAcp, true) => &["claude.exe", "claude.cmd"],
        (AgentBackend::KimiAcp, true) => &["kimi.exe", "kimi.cmd"],
        (AgentBackend::CodexAcp, false) => &["codex"],
        (AgentBackend::ClaudeAcp, false) => &["claude"],
        (AgentBackend::KimiAcp, false) => &["kimi"],
        (AgentBackend::Deepseek, _) => &[],
    }
}

fn find_agent_cli_in_path(backend: AgentBackend) -> Option<PathBuf> {
    agent_cli_names(backend, crate::platform::capabilities::is_windows())
        .iter()
        .find_map(|name| find_in_path(name))
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| nonempty_file(candidate))
}

/// `--version` 探测与 cli_status_success 一致限制 3 秒，避免卡住的 CLI 拖住状态轮询。
fn command_version_output(executable: &Path) -> Option<String> {
    let mut command = crate::platform::process::external_command(executable);
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = command.spawn().ok()?;
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

fn probe_cli(backend: AgentBackend, path: Option<PathBuf>) -> Option<ResolvedCli> {
    let path = path?;
    let version = command_version(&path);
    let install_source = detect_install_source(backend, &path);
    Some(ResolvedCli {
        path,
        version,
        install_source,
    })
}

/// 探测 brew 是否已安装该 Agent 的 CLI（macOS 版本过旧时走 brew upgrade）。
/// codex/claude-code 是 cask，kimi-code 是 formula（无 --cask）；
/// 非 macOS 平台 brew_available 恒 false。
fn brew_package_installed(backend: AgentBackend) -> bool {
    if !platform::brew_available() {
        return false;
    }
    let args: &[&str] = match backend {
        AgentBackend::CodexAcp => &["list", "--cask", "codex"],
        AgentBackend::ClaudeAcp => &["list", "--cask", "claude-code"],
        AgentBackend::KimiAcp => &["list", "kimi-code"],
        AgentBackend::Deepseek => return false,
    };
    std::process::Command::new(platform::brew_bin())
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// 各 Agent CLI 对应的 npm 全局包名。
fn npm_package(backend: AgentBackend) -> Option<&'static str> {
    match backend {
        AgentBackend::CodexAcp => Some("@openai/codex"),
        AgentBackend::ClaudeAcp => Some("@anthropic-ai/claude-code"),
        AgentBackend::KimiAcp => Some("@moonshot-ai/kimi-code"),
        AgentBackend::Deepseek => None,
    }
}

/// npm 可执行文件：Windows 上是 npm.cmd。
fn npm_executable() -> Option<PathBuf> {
    if crate::platform::capabilities::is_windows() {
        find_in_path("npm.cmd").or_else(|| find_in_path("npm"))
    } else {
        find_in_path("npm")
    }
}

/// `npm ls -g <pkg> --depth=0` 退出码 0 即视为 npm 全局安装；10 秒超时防挂住。
fn npm_global_installed(package: &str) -> bool {
    let Some(npm) = npm_executable() else {
        return false;
    };
    let mut command = crate::platform::process::external_command(&npm);
    command
        .args(["ls", "-g", package, "--depth=0"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    match child.wait_timeout(Duration::from_secs(10)) {
        Ok(Some(status)) => status.success(),
        Ok(None) | Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            false
        }
    }
}

/// 判定已解析 CLI 的安装来源。多份并存（如同机同时有 brew cask 与官方脚本版）
/// 时必须按「实际被解析使用的那一份」判定，否则升级会打到包管理器管理的另一份，
/// 正在使用的旧版原地不动。顺序：官方脚本目录前缀 → brew 前缀+brew 已装 →
/// npm 全局根+npm 已装 → 路径无法判定时回退 brew/npm 全局查询（维持原行为）。
fn detect_install_source(backend: AgentBackend, path: &Path) -> Option<&'static str> {
    if let Some(source) = path_install_source(backend, path) {
        return Some(source);
    }
    let brew_path_match = platform::brew_prefix().is_some_and(|prefix| path.starts_with(prefix));
    let brew_installed = brew_package_installed(backend);
    let npm_path_match = npm_global_root().is_some_and(|root| {
        path_in_npm_global(path, &root, crate::platform::capabilities::is_windows())
    });
    let npm_installed = npm_package(backend).is_some_and(npm_global_installed);
    finalize_install_source(
        brew_path_match,
        brew_installed,
        npm_path_match,
        npm_installed,
    )
}

/// 路径与包管理器双重判定的优先级：路径命中且包管理器确认已装才认来源；
/// 路径无法判定时回退包管理器全局查询。纯函数，便于单测覆盖多份并存场景。
fn finalize_install_source(
    brew_path_match: bool,
    brew_installed: bool,
    npm_path_match: bool,
    npm_installed: bool,
) -> Option<&'static str> {
    if brew_path_match && brew_installed {
        return Some("brew");
    }
    if npm_path_match && npm_installed {
        return Some("npm");
    }
    if brew_installed {
        return Some("brew");
    }
    if npm_installed {
        return Some("npm");
    }
    None
}

/// `npm prefix -g` 输出的全局根目录；npm 不可用或超时返回 None。
fn npm_global_root() -> Option<PathBuf> {
    let npm = npm_executable()?;
    let mut command = crate::platform::process::external_command(&npm);
    command
        .args(["prefix", "-g"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = command.spawn().ok()?;
    match child.wait_timeout(Duration::from_secs(10)) {
        Ok(Some(status)) if status.success() => {
            let mut stdout = String::new();
            child.stdout.take()?.read_to_string(&mut stdout).ok()?;
            let root = stdout.trim();
            (!root.is_empty()).then_some(PathBuf::from(root))
        }
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

/// 路径是否位于 npm 全局根下：unix 可执行文件链接在 <root>/bin，Windows 直接在根目录。
fn path_in_npm_global(path: &Path, root: &Path, windows: bool) -> bool {
    let bin = if windows {
        root.to_path_buf()
    } else {
        root.join("bin")
    };
    path.starts_with(bin)
}

/// 按路径前缀判定官方脚本来源（codex/claude ~/.local/bin、kimi ~/.kimi-code/bin）。
fn path_install_source(backend: AgentBackend, path: &Path) -> Option<&'static str> {
    let home = crate::platform::os::user_home_dir();
    match backend {
        AgentBackend::CodexAcp
            if platform::codex_official_install_path()
                .parent()
                .is_some_and(|directory| path.starts_with(directory)) =>
        {
            Some("script")
        }
        AgentBackend::ClaudeAcp if path.starts_with(home.join(".local").join("bin")) => {
            Some("script")
        }
        AgentBackend::KimiAcp if path.starts_with(home.join(".kimi-code").join("bin")) => {
            Some("script")
        }
        _ => None,
    }
}

/// installed=false 时的安装动作：探测到过旧 CLI 且来源可识别时优先包管理器
/// 升级（brew/npm），其余来源或无 CLI 时维持各 Agent 的默认安装方式。
fn install_action_for(
    _backend: AgentBackend,
    install_source: Option<&'static str>,
    npm_available: bool,
    official_script_supported: bool,
) -> &'static str {
    match install_source {
        Some("brew") => "brew_upgrade",
        Some("npm") if npm_available => "npm_upgrade",
        _ if official_script_supported => "official_script",
        _ => "manual",
    }
}

/// install_acp_agent 可选 action override 校验。
fn parse_install_action(action: &str) -> Result<&'static str> {
    match action {
        "none" => Ok("none"),
        "brew_upgrade" => Ok("brew_upgrade"),
        "npm_upgrade" => Ok("npm_upgrade"),
        "official_script" => Ok("official_script"),
        "manual" => Ok("manual"),
        other => bail!(
            "非法安装动作: {other}（可选值: none / brew_upgrade / npm_upgrade / official_script / manual）"
        ),
    }
}

/// brew 安装/升级命令参数：codex 已通过 brew 安装走 upgrade，未安装走 install；
/// claude/kimi 只在探测到 brew 来源时调用，一律 upgrade（kimi-code 是 formula）。
/// 返回（命令描述，参数），供执行与诊断日志使用。
fn brew_install_args(
    backend: AgentBackend,
    brew_installed: bool,
) -> Option<(&'static str, Vec<&'static str>)> {
    match backend {
        AgentBackend::CodexAcp if brew_installed => Some((
            "brew upgrade --cask codex",
            vec!["upgrade", "--cask", "codex"],
        )),
        AgentBackend::CodexAcp => Some((
            "brew install --cask codex",
            vec!["install", "--cask", "codex"],
        )),
        AgentBackend::ClaudeAcp => Some((
            "brew upgrade --cask claude-code",
            vec!["upgrade", "--cask", "claude-code"],
        )),
        AgentBackend::KimiAcp => Some(("brew upgrade kimi-code", vec!["upgrade", "kimi-code"])),
        AgentBackend::Deepseek => None,
    }
}

/// npm 全局升级参数：`npm install -g <pkg>@latest`。
fn npm_upgrade_args(backend: AgentBackend) -> Option<Vec<String>> {
    Some(vec![
        "install".to_string(),
        "-g".to_string(),
        format!("{}@latest", npm_package(backend)?),
    ])
}

fn codex_version_changed(previous: Option<&str>, current: Option<&str>) -> bool {
    current.is_some() && current != previous
}

/// 裸 semver（`0.31.1`）：claude/kimi 的版本门禁都要求完整三段数字，
/// 旧 Python 版 kimi-cli 等非标准输出一律判不合规。
fn is_bare_semver(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        })
}

/// claude `--version` 输出形如 `2.1.163 (Claude Code)`，取首个空白分隔 token。
fn claude_version_supported(version: &str) -> bool {
    version
        .split_whitespace()
        .next()
        .is_some_and(|token| is_bare_semver(token) && version_at_least(token, MIN_CLAUDE_VERSION))
}

/// kimi `--version` 输出为裸 semver（如 `0.31.1`），解析失败一律不合规。
fn kimi_version_supported(version: &str) -> bool {
    is_bare_semver(version) && version_at_least(version, MIN_KIMI_VERSION)
}

/// 官方安装脚本覆盖的平台。Claude 脚本与其原生运行时平台集一致；
/// Kimi 脚本额外排除 Linux musl（官方只发布 glibc 构建）。
fn official_script_supported(backend: AgentBackend) -> bool {
    let (os, arch, musl) = (
        std::env::consts::OS,
        std::env::consts::ARCH,
        crate::platform::capabilities::is_musl(),
    );
    match backend {
        AgentBackend::CodexAcp => {
            matches!(os, "macos" | "linux" | "windows") && matches!(arch, "x86_64" | "aarch64")
        }
        AgentBackend::ClaudeAcp => claude_native_runtime(os, arch, musl).is_some(),
        AgentBackend::KimiAcp => {
            matches!(arch, "x86_64" | "aarch64")
                && (matches!(os, "macos" | "windows") || (os == "linux" && !musl))
        }
        _ => false,
    }
}

/// 执行官方安装脚本（unix: `curl -fsSL <url> | bash`，Windows: `irm <url> | iex`），
/// 10 分钟超时，输出尾部写入诊断日志。
async fn run_official_install_script(backend: AgentBackend, operation_id: &str) -> Result<()> {
    let (unix_url, windows_url) = match backend {
        AgentBackend::CodexAcp => (CODEX_INSTALL_SCRIPT_UNIX, CODEX_INSTALL_SCRIPT_WINDOWS),
        AgentBackend::ClaudeAcp => (CLAUDE_INSTALL_SCRIPT_UNIX, CLAUDE_INSTALL_SCRIPT_WINDOWS),
        AgentBackend::KimiAcp => (KIMI_INSTALL_SCRIPT_UNIX, KIMI_INSTALL_SCRIPT_WINDOWS),
        _ => bail!("{} 不支持官方脚本安装", backend.display_name()),
    };
    let mut command = crate::platform::process::install_script_command(unix_url, windows_url);
    if backend == AgentBackend::CodexAcp {
        // OpenAI 官方脚本默认安装 latest；非交互模式避免桌面应用后台等待 PATH 冲突确认。
        command.env("CODEX_NON_INTERACTIVE", "1");
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| format!("启动 {} 安装脚本失败", backend.display_name()))?;
    let stdout = child.stdout.take().context("读取安装脚本标准输出失败")?;
    let stderr = child.stderr.take().context("读取安装脚本错误输出失败")?;
    let stdout_reader = tokio::spawn(read_pipe_to_string(stdout));
    let stderr_reader = tokio::spawn(read_pipe_to_string(stderr));
    const TIMEOUT: Duration = Duration::from_secs(600);
    let status = match tokio::time::timeout(TIMEOUT, child.wait()).await {
        Ok(result) => result.context("等待安装脚本进程失败")?,
        Err(_) => {
            diagnostics::write(
                operation_id,
                "script:timeout",
                format!("timeout_seconds={}", TIMEOUT.as_secs()),
            );
            let _ = child.kill().await;
            let _ = child.wait().await;
            bail!(
                "{} 安装脚本超过 10 分钟仍未完成，请检查网络后重试",
                backend.display_name()
            );
        }
    };
    let stdout = stdout_reader.await.unwrap_or_default();
    let stderr = stderr_reader.await.unwrap_or_default();
    diagnostics::write(
        operation_id,
        "script:output",
        format!(
            "status={status} stdout_tail={} stderr_tail={}",
            output_tail(&stdout, 20),
            output_tail(&stderr, 20)
        ),
    );
    if !status.success() {
        bail!(
            "{} 安装脚本退出: {status}；stderr: {}",
            backend.display_name(),
            output_tail(&stderr, 4)
        );
    }
    Ok(())
}

/// 执行 `npm install -g <pkg>@latest` 全局升级（Windows 上 npm.cmd 经 cmd 启动），
/// 10 分钟超时，输出尾部写入诊断日志。
async fn run_npm_global_upgrade(backend: AgentBackend, operation_id: &str) -> Result<()> {
    let args = npm_upgrade_args(backend)
        .with_context(|| format!("{} 不支持 npm 全局升级", backend.display_name()))?;
    let npm = npm_executable().context("未检测到 npm，无法通过 npm 全局升级")?;
    let mut command = crate::platform::process::external_tokio_command(&npm);
    command.args(&args);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("启动 npm 全局升级失败")?;
    let stdout = child.stdout.take().context("读取 npm 标准输出失败")?;
    let stderr = child.stderr.take().context("读取 npm 错误输出失败")?;
    let stdout_reader = tokio::spawn(read_pipe_to_string(stdout));
    let stderr_reader = tokio::spawn(read_pipe_to_string(stderr));
    const TIMEOUT: Duration = Duration::from_secs(600);
    let status = match tokio::time::timeout(TIMEOUT, child.wait()).await {
        Ok(result) => result.context("等待 npm 全局升级进程失败")?,
        Err(_) => {
            diagnostics::write(
                operation_id,
                "npm:timeout",
                format!("timeout_seconds={}", TIMEOUT.as_secs()),
            );
            let _ = child.kill().await;
            let _ = child.wait().await;
            bail!(
                "{} npm 全局升级超过 10 分钟仍未完成，请检查网络后重试",
                backend.display_name()
            );
        }
    };
    let stdout = stdout_reader.await.unwrap_or_default();
    let stderr = stderr_reader.await.unwrap_or_default();
    diagnostics::write(
        operation_id,
        "npm:output",
        format!(
            "status={status} stdout_tail={} stderr_tail={}",
            output_tail(&stdout, 20),
            output_tail(&stderr, 20)
        ),
    );
    if !status.success() {
        bail!(
            "npm 全局升级 {} 退出: {status}；stderr: {}",
            backend.display_name(),
            output_tail(&stderr, 4)
        );
    }
    Ok(())
}

async fn read_pipe_to_string<R>(mut reader: R) -> String
where
    R: AsyncRead + Unpin,
{
    let mut output = String::new();
    let _ = reader.read_to_string(&mut output).await;
    output
}

fn output_tail(output: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = output.lines().collect();
    lines[lines.len().saturating_sub(max_lines)..].join(" / ")
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
    // Kimi Code 0.31+ 不读取裸 KIMI_API_KEY；只有成对的 KIMI_MODEL_* 覆盖
    // 会在内存中合成 provider/model。
    if nonempty_env("KIMI_MODEL_NAME") && nonempty_env("KIMI_MODEL_API_KEY") {
        return true;
    }
    let root = kimi_data_root();
    let oauth_credentials_valid =
        std::fs::read_to_string(root.join("credentials").join("kimi-code.json"))
            .is_ok_and(|raw| kimi_credentials_valid(&raw));
    let Ok(config) = std::fs::read_to_string(root.join("config.toml")) else {
        return false;
    };
    kimi_runtime_config_ready(&config, oauth_credentials_valid)
}

/// 仅有 OAuth 凭证并不代表 Kimi 已可用：官方登录还必须把 `/models` 返回结果
/// 写为默认模型及其 provider。要求默认模型能解析到现有 provider，避免登录后半段
/// 失败时把“凭证已写入”误报成“已登录”。
fn kimi_runtime_config_ready(raw: &str, oauth_credentials_valid: bool) -> bool {
    let Ok(config) = raw.parse::<toml::Value>() else {
        return false;
    };
    let Some(default_model) = config
        .get("default_model")
        .and_then(toml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return false;
    };
    let Some(model) = config
        .get("models")
        .and_then(|models| models.get(default_model))
        .and_then(toml::Value::as_table)
    else {
        return false;
    };
    let Some(provider) = model
        .get("provider")
        .and_then(toml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return false;
    };
    let model_ready = model
        .get("model")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        && model
            .get("max_context_size")
            .and_then(toml::Value::as_integer)
            .is_some_and(|value| value > 0);
    if !model_ready {
        return false;
    }
    let Some(provider) = config
        .get("providers")
        .and_then(|providers| providers.get(provider))
        .and_then(toml::Value::as_table)
    else {
        return false;
    };
    if !provider
        .get("type")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return false;
    }
    let direct_api_key = provider
        .get("api_key")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let configured_env_api_key = provider
        .get("env")
        .and_then(toml::Value::as_table)
        .is_some_and(|env| {
            env.iter().any(|(name, value)| {
                name.ends_with("_API_KEY")
                    && value.as_str().is_some_and(|value| !value.trim().is_empty())
            })
        });
    let oauth_ready =
        provider.get("oauth").is_some_and(toml::Value::is_table) && oauth_credentials_valid;
    direct_api_key || configured_env_api_key || oauth_ready
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
    if code.eq_ignore_ascii_case("model.not_configured")
        || normalized.contains("llm not set")
        || normalized.contains("send \"/login\"")
    {
        return "Kimi Code 尚未完成模型配置（model.not_configured），请重新登录".to_string();
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
    let mut command = crate::platform::process::external_command(executable);
    command.args(args);
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
    fn code_native_workspace_info_returns_temporary_execution_workspace() {
        let session_store = crate::features::sessions::SessionStore::boot().expect("session store");
        let record = SessionAgentRecord {
            code_session: true,
            ..SessionAgentRecord::default()
        };
        let info =
            code_native_workspace_info(&session_store, "code-native-unit-test", &record).unwrap();
        assert_eq!(info.workspace_kind, CodexWorkspaceKind::Temporary);
        assert!(info.workspace_available);
        assert_eq!(
            info.workspace_path,
            session_store
                .session_roots("code-native-unit-test")
                .unwrap()
                .execution
                .to_string_lossy()
        );
    }

    #[test]
    fn code_native_workspace_info_project_branch_tracks_directory_availability() {
        let session_store = crate::features::sessions::SessionStore::boot().expect("session store");
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-project-info-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let record = SessionAgentRecord {
            code_session: true,
            workspace_kind: CodexWorkspaceKind::Project,
            workspace_path: Some(root.clone()),
            ..SessionAgentRecord::default()
        };
        let info =
            code_native_workspace_info(&session_store, "code-native-proj-test", &record).unwrap();
        assert_eq!(info.workspace_kind, CodexWorkspaceKind::Project);
        assert!(info.workspace_available);
        assert_eq!(info.workspace_path, root.to_string_lossy());

        // 项目目录丢失时按 ACP 项目分支同一语义报告不可用（不静默退回临时目录）。
        std::fs::remove_dir_all(&root).unwrap();
        let info =
            code_native_workspace_info(&session_store, "code-native-proj-test", &record).unwrap();
        assert!(!info.workspace_available);

        // 记录缺失目录属于数据损坏，必须显式报错。
        let broken = SessionAgentRecord {
            code_session: true,
            workspace_kind: CodexWorkspaceKind::Project,
            workspace_path: None,
            ..SessionAgentRecord::default()
        };
        assert!(
            code_native_workspace_info(&session_store, "code-native-proj-test", &broken).is_err()
        );
    }

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
        // 用平台相关的绝对路径（/tmp 在 Windows 上不是绝对路径）。
        let temporary = std::env::temp_dir().join("pinvou-session-workspace");
        let project = std::env::temp_dir().join("pinvou-project");
        let recovered = acp_recovery_record(
            "pinvou-session",
            AgentBackend::KimiAcp,
            &state,
            project.clone(),
            &temporary,
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
            temporary.clone(),
            &temporary,
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
    fn kimi_runtime_config_requires_resolvable_default_model() {
        let ready = r#"
default_model = "kimi-code/kimi-k2"

[providers."managed:kimi-code"]
type = "kimi"

[providers."managed:kimi-code".oauth]
storage = "file"
key = "oauth/kimi-code"

[models."kimi-code/kimi-k2"]
provider = "managed:kimi-code"
model = "kimi-k2"
max_context_size = 262144
"#;
        assert!(kimi_runtime_config_ready(ready, true));
        assert!(!kimi_runtime_config_ready(ready, false));
        let api_key_ready = r#"
default_model = "custom/kimi-k2"

[providers.custom]
type = "kimi"
api_key = "configured-in-file"

[models."custom/kimi-k2"]
provider = "custom"
model = "kimi-k2"
max_context_size = 262144
"#;
        assert!(kimi_runtime_config_ready(api_key_ready, false));
        assert!(!kimi_runtime_config_ready(
            "# Login will populate managed Kimi provider and model entries.",
            true
        ));
        assert!(!kimi_runtime_config_ready(
            "default_model = \"kimi-code/missing\"\n[providers.\"managed:kimi-code\"]\ntype = \"kimi\"",
            true
        ));
        assert!(!kimi_runtime_config_ready("not = [valid", true));
    }

    #[test]
    fn captures_safe_kimi_login_failure_without_urls_or_credentials() {
        assert_eq!(
            kimi_login_failure_detail(
                "",
                "Login failed: Kimi Code models endpoint https://api.kimi.com/coding/v1 rejected OAuth credentials: HTTP 402 membership required\n",
            )
            .as_deref(),
            Some("Kimi Code models endpoint [敏感信息已隐藏] rejected OAuth credentials: HTTP 402 membership required")
        );
        assert_eq!(
            kimi_login_failure_detail("", "Login failed: access_token=private-value\n"),
            None
        );
        assert_eq!(
            kimi_login_failure_detail("device code: ABCD-EFGH", ""),
            None
        );
        assert_eq!(
            sanitize_kimi_login_failure_detail("device code ABCD-EFGH expired").as_deref(),
            Some("device code [敏感信息已隐藏] expired")
        );
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
            format_kimi_provider_error(
                "model.not_configured",
                "LLM not set, send \"/login\" to login"
            ),
            "Kimi Code 尚未完成模型配置（model.not_configured），请重新登录"
        );
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
            version: Some("0.146.0".to_string()),
            latest_version: Some("0.147.0".to_string()),
            installed: false,
            update_available: true,
            update_required: true,
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
            min_codex_version: MIN_CODEX_VERSION,
            min_version: MIN_CODEX_VERSION,
            install_action: "official_script",
            install_source: Some("npm".to_string()),
            brew_available: false,
            system_codex_incompatible: true,
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
        assert_eq!(value["version"], json!("0.146.0"));
        assert_eq!(value["latest_version"], json!("0.147.0"));
        assert_eq!(value["min_codex_version"], json!(MIN_CODEX_VERSION));
        assert_eq!(value["min_version"], json!(MIN_CODEX_VERSION));
        assert_eq!(value["install_action"], json!("official_script"));
        assert_eq!(value["install_source"], json!("npm"));
        assert_eq!(value["update_available"], json!(true));
        assert_eq!(value["update_required"], json!(true));
        assert_eq!(value["brew_available"], json!(false));
        assert_eq!(value["system_codex_incompatible"], json!(true));

        let mut cli_ready_without_bridge = status.clone();
        cli_ready_without_bridge.codex_available = true;
        cli_ready_without_bridge.update_available = false;
        cli_ready_without_bridge.update_required = false;
        assert!(
            latest::ensure_agent_cli_ready(AgentBackend::CodexAcp, &cli_ready_without_bridge)
                .is_ok(),
            "Codex CLI installation must not fail solely because its bundled bridge is unavailable"
        );
        cli_ready_without_bridge.update_required = true;
        assert!(
            latest::ensure_agent_cli_ready(AgentBackend::CodexAcp, &cli_ready_without_bridge)
                .is_err()
        );
        cli_ready_without_bridge.update_required = false;
        cli_ready_without_bridge.update_available = true;
        assert!(
            latest::ensure_agent_cli_ready(AgentBackend::CodexAcp, &cli_ready_without_bridge)
                .is_ok(),
            "advisory latest upgrade must not fail the install when the package manager lags behind"
        );
    }

    #[test]
    fn install_source_follows_path_of_cli_in_use() {
        // 多份并存时以「正在使用的这份」的路径为准，避免升级打到另一份：
        // 路径命中 brew 前缀且 brew 确认已装 → brew；npm 全局根同理。
        assert_eq!(
            finalize_install_source(true, true, false, false),
            Some("brew")
        );
        assert_eq!(
            finalize_install_source(false, false, true, true),
            Some("npm")
        );
        // 路径命中但包管理器查无此包（如手动拷贝进前缀目录）→ 不冒认，继续回退。
        assert_eq!(
            finalize_install_source(true, false, false, true),
            Some("npm")
        );
        assert_eq!(finalize_install_source(true, false, false, false), None);
        // 路径无法判定时回退包管理器全局查询（维持原行为）。
        assert_eq!(
            finalize_install_source(false, true, false, false),
            Some("brew")
        );
        assert_eq!(
            finalize_install_source(false, false, false, true),
            Some("npm")
        );
        assert_eq!(finalize_install_source(false, false, false, false), None);
    }

    #[test]
    fn npm_global_path_matches_platform_layout() {
        let root = Path::new("/home/user/.nvm/versions/node/v22.0.0");
        assert!(path_in_npm_global(
            &root.join("bin").join("claude"),
            root,
            false
        ));
        assert!(!path_in_npm_global(
            &root
                .join("lib")
                .join("node_modules")
                .join(".bin")
                .join("claude"),
            root,
            false
        ));
        assert!(!path_in_npm_global(
            Path::new("/usr/local/bin/claude"),
            root,
            false
        ));
        let win_root = Path::new("C:\\Users\\u\\AppData\\Roaming\\npm");
        assert!(path_in_npm_global(
            &win_root.join("claude.cmd"),
            win_root,
            true
        ));
        assert!(!path_in_npm_global(
            Path::new("C:\\tools\\claude.cmd"),
            win_root,
            true
        ));
    }

    #[test]
    fn install_action_follows_detected_install_source() {
        // brew 来源一律 brew_upgrade（三 Agent 相同）。
        for backend in [
            AgentBackend::CodexAcp,
            AgentBackend::ClaudeAcp,
            AgentBackend::KimiAcp,
        ] {
            assert_eq!(
                install_action_for(backend, Some("brew"), false, true),
                "brew_upgrade"
            );
            // npm 来源且 npm 可执行时 npm_upgrade。
            assert_eq!(
                install_action_for(backend, Some("npm"), true, true),
                "npm_upgrade"
            );
        }
        // 三 Agent 的 script、未知来源和 npm 不可用场景都回到官方脚本。
        for backend in [
            AgentBackend::CodexAcp,
            AgentBackend::ClaudeAcp,
            AgentBackend::KimiAcp,
        ] {
            assert_eq!(
                install_action_for(backend, Some("script"), false, true),
                "official_script"
            );
            assert_eq!(
                install_action_for(backend, None, false, true),
                "official_script"
            );
            assert_eq!(
                install_action_for(backend, Some("npm"), false, true),
                "official_script"
            );
            assert_eq!(install_action_for(backend, None, false, false), "manual");
        }
    }

    #[test]
    fn install_action_override_accepts_only_known_actions() {
        for action in [
            "none",
            "brew_upgrade",
            "npm_upgrade",
            "official_script",
            "manual",
        ] {
            assert_eq!(parse_install_action(action).unwrap(), action);
        }
        assert!(parse_install_action("brew").is_err());
        assert!(parse_install_action("").is_err());
        assert!(parse_install_action("managed").is_err());
        assert!(parse_install_action(concat!("managed_", "download")).is_err());
    }

    #[test]
    fn installing_state_is_isolated_per_agent() {
        let installing_agents = Arc::new(parking_lot::RwLock::new(HashSet::new()));
        let claude = AgentInstallGuard::try_start(&installing_agents, AgentBackend::ClaudeAcp)
            .expect("Claude install should start");

        assert!(installing_agents.read().contains(&AgentBackend::ClaudeAcp));
        assert!(!installing_agents.read().contains(&AgentBackend::CodexAcp));
        assert!(!installing_agents.read().contains(&AgentBackend::KimiAcp));
        assert!(
            AgentInstallGuard::try_start(&installing_agents, AgentBackend::ClaudeAcp).is_none(),
            "the same Agent must remain mutually exclusive"
        );

        let codex = AgentInstallGuard::try_start(&installing_agents, AgentBackend::CodexAcp)
            .expect("a different Agent should not share Claude's install lock");
        assert!(installing_agents.read().contains(&AgentBackend::CodexAcp));

        drop(claude);
        assert!(!installing_agents.read().contains(&AgentBackend::ClaudeAcp));
        assert!(installing_agents.read().contains(&AgentBackend::CodexAcp));
        drop(codex);
        assert!(installing_agents.read().is_empty());
    }

    #[test]
    fn runtime_errors_are_isolated_per_agent() {
        let errors = AgentRuntimeErrors::default();
        errors.set(AgentBackend::ClaudeAcp, "Claude install failed".to_string());
        assert_eq!(
            errors.get(AgentBackend::ClaudeAcp).as_deref(),
            Some("Claude install failed")
        );
        assert_eq!(errors.get(AgentBackend::CodexAcp), None);
        assert_eq!(errors.get(AgentBackend::KimiAcp), None);

        errors.set(AgentBackend::CodexAcp, "Codex update required".to_string());
        errors.clear(AgentBackend::ClaudeAcp);
        assert_eq!(errors.get(AgentBackend::ClaudeAcp), None);
        assert_eq!(
            errors.get(AgentBackend::CodexAcp).as_deref(),
            Some("Codex update required")
        );
    }

    #[test]
    fn windows_cli_candidates_cover_native_and_npm_shims() {
        assert_eq!(
            agent_cli_names(AgentBackend::CodexAcp, true),
            &["codex.cmd", "codex.exe"]
        );
        assert_eq!(
            agent_cli_names(AgentBackend::ClaudeAcp, true),
            &["claude.exe", "claude.cmd"]
        );
        assert_eq!(
            agent_cli_names(AgentBackend::KimiAcp, true),
            &["kimi.exe", "kimi.cmd"]
        );
        assert_eq!(agent_cli_names(AgentBackend::KimiAcp, false), &["kimi"]);
    }

    #[test]
    fn dynamic_codex_upgrade_gate_requires_an_actual_version_change() {
        assert!(!codex_version_changed(Some("0.146.0"), Some("0.146.0")));
        assert!(codex_version_changed(Some("0.146.0"), Some("0.147.0")));
        assert!(!codex_version_changed(Some("0.146.0"), None));
        assert!(codex_version_changed(None, Some("0.146.0")));
    }

    #[test]
    fn brew_install_args_cover_all_agents() {
        // codex：已装 brew cask 走 upgrade，未装走 install。
        assert_eq!(
            brew_install_args(AgentBackend::CodexAcp, true),
            Some((
                "brew upgrade --cask codex",
                vec!["upgrade", "--cask", "codex"]
            ))
        );
        assert_eq!(
            brew_install_args(AgentBackend::CodexAcp, false),
            Some((
                "brew install --cask codex",
                vec!["install", "--cask", "codex"]
            ))
        );
        assert_eq!(
            brew_install_args(AgentBackend::ClaudeAcp, true),
            Some((
                "brew upgrade --cask claude-code",
                vec!["upgrade", "--cask", "claude-code"]
            ))
        );
        // kimi-code 是 formula，无 --cask。
        assert_eq!(
            brew_install_args(AgentBackend::KimiAcp, true),
            Some(("brew upgrade kimi-code", vec!["upgrade", "kimi-code"]))
        );
        assert_eq!(brew_install_args(AgentBackend::Deepseek, true), None);
    }

    #[test]
    fn npm_upgrade_args_target_latest_global_package() {
        assert_eq!(
            npm_upgrade_args(AgentBackend::CodexAcp),
            Some(vec![
                "install".to_string(),
                "-g".to_string(),
                "@openai/codex@latest".to_string()
            ])
        );
        assert_eq!(
            npm_upgrade_args(AgentBackend::ClaudeAcp),
            Some(vec![
                "install".to_string(),
                "-g".to_string(),
                "@anthropic-ai/claude-code@latest".to_string()
            ])
        );
        assert_eq!(
            npm_upgrade_args(AgentBackend::KimiAcp),
            Some(vec![
                "install".to_string(),
                "-g".to_string(),
                "@moonshot-ai/kimi-code@latest".to_string()
            ])
        );
        assert_eq!(npm_upgrade_args(AgentBackend::Deepseek), None);
    }

    #[test]
    fn path_install_source_recognizes_official_script_dirs() {
        let home = crate::platform::os::user_home_dir();
        assert_eq!(
            path_install_source(AgentBackend::CodexAcp, &home.join(".local/bin/codex")),
            Some("script")
        );
        assert_eq!(
            path_install_source(AgentBackend::ClaudeAcp, &home.join(".local/bin/claude")),
            Some("script")
        );
        assert_eq!(
            path_install_source(AgentBackend::KimiAcp, &home.join(".kimi-code/bin/kimi")),
            Some("script")
        );
        let legacy_managed = crate::platform::paths::pinvou3_home()
            .join("runtimes")
            .join("codex")
            .join("codex-0.144.6-macos-aarch64")
            .join("codex");
        assert_eq!(
            path_install_source(AgentBackend::CodexAcp, &legacy_managed),
            None
        );
        // 其他路径与其他 Agent 组合一律未知来源。
        assert_eq!(
            path_install_source(AgentBackend::CodexAcp, Path::new("/usr/local/bin/codex")),
            None
        );
        assert_eq!(
            path_install_source(AgentBackend::KimiAcp, &home.join(".local/bin/kimi")),
            None
        );
    }

    #[test]
    fn claude_and_kimi_version_gates_enforce_minimums() {
        assert!(claude_version_supported("2.1.163 (Claude Code)"));
        assert!(claude_version_supported("2.0.0 (Claude Code)"));
        assert!(!claude_version_supported("1.9.9 (Claude Code)"));
        assert!(!claude_version_supported("unknown"));
        assert!(kimi_version_supported("0.31.1"));
        assert!(kimi_version_supported("0.9.0"));
        assert!(!kimi_version_supported("0.8.9"));
        // 旧 Python 版 kimi-cli 等非裸 semver 输出一律判不合规。
        assert!(!kimi_version_supported("kimi-cli 0.31.1"));
        assert!(!kimi_version_supported("0.31"));
        assert!(!kimi_version_supported("native"));
    }

    #[test]
    fn sidecar_startup_recovery_restores_backfills_and_cleans_leftovers() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-sidecar-startup-recovery-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("session-agents.json");
        // 写入一个已绑定的原生代码会话（索引 + sidecar）。
        let writer = SessionAgentStore::for_test(path.clone());
        writer
            .bind_code_native_session("code-1", CodexWorkspaceKind::Project, Some(root.clone()))
            .unwrap();
        // 模拟辅助索引丢失：空内存索引 + 磁盘 sidecar 仍在 → 真实恢复一次。
        let agents = SessionAgentStore::for_test(path.clone());
        let summary = restore_code_native_sessions_from_sidecars(&agents);
        assert_eq!(
            summary,
            SidecarRecoverySummary {
                restored: 1,
                backfilled: 0,
                cleaned: 0
            }
        );
        assert!(agents.is_code_session("code-1"));
        assert_eq!(
            agents.get("code-1").workspace_path.as_deref(),
            Some(root.as_path())
        );
        // 索引完好时不再误报恢复信号。
        let summary = restore_code_native_sessions_from_sidecars(&agents);
        assert_eq!(summary, SidecarRecoverySummary::default());
        // sidecar 缺失（存量会话/绑定时写失败）时按索引回填自愈。
        let sidecar_path = root
            .join("sessions")
            .join("code-1")
            .join("code-session.json");
        std::fs::remove_file(&sidecar_path).unwrap();
        let summary = restore_code_native_sessions_from_sidecars(&agents);
        assert_eq!(
            summary,
            SidecarRecoverySummary {
                restored: 0,
                backfilled: 1,
                cleaned: 0
            }
        );
        assert!(sidecar_path.is_file());
        // ACP 会话的残留 sidecar：清理一次并如实计数，而不是每次启动报 degraded。
        agents
            .set_acp_workspace(
                "acp-1",
                AgentBackend::CodexAcp,
                CodexWorkspaceKind::Temporary,
                None,
            )
            .unwrap();
        let leftover_dir = root.join("sessions").join("acp-1");
        std::fs::create_dir_all(&leftover_dir).unwrap();
        std::fs::write(
            leftover_dir.join("code-session.json"),
            serde_json::to_vec(&store::CodeSessionSidecar {
                version: 1,
                workspace_kind: CodexWorkspaceKind::Temporary,
                workspace_path: None,
                bound_at: None,
            })
            .unwrap(),
        )
        .unwrap();
        let summary = restore_code_native_sessions_from_sidecars(&agents);
        assert_eq!(
            summary,
            SidecarRecoverySummary {
                restored: 0,
                backfilled: 0,
                cleaned: 1
            }
        );
        assert!(!leftover_dir.join("code-session.json").exists());
        assert_eq!(agents.get("acp-1").backend, AgentBackend::CodexAcp);
        // 残留已清理：再次启动扫描无任何动作。
        let summary = restore_code_native_sessions_from_sidecars(&agents);
        assert_eq!(summary, SidecarRecoverySummary::default());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
