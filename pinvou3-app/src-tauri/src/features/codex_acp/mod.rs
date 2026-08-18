//! 外部 Agent ACP 运行时。
//!
//! pinvou3 只做 ACP client、进程托管、权限路由、事件持久化和 `acp:event` 投影；
//! Codex、Claude Code 与 Kimi 的模型调用、工具循环、会话与权限协议都由各自
//! ACP Agent 提供。

mod agent_probe;
mod attachments;
mod auth_probe;
mod diagnostics;
mod events;
mod install;
mod introspect;
mod latest;
mod login;
mod operation_gate;
mod platform;
mod providers;
pub(crate) mod reader_window;
mod runtime;
mod store;
pub(crate) mod workspace;

// 纯提取：连接池自身仍是本 facade 的 impl 块；安装、登录与 Kimi 内省的
// 无副作用自由函数已迁入对应子模块，这里 glob 引入以保持调用点不变。
#[allow(unused_imports)]
use install::*;
#[allow(unused_imports)]
use introspect::*;
#[allow(unused_imports)]
use login::*;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{BufRead, Read};
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
use agent_probe::{CliProbeCache, CliProbeGates, ResolvedCli};
use anyhow::{bail, Context, Result};
use auth_probe::{AgentAuthProbeState, CachedAuthStatus};
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use wait_timeout::ChildExt;

use crate::features::sessions::SessionStore;
use attachments::{prepare_codex_prompt, CodexDisplayAttachment};
use deepseek_tui::session_manager::SessionMetadata;
use events::{
    load_timeline, load_web_timeline_page, persist_acp_state, EventBridge, WebAcpTimelineSlice,
};
pub use events::{
    project_acp_elicitation_request_for_web, project_acp_permission_request_for_web,
    AcpEventEnvelope,
};
use latest::LatestVersionProbe;
use operation_gate::begin_prompt;
pub use providers::{
    AcpProvidersView, ImportResult, ProviderManager, ProviderRecord, ProviderWireApi,
};
use runtime::{
    codex_version, probe_codex_runtime, version_at_least, ResolvedCodex, MIN_CODEX_VERSION,
};
pub use store::{
    validate_codex_project_workspace, AgentBackend, CodexWorkspaceKind, SessionAgentStore,
};
use store::{AcpConfigDefaultsStore, SessionAgentRecord, SessionMode};

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

pub(super) fn codex_authenticated(codex: &Path) -> bool {
    if nonempty_env("OPENAI_API_KEY") || auth_probe::codex_oauth_credentials_present() {
        return true;
    }
    // 第三方 Provider（中转）激活时，注入的 key 只存在于被 spawn 的 Codex 子进程
    // env 中，探测进程看不到；config.toml 有指向存在的表且 env_key 非空的
    // model_provider 即视为已认证，避免在 relay 场景误报需要登录。
    if let Ok(raw) = std::fs::read_to_string(
        crate::platform::os::user_home_dir()
            .join(".codex")
            .join("config.toml"),
    ) {
        if providers::codex_config_relay_env_key_present(&raw) {
            return true;
        }
    }
    cli_status_success(codex, &["login", "status"])
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
        mode: SessionMode::Plain,
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
/// 根定义漂移。对每个 sidecar：索引已持有 code 模式记录时跳过（不误报
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
        if record.mode.is_code() {
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
    /// 安装进行中的实际执行命令行（如 `irm https://claude.ai/install.ps1 | iex`）。
    /// command 事件只在安装开始时发一次，设置页/App 关闭重开后事件流已错过，
    /// 前端靠本字段跨挂载恢复「执行命令」行；安装结束由收口逻辑清除。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_command: Option<String>,
    /// 安装进行中的输出最新一行（同样供重挂载恢复展示）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_latest_line: Option<String>,
    pub error: Option<String>,
    /// 稳定的英文提示代码，前端映射 i18n 文案：
    /// "kimi_cli_missing" / "kimi_auth_required" / "claude_auth_required"。
    pub setup_hint: Option<&'static str>,
}

/// 安装进度共享 store（tauri managed state）：安装子进程逐行 emit 时同步记录
/// 执行命令与最新一行，status 查询时带回——前端设置页/App 关闭重开后从
/// status 恢复「执行命令 + 最新一行」进度展示，不依赖一次性事件。
#[derive(Default)]
struct InstallProgressStore(parking_lot::RwLock<HashMap<AgentBackend, InstallProgressInfo>>);

#[derive(Default)]
struct InstallProgressInfo {
    command: String,
    latest_line: Option<String>,
}

/// 轻量 ACP Agent 目录项。列表请求只回答“有哪些 Agent”，不触发 CLI、认证、
/// npm 或 Homebrew 探测；具体运行状态由选中 Agent 的状态接口按需返回。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AcpAgentDescriptor {
    pub agent_id: &'static str,
    pub agent_name: &'static str,
}

#[derive(Debug, Clone, Default)]
struct AgentLoginState {
    in_progress: bool,
    pub(super) url: Option<String>,
    pub(super) code: Option<String>,
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
    /// 会话级 Provider 覆盖（F11）：本会话固定使用的 Provider id；null = 跟随全局。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
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
            // 会话级 Provider 覆盖由 session_info() 从会话记录补齐
            provider: None,
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

/// 安装已取消的结构化错误标记：前端按它收口「已取消」阶段，不依赖中文
/// 文案子串匹配（复审低危 5）。
pub(crate) static INSTALL_CANCELLED_MARKER: &str = "install_cancelled:";

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
    /// 安装子进程 pid 注册表：取消安装时按 pid 杀进程树。
    install_children: Arc<parking_lot::Mutex<HashMap<AgentBackend, u32>>>,
    /// 取消标记：取消命令写入，安装等待侧在失败出口消费并以「安装已取消」收尾。
    install_cancelled: Arc<parking_lot::Mutex<HashSet<AgentBackend>>>,
    login_states: Arc<parking_lot::RwLock<HashMap<AgentBackend, AgentLoginState>>>,
    login_inputs: Arc<Mutex<HashMap<AgentBackend, ChildStdin>>>,
    codex_upgrade_required: Arc<AtomicBool>,
    runtime_errors: AgentRuntimeErrors,
    runtime_probe: Arc<parking_lot::RwLock<RuntimeProbeCache>>,
    runtime_probe_gate: Arc<Mutex<()>>,
    runtime_probe_generation: Arc<AtomicU64>,
    cli_probe: Arc<parking_lot::RwLock<CliProbeCache>>,
    latest_version_probe: LatestVersionProbe,
    cli_probe_gates: Arc<CliProbeGates>,
    auth_cache: Arc<parking_lot::RwLock<HashMap<AgentBackend, CachedAuthStatus>>>,
    auth_probe: Arc<AgentAuthProbeState>,
    bundled_adapter: Option<PathBuf>,
    bundled_claude_adapter: Option<PathBuf>,
    bundled_node: Option<PathBuf>,
    /// 第三方 Provider（中转）管理：store + 凭据 + 三写入器。
    providers: ProviderManager,
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

/// 安装子进程 pid 登记 guard：spawn 成功后登记，drop 时（完成/失败/超时/取消
/// 任意出口）统一注销，避免残留 pid 在后续取消时误杀被系统复用的进程。
struct InstallChildGuard {
    install_children: Arc<parking_lot::Mutex<HashMap<AgentBackend, u32>>>,
    backend: AgentBackend,
}

impl InstallChildGuard {
    fn register(
        install_children: &Arc<parking_lot::Mutex<HashMap<AgentBackend, u32>>>,
        backend: AgentBackend,
        pid: Option<u32>,
    ) -> Self {
        if let Some(pid) = pid {
            install_children.lock().insert(backend, pid);
        }
        Self {
            install_children: install_children.clone(),
            backend,
        }
    }
}

impl Drop for InstallChildGuard {
    fn drop(&mut self) {
        self.install_children.lock().remove(&self.backend);
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
        app.manage(InstallProgressStore::default());
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
            install_children: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            install_cancelled: Arc::new(parking_lot::Mutex::new(HashSet::new())),
            login_states: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            login_inputs: Arc::new(Mutex::new(HashMap::new())),
            codex_upgrade_required: Arc::new(AtomicBool::new(false)),
            runtime_errors: AgentRuntimeErrors::default(),
            runtime_probe: Arc::new(parking_lot::RwLock::new(RuntimeProbeCache::default())),
            runtime_probe_gate: Arc::new(Mutex::new(())),
            runtime_probe_generation: Arc::new(AtomicU64::new(0)),
            cli_probe: Arc::new(parking_lot::RwLock::new(CliProbeCache::default())),
            latest_version_probe: LatestVersionProbe::new()?,
            cli_probe_gates: Arc::new(CliProbeGates::default()),
            auth_cache: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            auth_probe: Arc::new(AgentAuthProbeState::default()),
            bundled_adapter,
            bundled_claude_adapter,
            bundled_node,
            providers: ProviderManager::new(
                crate::platform::credential_store::SystemCredentialStore::new(),
            )?,
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
        if record.mode.is_code() {
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
        // 未来直接使用本方法的路径对 code 模式会话也有正确语义。
        if record.mode.is_code() {
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

    pub fn agent_catalog() -> Vec<AcpAgentDescriptor> {
        AgentBackend::ACP_BACKENDS
            .into_iter()
            .filter_map(|backend| {
                Some(AcpAgentDescriptor {
                    agent_id: backend.agent_id()?,
                    agent_name: backend.display_name(),
                })
            })
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
            let kimi = self.cli_probe_for(backend);
            let installing = self.installing_agents.read().contains(&backend);
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
                && !installing
                && kimi
                    .as_ref()
                    .is_some_and(|cli| self.cached_agent_authenticated(backend, &cli.path));
            let mut status = CodexAcpStatus {
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
                installing,
                error: login.error.or_else(|| self.runtime_errors.get(backend)),
                install_command: None,
                install_latest_line: None,
                setup_hint: if !cli_ready {
                    Some("kimi_cli_missing")
                } else if !authenticated {
                    Some("kimi_auth_required")
                } else {
                    None
                },
            };
            fill_install_progress(&self.app, backend, &mut status);
            return status;
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
            .then(|| self.cli_probe_for(backend))
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
        let installing = self.installing_agents.read().contains(&backend);
        let authenticated = !login.in_progress
            && !installing
            && match backend {
                AgentBackend::CodexAcp => codex.as_ref().is_some_and(|resolved| {
                    self.cached_agent_authenticated(backend, &resolved.probe_path)
                }),
                AgentBackend::ClaudeAcp => claude
                    .as_ref()
                    .is_some_and(|cli| self.cached_agent_authenticated(backend, &cli.path)),
                AgentBackend::Deepseek | AgentBackend::KimiAcp => unreachable!(),
            };
        let mut status = CodexAcpStatus {
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
            installing,
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
            install_command: None,
            install_latest_line: None,
        };
        fill_install_progress(&self.app, backend, &mut status);
        status
    }

    pub async fn refresh_status(&self) -> CodexAcpStatus {
        self.refresh_runtime_probe(false).await;
        self.status_async().await
    }

    async fn refresh_runtime_probe(&self, force: bool) {
        let observed_generation = self.runtime_probe_generation.load(Ordering::Acquire);
        if !force && self.runtime_probe.read().initialized {
            return;
        }
        let _gate = self.runtime_probe_gate.lock().await;
        if !force && self.runtime_probe.read().initialized {
            return;
        }
        if force
            && self.runtime_probe.read().initialized
            && self.runtime_probe_generation.load(Ordering::Acquire) != observed_generation
        {
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
        let resolve_install_source = force || self.codex_upgrade_required.load(Ordering::Acquire);
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
        // Codex、Node 与 Homebrew 探测彼此独立，并行执行；Codex 候选探测同时
        // 返回版本兼容性，避免为同一系统 CLI 重复执行两次 `--version`。
        let runtime_task = tokio::task::spawn_blocking(move || {
            let candidates = probe_codex_runtime(system_codex.clone(), legacy_codex);
            let codex = candidates.resolved;
            // CLI 正常可用时安装来源只做路径判断；版本过旧、服务端要求升级或
            // 用户主动重查时才执行较重的 npm/brew 来源探测。
            let source_path = if candidates.system_codex_incompatible {
                system_codex.clone()
            } else {
                codex.as_ref().map(|resolved| resolved.path.clone())
            };
            let codex_install_source = source_path.as_deref().and_then(|path| {
                if candidates.system_codex_incompatible || resolve_install_source {
                    detect_install_source(AgentBackend::CodexAcp, path)
                } else {
                    path_install_source(AgentBackend::CodexAcp, path)
                }
            });
            (
                codex,
                codex_install_source,
                candidates.system_codex_incompatible,
            )
        });
        let node_task =
            tokio::task::spawn_blocking(move || node.as_deref().and_then(installed_node_version));
        let brew_task = tokio::task::spawn_blocking(platform::brew_available);
        let detected = match tokio::join!(runtime_task, node_task, brew_task) {
            (
                Ok((codex, codex_install_source, system_codex_incompatible)),
                Ok(node_version),
                Ok(brew_available),
            ) => Ok(RuntimeProbeCache {
                initialized: true,
                node_version,
                codex,
                brew_available,
                codex_install_source,
                system_codex_incompatible,
            }),
            _ => Err(()),
        };

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
            Err(_join_error) => {
                diagnostics::write(
                    &operation_id,
                    "probe:failed",
                    format!(
                        "elapsed_ms={} error=探测任务异常退出",
                        started.elapsed().as_millis()
                    ),
                );
                *self.runtime_probe.write() = RuntimeProbeCache {
                    initialized: true,
                    ..RuntimeProbeCache::default()
                };
            }
        }
        self.runtime_probe_generation.fetch_add(1, Ordering::AcqRel);
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
        // 清掉上一次安装可能残留的取消标记：本次失败语义只来自本次取消。
        self.install_cancelled.lock().remove(&backend);
        diagnostics::write(
            &operation_id,
            "homebrew:start",
            format!("agent={}", backend.agent_id().unwrap_or("unknown")),
        );
        // brew install/upgrade 是阻塞式子进程，放到 spawn_blocking 避免卡住 async runtime。
        let install_children = self.install_children.clone();
        let install_cancelled = self.install_cancelled.clone();
        let app = self.app.clone();
        let result = tokio::task::spawn_blocking(move || {
            let run_brew = |args: &[&str], command: &str| -> Result<std::process::Output> {
                let mut brew_command = std::process::Command::new(platform::brew_bin());
                brew_command
                    .args(args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                // 独立进程组：取消时按组杀，brew 派生进程不孤儿化。
                crate::platform::process::std_process_group_leader(&mut brew_command);
                let mut child = brew_command.spawn().context("启动 Homebrew 失败")?;
                // 登记 pid 供取消命令杀进程树；guard 在 run_brew 出口注销。
                let _child_guard =
                    InstallChildGuard::register(&install_children, backend, Some(child.id()));
                // 取消可能发生在 spawn 与登记之间：登记后立即补检一次。
                if install_cancelled.lock().contains(&backend) {
                    let _ = child.kill();
                }
                emit_install_progress(&app, backend, "command", command);
                // 逐行转发输出为进度事件；完整输出经 mpsc 回收，保留给尾部诊断。
                let stdout = child.stdout.take().context("读取 Homebrew 标准输出失败")?;
                let stderr = child.stderr.take().context("读取 Homebrew 错误输出失败")?;
                let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
                let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
                let app_stdout = app.clone();
                let app_stderr = app.clone();
                let stdout_thread = std::thread::spawn(move || {
                    stream_std_lines(stdout, "stdout", stdout_tx, app_stdout, backend)
                });
                let stderr_thread = std::thread::spawn(move || {
                    stream_std_lines(stderr, "stderr", stderr_tx, app_stderr, backend)
                });
                let status = child.wait().context("等待 Homebrew 进程失败")?;
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                let mut stdout = String::new();
                for line in stdout_rx {
                    stdout.push_str(&line);
                    stdout.push('\n');
                }
                let mut stderr = String::new();
                for line in stderr_rx {
                    stderr.push_str(&line);
                    stderr.push('\n');
                }
                Ok(std::process::Output {
                    status,
                    stdout: stdout.into_bytes(),
                    stderr: stderr.into_bytes(),
                })
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
            let output = run_brew(&args, &command)?;
            if output.status.success() || already_done(&output) {
                return Ok(());
            }
            // 进程被用户取消（taskkill/kill）会以失败状态走到这里：改写为已取消语义。
            if install_cancelled.lock().remove(&backend) {
                bail!(
                    "{INSTALL_CANCELLED_MARKER}{} 安装已取消",
                    backend.display_name()
                );
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
                // 用户主动取消不是错误：不写 status.error，前端以「已取消」阶段
                // 与通知收口，避免红错区重复显示。
                if !detail.contains(INSTALL_CANCELLED_MARKER) {
                    self.runtime_errors.set(
                        backend,
                        format!(
                            "{detail}（诊断编号：{operation_id}；日志：{}）",
                            diagnostics::log_path().display()
                        ),
                    );
                }
                Err(error)
            }
        }
    }

    /// 安装前自检：把「脚本源不可达」和「目标路径存在不可用的坏残留」挡在
    /// 安装开始前，避免安装跑到一半才失败，或覆盖坏安装后依旧不可用。
    ///
    /// - official_script：HEAD 探测脚本 URL（连接 3s / 总 5s 超时，传输层失败
    ///   即视为不可达）；并检查官方脚本目标路径的残留是否可用——存在但探测
    ///   没解析到它（或该文件本身跑不起 `--version`）就是半成品/被替换的坏
    ///   文件，覆盖安装未必能修复，先拦截并提示删除。
    /// - npm_upgrade：npm 可执行存在性由 run_npm_global_upgrade 保证；npm
    ///   全局安装幂等，半装残留会由 npm 自身收敛，不做额外检测。
    /// - brew_upgrade：brew 自身处理幂等与升级，无需预检。
    async fn preflight_install(&self, backend: AgentBackend, action: &str) -> Result<()> {
        if action != "official_script" {
            return Ok(());
        }
        let (unix_url, windows_url) = official_script_urls(backend);
        let url = if crate::platform::capabilities::is_windows() {
            windows_url
        } else {
            unix_url
        };
        if !script_url_reachable(url).await {
            let npm_pkg = npm_package(backend).unwrap_or("");
            bail!(
                "无法连接 {} 官方安装脚本（{url}），请检查网络或稍后重试；\
                 也可手动安装：npm install -g {npm_pkg}",
                backend.display_name()
            );
        }
        if let Some(path) = self.stale_official_install(backend) {
            bail!(
                "检测到不完整的 {} 安装残留：{} 存在但当前不可用。为避免安装出错，\
                 请先删除它后重试安装（或点击「重新检测」确认环境）",
                backend.display_name(),
                path.display()
            );
        }
        Ok(())
    }

    /// 官方脚本目标路径存在但当前探测没有「解析到该路径且版本可用」的文件，
    /// 进一步对该文件本身跑一次 `--version`，区分「可用但版本过旧」（脚本升级
    /// 可修复，不拦截）与「无法运行的坏文件」（拦截）。
    fn stale_official_install(&self, backend: AgentBackend) -> Option<PathBuf> {
        let resolved_ok = match backend {
            AgentBackend::CodexAcp => self
                .runtime_probe
                .read()
                .codex
                .as_ref()
                .map(|resolved| resolved.path.clone()),
            AgentBackend::ClaudeAcp | AgentBackend::KimiAcp => self
                .cli_probe_for(backend)
                .filter(|cli| cli.version.is_some())
                .map(|cli| cli.path.clone()),
            AgentBackend::Deepseek => None,
        };
        let targets = providers::lifecycle::official_script_paths(backend);
        targets
            .iter()
            .find(|target| stale_official_target(target, resolved_ok.as_deref()))
            .and_then(|target| {
                if self.official_target_works(backend, target) {
                    None
                } else {
                    Some(target.clone())
                }
            })
    }

    /// 目标文件本身是否可运行（直接对路径跑 `--version` 且输出版本号）。
    fn official_target_works(&self, backend: AgentBackend, target: &Path) -> bool {
        match backend {
            AgentBackend::CodexAcp => codex_version(target).is_some(),
            AgentBackend::ClaudeAcp | AgentBackend::KimiAcp => {
                command_version_output(target).is_some()
            }
            AgentBackend::Deepseek => false,
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
        // 安装前自检：把网络不可达与坏残留挡在开始前（见 preflight_install）。
        // 必须放在停会话之前——自检失败时不要白白关掉用户运行中的会话。
        self.preflight_install(backend, action).await?;
        // 安装/升级前停掉该 Agent 的运行中会话：Windows 下被会话占用的
        // CLI 二进制无法替换（npm EBUSY / 脚本覆盖失败），先 shutdown 再安装，
        // 与卸载的前置检查同一原则。
        self.restart_agent_sessions(backend).await;
        // 升级前版本与安装态：安装/升级完成后用它校验「版本真的变了」（见
        // verify_upgrade_effective）——官方脚本假成功、npm allowScripts 拦截
        // postinstall 等都会让命令 exit 0 但版本原地不动。previous 可能因探测
        // 超时/占用而缺失，此时用 previous_installed 区分「全新安装」与
        // 「已安装但探测失败」，后者同样不能跳过校验。
        let operation_id = diagnostics::operation_id("install");
        let previous_version = status.version.clone();
        let previous_installed = status.installed;
        let result = match action {
            "none" => Ok(status),
            "brew_upgrade" => self.upgrade_via_homebrew(backend).await,
            "npm_upgrade" => self.upgrade_via_npm(backend).await,
            "official_script" => self.install_via_official_script(backend).await,
            "manual" => bail!(
                "当前平台不支持自动安装 {} CLI，请按官方文档手动安装后重试",
                backend.display_name()
            ),
            other => bail!("未知安装方式: {other}"),
        };
        // 升级有效性校验：命令 exit 0 不等于升级真的生效。官方脚本假成功、
        // npm allowScripts 拦截 postinstall、二进制被占用覆盖失败等都会让
        // 版本原地不动——如实报错，而不是把「命令成功」误报为「升级成功」。
        // none 分支未执行任何安装动作，跳过校验。
        // 收口全程写诊断日志：命令返回前任意一步卡住都能从日志定位。
        let status = match result {
            Ok(status) => status,
            Err(error) => {
                let detail = format!("{error:#}");
                diagnostics::write(&operation_id, "install:failed", &detail);
                clear_install_progress(&self.app, backend);
                return Err(error);
            }
        };
        if action != "none" {
            match self.verify_upgrade_effective(
                backend,
                previous_version,
                previous_installed,
                &status,
            ) {
                Ok(()) => {
                    diagnostics::write(&operation_id, "install:verify_ok", "result=version_ok")
                }
                Err(error) => {
                    let detail = format!("{error:#}");
                    diagnostics::write(&operation_id, "install:verify_failed", &detail);
                    return Err(error);
                }
            }
        }
        // 安装收口：共享进度只保留「进行中」，结束后清除，避免 status 长期带过期命令。
        clear_install_progress(&self.app, backend);
        diagnostics::write(&operation_id, "install:returning", "result=success");
        Ok(status)
    }

    /// 升级后版本有效性校验：升级目标（官方 latest）明确高于升级前版本时，
    /// 装完必须看到版本变化（或探测到新版本）。原地不动或探测失败说明
    /// 安装未真正生效，给出可操作错误而非报成功。
    fn verify_upgrade_effective(
        &self,
        backend: AgentBackend,
        previous_version: Option<String>,
        previous_installed: bool,
        status: &CodexAcpStatus,
    ) -> Result<()> {
        let Some(latest) = status.latest_version.as_deref() else {
            // 官方最新版本未知（离线/接口异常）：不误报，交由用户判断。
            return Ok(());
        };
        let Some(previous) = previous_version.as_deref() else {
            // 升级前版本探测失败（CLI 被占用导致 --version 超时）时，previous
            // 缺失 ≠ 全新安装：升级后仍必须探测到可用 CLI，否则说明占用/拦截
            // 依旧存在，如实报错而不是把「命令成功」误报为「升级成功」。
            if previous_installed && !status.codex_available {
                bail!(
                    "{} 升级命令已成功执行，但升级后仍检测不到可用的 {} CLI。\
                     安装目录可能被占用或被安全软件拦截；请关闭占用进程后重试",
                    backend.display_name(),
                    backend.display_name()
                );
            }
            return Ok(());
        };
        if latest == previous {
            // 已是官方最新：幂等升级不要求版本变化。
            return Ok(());
        }
        match status.version.as_deref() {
            Some(current) if current != previous => Ok(()),
            _ => {
                let npm_pkg = npm_package(backend).unwrap_or("");
                bail!(
                    "{} 升级命令已成功执行，但版本仍为 {previous}（官方最新 {latest}）。\
                     安装目录可能被占用或被安全软件拦截；请关闭占用进程后重试，\
                     或手动安装：npm install -g {npm_pkg}（若 npm 提示 allow-scripts \
                     拦截，请先运行 npm approve-scripts 后重试）",
                    backend.display_name()
                );
            }
        }
    }

    /// 安装/升级后按 Agent 强制重探测：Codex 走 runtime probe，Claude/Kimi
    /// 只失效自身缓存，不让一个 Agent 的操作拖慢另一个 Agent。
    async fn refresh_agent_cli_probe(&self, backend: AgentBackend) {
        self.invalidate_auth_cache(backend);
        match backend {
            AgentBackend::CodexAcp => self.refresh_runtime_probe(true).await,
            AgentBackend::ClaudeAcp | AgentBackend::KimiAcp => self.invalidate_cli_probe(backend),
            AgentBackend::Deepseek => {}
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
        // 清掉上一次安装可能残留的取消标记：本次失败语义只来自本次取消。
        self.install_cancelled.lock().remove(&backend);
        diagnostics::write(
            &operation_id,
            "npm:start",
            format!("agent={}", backend.agent_id().unwrap_or("unknown")),
        );
        let result = run_npm_global_upgrade(
            &self.app,
            backend,
            &operation_id,
            &self.install_children,
            &self.install_cancelled,
        )
        .await;
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
                // 用户主动取消不是错误：不写 status.error，前端以「已取消」阶段
                // 与通知收口，避免红错区重复显示。
                if !detail.contains(INSTALL_CANCELLED_MARKER) {
                    self.runtime_errors.set(backend, detail);
                }
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
        // 清掉上一次安装可能残留的取消标记：本次失败语义只来自本次取消。
        self.install_cancelled.lock().remove(&backend);
        // 升级前把官方脚本目标路径的旧二进制改名移开：Claude 官方 install
        // 子命令检测到「已存在 native installation」时会跳过覆盖却仍输出成功
        // （假成功，实测）。移开后脚本走全新安装路径；脚本失败时恢复旧文件，
        // 避免旧版本丢失。全新安装（无旧文件）本函数不移动任何文件。
        let moved_backups = match move_official_binaries_aside(backend) {
            Ok(moved) => {
                if !moved.is_empty() {
                    diagnostics::write(
                        &operation_id,
                        "script:move_aside",
                        format!(
                            "agent={} backups={}",
                            backend.agent_id().unwrap_or("unknown"),
                            moved
                                .iter()
                                .map(|(_, backup)| backup.display().to_string())
                                .collect::<Vec<_>>()
                                .join(",")
                        ),
                    );
                }
                moved
            }
            Err(error) => {
                diagnostics::write(
                    &operation_id,
                    "script:move_aside_failed",
                    &format!("{error:#}"),
                );
                return Err(error);
            }
        };
        diagnostics::write(
            &operation_id,
            "script:start",
            format!("agent={}", backend.agent_id().unwrap_or("unknown")),
        );
        let result = run_official_install_script(
            &self.app,
            backend,
            &operation_id,
            &self.install_children,
            &self.install_cancelled,
        )
        .await;
        if result.is_err() && !moved_backups.is_empty() {
            // 脚本失败（含用户取消）：恢复旧文件，避免旧版本丢失。
            for (original, backup) in &moved_backups {
                if backup.is_file() && !original.exists() {
                    let _ = std::fs::rename(backup, original);
                }
            }
            diagnostics::write(
                &operation_id,
                "script:restore_backups",
                format!(
                    "agent={} restored={}",
                    backend.agent_id().unwrap_or("unknown"),
                    moved_backups.len()
                ),
            );
        }
        drop(install_guard);
        // 无论成败都按 Agent 强制重新探测：脚本可能部分完成（如已写入二进制但
        // PATH 更新失败）；Codex 还需立即检查 ~/.local/bin 的官方安装绝对路径。
        self.refresh_agent_cli_probe(backend).await;
        match result {
            Ok(()) => {
                diagnostics::write(&operation_id, "script:complete", "result=success");
                // 备份不在这里删：finalize 内部做版本/就绪校验（verify），
                // **验证通过后才清理** .pre-upgrade 备份，释放磁盘空间。
                // 假成功（exit 0 但未生效）时保留旧二进制，避免旧 CLI 永久
                // 丢失（评审中危项：先删后验）。
                let finalized = self
                    .finalize_agent_install(backend, previous_codex_version)
                    .await;
                if finalized.is_ok() {
                    for (_, backup) in &moved_backups {
                        let _ = std::fs::remove_file(backup);
                    }
                }
                finalized
            }
            Err(error) => {
                let detail = format!("{error:#}");
                diagnostics::write(&operation_id, "script:failed", &detail);
                // 用户主动取消不是错误：不写 status.error，前端以「已取消」阶段
                // 与通知收口，避免红错区重复显示。
                if !detail.contains(INSTALL_CANCELLED_MARKER) {
                    self.runtime_errors.set(backend, detail);
                }
                Err(error)
            }
        }
    }

    /// 取消正在进行的 Agent CLI 安装：写入取消标记并按登记的 pid 杀进程树。
    /// 等待侧检测到取消标记后以「安装已取消」收尾，installing 随其出口恢复 false。
    pub async fn cancel_agent_install(&self, agent_id: &str) -> Result<CodexAcpStatus> {
        let backend = AgentBackend::parse(Some(agent_id))?;
        if !backend.is_acp() {
            bail!("Agent 不是 ACP 后端: {agent_id}");
        }
        if !self.installing_agents.read().contains(&backend) {
            bail!("{} 当前没有正在进行的安装", backend.display_name());
        }
        self.install_cancelled.lock().insert(backend);
        if let Some(pid) = self.install_children.lock().get(&backend).copied() {
            kill_install_process_tree(pid);
        }
        Ok(self.status_for_async(backend).await)
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
        self.invalidate_auth_cache(backend);
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

    // ------------------------------------------------------------------
    // 第三方 Provider（中转）管理
    // ------------------------------------------------------------------

    pub fn list_acp_providers(&self, agent: &str) -> Result<AcpProvidersView> {
        self.providers.list(agent)
    }

    /// 读取 Provider 的 API key（明文，仅编辑弹窗「显示密钥」按需调用）。
    pub fn get_acp_provider_key(&self, agent: &str, provider_id: &str) -> Result<Option<String>> {
        self.providers.api_key(agent, provider_id)
    }

    /// 保存 Provider。若保存的是**生效中**的 Provider，配置已重写但运行中的
    /// CLI 进程仍持旧配置（尤其 codex 的 key 在 spawn 时注入）；与 switch/
    /// delete/official 一致重启该 Agent 会话，否则 UI「生效中」与实际进程
    /// 不一致（复审 N2）。
    pub async fn save_acp_provider(
        &self,
        agent: &str,
        provider_id: Option<&str>,
        name: String,
        base_url: String,
        model: Option<String>,
        model_slots: Option<std::collections::HashMap<String, String>>,
        context_window: Option<i64>,
        wire_api: ProviderWireApi,
        api_key: Option<String>,
        api_key_action: crate::platform::credential_store::CredentialEditAction,
    ) -> Result<ProviderRecord> {
        let backend = AgentBackend::parse(Some(agent))?;
        let record = self.providers.save(
            agent,
            provider_id,
            name,
            base_url,
            model,
            model_slots,
            context_window,
            wire_api,
            api_key,
            api_key_action,
        )?;
        // 保存的是生效中 Provider：配置已重写，重启该 Agent 会话使新配置生效
        // （与 switch/delete/official 同一链路；codex 的 key 在 spawn 时注入）。
        if self.providers.store().current(agent).as_deref() == Some(record.id.as_str()) {
            self.invalidate_auth_cache(backend);
            self.restart_agent_sessions(backend).await;
        }
        Ok(record)
    }

    /// 删除 Provider；若删除的是当前 Provider，自动恢复官方登录并重启 runtime。
    /// 配置回退在 `ProviderManager::delete` 内部完成（先 revert 后删 store）。
    pub async fn delete_acp_provider(
        &self,
        agent: &str,
        provider_id: &str,
    ) -> Result<CodexAcpStatus> {
        let backend = AgentBackend::parse(Some(agent))?;
        let was_current = self.providers.store().current(agent).as_deref() == Some(provider_id);
        self.providers.delete(agent, provider_id)?;
        if was_current {
            self.invalidate_auth_cache(backend);
            self.restart_agent_sessions(backend).await;
        }
        Ok(self.status_for_async(backend).await)
    }

    /// 切换 Provider：写入 CLI 配置文件 → store 持久化 → 刷新探测缓存 → 重启
    /// 该 Agent 的运行中会话 → 返回最新状态。任一步失败返回错误且不报成功。
    pub async fn switch_acp_provider(
        &self,
        agent: &str,
        provider_id: &str,
    ) -> Result<CodexAcpStatus> {
        let backend = AgentBackend::parse(Some(agent))?;
        self.providers.switch(agent, provider_id)?;
        self.invalidate_auth_cache(backend);
        self.restart_agent_sessions(backend).await;
        Ok(self.status_for_async(backend).await)
    }

    /// 恢复官方登录：只删除本功能写入的键/表，然后走同一套重启链路。
    pub async fn switch_acp_provider_official(&self, agent: &str) -> Result<CodexAcpStatus> {
        let backend = AgentBackend::parse(Some(agent))?;
        self.providers.switch_official(agent)?;
        self.invalidate_auth_cache(backend);
        self.restart_agent_sessions(backend).await;
        Ok(self.status_for_async(backend).await)
    }

    pub fn export_acp_providers(&self, agent: &str) -> Result<String> {
        self.providers.export(agent)
    }

    pub fn import_acp_providers(&self, agent: &str, json: &str) -> Result<ImportResult> {
        self.providers.import(agent, json)
    }

    /// 会话级 Provider 覆盖（F11）：写入 per-session 配置的 "provider" 键并重启该
    /// 会话 runtime；`provider_id=None` 恢复该会话官方登录。解析优先级：
    /// 会话 option > 全局 current_provider。
    pub async fn set_acp_session_provider(
        &self,
        session_id: &str,
        provider_id: Option<String>,
    ) -> Result<CodexAcpSessionInfo> {
        if !self.is_acp(session_id) {
            bail!("当前会话不是 ACP 会话");
        }
        let backend = self.backend(session_id);
        let agent = backend.agent_id().context("非 ACP 会话")?;
        match provider_id {
            Some(provider_id) => {
                self.providers
                    .store()
                    .get(agent, &provider_id)
                    .with_context(|| format!("Provider 不存在: {provider_id}"))?;
                self.agents
                    .set_acp_config_value(session_id, "provider", &provider_id)?;
            }
            None => {
                self.agents.clear_acp_config_value(session_id, "provider")?;
            }
        }
        self.restart_agent_sessions(backend).await;
        self.session_info(session_id).await
    }

    /// 当前会话生效的 Provider key（会话 option > 全局 current_provider）。
    /// 仅用于 Codex 的 spawn env 注入。
    fn session_provider_api_key(&self, session_id: &str) -> Result<Option<String>> {
        let backend = self.backend(session_id);
        let Some(agent) = backend.agent_id() else {
            return Ok(None);
        };
        let session_provider = self
            .agents
            .get(session_id)
            .acp_config_values
            .get("provider")
            .cloned();
        let provider_id = match session_provider {
            Some(provider_id) => Some(provider_id),
            None => self.providers.store().current(agent),
        };
        let Some(provider_id) = provider_id else {
            return Ok(None);
        };
        if self.providers.store().get(agent, &provider_id).is_none() {
            return Ok(None);
        }
        self.providers.api_key(agent, &provider_id)
    }

    /// 卸载 ACP Agent CLI。有运行中会话时拒绝；`cleanup=true` 时额外删除该 Agent
    /// 的配置目录、受管 Provider 与对应凭据（前端默认不勾选）。
    pub async fn uninstall_acp_agent(&self, agent: &str, cleanup: bool) -> Result<CodexAcpStatus> {
        let backend = AgentBackend::parse(Some(agent))?;
        if !backend.is_acp() {
            bail!("Agent 不是 ACP 后端: {agent}");
        }
        // 请求日志必须在任何前置检查之前：被「运行中会话」等拦截时也要留下
        // 记录，否则用户报「卸载没反应」时日志里什么都没有。
        let operation_id = diagnostics::operation_id("uninstall");
        diagnostics::write(
            &operation_id,
            "uninstall:requested",
            format!("agent={agent} cleanup={cleanup}"),
        );
        {
            let sessions = self.sessions.lock().await;
            let running = sessions
                .keys()
                .filter(|session_id| self.agents.backend(session_id) == backend)
                .count();
            if running > 0 {
                diagnostics::write(
                    &operation_id,
                    "uninstall:rejected",
                    format!("agent={agent} running_sessions={running}"),
                );
                bail!(
                    "{} 有 {running} 个运行中的会话，请先关闭会话后再卸载",
                    backend.display_name()
                );
            }
        }
        // 按安装来源分派卸载命令；先刷新探测缓存拿到准确来源
        match backend {
            AgentBackend::CodexAcp => {
                self.refresh_runtime_probe(false).await;
            }
            _ => {
                self.refresh_agent_cli_probe(backend).await;
            }
        }
        let install_source = match backend {
            AgentBackend::CodexAcp => self
                .runtime_probe
                .read()
                .codex_install_source
                .map(str::to_string),
            AgentBackend::ClaudeAcp | AgentBackend::KimiAcp => self
                .cli_probe_for(backend)
                .and_then(|cli| cli.install_source)
                .map(str::to_string),
            AgentBackend::Deepseek => None,
        };
        // 卸载前自检：刷新探测后既无安装来源、官方脚本路径也无残留时，明确告知
        // 「无需卸载」，而不是静默执行空操作给用户「卸完了」的错觉。
        if install_source.is_none()
            && !providers::lifecycle::official_script_paths(backend)
                .into_iter()
                .any(|path| path.exists())
        {
            bail!("未检测到已安装的 {} CLI，无需卸载", backend.display_name());
        }
        let command = providers::lifecycle::uninstall_command(backend, install_source.as_deref());
        match command {
            providers::lifecycle::UninstallCommand::Spawn((program, args)) => {
                diagnostics::write(
                    &operation_id,
                    "uninstall:spawn",
                    format!("agent={agent} program={program}"),
                );
                tokio::task::spawn_blocking(move || {
                    let mut command =
                        crate::platform::process::external_command(std::path::Path::new(&program));
                    command.args(&args);
                    let status = command
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .with_context(|| format!("启动卸载命令失败: {program}"))?;
                    if !status.success() {
                        anyhow::bail!(
                            "卸载命令失败({program}): {}",
                            status
                                .code()
                                .map_or_else(|| "信号终止".to_string(), |code| code.to_string())
                        );
                    }
                    Ok::<(), anyhow::Error>(())
                })
                .await
                .context("卸载任务异常退出")??;
            }
            providers::lifecycle::UninstallCommand::RemovePaths(paths) => {
                let rendered = paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                diagnostics::write(
                    &operation_id,
                    "uninstall:remove_paths",
                    format!("agent={agent} paths={rendered}"),
                );
                for path in paths {
                    if path.is_dir() {
                        std::fs::remove_dir_all(&path)
                            .with_context(|| format!("删除 {} 失败", path.display()))?;
                    } else if path.exists() {
                        std::fs::remove_file(&path)
                            .with_context(|| format!("删除 {} 失败", path.display()))?;
                    }
                }
            }
        }
        self.refresh_agent_cli_probe(backend).await;
        if cleanup {
            let agent_id = backend.agent_id().context("非 ACP 后端")?;
            for path in providers::lifecycle::config_paths(backend) {
                if path.exists() {
                    std::fs::remove_dir_all(&path)
                        .with_context(|| format!("删除配置目录 {} 失败", path.display()))?;
                }
            }
            for record in self.providers.store().state(agent_id).providers {
                let _ = self.providers.delete(agent_id, &record.id);
            }
            let _ = self.providers.store().set_current(agent_id, None);
        }
        // 渠道翻转防护：卸载后仍探测到「已安装」说明存在另一渠道的安装（如
        // 脚本目录删除后探测回落到 npm/PATH 的另一份）。如实告知，避免
        // 「卸不掉」的错觉；再次卸载会按新探测到的渠道处理另一份。
        let status = self.status_for_async(backend).await;
        if status.installed {
            self.runtime_errors.set(
                backend,
                format!(
                    "{} 已按 {} 渠道卸载，但检测到另一渠道的安装仍存在；如需一并移除请再次卸载",
                    backend.display_name(),
                    install_source.as_deref().unwrap_or("script"),
                ),
            );
        }
        Ok(status)
    }

    /// 官方登出：运行 CLI 的登出子命令（codex `logout` / claude `auth logout` /
    /// kimi `provider remove managed:kimi-code`——已实测等效登出，见 lifecycle.rs）。
    /// 不重启会话：中转会话不受影响；官方登录态的会话在下次 spawn 时会按
    /// 未认证处理。
    pub async fn logout_acp_agent(&self, agent: &str) -> Result<CodexAcpStatus> {
        let backend = AgentBackend::parse(Some(agent))?;
        if !backend.is_acp() {
            bail!("Agent 不是 ACP 后端: {agent}");
        }
        let args = providers::lifecycle::logout_args(backend).with_context(|| {
            format!(
                "{} 的 CLI 不支持非交互登出（可在终端内使用其 TUI 的 /logout）",
                backend.display_name()
            )
        })?;
        let executable = self
            .login_executable(backend)
            .with_context(|| format!("未检测到可用的 {} CLI", backend.display_name()))?;
        let operation_id = diagnostics::operation_id("logout");
        diagnostics::write(
            &operation_id,
            "logout:spawn",
            format!("agent={agent} executable={}", executable.display()),
        );
        tokio::task::spawn_blocking(move || {
            let mut command = crate::platform::process::external_command(&executable);
            command.args(&args);
            let status = command
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .with_context(|| format!("启动登出命令失败: {}", executable.display()))?;
            if !status.success() {
                anyhow::bail!(
                    "登出命令失败: {}",
                    status
                        .code()
                        .map_or_else(|| "信号终止".to_string(), |code| code.to_string())
                );
            }
            Ok::<(), anyhow::Error>(())
        })
        .await
        .context("登出任务异常退出")??;
        self.invalidate_auth_cache(backend);
        Ok(self.status_for_async(backend).await)
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
        self.invalidate_auth_cache(backend);
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
        let prepared = prepare_codex_prompt(
            &content,
            &attachments,
            &workspace_references,
            &runtime.prompt_capabilities,
        )?;
        begin_prompt(&runtime.busy, &runtime.configuring)?;
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
        // 下一次状态读取需要补充真实安装来源以选择正确升级路径；正常首屏不为
        // 这个低频分支预付 npm/brew 探测成本。
        self.runtime_probe.write().initialized = false;
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
        let mut info = self
            .get_or_spawn(session_id)
            .await?
            .info(pending_permissions, pending_elicitations);
        info.provider = self
            .agents
            .get(session_id)
            .acp_config_values
            .get("provider")
            .cloned();
        Ok(info)
    }

    /// 一次性模型探针：切换/删除 Provider（或恢复官方）后，草稿态会话本不连接
    /// ACP；这里用一个即用即弃的 pinvou 会话真实 spawn 一次，借 session/new 的
    /// 上报拿到新 Provider 的真实模型/配置列表，供前端刷新草稿快照。探针是
    /// 草稿懒加载的唯一例外：拿到结果后立即 evict 进程、删除 store 记录并清掉
    /// 临时工作区目录；即使失败也尽力清理，错误原样透传（前端回退到 reseed
    /// 占位快照）。
    pub async fn probe_agent_model_options(&self, agent: &str) -> Result<CodexAcpSessionInfo> {
        let backend = AgentBackend::parse(Some(agent))?;
        if !backend.is_acp() {
            bail!("Agent 不是 ACP 后端: {agent}");
        }
        // 探针会话 id 必须唯一（同进程可多次探测），且只含合法 session id 字符
        // （execution_workspace 会校验）。
        static PROBE_SEQ: AtomicU64 = AtomicU64::new(0);
        let probe_id = format!(
            "model-probe-{}-{}-{}",
            agent,
            std::process::id(),
            PROBE_SEQ.fetch_add(1, Ordering::Relaxed),
        );
        // 临时工作区：spawn 时自动创建独立目录，不污染真实项目。
        self.agents
            .set_acp_workspace(&probe_id, backend, CodexWorkspaceKind::Temporary, None)?;
        let result = self.session_info(&probe_id).await;
        // 无论成败都必须收口，不得留下运行中的探针进程或 store 残留记录；
        // 清理失败只告警，主结果（上报或原始错误）优先透传。
        self.evict(&probe_id).await;
        if let Err(error) = self.agents.remove(&probe_id) {
            eprintln!("[pinvou3-app] 清理模型探针会话记录失败（{probe_id}）: {error:#}");
        }
        let probe_dir = crate::platform::paths::sessions_root().join(&probe_id);
        match std::fs::remove_dir_all(&probe_dir) {
            Ok(()) => {}
            // 探针未走到 spawn（如 CLI 未安装）时目录本就不存在。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "[pinvou3-app] 清理模型探针临时目录失败（{}）: {error:#}",
                probe_dir.display()
            ),
        }
        result
    }

    pub fn timeline(&self, session_id: &str) -> Result<Vec<AcpEventEnvelope>> {
        if !self.is_acp(session_id) {
            bail!("当前会话不是 ACP 会话");
        }
        load_timeline(session_id)
    }

    pub(crate) fn web_timeline_page(
        &self,
        session_id: &str,
        after_seq: u64,
        cursor: Option<u64>,
        limit: usize,
        max_page_bytes: usize,
        max_event_bytes: usize,
    ) -> Result<WebAcpTimelineSlice> {
        if !self.is_acp(session_id) {
            bail!("当前会话不是 ACP 会话");
        }
        load_web_timeline_page(
            session_id,
            after_seq,
            cursor,
            limit,
            max_page_bytes,
            max_event_bytes,
        )
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
                self.configure_codex_provider_env(&mut command, pinvou_session_id)?;
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

    /// Codex 的 Provider key 注入：config.toml 只支持 env_key 引用，实际 key 在
    /// spawn 时注入子进程 env。进程 env 已设置时优先（用户显式配置），不覆盖。
    fn configure_codex_provider_env(
        &self,
        command: &mut Command,
        pinvou_session_id: &str,
    ) -> Result<()> {
        if std::env::var_os("OPENAI_API_KEY").is_some_and(|value| !value.is_empty()) {
            return Ok(());
        }
        let Some(key) = self.session_provider_api_key(pinvou_session_id)? else {
            return Ok(());
        };
        command.env("OPENAI_API_KEY", key);
        eprintln!("[pinvou3-app] injected OPENAI_API_KEY for codex session {pinvou_session_id}");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_native_workspace_info_returns_temporary_execution_workspace() {
        // 隔离目录，不触碰真实 ~/.pinvou3（评审测试建议）
        let boot_root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-info-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&boot_root);
        let session_store = crate::features::sessions::SessionStore::boot_at_test_dir(&boot_root)
            .expect("session store");
        let record = SessionAgentRecord {
            mode: SessionMode::Code,
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
        let _ = std::fs::remove_dir_all(&boot_root);
    }

    #[test]
    fn code_native_workspace_info_project_branch_tracks_directory_availability() {
        // 隔离目录，不触碰真实 ~/.pinvou3（评审测试建议）
        let boot_root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-project-store-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&boot_root);
        let session_store = crate::features::sessions::SessionStore::boot_at_test_dir(&boot_root)
            .expect("session store");
        let root = std::env::temp_dir().join(format!(
            "pinvou3-code-native-project-info-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let record = SessionAgentRecord {
            mode: SessionMode::Code,
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
            mode: SessionMode::Code,
            workspace_kind: CodexWorkspaceKind::Project,
            workspace_path: None,
            ..SessionAgentRecord::default()
        };
        assert!(
            code_native_workspace_info(&session_store, "code-native-proj-test", &broken).is_err()
        );
        let _ = std::fs::remove_dir_all(&boot_root);
    }

    #[test]
    fn acp_agent_catalog_is_complete_and_stable() {
        let catalog = AcpPool::agent_catalog();
        assert_eq!(
            catalog,
            vec![
                AcpAgentDescriptor {
                    agent_id: "codex",
                    agent_name: "Codex",
                },
                AcpAgentDescriptor {
                    agent_id: "claude",
                    agent_name: "Claude Code",
                },
                AcpAgentDescriptor {
                    agent_id: "kimi",
                    agent_name: "Kimi",
                },
            ]
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

    /// 官方脚本目标路径的坏残留判定：非空文件存在但探测未解析到它（或解析到
    /// 别的路径）都算残留；探测解析到同一路径、空目录、目标不存在则不算。
    #[test]
    fn stale_official_target_detects_broken_residual() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-acp-stale-target-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let target = root.join("claude");
        std::fs::write(&target, "not a real binary").unwrap();
        // 探测没解析到任何路径 → 坏残留。
        assert!(stale_official_target(&target, None));
        // 探测解析到另一份拷贝 → 该路径文件仍是不可用残留。
        assert!(stale_official_target(
            &target,
            Some(Path::new("/elsewhere/claude"))
        ));
        // 探测解析到同一路径（版本可用）→ 正常安装，不拦截。
        assert!(!stale_official_target(&target, Some(&target)));
        // 0 字节半成品（下载中断）→ 坏残留。
        let empty = root.join("kimi");
        std::fs::write(&empty, "").unwrap();
        assert!(stale_official_target(&empty, None));
        // 目录占据文件路径（脚本失败遗留）→ 挡住脚本写入，坏残留。
        let dir = root.join("codex");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(stale_official_target(&dir, None));
        // 目标不存在 → 全新安装，不拦截。
        assert!(!stale_official_target(&root.join("absent"), None));

        std::fs::remove_dir_all(&root).unwrap();
    }
}
