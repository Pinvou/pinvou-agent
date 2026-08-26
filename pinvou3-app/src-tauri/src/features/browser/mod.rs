//! 浏览器功能模块：管理 Agent 与用户共享的原生浏览器会话、导航与多标签页控制。
//! 用户展示链路只使用主窗口内的系统 WebView 子视图；原生表面不可用时显式报错，
//! 不启动连续截图流或外部浏览器作为浏览回退。
//!
//! 与 MCP wrapper（`bundle/mcp-servers/browser-wrapper.mjs`）通过
//! `~/.pinvou3/browser/cdp-port.json` 协调同一浏览器实例。Windows 上 wrapper 先写
//! `host-requests/*.json` 请求主应用按对话创建 WebView2，再让 chrome-devtools-mcp 连接其
//! 回环 CDP 端口。CDP 只承担 Windows Agent 自动化，不承担用户画面传输。
//!
//! 端范围：**本期仅桌面端**。`browser:*` 事件仅本地 `emit`，不转发远端 WebUI
//! （relay 的 `access-policy.json` 白名单不含任何 `browser:*` 事件/命令，
//! 转发只会被拒绝并刷日志）——web/移动端暂不提供浏览器 Tab 与交互
//! （"三端共享"为后续迭代项，勿在文档中宣称已支持）。

mod cdp;
mod core;
mod platform;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::platform::paths;
use platform::state::{
    NativeRequestCancel, NativeRequestClaim, NativeTabLease, RetainedAgentOperation,
};

pub use cdp::CdpSession;

/// Must run before Tauri creates any WebKit context.
pub(crate) fn prepare_process_environment() {
    platform::prepare_process_environment();
}

/// Installs platform automation state during Tauri setup, before BrowserManager
/// can create a task-owned browser child WebView.
pub(crate) fn install_automation_context(app: &mut tauri::App) {
    platform::install_automation_context(app);
}

#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSurfaceBounds {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl NativeSurfaceBounds {
    fn validate(self) -> Result<Self, String> {
        const MAX_COORDINATE: i32 = 32_768;
        const MAX_SIZE: i32 = 16_384;
        if !(-MAX_COORDINATE..=MAX_COORDINATE).contains(&self.x)
            || !(-MAX_COORDINATE..=MAX_COORDINATE).contains(&self.y)
            || !(1..=MAX_SIZE).contains(&self.width)
            || !(1..=MAX_SIZE).contains(&self.height)
        {
            return Err("原生浏览器区域超出有效范围".to_string());
        }
        Ok(self)
    }
}

/// 标签页身份（targetId）→ flatten sessionId 缓存。用于自动化连接内复用 attach，
/// 并在 target 生命周期变化时自愈当前激活状态；不作为 UI 事件作用域来源。
type PageSessions = Arc<parking_lot::Mutex<HashMap<String, String>>>;
type BrowserSessionValidator = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// 单个页面标签页。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TabInfo {
    pub target_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_id: Option<u64>,
    pub title: String,
    pub url: String,
}

#[derive(Default)]
struct Inner {
    port: Option<u16>,
    /// browser 级 CDP 会话（一条连接管所有标签页）。
    session: Option<Arc<CdpSession>>,
    /// 当前激活标签页的 flatten sessionId。
    active_session: Option<String>,
    /// 当前激活标签页的 targetId（与 active_session 同步维护）。对外（status /
    /// 事件 payload）的标签页身份一律用 targetId——sessionId 是每次 attach 的
    /// 产物、同一标签页每次 attach 都不同，不能作为身份。
    active_target: Option<String>,
    /// 事件循环任务句柄（防重复启动/可中止）。
    loop_task: Option<tokio::task::JoinHandle<()>>,
    /// CDP WebSocket 读循环任务句柄（stop/崩溃重置时可中止，防读循环残留）。
    reader_task: Option<tokio::task::JoinHandle<()>>,
}

/// 浏览器管理器（Tauri State 注入，单例）。
pub struct BrowserManager {
    inner: tokio::sync::Mutex<Inner>,
    /// 启动临界区互斥：串行化整个启动序列（协调浏览器 → 自动化连接 → attach →
    /// 事件循环），避免 watch 轮询与 Tauri 命令并发进入产生双事件循环或句柄
    /// 丢失（single-flight）。stop() 也参与本锁，
    /// 保证 stop 不会在启动序列中途"看到空状态提前返回"而被启动方随后覆盖。
    start_mtx: tokio::sync::Mutex<()>,
    /// 停止代际计数：stop() 每次 +1；ensure_started 启动前记录、完成后核对，
    /// 启动期间被 stop 打断时丢弃本次启动结果（避免 stop 被吞、浏览器残留）。
    stop_gen: std::sync::atomic::AtomicU64,
    /// "已向前端 emit 过 browser:activated"标记（watch 与 stop 共享）：
    /// stop()/崩溃路径置 false，保证再次接入时必重新 emit（前端 Tab 重现）。
    activated: std::sync::atomic::AtomicBool,
    /// 主进程退出标记：shutdown_on_exit 置位后 watch 退出、ensure_started 拒绝——
    /// 防止退出瞬间 watch 重新拉起 Chrome 成为无人回收的孤儿进程。
    shutting_down: std::sync::atomic::AtomicBool,
    /// 宿主签发 renderer generation，并在同 generation 内只接受递增 sequence。
    /// renderer/HMR 重载后先取得新 generation，旧 renderer 的迟到 show/hide 即使
    /// sequence 更大也无法覆盖新任务。该锁覆盖原生 visibility mutation 的提交点。
    surface_visibility: parking_lot::Mutex<SurfaceVisibilityClock>,
    /// target_id → flatten sessionId 缓存：同一标签页复用 attach。CDP 对同一
    /// target 的每次 attach 都产生独立 session 且不自动释放，无缓存会在高频
    /// 枚举/切换下无界泄漏 Chrome 侧 session。
    page_sessions: PageSessions,
    app: parking_lot::Mutex<Option<AppHandle>>,
    /// Session 生命周期由 composition root 注入窄校验器；browser feature 不依赖
    /// sessions sibling。删除事件先写本地 deny tombstone，再异步销毁 WebView/文件。
    session_validator: parking_lot::Mutex<Option<BrowserSessionValidator>>,
    deleted_session_ids: parking_lot::RwLock<HashSet<String>>,
    /// 原生 mutation 已物理提交后，恢复清单/权威映射的 I/O 失败不能把操作伪装成
    /// 失败。这里保留按任务可见的降级状态，并由单任务退避队列持续修复。
    /// 恢复点写入和告警/worker 状态提交必须处于同一临界区，避免旧 worker 成功退出时
    /// 吞掉刚发生的新失败，留下“有告警但无人重试”的状态。
    persistence_io: parking_lot::Mutex<()>,
    persistence_warnings: parking_lot::Mutex<HashMap<String, String>>,
    persistence_retries: parking_lot::Mutex<HashSet<String>>,
    /// A durable Prepare journal is recovered synchronously before transient
    /// host requests are reset. Any recovery failure blocks every browser
    /// entry point so a stale restore manifest cannot be published first.
    prepare_recovery_error: parking_lot::Mutex<Option<String>>,
    /// Only commits produced by this process may use disappearance of its
    /// transient request/response artifacts as a success acknowledgement.
    /// Recovered commits are deliberately absent: process-start reset also
    /// makes those artifacts disappear and must not be mistaken for wrapper ACK.
    locally_committed_prepares: parking_lot::Mutex<HashSet<(String, String)>>,
    /// 启动孤儿清理只允许删除这个时刻之前已存在的文件。本进程在静态 session
    /// 快照之后并发创建的 restore/workspace/mcp 文件即使不在快照中也绝不命中。
    startup_reconcile_cutoff: SystemTime,
    /// 三端原生浏览器承载状态。平台细节封装在 feature 内的 platform 适配层；
    /// 不支持的平台显式返回 unsupported，不切换到截图或外部浏览器。
    native_surface: parking_lot::Mutex<platform::NativeBrowserSurface>,
}

#[derive(Debug, serde::Deserialize)]
struct HostedBrowserRequest {
    protocol_version: u8,
    request_id: String,
    idempotency_key: String,
    session_id: String,
    session_token: String,
    caller_pid: u32,
    wrapper_instance_nonce: String,
    operation: HostedBrowserOperation,
    requested_at: u64,
    tab_token: Option<String>,
    authorization_tab_token: Option<String>,
    creation_id: Option<String>,
    url: Option<String>,
    target_id: Option<String>,
    revision: Option<u64>,
    lease: Option<String>,
    tool_name: Option<String>,
    tool_arguments: Option<Value>,
    #[serde(default)]
    background: bool,
    #[serde(default)]
    emits_trusted_input: bool,
}

#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum HostedBrowserOperation {
    Prepare,
    CreateTab,
    ActivateTab,
    CloseTab,
    RollbackCreatedTab,
    AssertHostLease,
    BeginAgentOperation,
    RefreshAgentOperation,
    RefreshAgentInput,
    EndAgentOperation,
    CoreTool,
}

impl HostedBrowserOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::CreateTab => "create_tab",
            Self::ActivateTab => "activate_tab",
            Self::CloseTab => "close_tab",
            Self::RollbackCreatedTab => "rollback_created_tab",
            Self::AssertHostLease => "assert_host_lease",
            Self::BeginAgentOperation => "begin_agent_operation",
            Self::RefreshAgentOperation => "refresh_agent_operation",
            Self::RefreshAgentInput => "refresh_agent_input",
            Self::EndAgentOperation => "end_agent_operation",
            Self::CoreTool => "core_tool",
        }
    }

    /// Control-plane requests must never queue behind a different session's
    /// slow prepare/CDP/BrowserCore operation. They touch only the in-memory
    /// lease state and are serviced by a dedicated lightweight scanner.
    const fn is_control_plane(self) -> bool {
        matches!(
            self,
            Self::AssertHostLease
                | Self::BeginAgentOperation
                | Self::RefreshAgentOperation
                | Self::RefreshAgentInput
                | Self::EndAgentOperation
        )
    }

    /// Match the wrapper's externally observable request deadlines. A request
    /// that could no longer have a live caller must not acquire fresh Agent
    /// authority merely because a crashed wrapper left its artifact behind.
    const fn maximum_artifact_age_ms(self) -> u64 {
        match self {
            Self::Prepare | Self::CoreTool => 25_000,
            _ => 12_000,
        }
    }

    /// Cleanup requests must remain executable after their wrapper epoch has
    /// died. Every other request can grant authority or touch an external
    /// browser surface, and therefore requires a live matching caller epoch.
    const fn requires_live_caller(self) -> bool {
        !matches!(self, Self::EndAgentOperation | Self::RollbackCreatedTab)
    }
}

#[derive(Debug, serde::Deserialize)]
struct HostedBrowserCancellation {
    protocol_version: u8,
    kind: String,
    request_id: String,
    idempotency_key: String,
    session_id: String,
    session_token: String,
    caller_pid: u32,
    wrapper_instance_nonce: String,
    #[serde(default)]
    prepare_compensation: Option<HostedPrepareCompensation>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum HostedPreparePhase {
    Pending,
    Prepared,
    Committed,
    Cancelled,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct HostedPrepareCompensation {
    protocol_version: u8,
    kind: String,
    request_id: String,
    idempotency_key: String,
    session_id: String,
    session_token: String,
    caller_pid: u32,
    wrapper_instance_nonce: String,
    rollback_kind: String,
    revision: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HostedPrepareJournal {
    protocol_version: u8,
    kind: String,
    phase: HostedPreparePhase,
    compensation: HostedPrepareCompensation,
    requested_at: u64,
    updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    response: Option<Value>,
}

#[derive(Debug, serde::Deserialize)]
struct HostedCallerHeartbeat {
    protocol_version: u8,
    kind: String,
    session_id: String,
    session_token: String,
    caller_pid: u32,
    wrapper_instance_nonce: String,
    heartbeat_at: u64,
}

struct HostedBrowserOutcome {
    result: Value,
    error: Option<String>,
    /// Serialized compensation metadata retained in the in-memory request
    /// ledger. A cancellation that arrives after completion uses it to undo
    /// only resources created by this request.
    rollback: Value,
}

impl HostedBrowserOutcome {
    fn new(result: Value) -> Self {
        Self {
            result,
            error: None,
            rollback: json!({ "kind": "none" }),
        }
    }

    fn with_rollback(result: Value, rollback: Value) -> Self {
        Self {
            result,
            error: None,
            rollback,
        }
    }

    fn failed_with_rollback(error: String, rollback: Value) -> Self {
        Self {
            result: Value::Null,
            error: Some(error),
            rollback,
        }
    }
}

#[derive(Debug)]
struct PrepareWorkspaceError {
    message: String,
    rollback: Option<Value>,
}

impl PrepareWorkspaceError {
    fn compensated(message: String, rollback: Value) -> Self {
        Self {
            message,
            rollback: Some(rollback),
        }
    }
}

impl From<String> for PrepareWorkspaceError {
    fn from(message: String) -> Self {
        Self {
            message,
            rollback: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreWorkspaceOutcome {
    Missing,
    Existing,
    Restored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreparedWorkspaceDisposition {
    Existing,
    CreatedBlank,
    RestoredExisting,
}

fn has_restore_automation_backend(
    capabilities: platform::NativeSurfaceCapabilities,
    browser_core_available: bool,
) -> bool {
    capabilities.chrome_devtools_protocol || browser_core_available
}

impl PreparedWorkspaceDisposition {
    const fn rollback_kind(self) -> Option<&'static str> {
        match self {
            Self::Existing => None,
            Self::CreatedBlank => Some("prepared_session"),
            Self::RestoredExisting => Some("restored_session"),
        }
    }
}

fn hosted_prepare_rollback_record(
    disposition: PreparedWorkspaceDisposition,
    session_id: &str,
    request_id: &str,
    revision: u64,
) -> Option<Value> {
    disposition.rollback_kind().map(|kind| {
        json!({
            "kind": kind,
            "session_id": session_id,
            "request_id": request_id,
            "revision": revision,
        })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopedStopAction {
    CloseNativeSession,
    IgnoreUnknownNativeSession,
    StopManagedRuntime,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct SurfaceVisibilityClock {
    generation: u64,
    sequence: u64,
}

impl SurfaceVisibilityClock {
    fn begin_generation(&mut self) -> u64 {
        self.generation = self.generation.saturating_add(1).max(1);
        self.sequence = 0;
        self.generation
    }

    fn claim(&mut self, generation: u64, sequence: u64) -> bool {
        if generation == 0
            || generation != self.generation
            || sequence == 0
            || sequence <= self.sequence
        {
            return false;
        }
        self.sequence = sequence;
        true
    }
}

/// 原生工作区可按对话关闭。只要注册表中还有任意工作区，未知 session 都必须按幂等
/// 成功处理，绝不能退化成全局 stop 误伤其他对话；注册表为空时则清理共享自动化运行时。
fn scoped_stop_action(requested_exists: bool, has_native_sessions: bool) -> ScopedStopAction {
    if requested_exists {
        ScopedStopAction::CloseNativeSession
    } else if has_native_sessions {
        ScopedStopAction::IgnoreUnknownNativeSession
    } else {
        ScopedStopAction::StopManagedRuntime
    }
}

impl BrowserManager {
    pub fn new() -> Self {
        Self {
            inner: tokio::sync::Mutex::new(Inner::default()),
            start_mtx: tokio::sync::Mutex::new(()),
            stop_gen: std::sync::atomic::AtomicU64::new(0),
            activated: std::sync::atomic::AtomicBool::new(false),
            shutting_down: std::sync::atomic::AtomicBool::new(false),
            surface_visibility: parking_lot::Mutex::new(SurfaceVisibilityClock::default()),
            page_sessions: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            app: parking_lot::Mutex::new(None),
            session_validator: parking_lot::Mutex::new(None),
            deleted_session_ids: parking_lot::RwLock::new(HashSet::new()),
            persistence_io: parking_lot::Mutex::new(()),
            persistence_warnings: parking_lot::Mutex::new(HashMap::new()),
            persistence_retries: parking_lot::Mutex::new(HashSet::new()),
            prepare_recovery_error: parking_lot::Mutex::new(None),
            locally_committed_prepares: parking_lot::Mutex::new(HashSet::new()),
            startup_reconcile_cutoff: SystemTime::now(),
            native_surface: parking_lot::Mutex::new(platform::NativeBrowserSurface::default()),
        }
    }

    pub(crate) fn bind_session_validator(&self, validator: BrowserSessionValidator) {
        *self.session_validator.lock() = Some(validator);
    }

    /// Composition root 在收到删除事件后、开始任何异步清理前调用。之后所有迟到的
    /// wrapper 请求都 fail-closed；实际 WebView/restore/MCP 文件清理可独立重试。
    pub(crate) fn mark_session_deleted(&self, session_id: &str) {
        self.deleted_session_ids
            .write()
            .insert(session_id.to_string());
    }

    fn ensure_browser_session_allowed(&self, session_id: &str) -> Result<(), String> {
        if let Some(error) = self.prepare_recovery_error.lock().clone() {
            if error.starts_with("browser/host-consumer-unavailable:") {
                return Err(error);
            }
            return Err(format!(
                "browser/prepare-recovery-pending: 持久 Prepare 补偿尚未完成: {error}"
            ));
        }
        if self.deleted_session_ids.read().contains(session_id) {
            return Err("任务已删除；拒绝迟到的浏览器宿主请求".to_string());
        }
        let validator = self
            .session_validator
            .lock()
            .clone()
            .ok_or_else(|| "任务生命周期校验器尚未就绪".to_string())?;
        if !validator(session_id) {
            return Err("任务不存在；拒绝孤儿浏览器宿主请求".to_string());
        }
        Ok(())
    }

    /// 每个 renderer 生命周期先向宿主申请 generation；generation 只在 Rust 进程内
    /// 单调递增，不依赖会因 HMR/崩溃归零的 JS 模块变量。
    pub fn begin_surface_generation(&self) -> u64 {
        self.surface_visibility.lock().begin_generation()
    }

    /// 尝试显示当前平台的系统原生 WebView 浏览器表面。返回 false 表示原生表面
    /// 尚未创建；前端会显示错误与重试，不切换到其他显示链路。
    pub async fn show_native_surface(
        &self,
        window: &tauri::Window,
        session_id: &str,
        bounds: NativeSurfaceBounds,
        visibility_generation: u64,
        visibility_sequence: u64,
    ) -> Result<bool, String> {
        let bounds = bounds.validate()?;
        let mut visibility = self.surface_visibility.lock();
        if !visibility.claim(visibility_generation, visibility_sequence) {
            return Ok(false);
        }
        // visibility 锁一直持有到原生 mutation 提交，确保新的 generation 签发不会
        // 夹在 claim 与 show 之间，从而让旧 renderer 在签发之后才落地。
        self.native_surface.lock().show(window, session_id, bounds)
    }

    pub fn hide_native_surface(
        &self,
        session_id: &str,
        visibility_generation: u64,
        visibility_sequence: u64,
    ) -> Result<(), String> {
        let mut visibility = self.surface_visibility.lock();
        if !visibility.claim(visibility_generation, visibility_sequence) {
            return Ok(());
        }
        let app = self.app.lock().clone();
        self.native_surface
            .lock()
            .hide(app.as_ref(), Some(session_id))
    }

    async fn prepare_requested_native_surfaces(&self, app: &AppHandle) -> Result<bool, String> {
        self.prepare_requested_native_surfaces_filtered(app, false)
            .await
    }

    async fn prepare_requested_native_control_requests(
        &self,
        app: &AppHandle,
    ) -> Result<bool, String> {
        self.prepare_requested_native_surfaces_filtered(app, true)
            .await
    }

    async fn prepare_requested_native_surfaces_filtered(
        &self,
        app: &AppHandle,
        control_only: bool,
    ) -> Result<bool, String> {
        let dir = paths::browser_host_requests_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(false);
        };
        let mut cancellation_paths = Vec::new();
        let mut request_paths = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            match path.extension().and_then(|value| value.to_str()) {
                Some("cancelled") => cancellation_paths.push(path),
                Some("json") => request_paths.push(path),
                _ => {}
            }
        }
        cancellation_paths.sort();
        request_paths.sort();

        let mut handled = false;
        let mut errors = Vec::new();
        let mut blocked_requests = HashSet::new();
        // Tombstones win over requests even when both artifacts were already
        // present when the watcher woke. This prevents a timed-out wrapper
        // call from creating a late workspace or tab.
        for cancellation_path in cancellation_paths {
            if control_only {
                continue;
            }
            // 同 stem 请求本轮一律不能越过 tombstone；即使补偿失败，也只能保留
            // 两份 artifact 等待重试，不能继续执行原始 create/close。
            blocked_requests.insert(cancellation_path.with_extension("json"));
            // 补偿失败必须保留 tombstone + ledger record，并由 watcher 显式重试；
            // 删除文件会把瞬时 WebView/I/O 失败永久变成资源泄漏。
            if let Err(error) = self
                .process_hosted_cancellation(app, &cancellation_path)
                .await
            {
                errors.push(format!("{}: {error}", cancellation_path.display()));
            }
            handled = true;
        }

        for request_path in request_paths {
            if blocked_requests.contains(&request_path) {
                continue;
            }
            let raw = match std::fs::read_to_string(&request_path) {
                Ok(raw) => raw,
                Err(error) => {
                    eprintln!("[browser] 读取浏览器宿主请求失败: {error}");
                    let _ = std::fs::remove_file(&request_path);
                    continue;
                }
            };
            let request = match serde_json::from_str::<HostedBrowserRequest>(&raw) {
                Ok(request) => request,
                Err(error) => {
                    eprintln!("[browser] 浏览器宿主请求格式无效: {error}");
                    let _ = std::fs::remove_file(&request_path);
                    continue;
                }
            };
            if control_only && !request.operation.is_control_plane() {
                continue;
            }
            if let Err(error) = validate_hosted_request(&request, &request_path) {
                eprintln!("[browser] 浏览器宿主请求身份无效: {error}");
                let _ = std::fs::remove_file(&request_path);
                handled = true;
                continue;
            }
            if let Err(error) = self.ensure_browser_session_allowed(&request.session_id) {
                let response = hosted_response(&request, Err(error));
                if let Err(error) = write_hosted_response(&request_path, &response) {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
                let _ = std::fs::remove_file(&request_path);
                handled = true;
                continue;
            }

            // Claiming creates the in-memory idempotency generation. Re-read
            // the epoch immediately at that boundary instead of relying only
            // on the earlier artifact/schema validation.
            if let Err(error) = ensure_hosted_caller_live(&request) {
                let response = hosted_response(&request, Err(error));
                if let Err(error) = write_hosted_response(&request_path, &response) {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
                let _ = std::fs::remove_file(&request_path);
                handled = true;
                continue;
            }

            let claim = self
                .native_surface
                .lock()
                .claim_request(&request.session_id, &request.request_id);
            let claim = match claim {
                Ok(claim) => claim,
                Err(error) => {
                    let response = hosted_response(&request, Err(error));
                    if let Err(error) = write_hosted_response(&request_path, &response) {
                        errors.push(format!("{}: {error}", request_path.display()));
                        continue;
                    }
                    let _ = std::fs::remove_file(&request_path);
                    handled = true;
                    continue;
                }
            };
            match claim {
                NativeRequestClaim::Canceled => {
                    let _ = std::fs::remove_file(&request_path);
                    handled = true;
                    continue;
                }
                NativeRequestClaim::Replay(record) => {
                    if request_path.with_extension("cancelled").exists() {
                        if let Err(error) = self
                            .process_hosted_cancellation(
                                app,
                                &request_path.with_extension("cancelled"),
                            )
                            .await
                        {
                            errors.push(format!("{}: {error}", request_path.display()));
                            continue;
                        }
                    } else if let Some(response) = record.get("response") {
                        if let Err(error) = write_hosted_response(&request_path, response) {
                            errors.push(format!("{}: {error}", request_path.display()));
                            continue;
                        }
                    }
                    let _ = std::fs::remove_file(&request_path);
                    handled = true;
                    continue;
                }
                NativeRequestClaim::InFlight => {
                    // A dedicated control-plane scanner may observe the same
                    // artifact after the data-plane worker has claimed it (or
                    // vice versa). The claimant still owns the response path;
                    // never remove that shared artifact here.
                    continue;
                }
                NativeRequestClaim::Execute => {}
            }

            let cancellation_path = request_path.with_extension("cancelled");
            let cancelled_before_execution = if cancellation_path.exists() {
                if let Err(error) = self
                    .process_hosted_cancellation(app, &cancellation_path)
                    .await
                {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
                true
            } else {
                false
            };

            // A wrapper can cancel after this process has claimed the request
            // but while BrowserCore is resolving DOM state or waiting for an
            // already-dispatched native action. Revoke authorization as soon
            // as the tombstone wins, but keep polling the core future to its
            // bounded real settlement. Dropping the HTTP/WebKit future would
            // release the operation gate while the platform could still commit
            // the old action, allowing a newer mutation to overtake it.
            let mut core_cancellation_needs_compensation = false;
            let outcome = if cancelled_before_execution {
                core_cancellation_needs_compensation =
                    matches!(request.operation, HostedBrowserOperation::CoreTool);
                Err("browser/request-cancelled".to_string())
            } else if matches!(request.operation, HostedBrowserOperation::CoreTool) {
                let request_future = self.handle_hosted_browser_request(app, &request);
                tokio::pin!(request_future);
                let raced_outcome = tokio::select! {
                    biased;
                    _ = wait_for_hosted_cancellation(&cancellation_path) => None,
                    outcome = &mut request_future => Some(outcome),
                };
                match raced_outcome {
                    Some(outcome) => outcome,
                    None => {
                        core_cancellation_needs_compensation = true;
                        let revoke_error = self
                            .native_surface
                            .lock()
                            .cancel_in_flight_core_request(
                                Some(app),
                                &request.session_id,
                                &request.request_id,
                            )
                            .err();
                        // The platform request is bounded (including its own
                        // transport timeout). Its result is intentionally not
                        // published after cancellation; awaiting it only keeps
                        // mutation ordering and commit semantics truthful.
                        let _settled_outcome = request_future.await;
                        match revoke_error {
                            Some(error) => Err(format!(
                                "browser/request-cancelled; immediate compensation failed: {error}"
                            )),
                            None => Err("browser/request-cancelled".to_string()),
                        }
                    }
                }
            } else {
                self.handle_hosted_browser_request(app, &request).await
            };
            let (response, rollback) = match outcome {
                Ok(outcome) => {
                    let response = match outcome.error {
                        Some(error) => hosted_response(&request, Err(error)),
                        None => hosted_response(&request, Ok(outcome.result)),
                    };
                    (response, outcome.rollback)
                }
                Err(error) => {
                    let rollback = if core_cancellation_needs_compensation {
                        json!({
                            "kind": "cancelled_core_request",
                            "session_id": request.session_id,
                            "request_id": request.request_id,
                        })
                    } else {
                        json!({ "kind": "none" })
                    };
                    (hosted_response(&request, Err(error)), rollback)
                }
            };
            let record = json!({ "response": response, "rollback": rollback });

            // A timeout can arrive while prepare is waiting for WebView2/CDP.
            // Re-check immediately before committing the result to the ledger.
            if cancellation_path.exists() {
                if let Err(error) = self
                    .process_hosted_cancellation(app, &cancellation_path)
                    .await
                {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
            }
            let committed = match self.native_surface.lock().complete_request(
                &request.session_id,
                &request.request_id,
                record.clone(),
            ) {
                Ok(committed) => committed,
                Err(error) => {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
            };
            if !committed {
                if let Err(error) = self.rollback_hosted_record(app, &record).await {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
                if let Err(error) = self
                    .native_surface
                    .lock()
                    .acknowledge_request_cancellation(&request.session_id, &request.request_id)
                {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
                if matches!(request.operation, HostedBrowserOperation::Prepare) {
                    if let Err(error) = remove_matching_hosted_prepare_journal_for_request(&request)
                    {
                        errors.push(format!("{}: {error}", request_path.display()));
                        continue;
                    }
                    self.locally_committed_prepares
                        .lock()
                        .remove(&(request.session_token.clone(), request.request_id.clone()));
                }
                if let Err(error) = remove_hosted_request_artifacts(&cancellation_path) {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
                let _ = std::fs::remove_file(&request_path);
                handled = true;
                continue;
            }

            // Close the final race between ledger commit and response write.
            // The wrapper also quarantines late responses, so either side of
            // this boundary produces at most one observable outcome.
            if cancellation_path.exists() {
                if let Err(error) = self
                    .process_hosted_cancellation(app, &cancellation_path)
                    .await
                {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
            } else {
                if let Err(error) = write_hosted_response(&request_path, &record["response"]) {
                    errors.push(format!("{}: {error}", request_path.display()));
                    continue;
                }
            }
            let _ = std::fs::remove_file(&request_path);
            handled = true;
        }
        if !control_only {
            if let Err(error) = reap_acknowledged_committed_prepare_journals(
                &mut self.locally_committed_prepares.lock(),
            ) {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(handled)
        } else {
            // watcher 会重排下一轮，但本轮其他 session 已经得到公平处理。
            Err(errors.join("; "))
        }
    }

    async fn process_hosted_cancellation(
        &self,
        app: &AppHandle,
        cancellation_path: &std::path::Path,
    ) -> Result<(), String> {
        let raw = std::fs::read_to_string(cancellation_path)
            .map_err(|error| format!("读取浏览器宿主取消记录失败: {error}"))?;
        let cancellation: HostedBrowserCancellation = serde_json::from_str(&raw)
            .map_err(|error| format!("浏览器宿主取消记录格式无效: {error}"))?;
        validate_hosted_cancellation(&cancellation, cancellation_path)?;
        let disposition = self
            .native_surface
            .lock()
            .cancel_request(&cancellation.session_id, &cancellation.request_id)?;
        match disposition {
            NativeRequestCancel::AlreadyCompleted(record) => {
                self.rollback_hosted_record(app, &record).await?;
                self.native_surface
                    .lock()
                    .acknowledge_request_cancellation(
                        &cancellation.session_id,
                        &cancellation.request_id,
                    )?;
                remove_matching_hosted_prepare_journal(&cancellation)?;
                self.locally_committed_prepares.lock().remove(&(
                    cancellation.session_token.clone(),
                    cancellation.request_id.clone(),
                ));
                remove_hosted_request_artifacts(cancellation_path)?;
            }
            // 请求仍在同一个串行 consumer 的执行路径中。保留 tombstone；执行方
            // 提交 rollback record 后会在上方 `!committed` 分支补偿并 ACK。
            NativeRequestCancel::AwaitingCompletion => {}
            NativeRequestCancel::Tombstoned | NativeRequestCancel::AlreadyCanceled => {
                let journal = matching_hosted_prepare_journal_for_cancellation(&cancellation)?;
                if let Some(journal) = journal.as_ref() {
                    self.rollback_prepare_journal(app, journal).await?;
                } else if let Some(compensation) = cancellation.prepare_compensation.as_ref() {
                    if let Some(rollback) = compensation.rollback_value() {
                        self.rollback_hosted_record(app, &json!({ "rollback": rollback }))
                            .await?;
                    }
                }
                remove_matching_hosted_prepare_journal(&cancellation)?;
                self.locally_committed_prepares.lock().remove(&(
                    cancellation.session_token.clone(),
                    cancellation.request_id.clone(),
                ));
                remove_hosted_request_artifacts(cancellation_path)?;
            }
        }
        Ok(())
    }

    async fn rollback_hosted_record(&self, app: &AppHandle, record: &Value) -> Result<(), String> {
        let rollback = &record["rollback"];
        let session_id = rollback
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match rollback.get("kind").and_then(Value::as_str) {
            Some(kind @ ("prepared_session" | "restored_session")) if !session_id.is_empty() => {
                let request_id = rollback
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let revision = rollback
                    .get("revision")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                if !request_id.is_empty() && revision > 0 {
                    self.rollback_prepared_session(
                        session_id,
                        request_id,
                        revision,
                        kind == "restored_session",
                    )
                    .await?;
                }
            }
            Some("created_tab") if !session_id.is_empty() => {
                let tab_token = rollback
                    .get("tab_token")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let creation_id = rollback
                    .get("creation_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !tab_token.is_empty() && !creation_id.is_empty() {
                    // false 是安全终态：用户接管/后续 mutation 已使 generation 失效，
                    // 页面必须保留，但 tombstone 可以 ACK；只有 Err 才需要重试。
                    let _ = self.native_surface.lock().rollback_created_tab(
                        Some(app),
                        session_id,
                        tab_token,
                        creation_id,
                    )?;
                    self.persist_native_restore_best_effort(session_id);
                }
            }
            Some("cancelled_core_request") if !session_id.is_empty() => {
                let request_id = rollback
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !request_id.is_empty() {
                    self.native_surface.lock().cancel_in_flight_core_request(
                        Some(app),
                        session_id,
                        request_id,
                    )?;
                    self.persist_native_restore_best_effort(session_id);
                }
            }
            Some("agent_control") if !session_id.is_empty() => {
                let activated_tab = rollback
                    .get("activated_tab")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let previous_tab = rollback
                    .get("previous_tab")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let revision = rollback
                    .get("revision")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                let previous_owner = match rollback.get("previous_owner").and_then(Value::as_str) {
                    Some("unclaimed") => Some(platform::state::NativeControlOwner::Unclaimed),
                    Some("agent") => Some(platform::state::NativeControlOwner::Agent),
                    Some("user") => Some(platform::state::NativeControlOwner::User),
                    _ => None,
                };
                if !activated_tab.is_empty() && !previous_tab.is_empty() && revision > 0 {
                    if let Some(previous_owner) = previous_owner {
                        // false is the safe terminal state: later user/Agent work
                        // superseded this activation generation, so preserve it.
                        let _ = self.native_surface.lock().rollback_agent_activation(
                            Some(app),
                            session_id,
                            activated_tab,
                            previous_tab,
                            revision,
                            previous_owner,
                        )?;
                        self.persist_native_restore_best_effort(session_id);
                    }
                }
            }
            Some("agent_input") => {
                if let Ok(lease) = native_lease_from_value(rollback) {
                    self.native_surface.lock().end_agent_operation(&lease);
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_hosted_browser_request(
        &self,
        app: &AppHandle,
        request: &HostedBrowserRequest,
    ) -> Result<HostedBrowserOutcome, String> {
        self.ensure_browser_session_allowed(&request.session_id)?;
        match request.operation {
            HostedBrowserOperation::Prepare => {
                ensure_hosted_caller_live(request)?;
                let prepared = self
                    .prepare_native_workspace(
                        app,
                        &request.session_id,
                        &request.session_token,
                        true,
                        Some(&request.request_id),
                        Some(request),
                    )
                    .await;
                let (result, disposition) = match prepared {
                    Ok(prepared) => prepared,
                    Err(error) => match error.rollback {
                        Some(rollback) => {
                            return Ok(HostedBrowserOutcome::failed_with_rollback(
                                error.message,
                                rollback,
                            ));
                        }
                        None => return Err(error.message),
                    },
                };
                match disposition.rollback_kind() {
                    None => Ok(HostedBrowserOutcome::new(result)),
                    Some(kind) => {
                        let rollback_revision = self
                            .native_surface
                            .lock()
                            .prepare_generation_revision(&request.session_id, &request.request_id);
                        match rollback_revision {
                            Some(revision) => Ok(HostedBrowserOutcome::with_rollback(
                                result,
                                json!({
                                    "kind": kind,
                                    "session_id": request.session_id,
                                    "request_id": request.request_id,
                                    "revision": revision,
                                }),
                            )),
                            None => Ok(HostedBrowserOutcome::new(result)),
                        }
                    }
                }
            }
            HostedBrowserOperation::CreateTab => {
                let tab_token = request
                    .tab_token
                    .as_deref()
                    .ok_or_else(|| "新建标签页请求缺少 tab_token".to_string())?;
                let requested_url = request.url.as_deref().unwrap_or("about:blank");
                if !is_allowed_url(requested_url) {
                    return Err("仅支持 http/https/about:blank 协议".to_string());
                }
                let authorization = native_mutation_lease_from_request(request)?;
                // 先用唯一 marker 建立 WebView2 target；宿主发现并绑定 target 后在隐藏
                // 表面完成 requested_url 首航，再通过 lease CAS 发布，避免弹窗与错页导航。
                let marker = format!("about:blank#pinvou-tab-{tab_token}");
                ensure_hosted_caller_live(request)?;
                let created = self.native_surface.lock().create_tab_for_agent(
                    app,
                    &request.session_id,
                    tab_token,
                    &marker,
                    request.background,
                    &authorization,
                    &request.request_id,
                )?;
                if created.is_none() {
                    return Err("当前会话不是可自动化的原生浏览器工作区".to_string());
                }
                if let Err(error) = ensure_hosted_caller_live(request) {
                    return match self.rollback_staged_agent_tab(
                        app,
                        &request.session_id,
                        tab_token,
                        &request.request_id,
                    ) {
                        Ok(()) => Err(error),
                        Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
                    };
                }
                let target_id = match self
                    .bind_staged_native_target(app, &request.session_id, tab_token)
                    .await
                {
                    Ok(target_id) => target_id,
                    Err(error) => {
                        return match self.rollback_staged_agent_tab(
                            app,
                            &request.session_id,
                            tab_token,
                            &request.request_id,
                        ) {
                            Ok(()) => Err(error),
                            Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
                        };
                    }
                };
                if let Err(error) = ensure_hosted_caller_live(request) {
                    return match self.rollback_staged_agent_tab(
                        app,
                        &request.session_id,
                        tab_token,
                        &request.request_id,
                    ) {
                        Ok(()) => Err(error),
                        Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
                    };
                }
                if !self.native_surface.lock().commit_created_tab_for_agent(
                    app,
                    &request.session_id,
                    tab_token,
                    &target_id,
                    requested_url,
                    request.background,
                    &authorization,
                    &request.request_id,
                    None,
                    || ensure_hosted_caller_live(request),
                )? {
                    return Err("新建标签页在提交前已关闭".to_string());
                }
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": request.session_id, "tab": tab_token }),
                );
                self.persist_native_restore_best_effort(&request.session_id);
                let result = json!({
                    "tabToken": tab_token,
                    "targetId": target_id,
                    "creationId": request.request_id,
                });
                Ok(HostedBrowserOutcome::with_rollback(
                    result,
                    json!({
                        "kind": "created_tab",
                        "session_id": request.session_id,
                        "tab_token": tab_token,
                        "creation_id": request.request_id,
                    }),
                ))
            }
            HostedBrowserOperation::ActivateTab => {
                let tab_token = request
                    .tab_token
                    .as_deref()
                    .ok_or_else(|| "切换标签页请求缺少 tab_token".to_string())?;
                ensure_hosted_caller_live(request)?;
                let (previous_tab, previous_control, lease) = {
                    let mut surface = self.native_surface.lock();
                    let previous_tab = surface
                        .active_tab_token(&request.session_id)
                        .ok_or_else(|| "当前会话不是可自动化的原生浏览器工作区".to_string())?;
                    let previous_control = surface
                        .control_state(&request.session_id)
                        .ok_or_else(|| "当前会话不是可自动化的原生浏览器工作区".to_string())?;
                    let Some(lease) = surface.activate_tab_with_lease(
                        Some(app),
                        &request.session_id,
                        tab_token,
                    )?
                    else {
                        return Err("当前会话不是可自动化的原生浏览器工作区".to_string());
                    };
                    (previous_tab, previous_control, lease)
                };
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": request.session_id, "tab": tab_token }),
                );
                self.persist_native_restore_best_effort(&request.session_id);
                let result = lease.to_json_value()?;
                Ok(HostedBrowserOutcome::with_rollback(
                    result,
                    json!({
                        "kind": "agent_control",
                        "session_id": request.session_id,
                        "activated_tab": tab_token,
                        "previous_tab": previous_tab,
                        "previous_owner": previous_control.owner.as_str(),
                        "revision": lease.revision,
                    }),
                ))
            }
            HostedBrowserOperation::CloseTab => {
                let tab_token = request
                    .tab_token
                    .as_deref()
                    .ok_or_else(|| "关闭标签页请求缺少 tab_token".to_string())?;
                let authorization = native_mutation_lease_from_request(request)?;
                if authorization.tab_token != tab_token {
                    return Err("关闭标签的 authorization_tab_token 必须等于目标标签".to_string());
                }
                ensure_hosted_caller_live(request)?;
                if !self.native_surface.lock().close_tab_for_agent(
                    Some(app),
                    &request.session_id,
                    tab_token,
                    &authorization,
                )? {
                    return Err("当前会话不是可自动化的原生浏览器工作区".to_string());
                }
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": request.session_id }),
                );
                self.persist_native_restore_best_effort(&request.session_id);
                Ok(HostedBrowserOutcome::new(json!({})))
            }
            HostedBrowserOperation::RollbackCreatedTab => {
                let tab_token = request
                    .tab_token
                    .as_deref()
                    .ok_or_else(|| "创建补偿请求缺少 tab_token".to_string())?;
                let creation_id = request
                    .creation_id
                    .as_deref()
                    .ok_or_else(|| "创建补偿请求缺少 creation_id".to_string())?;
                if !valid_host_request_id(creation_id) {
                    return Err("创建补偿 generation 无效".to_string());
                }
                if !self.native_surface.lock().rollback_created_tab(
                    Some(app),
                    &request.session_id,
                    tab_token,
                    creation_id,
                )? {
                    return Err("待补偿的创建标签不存在".to_string());
                }
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": request.session_id }),
                );
                self.persist_native_restore_best_effort(&request.session_id);
                Ok(HostedBrowserOutcome::new(json!({})))
            }
            HostedBrowserOperation::AssertHostLease => {
                let lease = native_lease_from_request(request)?;
                ensure_hosted_caller_live(request)?;
                if !self.native_surface.lock().assert_lease(&lease)? {
                    return Err("浏览器宿主 lease 已失效；页面可能已被用户接管".to_string());
                }
                Ok(HostedBrowserOutcome::new(json!({})))
            }
            HostedBrowserOperation::BeginAgentOperation => {
                let lease = native_lease_from_request(request)?;
                ensure_hosted_caller_live(request)?;
                if !self.native_surface.lock().begin_agent_operation(
                    &lease,
                    request.emits_trusted_input,
                    request.caller_pid,
                    &request.wrapper_instance_nonce,
                )? {
                    return Err("浏览器宿主 lease 已失效；已阻止工具执行".to_string());
                }
                Ok(HostedBrowserOutcome::with_rollback(
                    json!({}),
                    json!({
                        "kind": "agent_input",
                        "session_id": lease.session_id,
                        "tab_token": lease.tab_token,
                        "target_id": lease.target_id,
                        "revision": lease.revision,
                        "lease": lease.lease,
                    }),
                ))
            }
            HostedBrowserOperation::RefreshAgentInput => {
                let lease = native_lease_from_request(request)?;
                ensure_hosted_caller_live(request)?;
                if !self.native_surface.lock().refresh_agent_input(&lease)? {
                    return Err(
                        "browser/agent-input-refresh-rejected: 工具操作已结束或 lease 已失效"
                            .to_string(),
                    );
                }
                Ok(HostedBrowserOutcome::new(json!({})))
            }
            HostedBrowserOperation::RefreshAgentOperation => {
                let lease = native_lease_from_request(request)?;
                ensure_hosted_caller_live(request)?;
                if !self.native_surface.lock().refresh_agent_operation(&lease)? {
                    return Err(
                        "browser/agent-operation-refresh-rejected: 工具操作已结束或 lease 已失效"
                            .to_string(),
                    );
                }
                Ok(HostedBrowserOutcome::new(json!({})))
            }
            HostedBrowserOperation::EndAgentOperation => {
                let lease = native_lease_from_request(request)?;
                self.native_surface.lock().end_agent_operation(&lease);
                Ok(HostedBrowserOutcome::new(json!({})))
            }
            HostedBrowserOperation::CoreTool => self.handle_browser_core_tool(app, request).await,
        }
    }

    async fn handle_browser_core_tool(
        &self,
        app: &AppHandle,
        request: &HostedBrowserRequest,
    ) -> Result<HostedBrowserOutcome, String> {
        ensure_hosted_caller_live(request)?;
        if !platform::browser_core_available() {
            return Err("browser/core-backend-unavailable".to_string());
        }
        let tool_name = request
            .tool_name
            .as_deref()
            .ok_or_else(|| "browser/missing-tool-name".to_string())?;
        let arguments = request
            .tool_arguments
            .as_ref()
            .cloned()
            .unwrap_or_else(|| json!({}));
        let arguments = arguments
            .as_object()
            .ok_or_else(|| "browser/tool-arguments-must-be-object".to_string())?;
        let arguments = Value::Object(arguments.clone());

        if tool_name == "list_pages" {
            ensure_hosted_caller_live(request)?;
            let surface = self.native_surface.lock();
            let tabs = surface
                .list_tabs(Some(app), &request.session_id)
                .ok_or_else(|| "browser/workspace-unavailable".to_string())?;
            let active = surface
                .active_tab_token(&request.session_id)
                .ok_or_else(|| "browser/workspace-unavailable".to_string())?;
            let pages = tabs
                .iter()
                .map(|tab| {
                    let page_id = tab
                        .page_id
                        .ok_or_else(|| "browser/page-id-unavailable".to_string())?;
                    Ok(json!({
                        "id": page_id,
                        "url": tab.url,
                        "title": tab.title,
                        "selected": tab.target_id == active,
                    }))
                })
                .collect::<Result<Vec<_>, String>>()?;
            let text = pages
                .iter()
                .map(|page| {
                    format!(
                        "{}: {}{}",
                        page["id"].as_u64().unwrap_or_default(),
                        page["url"].as_str().unwrap_or_default(),
                        if page["selected"].as_bool().unwrap_or(false) {
                            " [selected]"
                        } else {
                            ""
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            return Ok(HostedBrowserOutcome::new(browser_core_tool_result(
                if text.is_empty() {
                    "No pages".to_string()
                } else {
                    text
                },
                Some(json!({ "pages": pages })),
            )));
        }

        let tab_token_for_page = |page_id: u64| -> Result<String, String> {
            let surface = self.native_surface.lock();
            surface
                .tab_token_for_page_id(&request.session_id, page_id)
                .ok_or_else(|| "browser/page-not-found".to_string())
        };

        if tool_name == "new_page" {
            let requested_url = arguments
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| "browser/missing-argument: url".to_string())?;
            if !is_allowed_url(requested_url) {
                return Err("browser/url-not-allowed".to_string());
            }
            let background = arguments
                .get("background")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let authorization_tab = self
                .native_surface
                .lock()
                .active_tab_token(&request.session_id)
                .ok_or_else(|| "browser/workspace-unavailable".to_string())?;
            let reuse_initial_blank = {
                let surface = self.native_surface.lock();
                let tabs = surface
                    .list_tabs(Some(app), &request.session_id)
                    .ok_or_else(|| "browser/workspace-unavailable".to_string())?;
                should_reuse_browser_core_initial_tab(
                    &request.session_id,
                    &authorization_tab,
                    &tabs,
                    background,
                )
            };
            ensure_hosted_caller_live(request)?;
            let authorization = self
                .native_surface
                .lock()
                .activate_tab_with_lease(Some(app), &request.session_id, &authorization_tab)?
                .ok_or_else(|| "browser/workspace-unavailable".to_string())?;
            let tab_token = self.native_surface.lock().generate_tab_token();
            if reuse_initial_blank {
                ensure_hosted_caller_live(request)?;
                if !self.native_surface.lock().begin_agent_operation(
                    &authorization,
                    false,
                    request.caller_pid,
                    &request.wrapper_instance_nonce,
                )? {
                    return Err("browser/control-lease-lost".to_string());
                }
                if let Err(error) = ensure_hosted_caller_live(request) {
                    self.native_surface
                        .lock()
                        .end_agent_operation(&authorization);
                    return Err(error);
                }
                let navigation_result = (|| {
                    let label = self
                        .native_surface
                        .lock()
                        .webview_label_for_tab(&request.session_id, &authorization_tab)
                        .ok_or_else(|| "browser/native-surface-missing".to_string())?;
                    let webview = app
                        .get_webview(&label)
                        .ok_or_else(|| "browser/native-surface-missing".to_string())?;
                    webview
                        .navigate(
                            requested_url
                                .parse()
                                .map_err(|error| format!("browser/invalid-url: {error}"))?,
                        )
                        .map_err(|error| {
                            format!(
                                "browser/action-commit-unknown-after-navigation-dispatch: {error}"
                            )
                        })
                })();
                self.native_surface
                    .lock()
                    .end_agent_operation(&authorization);
                if let Err(error) = navigation_result {
                    return match core::committed_platform_outcome("Navigation", &error) {
                        Some(outcome) => Ok(HostedBrowserOutcome::new(outcome)),
                        None => Err(error),
                    };
                }
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": request.session_id, "tab": authorization_tab }),
                );
                self.persist_native_restore_best_effort(&request.session_id);
                return Ok(HostedBrowserOutcome::new(browser_core_tool_result(
                    format!("Opened page: {requested_url}"),
                    Some(json!({
                        "tabToken": authorization_tab,
                        "targetId": format!("native:{authorization_tab}"),
                        "reusedInitialBlank": true,
                    })),
                )));
            }

            ensure_hosted_caller_live(request)?;
            if !self.native_surface.lock().begin_agent_operation(
                &authorization,
                false,
                request.caller_pid,
                &request.wrapper_instance_nonce,
            )? {
                return Err("browser/control-lease-lost".to_string());
            }
            let creation_result = async {
                ensure_hosted_caller_live(request)?;
                self.native_surface.lock().create_tab_for_agent(
                    app,
                    &request.session_id,
                    &tab_token,
                    "about:blank",
                    background,
                    &authorization,
                    &request.request_id,
                )?;
                if let Err(error) = ensure_hosted_caller_live(request) {
                    return match self.rollback_staged_agent_tab(
                        app,
                        &request.session_id,
                        &tab_token,
                        &request.request_id,
                    ) {
                        Ok(()) => Err(error),
                        Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
                    };
                }
                let target_id = match self
                    .bind_staged_native_target(app, &request.session_id, &tab_token)
                    .await
                {
                    Ok(target_id) => target_id,
                    Err(error) => {
                        return match self.rollback_staged_agent_tab(
                            app,
                            &request.session_id,
                            &tab_token,
                            &request.request_id,
                        ) {
                            Ok(()) => Err(error),
                            Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
                        };
                    }
                };
                if let Err(error) = ensure_hosted_caller_live(request) {
                    return match self.rollback_staged_agent_tab(
                        app,
                        &request.session_id,
                        &tab_token,
                        &request.request_id,
                    ) {
                        Ok(()) => Err(error),
                        Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
                    };
                }
                if !self.native_surface.lock().commit_created_tab_for_agent(
                    app,
                    &request.session_id,
                    &tab_token,
                    &target_id,
                    requested_url,
                    background,
                    &authorization,
                    &request.request_id,
                    None,
                    || ensure_hosted_caller_live(request),
                )? {
                    return Err("browser/create-tab-cancelled".to_string());
                }
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": request.session_id, "tab": tab_token }),
                );
                self.persist_native_restore_best_effort(&request.session_id);
                Ok(HostedBrowserOutcome::with_rollback(
                    browser_core_tool_result(
                        format!("Opened page: {requested_url}"),
                        Some(json!({ "tabToken": tab_token, "targetId": target_id })),
                    ),
                    json!({
                        "kind": "created_tab",
                        "session_id": request.session_id,
                        "tab_token": tab_token,
                        "creation_id": request.request_id,
                    }),
                ))
            }
            .await;
            self.native_surface
                .lock()
                .end_agent_operation(&authorization);
            return creation_result;
        }

        let page_id_value = arguments
            .get("pageId")
            .ok_or_else(|| "browser/missing-argument: pageId".to_string())?;
        let page_id = page_id_value
            .as_u64()
            .filter(|page_id| *page_id <= (1_u64 << 53) - 1)
            .ok_or_else(|| "browser/invalid-argument: pageId".to_string())?;
        let tab_token = tab_token_for_page(page_id)?;

        if tool_name == "select_page" {
            ensure_hosted_caller_live(request)?;
            self.native_surface
                .lock()
                .activate_tab_with_lease(Some(app), &request.session_id, &tab_token)?
                .ok_or_else(|| "browser/page-not-found".to_string())?;
            let _ = app.emit(
                "browser:tabs-changed",
                json!({ "sessionId": request.session_id, "tab": tab_token }),
            );
            return Ok(HostedBrowserOutcome::new(browser_core_tool_result(
                "Selected page".to_string(),
                None,
            )));
        }

        ensure_hosted_caller_live(request)?;
        let lease = self
            .native_surface
            .lock()
            .activate_tab_with_lease(Some(app), &request.session_id, &tab_token)?
            .ok_or_else(|| "browser/page-not-found".to_string())?;

        if tool_name == "close_page" {
            ensure_hosted_caller_live(request)?;
            if !self.native_surface.lock().begin_agent_operation(
                &lease,
                false,
                request.caller_pid,
                &request.wrapper_instance_nonce,
            )? {
                return Err("browser/control-lease-lost".to_string());
            }
            if let Err(error) = ensure_hosted_caller_live(request) {
                self.native_surface.lock().end_agent_operation(&lease);
                return Err(error);
            }
            let close_result = self.native_surface.lock().close_tab_for_agent(
                Some(app),
                &request.session_id,
                &tab_token,
                &lease,
            );
            self.native_surface.lock().end_agent_operation(&lease);
            let closed = match close_result {
                Ok(closed) => closed,
                Err(error) => {
                    if let Some(outcome) = core::committed_platform_outcome("Close page", &error) {
                        return Ok(HostedBrowserOutcome::new(outcome));
                    }
                    return Err(error);
                }
            };
            if !closed {
                return Err("browser/page-not-found".to_string());
            }
            let _ = app.emit(
                "browser:tabs-changed",
                json!({ "sessionId": request.session_id }),
            );
            self.persist_native_restore_best_effort(&request.session_id);
            return Ok(HostedBrowserOutcome::new(browser_core_tool_result(
                "Closed page".to_string(),
                None,
            )));
        }

        // BrowserCore adapters revalidate the original lease at the native
        // dispatch boundary. Mouse/key dispatch opens the short provenance
        // window there; DOM resolution must not suppress real user takeover.
        ensure_hosted_caller_live(request)?;
        if !self.native_surface.lock().begin_agent_operation(
            &lease,
            false,
            request.caller_pid,
            &request.wrapper_instance_nonce,
        )? {
            return Err("browser/control-lease-lost".to_string());
        }

        let result = async {
            ensure_hosted_caller_live(request)?;
            let label = self
                .native_surface
                .lock()
                .webview_label_for_tab(&request.session_id, &tab_token)
                .ok_or_else(|| "browser/page-not-found".to_string())?;
            let webview = app
                .get_webview(&label)
                .ok_or_else(|| "browser/native-surface-missing".to_string())?;

            if tool_name == "navigate_page" {
                let navigation_type = arguments
                    .get("type")
                    .and_then(Value::as_str)
                    .or_else(|| arguments.get("url").map(|_| "url"))
                    .ok_or_else(|| "browser/missing-argument: type".to_string())?;
                match navigation_type {
                    "url" => {
                        let url = arguments
                            .get("url")
                            .and_then(Value::as_str)
                            .ok_or_else(|| "browser/missing-argument: url".to_string())?;
                        if !is_allowed_url(url) {
                            return Err("browser/url-not-allowed".to_string());
                        }
                        ensure_hosted_caller_live(request)?;
                        webview
                            .navigate(
                                url.parse()
                                    .map_err(|error| format!("browser/invalid-url: {error}"))?,
                            )
                            .map_err(|error| {
                                format!(
                                    "browser/action-commit-unknown-after-navigation-dispatch: URL navigation acknowledgement was inconclusive: {error}"
                                )
                            })?;
                    }
                    "back" => {
                        ensure_hosted_caller_live(request)?;
                        webview.eval("history.back()").map_err(|error| {
                            format!(
                                "browser/action-commit-unknown-after-navigation-dispatch: back navigation acknowledgement was inconclusive: {error}"
                            )
                        })?
                    }
                    "forward" => {
                        ensure_hosted_caller_live(request)?;
                        webview.eval("history.forward()").map_err(|error| {
                            format!(
                                "browser/action-commit-unknown-after-navigation-dispatch: forward navigation acknowledgement was inconclusive: {error}"
                            )
                        })?
                    }
                    "reload" => {
                        ensure_hosted_caller_live(request)?;
                        webview.reload().map_err(|error| {
                            format!(
                                "browser/action-commit-unknown-after-navigation-dispatch: reload acknowledgement was inconclusive: {error}"
                            )
                        })?
                    }
                    _ => return Err("browser/invalid-navigation-type".to_string()),
                }
                Ok(browser_core_tool_result(
                    format!("Navigation requested: {navigation_type}"),
                    None,
                ))
            } else {
                ensure_hosted_caller_live(request)?;
                core::execute_page_tool(&webview, &lease, tool_name, &arguments).await
            }
        }
        .await;
        self.native_surface.lock().end_agent_operation(&lease);
        match result {
            Ok(result) => Ok(HostedBrowserOutcome::new(result)),
            Err(error) => match core::committed_platform_outcome("Navigation", &error) {
                Some(outcome) => Ok(HostedBrowserOutcome::new(outcome)),
                None => Err(error),
            },
        }
    }

    /// Caller holds `start_mtx`. A single host-owned journal is allowed per
    /// session. Resolve an older generation before creating a new intent so an
    /// old CreatedBlank rollback can never race a newer committed manifest.
    async fn begin_hosted_prepare_with_start_lock(
        &self,
        app: &AppHandle,
        session_id: &str,
        session_token: &str,
        hosted_request: Option<&HostedBrowserRequest>,
    ) -> Result<(), PrepareWorkspaceError> {
        if let Some(request) = hosted_request {
            if !matches!(request.operation, HostedBrowserOperation::Prepare)
                || request.session_id != session_id
                || request.session_token != session_token
            {
                return Err("浏览器 Prepare 日志请求身份不一致".to_string().into());
            }
        }

        let journal_path = hosted_prepare_journal_path_for(session_token);
        let mut superseded_commit = None;
        if journal_path.exists() {
            let journal = read_hosted_prepare_journal(&journal_path)?;
            if journal.compensation.session_id != session_id {
                return Err("浏览器 Prepare 日志与当前任务不一致".to_string().into());
            }
            let cancellation_wins =
                matching_hosted_cancellation_for_compensation(&journal.compensation)?;
            if journal.phase == HostedPreparePhase::Committed && !cancellation_wins {
                if let Some(request) = hosted_request {
                    let same_request = journal.compensation.request_id == request.request_id
                        && journal.compensation.idempotency_key == request.idempotency_key
                        && journal.compensation.caller_pid == request.caller_pid
                        && journal.compensation.wrapper_instance_nonce
                            == request.wrapper_instance_nonce;
                    if same_request {
                        return Err(
                            "browser/prepare-acknowledgement-pending: identical committed Prepare is still awaiting caller settlement"
                                .to_string()
                                .into(),
                        );
                    }
                    // A distinct Prepare for the same task explicitly adopts
                    // the committed workspace. Its Pending record atomically
                    // replaces the old WAL below, so there is no record-free
                    // crash window in which the old manifest can be orphaned.
                    superseded_commit = Some((
                        journal.compensation.session_token.clone(),
                        journal.compensation.request_id.clone(),
                    ));
                } else {
                    // A direct in-app Prepare is also an explicit adoption, but
                    // has no wrapper generation that requires a replacement WAL.
                    remove_hosted_prepare_journal(&journal_path)?;
                    self.locally_committed_prepares.lock().remove(&(
                        journal.compensation.session_token.clone(),
                        journal.compensation.request_id.clone(),
                    ));
                }
            } else {
                self.rollback_prepare_journal_with_start_lock(app, &journal)
                    .await?;
                remove_hosted_prepare_journal(&journal_path)?;
                self.locally_committed_prepares.lock().remove(&(
                    journal.compensation.session_token.clone(),
                    journal.compensation.request_id.clone(),
                ));
            }
        }

        let Some(request) = hosted_request else {
            return Ok(());
        };
        let had_session = self.native_surface.lock().has_session(session_id);
        let rollback_kind = if had_session {
            "none"
        } else if paths::browser_workspace_restore_json(session_token).exists() {
            "restored_session"
        } else {
            "prepared_session"
        };
        let journal = new_hosted_prepare_journal(request, rollback_kind, hosted_protocol_now_ms()?);
        write_hosted_prepare_journal(&journal)?;
        if let Some(old_key) = superseded_commit {
            self.locally_committed_prepares.lock().remove(&old_key);
        }
        Ok(())
    }

    /// Caller holds `start_mtx`. Pending journals without a revision represent
    /// a crash during restore/build; no later generation can exist because a
    /// new prepare cannot pass this gate until cleanup succeeds.
    async fn rollback_prepare_journal_with_start_lock(
        &self,
        app: &AppHandle,
        journal: &HostedPrepareJournal,
    ) -> Result<(), String> {
        let compensation = &journal.compensation;
        match (compensation.rollback_kind.as_str(), compensation.revision) {
            ("prepared_session", Some(revision)) => {
                self.rollback_prepared_session_with_start_lock(
                    &compensation.session_id,
                    &compensation.request_id,
                    revision,
                    false,
                )
                .await?;
            }
            ("restored_session", Some(revision)) => {
                self.rollback_prepared_session_with_start_lock(
                    &compensation.session_id,
                    &compensation.request_id,
                    revision,
                    true,
                )
                .await?;
            }
            ("prepared_session", None) => {
                let has_remaining = self
                    .native_surface
                    .lock()
                    .close_session(Some(app), &compensation.session_id)?;
                if !has_remaining {
                    self.stop_with_start_lock().await?;
                }
            }
            ("restored_session", None) => {
                let has_remaining = self
                    .native_surface
                    .lock()
                    .close_session_preserving_restore(Some(app), &compensation.session_id)?;
                if !has_remaining {
                    self.stop_with_start_lock().await?;
                }
            }
            ("none", None) => {}
            _ => return Err("浏览器 Prepare 日志补偿 generation 无效".to_string()),
        }
        if compensation.rollback_kind == "prepared_session" {
            remove_file_and_verify_absent(
                &paths::browser_workspace_restore_json(&compensation.session_token),
                "删除未提交 Prepare 恢复清单",
            )?;
        }
        Ok(())
    }

    async fn rollback_prepare_journal(
        &self,
        app: &AppHandle,
        journal: &HostedPrepareJournal,
    ) -> Result<(), String> {
        let _start_guard = self.start_mtx.lock().await;
        self.rollback_prepare_journal_with_start_lock(app, journal)
            .await
    }

    async fn prepare_native_workspace(
        &self,
        app: &AppHandle,
        session_id: &str,
        session_token: &str,
        agent_initiated: bool,
        prepare_request_id: Option<&str>,
        hosted_request: Option<&HostedBrowserRequest>,
    ) -> Result<(Value, PreparedWorkspaceDisposition), PrepareWorkspaceError> {
        if !crate::platform::capabilities::browser_product_enabled() {
            return Err("当前产品构建尚未开放应用内浏览器".to_string().into());
        }
        // Restore and prepare form one lifecycle transaction. Holding this
        // guard across both phases prevents a completed stop_for_session from
        // being followed by an older prepare recreating a blank workspace.
        let _start_guard = self.start_mtx.lock().await;
        self.ensure_browser_session_allowed(session_id)?;
        self.begin_hosted_prepare_with_start_lock(app, session_id, session_token, hosted_request)
            .await?;
        let prepared = async {
            if platform::browser_core_available() {
                let restore_outcome = if !self.native_surface.lock().has_session(session_id) {
                    self.restore_saved_workspace_with_start_lock(session_id)
                        .await?
                } else {
                    RestoreWorkspaceOutcome::Existing
                };
                return self
                    .prepare_browser_core_workspace_with_start_lock(
                        app,
                        session_id,
                        session_token,
                        restore_outcome,
                        prepare_request_id,
                        hosted_request,
                    )
                    .await;
            }

            // Wrapper 可能比前端状态查询更早到达（例如应用重启后立即继续任务）。先按
            // URL 清单重建，避免普通 prepare 用一个空白页覆盖尚未恢复的多标签工作区。
            let restore_outcome = if !self.native_surface.lock().has_session(session_id) {
                self.restore_saved_workspace_with_start_lock(session_id)
                    .await?
            } else {
                RestoreWorkspaceOutcome::Existing
            };
            let (had_session, had_sessions) = {
                let surface = self.native_surface.lock();
                (surface.has_session(session_id), surface.has_sessions())
            };
            let existing_port = live_port().await;
            if had_sessions {
                let port = existing_port.ok_or_else(|| {
                    PrepareWorkspaceError::from(
                        "原生浏览器工作区仍在运行，但自动化端点状态缺失；已保留现有任务，请重试"
                            .to_string(),
                    )
                })?;
                if !self.native_surface.lock().owns_port(port) {
                    return Err("自动化端点与现有原生浏览器工作区不匹配".to_string().into());
                }
            } else if let Some(port) = existing_port {
                if !self.native_surface.lock().owns_port(port) {
                    return Err("检测到不属于当前原生浏览器工作区的自动化端点"
                        .to_string()
                        .into());
                }
            }
            let port = match existing_port {
                Some(port) => port,
                None => pick_free_port().await?,
            };
            let profile = paths::browser_webview_profile_dir();
            let prepared = if agent_initiated {
                self.native_surface.lock().prepare(
                    app,
                    session_id,
                    session_token,
                    port,
                    &profile,
                )?
            } else {
                self.native_surface.lock().prepare_unclaimed(
                    app,
                    session_id,
                    session_token,
                    port,
                    &profile,
                )?
            };
            if !prepared {
                return Err("当前平台不支持原生浏览器表面".to_string().into());
            }
            if existing_port.is_none() {
                if !probe_cdp(port, Duration::from_secs(15)).await {
                    self.rollback_new_native_workspace(app, session_id, had_session);
                    return Err("WebView2 已创建但 CDP 未就绪".to_string().into());
                }
                if let Err(error) = write_port_file(port, "app", None) {
                    self.rollback_new_native_workspace(app, session_id, had_session);
                    return Err(error.into());
                }
            }
            let unbound_tabs = self.native_surface.lock().unbound_tabs(session_id);
            for tab_token in unbound_tabs {
                let target_id = match discover_native_target(port, &tab_token).await {
                    Ok(target_id) => target_id,
                    Err(error) => {
                        self.rollback_new_native_workspace(app, session_id, had_session);
                        return Err(error.into());
                    }
                };
                if let Err(error) = self
                    .native_surface
                    .lock()
                    .bind_target(session_id, &tab_token, &target_id)
                {
                    self.rollback_new_native_workspace(app, session_id, had_session);
                    return Err(error.into());
                }
            }
            let disposition = match restore_outcome {
                RestoreWorkspaceOutcome::Restored => PreparedWorkspaceDisposition::RestoredExisting,
                RestoreWorkspaceOutcome::Existing => PreparedWorkspaceDisposition::Existing,
                RestoreWorkspaceOutcome::Missing if had_session => {
                    // Another prepare won the race while this request waited for start_mtx.
                    PreparedWorkspaceDisposition::Existing
                }
                RestoreWorkspaceOutcome::Missing => PreparedWorkspaceDisposition::CreatedBlank,
            };
            let result = json!({ "sessionId": session_id });
            self.publish_prepared_workspace_with_start_lock(
                app,
                session_id,
                disposition,
                prepare_request_id,
                hosted_request,
                &result,
            )
            .await?;
            Ok((result, disposition))
        }
        .await;

        match prepared {
            Ok(prepared) => Ok(prepared),
            Err(mut error) => {
                if hosted_request.is_some() {
                    let journal_path = hosted_prepare_journal_path_for(session_token);
                    if journal_path.exists() {
                        match read_hosted_prepare_journal(&journal_path) {
                            Ok(journal) => {
                                if let Err(cleanup_error) = self
                                    .rollback_prepare_journal_with_start_lock(app, &journal)
                                    .await
                                    .and_then(|()| remove_hosted_prepare_journal(&journal_path))
                                {
                                    error.message = format!(
                                        "{}; Prepare 失败后的持久补偿尚未完成: {cleanup_error}",
                                        error.message
                                    );
                                    if error.rollback.is_none() {
                                        error.rollback = journal.compensation.rollback_value();
                                    }
                                }
                            }
                            Err(cleanup_error) => {
                                error.message = format!(
                                    "{}; Prepare 失败后的持久日志不可读: {cleanup_error}",
                                    error.message
                                );
                            }
                        }
                    }
                }
                Err(error)
            }
        }
    }

    /// Caller holds `start_mtx` across restore and this final prepare phase.
    async fn prepare_browser_core_workspace_with_start_lock(
        &self,
        app: &AppHandle,
        session_id: &str,
        session_token: &str,
        restore_outcome: RestoreWorkspaceOutcome,
        prepare_request_id: Option<&str>,
        hosted_request: Option<&HostedBrowserRequest>,
    ) -> Result<(Value, PreparedWorkspaceDisposition), PrepareWorkspaceError> {
        let had_session = self.native_surface.lock().has_session(session_id);
        if !had_session {
            let profile = paths::browser_webview_profile_dir();
            if !self.native_surface.lock().prepare_display_only(
                app,
                session_id,
                session_token,
                &profile,
            )? {
                return Err("browser/native-surface-unavailable".to_string().into());
            }
        }

        if let Err(error) = platform::wait_browser_core_ready().await {
            self.rollback_new_native_workspace(app, session_id, had_session);
            return Err(error.into());
        }
        let unbound_tabs = self.native_surface.lock().unbound_tabs(session_id);
        for tab_token in unbound_tabs {
            let label = match self
                .native_surface
                .lock()
                .webview_label_for_tab(session_id, &tab_token)
            {
                Some(label) => label,
                None => {
                    self.rollback_new_native_workspace(app, session_id, had_session);
                    return Err("browser/native-surface-missing".to_string().into());
                }
            };
            let Some(webview) = app.get_webview(&label) else {
                self.rollback_new_native_workspace(app, session_id, had_session);
                return Err("browser/native-surface-missing".to_string().into());
            };
            if let Err(error) = platform::bind_browser_core_webview(&webview).await {
                self.rollback_new_native_workspace(app, session_id, had_session);
                return Err(error.into());
            }
            let target_id = format!("native:{tab_token}");
            let bound = match self
                .native_surface
                .lock()
                .bind_target(session_id, &tab_token, &target_id)
            {
                Ok(bound) => bound,
                Err(error) => {
                    self.rollback_new_native_workspace(app, session_id, had_session);
                    return Err(error.into());
                }
            };
            if !bound {
                self.rollback_new_native_workspace(app, session_id, had_session);
                return Err("browser/native-tab-binding-failed".to_string().into());
            }
        }
        let disposition = if matches!(restore_outcome, RestoreWorkspaceOutcome::Restored) {
            PreparedWorkspaceDisposition::RestoredExisting
        } else if had_session {
            PreparedWorkspaceDisposition::Existing
        } else {
            PreparedWorkspaceDisposition::CreatedBlank
        };
        let result = json!({
            "sessionId": session_id,
            "backend": platform::browser_core_backend_name(),
        });
        self.publish_prepared_workspace_with_start_lock(
            app,
            session_id,
            disposition,
            prepare_request_id,
            hosted_request,
            &result,
        )
        .await?;
        Ok((result, disposition))
    }

    /// Caller holds `start_mtx`. A hosted prepare may spend seconds restoring
    /// surfaces and probing its automation backend, so its wrapper epoch must
    /// be checked again at the publication boundary rather than only when the
    /// request artifact is claimed.
    async fn publish_prepared_workspace_with_start_lock(
        &self,
        app: &AppHandle,
        session_id: &str,
        disposition: PreparedWorkspaceDisposition,
        prepare_request_id: Option<&str>,
        hosted_request: Option<&HostedBrowserRequest>,
        prepared_result: &Value,
    ) -> Result<(), PrepareWorkspaceError> {
        self.native_surface.lock().record_prepare_generation(
            session_id,
            disposition.rollback_kind().and(prepare_request_id),
        )?;
        let prepare_revision = prepare_request_id.and_then(|request_id| {
            self.native_surface
                .lock()
                .prepare_generation_revision(session_id, request_id)
        });

        if let Some(request) = hosted_request {
            let journal_path = hosted_prepare_journal_path(request);
            let mut journal = read_hosted_prepare_journal(&journal_path)?;
            if journal.compensation.request_id != request.request_id
                || journal.compensation.idempotency_key != request.idempotency_key
                || journal.compensation.session_id != request.session_id
            {
                return Err("浏览器 Prepare 持久日志被其他 generation 占用"
                    .to_string()
                    .into());
            }
            journal.compensation.rollback_kind =
                disposition.rollback_kind().unwrap_or("none").to_string();
            journal.compensation.revision = match disposition.rollback_kind() {
                Some(_) => Some(prepare_revision.ok_or_else(|| {
                    PrepareWorkspaceError::from(
                        "hosted prepare generation metadata is missing".to_string(),
                    )
                })?),
                None => None,
            };
            journal.phase = HostedPreparePhase::Prepared;
            journal.updated_at = hosted_protocol_now_ms()?;
            journal.response = None;
            write_hosted_prepare_journal(&journal)?;

            let final_liveness = ensure_hosted_caller_live(request).and_then(|_| {
                if hosted_cancellation_path(request).exists() {
                    Err("browser/request-cancelled".to_string())
                } else {
                    Ok(())
                }
            });
            if let Err(liveness_error) = final_liveness {
                journal.phase = HostedPreparePhase::Cancelled;
                journal.updated_at = hosted_protocol_now_ms()?;
                let journal_error = write_hosted_prepare_journal(&journal).err();
                let cancellation_error = self
                    .ensure_internal_prepare_cancellation(app, request, Some(&journal.compensation))
                    .err();
                let mut details = Vec::new();
                if let Some(error) = journal_error {
                    details.push(format!("更新持久补偿 phase 失败: {error}"));
                }
                if let Some(error) = cancellation_error {
                    details.push(format!("写入取消记录失败: {error}"));
                }
                let message = if details.is_empty() {
                    liveness_error
                } else {
                    format!("{liveness_error}; {}", details.join("; "))
                };
                return match journal.compensation.rollback_value() {
                    Some(rollback) => Err(PrepareWorkspaceError::compensated(message, rollback)),
                    None => Err(message.into()),
                };
            }

            // This host-owned atomic phase transition is the publication
            // commit witness. It precedes every UI event and the wrapper-owned,
            // consumable response artifact. A matching cancellation still wins
            // until this process observes wrapper acknowledgement; therefore a
            // recovered Committed WAL remains durable across request-dir reset.
            journal.phase = HostedPreparePhase::Committed;
            journal.updated_at = hosted_protocol_now_ms()?;
            journal.response = Some(hosted_response(request, Ok(prepared_result.clone())));
            write_hosted_prepare_journal(&journal)?;
            self.locally_committed_prepares
                .lock()
                .insert((request.session_token.clone(), request.request_id.clone()));
        }

        self.activated
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = app.emit("browser:activated", json!({ "sessionId": session_id }));
        Ok(())
    }

    /// Publish a host-owned cancellation tombstone so the existing request
    /// ledger performs exact compensation before ACK. If the first atomic
    /// write fails, keep an in-memory retry alive; the failed Hosted outcome
    /// still carries the same rollback record, so a later tombstone can move a
    /// completed request into CancelPendingRollback and retry safely.
    fn ensure_internal_prepare_cancellation(
        &self,
        app: &AppHandle,
        request: &HostedBrowserRequest,
        prepare_compensation: Option<&HostedPrepareCompensation>,
    ) -> Result<(), String> {
        let cancellation_path = hosted_cancellation_path(request);
        let cancellation = hosted_internal_cancellation_value(
            request,
            hosted_protocol_now_ms()?,
            prepare_compensation,
        );
        let encoded = serde_json::to_vec(&cancellation)
            .map_err(|error| format!("编码浏览器宿主内部取消记录失败: {error}"))?;
        match crate::platform::filesystem::atomic_write(&cancellation_path, &encoded) {
            Ok(()) => Ok(()),
            Err(error) => {
                let first_error = format!(
                    "写入浏览器宿主内部取消记录 {} 失败: {error}",
                    cancellation_path.display()
                );
                let retry_app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let mut delay = Duration::from_millis(100);
                    loop {
                        tokio::time::sleep(delay).await;
                        if crate::platform::filesystem::atomic_write(&cancellation_path, &encoded)
                            .is_ok()
                        {
                            return;
                        }
                        let manager = retry_app.state::<BrowserManager>();
                        if manager
                            .shutting_down
                            .load(std::sync::atomic::Ordering::SeqCst)
                        {
                            eprintln!(
                                "[browser] 应用退出前仍未能持久化 Prepare 补偿记录: {}",
                                cancellation_path.display()
                            );
                            return;
                        }
                        delay = delay.saturating_mul(2).min(Duration::from_secs(5));
                    }
                });
                Err(first_error)
            }
        }
    }

    /// 普通模式的浏览器入口始终可见；用户首次展开时才创建当前任务的空白
    /// WebView 工作区，避免应用启动即为所有任务分配原生页面。
    pub async fn prepare_for_user(&self, browser_session_id: &str) -> Result<Value, String> {
        let app = self
            .app
            .lock()
            .clone()
            .ok_or_else(|| "应用句柄尚未就绪".to_string())?;
        let session_token = paths::browser_session_token(browser_session_id);
        let (result, _) = self
            .prepare_native_workspace(&app, browser_session_id, &session_token, false, None, None)
            .await
            .map_err(|error| error.message)?;
        Ok(result)
    }

    /// A late Prepare tombstone may close only the exact, untouched process-
    /// local generation created by that request. If UI/Agent work has already
    /// superseded the generation, rollback is an acknowledged no-op.
    async fn rollback_prepared_session(
        &self,
        browser_session_id: &str,
        request_id: &str,
        expected_revision: u64,
        preserve_restore: bool,
    ) -> Result<(), String> {
        let _start_guard = self.start_mtx.lock().await;
        self.rollback_prepared_session_with_start_lock(
            browser_session_id,
            request_id,
            expected_revision,
            preserve_restore,
        )
        .await
    }

    async fn rollback_prepared_session_with_start_lock(
        &self,
        browser_session_id: &str,
        request_id: &str,
        expected_revision: u64,
        preserve_restore: bool,
    ) -> Result<(), String> {
        let app = self.app.lock().clone();
        let rollback = self.native_surface.lock().rollback_prepare_generation(
            app.as_ref(),
            browser_session_id,
            request_id,
            expected_revision,
            preserve_restore,
        )?;
        let Some(has_remaining) = rollback else {
            return Ok(());
        };
        if !has_remaining {
            self.stop_with_start_lock().await?;
        } else if let Some(app) = app {
            let _ = app.emit(
                "browser:stopped",
                json!({ "sessionId": browser_session_id }),
            );
        }
        Ok(())
    }

    /// prepare 的后置探测/协调文件提交失败时只回滚本次新建的工作区。已有会话保持
    /// 原样；最后一个新工作区移除后才重置共享平台环境。
    fn rollback_new_native_workspace(
        &self,
        app: &AppHandle,
        session_id: &str,
        existed_before_prepare: bool,
    ) {
        if existed_before_prepare {
            return;
        }
        let mut surface = self.native_surface.lock();
        match surface.close_session(Some(app), session_id) {
            Ok(false) => {
                let _ = surface.close(Some(app));
                let _ = std::fs::remove_file(paths::browser_cdp_port_json());
            }
            Ok(true) => {}
            Err(error) => {
                eprintln!("[browser] 回滚原生浏览器工作区失败: {error}");
            }
        }
    }

    /// 绑定 AppHandle（setup 时调用一次）。
    pub fn bind_app(&self, app: AppHandle) {
        *self.app.lock() = Some(app);
    }

    /// 导航事件在 WebView 回调栈退出后调用此入口，避免重入 native_surface 锁。
    pub(crate) fn persist_native_restore(&self, browser_session_id: &str) -> Result<(), String> {
        let app = self
            .app
            .lock()
            .clone()
            .ok_or_else(|| "应用句柄尚未就绪".to_string())?;
        let _persistence_guard = self.persistence_io.lock();
        match self.persist_native_restore_once(&app, browser_session_id) {
            Ok(()) => {
                self.clear_persistence_warning(&app, browser_session_id);
                Ok(())
            }
            Err(error) => {
                self.record_persistence_warning(&app, browser_session_id, &error);
                self.schedule_persistence_retry(&app, browser_session_id);
                Err(error)
            }
        }
    }

    fn persist_native_restore_once(
        &self,
        app: &AppHandle,
        browser_session_id: &str,
    ) -> Result<(), String> {
        self.native_surface
            .lock()
            .persist_navigation_state(app, browser_session_id)
    }

    fn record_persistence_warning(&self, app: &AppHandle, session_id: &str, error: &str) {
        let changed = self
            .persistence_warnings
            .lock()
            .insert(session_id.to_string(), error.to_string())
            .as_deref()
            != Some(error);
        if changed {
            let _ = app.emit(
                "browser:persistence-warning",
                json!({ "sessionId": session_id, "error": error }),
            );
        }
    }

    fn clear_persistence_warning(&self, app: &AppHandle, session_id: &str) {
        if self
            .persistence_warnings
            .lock()
            .remove(session_id)
            .is_some()
        {
            let _ = app.emit(
                "browser:persistence-restored",
                json!({ "sessionId": session_id }),
            );
        }
    }

    fn clear_persistence_state(&self, session_id: &str) {
        self.persistence_warnings.lock().remove(session_id);
        self.persistence_retries.lock().remove(session_id);
    }

    fn schedule_persistence_retry(&self, app: &AppHandle, session_id: &str) {
        if !self
            .persistence_retries
            .lock()
            .insert(session_id.to_string())
        {
            return;
        }
        let retry_app = app.clone();
        let retry_session_id = session_id.to_string();
        tauri::async_runtime::spawn(async move {
            let mut delay = Duration::from_millis(250);
            loop {
                tokio::time::sleep(delay).await;
                let manager = retry_app.state::<BrowserManager>();
                let _persistence_guard = manager.persistence_io.lock();
                let session_gone = manager
                    .deleted_session_ids
                    .read()
                    .contains(&retry_session_id)
                    || !manager.native_surface.lock().has_session(&retry_session_id);
                if manager
                    .shutting_down
                    .load(std::sync::atomic::Ordering::SeqCst)
                    || session_gone
                {
                    manager.clear_persistence_state(&retry_session_id);
                    return;
                }
                match manager.persist_native_restore_once(&retry_app, &retry_session_id) {
                    Ok(()) => {
                        manager.clear_persistence_warning(&retry_app, &retry_session_id);
                        manager.persistence_retries.lock().remove(&retry_session_id);
                        return;
                    }
                    Err(error) => {
                        manager.record_persistence_warning(&retry_app, &retry_session_id, &error);
                        delay = delay.saturating_mul(2).min(Duration::from_secs(30));
                    }
                }
            }
        });
    }

    fn persist_native_restore_best_effort(&self, browser_session_id: &str) {
        if let Err(error) = self.persist_native_restore(browser_session_id) {
            eprintln!("[browser] 原生浏览器状态已提交，持久化将在后台重试: {error}");
        }
    }

    /// 如果当前对话存在上次进程保存的 URL 清单，则惰性重建原生页面。所有 WebView、
    /// tab token、CDP target 和 lease 都是本进程新建；清单中的 active index 只用于
    /// 选择新标签，不复用任何运行期身份。
    async fn restore_saved_workspace(
        &self,
        browser_session_id: &str,
    ) -> Result<RestoreWorkspaceOutcome, String> {
        let _start_guard = self.start_mtx.lock().await;
        let outcome = self
            .restore_saved_workspace_with_start_lock(browser_session_id)
            .await?;
        if matches!(outcome, RestoreWorkspaceOutcome::Restored) {
            self.native_surface
                .lock()
                .record_prepare_generation(browser_session_id, None)?;
            self.activated
                .store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(app) = self.app.lock().clone() {
                let _ = app.emit(
                    "browser:activated",
                    json!({ "sessionId": browser_session_id, "restored": true }),
                );
            }
        }
        Ok(outcome)
    }

    /// Restore phase for callers that already hold the shared lifecycle lock.
    /// Publication is deliberately left to the caller so a hosted Prepare can
    /// record its rollback generation before UI/user code can mutate it.
    async fn restore_saved_workspace_with_start_lock(
        &self,
        browser_session_id: &str,
    ) -> Result<RestoreWorkspaceOutcome, String> {
        // 发布门控关闭时连恢复清单都不读取，更不能据此创建隐藏 WebView；清单
        // 原样保留，未来正式开放或 preview 验收构建仍可恢复。
        if !crate::platform::capabilities::browser_product_enabled() {
            return Ok(RestoreWorkspaceOutcome::Missing);
        }
        if self
            .native_surface
            .lock()
            .has_published_session(browser_session_id)
        {
            return Ok(RestoreWorkspaceOutcome::Existing);
        }
        let session_token = paths::browser_session_token(browser_session_id);
        let journal_path = hosted_prepare_journal_path_for(&session_token);
        if journal_path.exists()
            && read_hosted_prepare_journal(&journal_path)?.phase == HostedPreparePhase::Committed
        {
            return Err(
                "browser/prepare-acknowledgement-pending: recovered Prepare is awaiting caller settlement"
                    .to_string(),
            );
        }
        let Some(restore) =
            platform::NativeBrowserSurface::read_restore_workspace(browser_session_id)?
        else {
            return Ok(RestoreWorkspaceOutcome::Missing);
        };

        let app = self
            .app
            .lock()
            .clone()
            .ok_or_else(|| "应用句柄尚未就绪".to_string())?;
        self.ensure_browser_session_allowed(browser_session_id)?;
        if self.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("应用正在退出，不恢复浏览器".to_string());
        }
        {
            let mut surface = self.native_surface.lock();
            if surface.has_published_session(browser_session_id) {
                return Ok(RestoreWorkspaceOutcome::Existing);
            }
            if surface.has_session(browser_session_id) {
                let has_remaining = surface
                    .close_session_preserving_restore(Some(&app), browser_session_id)
                    .map_err(|error| format!("上次浏览器恢复的残留表面尚未清理完成: {error}"))?;
                if !has_remaining {
                    surface
                        .close_preserving_restore(Some(&app))
                        .map_err(|error| format!("重置上次浏览器恢复环境失败: {error}"))?;
                }
            }
        }
        let capabilities = self.native_surface.lock().capabilities();
        if !capabilities.native_display {
            return Ok(RestoreWorkspaceOutcome::Missing);
        }
        let browser_core_available = platform::browser_core_available();
        if !has_restore_automation_backend(capabilities, browser_core_available) {
            return Err(
                "browser/core-backend-unavailable: cannot restore an automation workspace"
                    .to_string(),
            );
        }

        let existing_port = if capabilities.chrome_devtools_protocol {
            live_port().await
        } else {
            None
        };
        if let Some(port) = existing_port {
            if !self.native_surface.lock().owns_port(port) {
                return Err("检测到不属于当前应用的浏览器自动化端点".to_string());
            }
        }
        if capabilities.chrome_devtools_protocol
            && self.native_surface.lock().has_sessions()
            && existing_port.is_none()
        {
            return Err("其他对话的原生浏览器仍在运行，但自动化端点状态缺失".to_string());
        }
        let port = if capabilities.chrome_devtools_protocol {
            Some(match existing_port {
                Some(port) => port,
                None => pick_free_port().await?,
            })
        } else {
            None
        };
        let profile = paths::browser_webview_profile_dir();
        let tab_tokens = self.native_surface.lock().prepare_restored_surface(
            &app,
            browser_session_id,
            &session_token,
            port,
            &profile,
            &restore,
        )?;
        let created_new_port = capabilities.chrome_devtools_protocol && existing_port.is_none();

        let restored = async {
            if let Some(port) = port {
                if created_new_port {
                    if !probe_cdp(port, Duration::from_secs(15)).await {
                        return Err("WebView2 已重建但 CDP 未就绪".to_string());
                    }
                    write_port_file(port, "app", None)?;
                }
                for (tab_token, url) in tab_tokens.iter().zip(&restore.urls) {
                    let target_id = discover_native_target(port, tab_token).await?;
                    if !self.native_surface.lock().bind_target(
                        browser_session_id,
                        tab_token,
                        &target_id,
                    )? {
                        return Err("恢复标签在绑定新 automation target 前已关闭".to_string());
                    }
                    if !self.native_surface.lock().navigate_tab_after_bind(
                        Some(&app),
                        browser_session_id,
                        tab_token,
                        url,
                    )? {
                        return Err("恢复标签在导航前已关闭".to_string());
                    }
                }
            } else if browser_core_available {
                platform::wait_browser_core_ready().await?;
                for (tab_token, url) in tab_tokens.iter().zip(&restore.urls) {
                    let label = self
                        .native_surface
                        .lock()
                        .webview_label_for_tab(browser_session_id, tab_token)
                        .ok_or_else(|| "browser/native-surface-missing".to_string())?;
                    let webview = app
                        .get_webview(&label)
                        .ok_or_else(|| "browser/native-surface-missing".to_string())?;
                    platform::bind_browser_core_webview(&webview).await?;
                    let target_id = format!("native:{tab_token}");
                    if !self.native_surface.lock().bind_target(
                        browser_session_id,
                        tab_token,
                        &target_id,
                    )? {
                        return Err("恢复标签在绑定 BrowserCore target 前已关闭".to_string());
                    }
                    if !self.native_surface.lock().navigate_tab_after_bind(
                        Some(&app),
                        browser_session_id,
                        tab_token,
                        url,
                    )? {
                        return Err("恢复标签在 BrowserCore 首航前已关闭".to_string());
                    }
                }
            }
            // prepare_restored_surface 已按 active_index 设置当前标签。恢复过程不能调用
            // UI activate_tab：那会把“应用重启”伪装成用户接管，使 Agent 必须等待一次
            // 并不存在的手工交还。恢复后的中立 owner 由下一次真实操作原子认领。
            if tab_tokens.get(restore.active_index).is_none() {
                return Err("恢复清单当前标签无效".to_string());
            }
            // navigate 是异步提交；此处原样保留已验证清单，随后真实导航事件会
            // 用宿主 WebView 当前 URL 更新它，避免短暂 marker 覆盖恢复快照。
            platform::NativeBrowserSurface::write_restore_workspace(browser_session_id, &restore)?;
            Ok::<(), String>(())
        }
        .await;

        if let Err(error) = restored {
            let mut surface = self.native_surface.lock();
            let cleanup = surface
                .quarantine_failed_restore(Some(&app), browser_session_id)
                .and_then(|has_remaining| {
                    if has_remaining {
                        Ok(())
                    } else {
                        surface.close_preserving_restore(Some(&app))
                    }
                });
            drop(surface);
            if created_new_port {
                let _ = std::fs::remove_file(paths::browser_cdp_port_json());
            }
            let restore_write = platform::NativeBrowserSurface::write_restore_workspace(
                browser_session_id,
                &restore,
            );
            return match (cleanup, restore_write) {
                (Ok(()), Ok(())) => Err(error),
                (Err(cleanup_error), Ok(())) => Err(format!(
                    "{error}; 恢复失败后的表面对账尚未完成: {cleanup_error}"
                )),
                (Ok(()), Err(restore_error)) => Err(format!(
                    "{error}; 重写原始恢复清单失败: {restore_error}"
                )),
                (Err(cleanup_error), Err(restore_error)) => Err(format!(
                    "{error}; 恢复失败后的表面对账尚未完成: {cleanup_error}; 重写原始恢复清单失败: {restore_error}"
                )),
            };
        }

        Ok(RestoreWorkspaceOutcome::Restored)
    }

    /// 监听 `cdp-port.json`：检测到当前应用原生宿主发布的有效端口且品悟尚未接入时，
    /// 自动 `ensure_started` 并 emit `browser:activated` —— 前端据此在
    /// "工作模式 + 模型实际调用浏览器能力"时显示浏览器 Tab（不调用则永不出现/加载）。
    ///
    /// 另承担异常恢复：已接入但 WebView2 CDP 失联时只重置自动化连接并发送按任务
    /// `browser:automation-unavailable`；真实页面保持可见、可由用户手动操作。
    pub fn spawn_watch(app: AppHandle) {
        let manager = app.state::<BrowserManager>();
        match recover_hosted_prepare_journals_for_process_start() {
            Ok(()) => *manager.prepare_recovery_error.lock() = None,
            Err(error) => {
                *manager.prepare_recovery_error.lock() = Some(error.clone());
                eprintln!("[browser] 恢复持久 Prepare 补偿失败: {error}");
                return;
            }
        }
        // host request/response/cancelled 都是单进程临时协议，不属于可恢复任务状态。
        // watcher 注册前原子换出整个旧目录，保证上次崩溃遗留的 create/close 即使
        // session 仍存在也绝不会在新进程重放；换出失败则 fail-closed，不启动 consumer。
        if let Err(error) =
            reset_host_request_directory_for_process_start(&paths::browser_host_requests_dir())
        {
            *manager.prepare_recovery_error.lock() = Some(format!(
                "browser/host-consumer-unavailable: 隔离旧宿主请求失败: {error}"
            ));
            eprintln!("[browser] 隔离旧原生浏览器宿主请求失败: {error}");
            return;
        }
        // 先隔离上次进程的瞬时请求，再按产品语义 fail-closed。门控关闭时不创建
        // watcher、不扫描请求目录，也不启动 CDP/BrowserCore 健康检查。
        if !crate::platform::capabilities::browser_product_enabled() {
            return;
        }
        // 浏览器标签操作是前台交互，不能跟随 2s 的自动化健康检查节拍。数据面
        // 请求仍由单个消费者顺序处理，避免激活/关闭乱序；lease 心跳、begin/end
        // 另走只处理内存状态的轻量通道，不能被其他会话的慢 prepare/CDP 阻塞。
        let request_app = app.clone();
        tauri::async_runtime::spawn(async move {
            let request_dir = paths::browser_host_requests_dir();
            if let Err(error) = std::fs::create_dir_all(&request_dir) {
                *request_app
                    .state::<BrowserManager>()
                    .prepare_recovery_error
                    .lock() = Some(format!(
                    "browser/host-consumer-unavailable: 创建宿主请求目录失败: {error}"
                ));
                eprintln!("[browser] 创建原生浏览器请求目录失败: {error}");
                return;
            }
            let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
            let (control_tx, mut control_rx) = tokio::sync::mpsc::unbounded_channel();
            let notify_tx = event_tx.clone();
            let notify_control_tx = control_tx.clone();
            let watcher =
                match notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if let Ok(event) = event {
                        let contains_request = event.paths.iter().any(|path| {
                            matches!(
                                path.extension().and_then(|value| value.to_str()),
                                Some("json" | "cancelled")
                            )
                        });
                        if contains_request {
                            let _ = notify_tx.send(());
                            let _ = notify_control_tx.send(());
                        }
                    }
                }) {
                    Ok(watcher) => Some(watcher),
                    Err(error) => {
                        eprintln!(
                            "[browser] 初始化原生浏览器请求监听失败，退化为周期扫描: {error}"
                        );
                        None
                    }
                };
            let _watcher = watcher.and_then(|mut watcher: RecommendedWatcher| {
                match watcher.watch(&request_dir, RecursiveMode::NonRecursive) {
                    Ok(()) => Some(watcher),
                    Err(error) => {
                        eprintln!("[browser] 监听原生浏览器请求目录失败，退化为周期扫描: {error}");
                        None
                    }
                }
            });
            let control_app = request_app.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let mgr = control_app.state::<BrowserManager>();
                    if mgr.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    if let Err(error) = mgr
                        .prepare_requested_native_control_requests(&control_app)
                        .await
                    {
                        eprintln!("[browser] 处理浏览器宿主控制请求失败: {error}");
                    }
                    tokio::select! {
                        event = control_rx.recv() => {
                            if event.is_none() {
                                break;
                            }
                            while control_rx.try_recv().is_ok() {}
                        }
                        // notify 只是低延迟优化；即使文件系统事件丢失，也必须在
                        // Windows 400ms input-heartbeat 预算内发现控制请求。
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                    }
                }
            });
            // 注册 watcher 的窄窗口里可能已有本进程新请求，启动后立即扫描一次；
            // 上进程遗留请求已由同步启动屏障隔离，不会进入这里。
            {
                let mgr = request_app.state::<BrowserManager>();
                if let Err(error) = mgr.prepare_requested_native_surfaces(&request_app).await {
                    eprintln!("[browser] 处理原生浏览器请求失败: {error}");
                    // cancellation rollback 的瞬时 WebView/I/O 失败会保留 tombstone
                    // 与 ledger record；显式排队重试，不能依赖同一文件再次触发 notify。
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let _ = event_tx.send(());
                }
            }
            loop {
                let mgr = request_app.state::<BrowserManager>();
                if mgr.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                tokio::select! {
                    event = event_rx.recv() => {
                        if event.is_none() {
                            break;
                        }
                        // 合并一次原子写/rename 产生的重复通知。
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        while event_rx.try_recv().is_ok() {}
                        if let Err(error) = mgr.prepare_requested_native_surfaces(&request_app).await {
                            eprintln!("[browser] 处理原生浏览器请求失败: {error}");
                            tokio::time::sleep(Duration::from_millis(250)).await;
                            let _ = event_tx.send(());
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
                        // notify is only a latency optimization. Inotify can lose
                        // events (watch limits, overlay filesystems, atomic rename
                        // races), so the periodic branch must also reconcile the
                        // request directory instead of becoming a no-op heartbeat.
                        if let Err(error) = mgr.prepare_requested_native_surfaces(&request_app).await {
                            eprintln!("[browser] periodic hosted request reconciliation failed: {error}");
                        }
                    }
                }
            }
        });
        // 必须走 tauri::async_runtime：setup 闭包在 wry 事件循环主线程同步调用，
        // 无 tokio runtime 上下文，裸 tokio::spawn 会 panic（there is no reactor
        // running）导致应用启动即崩。
        tauri::async_runtime::spawn(async move {
            let mut fail_count = 0u32;
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let mgr = app.state::<BrowserManager>();
                // 主进程退出后不再重连自动化端点。
                if mgr.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                // 已接入但自动化端点持续失联时，只重置 CDP 连接。用户正在看的
                // 原生表面仍由宿主持有，不能因为自动化故障销毁页面或登录状态。
                {
                    let mut inner = mgr.inner.lock().await;
                    if inner.session.is_some() {
                        let port = inner
                            .port
                            .or_else(|| inner.session.as_ref().map(|s| s.port()))
                            .unwrap_or(0);
                        if probe_cdp(port, Duration::from_millis(800)).await {
                            fail_count = 0;
                            continue;
                        }
                        // 与下方 stale 端口文件路径同口径防抖：单次探测失败可能是
                        // 系统休眠/高负载下 /json/version 瞬时超时，直接拆毁会话会
                        // 误杀用户正在看的浏览器（全部标签页与模型工作现场丢失）。
                        fail_count += 1;
                        if fail_count < 5 {
                            continue;
                        }
                        fail_count = 0;
                        eprintln!("[browser] 自动化端点失联（端口 {port}），重置 CDP 连接");
                        if let Some(task) = inner.loop_task.take() {
                            task.abort();
                        }
                        if let Some(task) = inner.reader_task.take() {
                            task.abort();
                        }
                        if let Some(session) = inner.session.take() {
                            let _ = session.close().await;
                        }
                        inner.port = None;
                        inner.active_session = None;
                        inner.active_target = None;
                        mgr.page_sessions.lock().clear();
                        mgr.activated
                            .store(false, std::sync::atomic::Ordering::SeqCst);
                        // 前端严格按任务过滤事件；全局 payload 会被每个右侧面板忽略。
                        // 对仍由原生宿主持有的每个工作区发送精确 sessionId，显示页继续
                        // 保留，仅标记 Agent 自动化暂不可用。
                        let session_ids = mgr.native_surface.lock().session_ids();
                        for session_id in session_ids {
                            let _ = app.emit(
                                "browser:automation-unavailable",
                                json!({ "sessionId": session_id, "port": port }),
                            );
                        }
                        continue;
                    }
                }
                // 未接入：只连接当前应用原生宿主发布且仍归其所有的端点。
                let Some(port) = live_port().await else {
                    fail_count = 0;
                    continue;
                };
                if !mgr.native_surface.lock().owns_port(port) {
                    continue;
                }
                fail_count = 0;
                if mgr.ensure_started().await.is_ok() {
                    mgr.activated
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                } else {
                    eprintln!("[browser] 接入原生页面自动化端点失败，稍后重试");
                }
            }
        });
    }

    // -----------------------------------------------------------------------
    // 生命周期
    // -----------------------------------------------------------------------

    /// 复用现有 browser 级连接重新激活一个页面（首检路径与 start_mtx 二次快检
    /// 共用）：重开会泄漏旧读循环/事件循环任务（无 close/abort 即永久运行），
    /// 且两条连接同时收 browser 级 Target 事件会让前端收到重复通知。
    async fn reattach_existing(
        &self,
        session: Arc<cdp::CdpSession>,
        gen: u64,
    ) -> Result<(), String> {
        let (target_id, sid) = attach_first_page_cached(&session, &self.page_sessions).await?;
        let mut inner = self.inner.lock().await;
        // attach 期间若 stop() 已执行（代际变化），弃用本次结果——否则会把
        // 旧连接的流切到新 session 上（新流启动失败叠加旧流已停 → 帧流死亡）。
        if self.stop_gen.load(std::sync::atomic::Ordering::SeqCst) != gen {
            return Err("浏览器启动期间已被停止".to_string());
        }
        switch_active_session_locked(&mut inner, &sid).await?;
        self.page_sessions.lock().insert(target_id.clone(), sid);
        inner.active_target = Some(target_id);
        Ok(())
    }

    /// 确保 Windows 原生浏览器的 CDP 自动化连接已接入。幂等：已连接则直接复用。
    pub async fn ensure_started(&self) -> Result<(), String> {
        // 主进程退出中：拒绝启动（否则退出瞬间被 watch 拉起成孤儿 Chrome）。
        if self.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("应用正在退出，不再启动浏览器".to_string());
        }
        // 等锁前的 stop 代际快照：start_mtx 等待期间 stop() 完成（代际 +1）时，
        // 拿到锁后立即放弃——否则显式停止会被进行中的启动/ watch 轮询"复活"，
        // 退出路径下还会产出无人回收的孤儿 Chrome。
        let gen_before_wait = self.stop_gen.load(std::sync::atomic::Ordering::SeqCst);
        {
            let inner = self.inner.lock().await;
            if inner.session.is_some() && inner.active_session.is_some() {
                return Ok(());
            }
            // session 仍在但 active_session 为空（最后标签页被关闭后）：
            // 复用现有连接重新激活一个页面，而不是重开第二条 WebSocket——
            // 重开会泄漏旧读循环/事件循环任务（无 close/abort 即永久运行），
            // 且两条连接同时收 browser 级 Target 事件会让前端收到重复通知。
            if inner.session.is_some() {
                let session = inner.session.clone().expect("session is_some 已检查");
                let gen = self.stop_gen.load(std::sync::atomic::Ordering::SeqCst);
                // 不持 inner 锁做网络 await（attach 走 CDP 调用）；之后重新拿锁提交状态。
                drop(inner);
                return self.reattach_existing(session, gen).await;
            }
        }

        // single-flight：整个启动序列持 start_mtx，并发调用者在此等待后复用
        // 已完成的状态，而不是各自再启动一遍（双事件循环/句柄丢失）。
        let _start_guard = self.start_mtx.lock().await;
        if self.stop_gen.load(std::sync::atomic::Ordering::SeqCst) != gen_before_wait {
            return Err("浏览器启动等待期间已被停止".to_string());
        }
        // stop 代际快照：启动期间若 stop() 执行（代际 +1），完成后丢弃本次结果。
        let gen_at_start = self.stop_gen.load(std::sync::atomic::Ordering::SeqCst);
        {
            let inner = self.inner.lock().await;
            if let Some(session) = inner.session.clone() {
                // 二次快检必须与 start_mtx 外的首检同口径：等待锁期间状态可能已
                // 变为「session 在、active 空」（如 close_tab 关掉最后一个标签页），
                // 该形态落入下方全量启动会无清理地覆盖旧 session/loop_task/
                // reader_task（第二条 WS + 双事件循环，旧任务永久泄漏）。重附着
                // 涉及网络 await 且不能再进 start_mtx，先释放两把锁再走重附着。
                if inner.active_session.is_some() {
                    return Ok(());
                }
                drop(inner);
                drop(_start_guard);
                return self.reattach_existing(session, gen_at_start).await;
            }
        }

        // 1) 只接入宿主原生工作区公布的自动化端点。这里不再自启外部 Chrome：
        // 原生宿主失败必须原样暴露，不能静默切换页面表面、身份或交互语义。
        let port = live_port()
            .await
            .ok_or_else(|| "原生浏览器自动化端点尚未就绪".to_string())?;
        if !self.native_surface.lock().owns_port(port) {
            return Err("自动化端点不属于当前应用的原生浏览器工作区".to_string());
        }
        // 2-5) 连接 CDP / attach / 启域 / 事件循环。session/reader 句柄提到
        // 闭包外，任一步失败都要关闭 WS 并中止读循环。
        let mut boot_session: Option<Arc<cdp::CdpSession>> = None;
        let mut boot_reader: Option<tokio::task::JoinHandle<()>> = None;
        let boot: Result<(), String> = async {
            let connected = cdp::connect(port)
                .await
                .map_err(|e| format!("CDP 连接失败: {e:#}"))?;
            let session = connected.session;
            boot_session = Some(Arc::clone(&session));
            boot_reader = Some(connected.reader_task);

            // 开启 Target 发现：内部状态机需要 Target.targetCreated/targetDestroyed
            // 自愈 CDP target/session 映射。UI 事件由原生宿主按任务作用域发送。
            session
                .call(
                    None,
                    "Target.setDiscoverTargets",
                    json!({ "discover": true }),
                )
                .await
                .map_err(|e| format!("Target.setDiscoverTargets 失败: {e}"))?;

            let (target_id, session_id) =
                attach_first_page_cached(&session, &self.page_sessions).await?;

            session
                .call(Some(&session_id), "Page.enable", json!({}))
                .await
                .map_err(|e| format!("Page.enable 失败: {e}"))?;
            let app = self
                .app
                .lock()
                .clone()
                .ok_or_else(|| "BrowserManager 未绑定 AppHandle".to_string())?;
            let loop_task = tokio::spawn(run_event_loop(app, connected.events));

            // 启动期间被 stop() 打断（代际已变）：丢弃本次结果，避免 stop 被吞、
            // 浏览器以无 UI 状态残留（watch 视 session alive 而不再重置）。
            // WS 关闭与读循环中止统一由下方失败路径的 boot_session/boot_reader 完成。
            if self.stop_gen.load(std::sync::atomic::Ordering::SeqCst) != gen_at_start {
                return Err("浏览器启动期间已被停止".to_string());
            }

            let mut inner = self.inner.lock().await;
            // 锁内再核对一次代际与退出标记：上方 gen 检查到拿到 inner 锁之间存在
            // 窗口，期间 stop()/shutdown_on_exit（也 bump 代际）可能已完成；等锁
            // 期间被停止/退出时若照样提交 session，会留下无人管理的连接。丢弃走
            // 统一失败清理（关闭 boot_session 并中止 boot_reader）。
            if self.stop_gen.load(std::sync::atomic::Ordering::SeqCst) != gen_at_start
                || self.shutting_down.load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err("浏览器启动期间已被停止或应用正在退出".to_string());
            }
            inner.port = Some(port);
            inner.session = Some(session);
            inner.active_session = Some(session_id);
            inner.active_target = Some(target_id);
            inner.loop_task = Some(loop_task);
            inner.reader_task = boot_reader.take();
            Ok(())
        }
        .await;

        if let Err(e) = &boot {
            // 关闭本次启动建立的 WS 并中止读循环，避免每次重试净泄漏一条连接
            // 与一个读循环任务。close 幂等（gen 中断路径已关过也无害）。
            if let Some(session) = boot_session.take() {
                let _ = session.close().await;
            }
            if let Some(task) = boot_reader.take() {
                task.abort();
            }
            // 清空 page_sessions 缓存：sessionId 是每条 WebSocket 连接私有的，本次
            // 连接已在上方关闭——残留条目会让下次 ensure_started 的新连接命中死 sid
            // （必然失败 → watch 无限重试，且 browser:activated 从未 emit，用户无
            // UI 入口触发 stop，只能重启应用）。
            self.page_sessions.lock().clear();
            return Err(e.clone());
        }
        // 成功接入：清除历史失败记录，避免 24h 内向模型注入陈旧的「浏览器不可用」
        // 原因。
        let _ = std::fs::remove_file(paths::browser_last_error_json());
        Ok(())
    }

    /// 停止浏览器：断开自动化连接、关闭应用持有的原生页面、清理协调文件并通知前端
    /// （emit `browser:stopped`，前端据此隐藏浏览器面板）。
    ///
    /// 与 `ensure_started` 共享 `start_mtx`（同序：先 start_mtx 再 inner）：stop 不会
    /// 在启动序列中途"看到空状态提前返回"而被随后完成的启动覆盖；代际 +1 让进行中的
    /// 启动在完成后自弃结果。
    pub async fn stop(&self) -> Result<(), String> {
        // 先参与 single-flight（与 ensure_started 同序获取，无死锁），保证 stop 与
        // 启动序列及原生工作区创建串行。
        let _start_guard = self.start_mtx.lock().await;
        self.stop_with_start_lock().await
    }

    /// 调用方已持有 start_mtx 的完整停止路径。供按对话关闭最后一个原生工作区时
    /// 复用，避免释放生命周期锁后新工作区插入、再被旧的全局清理误伤。
    async fn stop_with_start_lock(&self) -> Result<(), String> {
        // +1 代际，让已被本 stop 打断的启动完成后自弃。
        self.stop_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Do not clear the publication bit before irreversible native close:
        // if close fails, a retry must still emit the eventual stopped event.
        let was_activated = self.activated.load(std::sync::atomic::Ordering::SeqCst);

        let mut inner = self.inner.lock().await;
        let had_session = inner.session.is_some();
        let native_initialized = {
            let surface = self.native_surface.lock();
            inner
                .port
                .map(|port| surface.owns_port(port))
                // 原生页面可能已创建但 CDP watch 尚未把端口提交到 inner；此时
                // 关闭最后一个工作区仍必须清理原生 runtime 和协调文件。
                .unwrap_or_else(|| surface.is_initialized())
        };
        // CDP 仅是应用内原生页面的自动化通道，不能用 Browser.close 关闭整个
        // WebView 运行时；先断开连接，再由宿主精确销毁其子视图。
        if let Some(session) = inner.session.take() {
            let _ = session.close().await;
        }
        if native_initialized {
            let app = self.app.lock().clone();
            self.native_surface.lock().close(app.as_ref())?;
            let _ = std::fs::remove_file(paths::browser_cdp_port_json());
        }
        // BrowserCore's WebDriver is process-shared across task workspaces. This full-stop path
        // is reached only for global stop or after the last native workspace has closed; scoped
        // close with remaining workspaces never comes here.
        platform::shutdown_browser_core_for_stop().await;
        if let Some(task) = inner.loop_task.take() {
            task.abort();
        }
        if let Some(task) = inner.reader_task.take() {
            task.abort();
        }
        inner.port = None;
        inner.active_session = None;
        inner.active_target = None;
        self.page_sessions.lock().clear();
        self.activated
            .store(false, std::sync::atomic::Ordering::SeqCst);
        // 通知前端隐藏浏览器 Tab（main.jsx / BrowserView 监听 browser:stopped）。
        // 仅在浏览器确实运行过/激活过时通知：从未启动的 stop（如 RunEvent::Exit
        // 兜底路径）不产生伪事件。
        if had_session || was_activated {
            if let Some(app) = self.app.lock().clone() {
                let _ = app.emit("browser:stopped", json!({}));
            }
        }
        self.persistence_warnings.lock().clear();
        self.persistence_retries.lock().clear();
        Ok(())
    }

    /// 停止当前对话的原生浏览器。还有其他对话页面时只销毁本页并保留共享
    /// WebView2 环境；最后一个页面关闭时复用完整 stop 路径清理 CDP 与协调文件。
    pub async fn stop_for_session(&self, browser_session_id: &str) -> Result<(), String> {
        // 与创建/全局停止串行：关闭最后一个工作区到共享 runtime 清理之间不能插入
        // 新工作区，否则旧关闭请求会把刚创建的其他对话一并销毁。
        let _start_guard = self.start_mtx.lock().await;
        let app = self.app.lock().clone();
        let action = {
            let surface = self.native_surface.lock();
            scoped_stop_action(
                surface.owns_session_resources(browser_session_id),
                surface.has_sessions(),
            )
        };
        let mut result = match action {
            ScopedStopAction::IgnoreUnknownNativeSession => {
                // 与导航/标签变更的恢复点写入使用同一锁；否则删除清单可能被
                // 迟到的持久化任务重新写入。
                let _persistence_guard = self.persistence_io.lock();
                platform::NativeBrowserSurface::delete_restore_workspace(browser_session_id)
            }
            // 注册表为空时清理共享自动化运行时；该分支不会触碰其他对话工作区。
            ScopedStopAction::StopManagedRuntime => {
                // 恢复点删除是“停止”语义的持久提交点。它失败时不能先销毁运行时，
                // 否则命令虽然报错，下次启动却仍会恢复已经被用户关掉的页面。
                {
                    let _persistence_guard = self.persistence_io.lock();
                    platform::NativeBrowserSurface::delete_restore_workspace(browser_session_id)?;
                }
                self.stop_with_start_lock().await
            }
            ScopedStopAction::CloseNativeSession => {
                // 在不可逆关闭 WebView 前先提交“不要再恢复”。宿主逐页对账：成功关闭
                // 的页面立即从注册表删除，失败项保留为 survivor 并重写恢复清单，下一次
                // stop 只重试真实 survivor，不能把已物理关闭的旧页面重新写回。
                let has_remaining = {
                    // 该锁只覆盖同步的“读旧清单→删除→关闭/补偿”提交区间，不能跨
                    // 下方 async runtime stop；否则 Tauri command future 将不再 Send。
                    let _persistence_guard = self.persistence_io.lock();
                    // 原生锁必须在读取/删除恢复点前取得：UI 标签操作虽不经过
                    // persistence_io，但都会经过 native_surface。否则它可能在“删除”
                    // 与“关闭”之间写回一份新清单，造成已停止页面下次启动复活。
                    let mut surface = self.native_surface.lock();
                    platform::NativeBrowserSurface::delete_restore_workspace(browser_session_id)?;
                    let close_result =
                        surface.close_session_preserving_restore(app.as_ref(), browser_session_id);
                    match close_result {
                        Ok(has_remaining) => has_remaining,
                        Err(close_error) => return Err(close_error),
                    }
                };
                if !has_remaining {
                    self.stop_with_start_lock().await
                } else {
                    if let Some(app) = app {
                        let _ = app.emit(
                            "browser:stopped",
                            json!({ "sessionId": browser_session_id }),
                        );
                    }
                    Ok(())
                }
            }
        };
        if result.is_ok() {
            match remove_hosted_prepare_journal_for_session(browser_session_id) {
                Ok(()) => {
                    let session_token = paths::browser_session_token(browser_session_id);
                    self.locally_committed_prepares
                        .lock()
                        .retain(|(token, _)| token != &session_token);
                }
                Err(error) => result = Err(error),
            }
        }
        if result.is_ok() {
            self.clear_persistence_state(browser_session_id);
        }
        result
    }

    /// 删除任务时使用的完整浏览器清理。普通 UI “关闭浏览器”保留该任务的 MCP
    /// 配置，任务删除则在 WebView/restore 清理成功后再删除配置；NotFound 幂等成功，
    /// 其他 I/O 错误返回给 composition-root 重试队列。
    pub async fn delete_for_session(&self, browser_session_id: &str) -> Result<(), String> {
        self.stop_for_session(browser_session_id).await?;
        match std::fs::remove_file(paths::browser_session_mcp_json(browser_session_id)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("删除任务浏览器 MCP 配置失败: {error}")),
        }
    }

    /// 进程启动时用当前仍存在的任务集合清理上次崩溃遗留的浏览器文件。磁盘文件名
    /// 只含 session token，因此先在内存中对 active session ID 做同样散列；不需要
    /// 把原始 session ID 写进恢复清单。任一 I/O 失败都会聚合返回，composition root
    /// 可用有限退避持续重试；单个孤儿已删除后再次执行仍幂等。
    pub fn reconcile_session_files(&self, active_session_ids: &[String]) -> Result<(), String> {
        let active_tokens = active_session_ids
            .iter()
            .map(|session_id| paths::browser_session_token(session_id))
            .collect::<HashSet<_>>();
        reconcile_browser_session_file_dirs(
            &active_tokens,
            &[
                paths::browser_workspace_restore_dir(),
                paths::browser_workspaces_dir(),
                paths::browser_session_mcp_dir(),
                hosted_prepare_journal_dir(),
            ],
            self.startup_reconcile_cutoff,
        )
    }

    /// `Target.targetCreated` 补激活（由事件循环调用）：全部标签页被关闭后
    /// （active 为空）模型经 MCP 新建标签页时，自动把自动化会话接到新页。
    async fn on_target_created(&self, target_id: &str) {
        let gen = self.stop_gen.load(std::sync::atomic::Ordering::SeqCst);
        let session = {
            let inner = self.inner.lock().await;
            if inner.active_session.is_some() {
                return; // 已有激活页：新建的是后台标签页，不动用户正在看的页面
            }
            let Some(session) = inner.session.clone() else {
                return;
            };
            session
        };
        // 不持 inner 锁做 attach（CDP 网络 await），完成后重新拿锁提交。
        let Ok(sid) = attach_page_cached(&session, &self.page_sessions, target_id).await else {
            return;
        };
        let mut inner = self.inner.lock().await;
        if self.stop_gen.load(std::sync::atomic::Ordering::SeqCst) != gen {
            return; // attach 期间被 stop：弃用结果
        }
        if inner.active_session.is_some() {
            return; // 并发路径已激活其他页
        }
        if switch_active_session_locked(&mut inner, &sid).await.is_ok() {
            inner.active_target = Some(target_id.to_string());
        }
    }

    /// `Target.targetDestroyed` 自愈（由事件循环调用）：激活标签页被 MCP/页面
    /// 脚本关闭时切到剩余页；无剩余页则清空 active——下次
    /// `ensure_started` 经 `reattach_existing` 复用连接重附着（不再冻结在
    /// 已销毁 target 的最后一帧上）。close_tab 主动关闭已先行处理，此处幂等。
    async fn on_target_destroyed(&self, target_id: &str) {
        self.page_sessions.lock().remove(target_id);
        let mut inner = self.inner.lock().await;
        if inner.active_target.as_deref() != Some(target_id) {
            return;
        }
        let Some(session) = inner.session.clone() else {
            inner.active_session = None;
            inner.active_target = None;
            return;
        };
        // 枚举剩余页（刚销毁的 target 可能仍在 Chrome 列表中，显式排除；
        // attach 失败的将死 target 由 list_page_tabs 内部跳过）。
        if let Ok(tabs) = list_page_tabs(&session, &self.page_sessions).await {
            if let Some(first) = tabs.iter().find(|t| t.target_id != target_id) {
                let sid = self.page_sessions.lock().get(&first.target_id).cloned();
                if let Some(sid) = sid {
                    if switch_active_session_locked(&mut inner, &sid).await.is_ok() {
                        inner.active_target = Some(first.target_id.clone());
                        return;
                    }
                }
            }
        }
        // 无剩余页（或切换失败）：被销毁 target 的 flatten session 已随它失效，
        // 无需停流，直接清空 active 等下次重附着。
        inner.active_session = None;
        inner.active_target = None;
    }

    /// 主进程退出时的同步兜底清理：关闭应用持有的原生页面、截断自动化连接并
    /// 清理协调文件。原生页面生命周期不依赖外部浏览器进程。
    ///
    /// 该方法由 Tauri 主线程调用，绝不能等待浏览器锁：持久化 worker 可能正持
    /// `native_surface` 等待主线程执行 WebView getter。锁竞争时保留最近一次原子
    /// restore 快照，WebView 交给进程退出销毁；协调文件仍无条件清理。
    pub fn shutdown_on_exit(&self) {
        // 先置退出标记：watch 下一轮退出、ensure_started 拒绝新连接。
        self.shutting_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // 同时 bump stop 代际：进行中的启动序列（网络 await 阶段不持 inner 锁，
        // 下方 try_lock 会成功空转）在提交点前核对该代际，不等则丢弃结果并走
        // 失败清理——否则退出瞬间在飞的启动会把连接提交进已清空的 inner。
        self.stop_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        platform::shutdown_browser_core_for_exit();
        let app = self.app.try_lock().and_then(|app| app.as_ref().cloned());
        if let Some(_persistence_io) = self.persistence_io.try_lock() {
            if let Some(mut surface) = self.native_surface.try_lock() {
                if surface.is_initialized() {
                    if let Some(app) = app.as_ref() {
                        if let Err(error) = surface.persist_all_restore(app) {
                            // 导航/标签变更已持续写盘；退出快照失败时保留上一份完整清单，
                            // 不以部分写入或删除破坏已有恢复点。
                            eprintln!("[browser] 退出时刷新浏览器恢复清单失败: {error}");
                        }
                    }
                    if let Err(error) = surface.close_preserving_restore(app.as_ref()) {
                        eprintln!("[browser] 退出时关闭原生浏览器页面失败: {error}");
                    }
                }
            } else {
                eprintln!(
                    "[browser] 退出时原生浏览器状态锁被占用，保留最近恢复点并交由进程销毁页面"
                );
            }
        } else {
            eprintln!("[browser] 退出时恢复持久化正在进行，保留最近恢复点并交由进程销毁页面");
        }

        if let Ok(mut inner) = self.inner.try_lock() {
            if let Some(task) = inner.loop_task.take() {
                task.abort();
            }
            if let Some(task) = inner.reader_task.take() {
                task.abort();
            }
            if let Some(session) = inner.session.take() {
                // 尽力关闭 WS 截断读循环（退出事件不会等待异步关闭完成）。
                let session = Arc::clone(&session);
                tauri::async_runtime::spawn(async move { session.close().await });
            }
            inner.port = None;
            inner.active_session = None;
            inner.active_target = None;
        } else {
            eprintln!("[browser] 退出时自动化状态锁被占用，交由进程退出回收连接");
        }
        if let Some(mut page_sessions) = self.page_sessions.try_lock() {
            page_sessions.clear();
        }
        self.activated
            .store(false, std::sync::atomic::Ordering::SeqCst);
        // 只有 app 会发布该端口；即使任一内存锁繁忙，退出后端点也必然失效。
        let _ = std::fs::remove_file(paths::browser_cdp_port_json());
        clear_host_request_files();
    }

    /// 查询状态（前端挂载/轮询用）。`activeTab` 为激活标签页的 targetId
    /// （前端标签身份统一用 targetId；sessionId 每次 attach 都不同，不可作身份）。
    pub async fn status(&self, browser_session_id: &str) -> Value {
        if !crate::platform::capabilities::browser_product_enabled() {
            return json!({
                "running": false,
                "sessionId": browser_session_id,
                "native": false,
                "missing": true,
                "unavailable": true,
            });
        }
        let restore_error = match self.restore_saved_workspace(browser_session_id).await {
            Ok(_) => None,
            Err(error) => {
                eprintln!("[browser] 恢复对话浏览器失败: {error}");
                Some(error)
            }
        };
        let app = self.app.lock().clone();
        let (session_state, control_state) = {
            let surface = self.native_surface.lock();
            (
                surface.session_state(app.as_ref(), browser_session_id),
                surface.control_state(browser_session_id),
            )
        };
        if let Some((token, mut url)) = session_state {
            if url.contains("#pinvou-session-") || url.contains("#pinvou-tab-") {
                url = "about:blank".to_string();
            }
            let mut status = json!({
                "running": true,
                "activeTab": token,
                "url": url,
                "sessionId": browser_session_id,
                "native": true,
            });
            if let Some(control) = control_state {
                status["controlOwner"] = json!(control.owner);
                status["controlRevision"] = json!(control.revision);
            }
            if let Some(warning) = self
                .persistence_warnings
                .lock()
                .get(browser_session_id)
                .cloned()
            {
                status["persistenceWarning"] = json!(warning);
            }
            return status;
        }
        if let Some(error) = restore_error {
            return json!({
                "running": false,
                "sessionId": browser_session_id,
                "native": true,
                "restoreError": error,
            });
        }
        json!({
            "running": false,
            "sessionId": browser_session_id,
            "native": true,
            "missing": true,
        })
    }

    /// UI hand-back is the immediate-resume shortcut and atomically signs a fresh
    /// Agent lease. Idle auto-release only changes owner after its revision guard
    /// succeeds; the next Agent activation signs its own lease. Opaque leases are
    /// never exposed to the React renderer.
    pub fn hand_back_to_agent(&self, browser_session_id: &str) -> Result<Value, String> {
        let app = self
            .app
            .lock()
            .clone()
            .ok_or_else(|| "应用句柄尚未就绪".to_string())?;
        let control = {
            let mut surface = self.native_surface.lock();
            if surface
                .hand_back_to_agent(Some(&app), browser_session_id)?
                .is_none()
            {
                return Err("指定对话的原生浏览器工作区不存在".to_string());
            }
            surface
                .control_state(browser_session_id)
                .ok_or_else(|| "浏览器控制权状态不可用".to_string())?
        };
        self.persist_native_restore_best_effort(browser_session_id);
        Ok(json!({
            "sessionId": browser_session_id,
            "controlOwner": control.owner,
            "controlRevision": control.revision,
        }))
    }

    pub(crate) fn release_user_control_if_idle(
        &self,
        browser_session_id: &str,
        expected_revision: u64,
    ) -> Result<bool, String> {
        let app = self
            .app
            .lock()
            .clone()
            .ok_or_else(|| "应用句柄尚未就绪".to_string())?;
        self.native_surface.lock().release_user_control_if_idle(
            &app,
            browser_session_id,
            expected_revision,
        )
    }

    /// 标签页列表（实时枚举 page 类型 target；attach 复用缓存，防 session 泄漏）。
    pub async fn list_tabs(&self, browser_session_id: &str) -> Result<Vec<TabInfo>, String> {
        let app = self.app.lock().clone();
        self.native_surface
            .lock()
            .list_tabs(app.as_ref(), browser_session_id)
            .ok_or_else(|| "指定对话的原生浏览器工作区不存在".to_string())
    }

    /// Resolve a hidden marker WebView to this process's automation identity without exposing
    /// it first. Linux binds the concrete WebKit WebView through BrowserCore/WebDriver; Windows
    /// keeps the existing CDP marker discovery path.
    async fn bind_staged_native_target(
        &self,
        app: &AppHandle,
        browser_session_id: &str,
        tab_token: &str,
    ) -> Result<String, String> {
        if platform::browser_core_available() {
            platform::wait_browser_core_ready().await?;
            let label = self
                .native_surface
                .lock()
                .webview_label_for_tab(browser_session_id, tab_token)
                .ok_or_else(|| "browser/native-staged-surface-missing".to_string())?;
            let webview = app
                .get_webview(&label)
                .ok_or_else(|| "browser/native-staged-surface-missing".to_string())?;
            platform::bind_browser_core_webview(&webview).await?;
            return Ok(format!("native:{tab_token}"));
        }

        let port = live_port()
            .await
            .ok_or_else(|| "原生浏览器自动化端点尚未就绪".to_string())?;
        if !self.native_surface.lock().owns_port(port) {
            return Err("自动化端点不属于当前应用的原生浏览器工作区".to_string());
        }
        discover_native_target(port, tab_token).await
    }

    fn rollback_staged_agent_tab(
        &self,
        app: &AppHandle,
        browser_session_id: &str,
        tab_token: &str,
        creation_id: &str,
    ) -> Result<(), String> {
        match self.native_surface.lock().rollback_created_tab(
            Some(app),
            browser_session_id,
            tab_token,
            creation_id,
        )? {
            true => Ok(()),
            false => Err("隐藏候选标签在绑定失败补偿前已丢失".to_string()),
        }
    }

    fn rollback_staged_user_tab(
        &self,
        app: &AppHandle,
        browser_session_id: &str,
        tab_token: &str,
    ) -> Result<(), String> {
        match self.native_surface.lock().rollback_user_created_tab(
            Some(app),
            browser_session_id,
            tab_token,
        )? {
            true => Ok(()),
            false => Err("用户隐藏候选标签在绑定失败补偿前已丢失".to_string()),
        }
    }

    /// Embedded WebView popups are always denied at the engine boundary and recreated as
    /// task-owned tabs. A popup raised inside an already-begun Agent dispatch carries the
    /// complete in-memory host lease through hidden staging and final CAS; a page popup with
    /// no such authorization is deliberately published as User-owned.
    pub(crate) async fn create_popup_tab(
        &self,
        browser_session_id: &str,
        url: String,
        authorization: Option<RetainedAgentOperation>,
    ) -> Result<String, String> {
        if let Some(retained) = authorization {
            let result = async {
                if !is_allowed_url(&url) {
                    return Err("仅支持 http/https/about:blank 协议".to_string());
                }
                let app = self
                    .app
                    .lock()
                    .clone()
                    .ok_or_else(|| "应用句柄尚未就绪".to_string())?;
                self.create_agent_popup_tab(&app, browser_session_id, url, &retained)
                    .await
            }
            .await;
            // The callback retained one exact holder before spawning. Release
            // it for every validation/setup/bind/commit outcome, including an
            // invalid URL or missing AppHandle before staging starts.
            self.native_surface
                .lock()
                .release_popup_agent_operation(&retained);
            return result;
        }
        if !is_allowed_url(&url) {
            return Err("仅支持 http/https/about:blank 协议".to_string());
        }
        let app = self
            .app
            .lock()
            .clone()
            .ok_or_else(|| "应用句柄尚未就绪".to_string())?;
        self.create_native_bound_tab(&app, browser_session_id, url, false)
            .await
    }

    async fn create_agent_popup_tab(
        &self,
        app: &AppHandle,
        browser_session_id: &str,
        url: String,
        retained: &RetainedAgentOperation,
    ) -> Result<String, String> {
        let authorization = retained.authorization();
        if authorization.session_id != browser_session_id {
            return Err("popup Agent lease 与当前对话不一致".to_string());
        }
        let caller_epoch = retained.caller_epoch();
        let session_token = paths::browser_session_token(browser_session_id);
        ensure_hosted_caller_epoch_live(
            browser_session_id,
            &session_token,
            caller_epoch.caller_pid(),
            caller_epoch.wrapper_instance_nonce(),
        )?;
        if !self
            .native_surface
            .lock()
            .authorize_popup_agent_operation(retained)
        {
            return Err("popup Agent operation holder 已失效".to_string());
        }
        let tab_token = self.native_surface.lock().generate_tab_token();
        let creation_id = format!(
            "popup-{:016x}{:016x}",
            rand::random::<u64>(),
            rand::random::<u64>()
        );
        let marker = format!("about:blank#pinvou-tab-{tab_token}");
        let created = self.native_surface.lock().create_tab_for_agent(
            app,
            browser_session_id,
            &tab_token,
            &marker,
            false,
            authorization,
            &creation_id,
        )?;
        if created.is_none() {
            return Err("指定对话的原生浏览器工作区不存在".to_string());
        }

        if let Err(error) = ensure_hosted_caller_epoch_live(
            browser_session_id,
            &session_token,
            caller_epoch.caller_pid(),
            caller_epoch.wrapper_instance_nonce(),
        ) {
            return match self.rollback_staged_agent_tab(
                app,
                browser_session_id,
                &tab_token,
                &creation_id,
            ) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
            };
        }
        if !self
            .native_surface
            .lock()
            .authorize_popup_agent_operation(retained)
        {
            return match self.rollback_staged_agent_tab(
                app,
                browser_session_id,
                &tab_token,
                &creation_id,
            ) {
                Ok(()) => Err("popup Agent operation holder 已失效".to_string()),
                Err(rollback_error) => Err(format!(
                    "popup Agent operation holder 已失效; {rollback_error}"
                )),
            };
        }

        let target_id = match self
            .bind_staged_native_target(app, browser_session_id, &tab_token)
            .await
        {
            Ok(target_id) => target_id,
            Err(error) => {
                return match self.rollback_staged_agent_tab(
                    app,
                    browser_session_id,
                    &tab_token,
                    &creation_id,
                ) {
                    Ok(()) => Err(error),
                    Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
                };
            }
        };
        // The retained operation keeps lease provenance across the async bind,
        // but caller liveness must still be re-read at the final native commit
        // boundary. A SIGKILL immediately after this check remains an explicit
        // acknowledgement-unknown window, not an at-most-once proof.
        if let Err(error) = ensure_hosted_caller_epoch_live(
            browser_session_id,
            &session_token,
            caller_epoch.caller_pid(),
            caller_epoch.wrapper_instance_nonce(),
        ) {
            return match self.rollback_staged_agent_tab(
                app,
                browser_session_id,
                &tab_token,
                &creation_id,
            ) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
            };
        }
        if !self
            .native_surface
            .lock()
            .authorize_popup_agent_operation(retained)
        {
            return match self.rollback_staged_agent_tab(
                app,
                browser_session_id,
                &tab_token,
                &creation_id,
            ) {
                Ok(()) => Err("popup Agent operation holder 已失效".to_string()),
                Err(rollback_error) => Err(format!(
                    "popup Agent operation holder 已失效; {rollback_error}"
                )),
            };
        }
        if !self.native_surface.lock().commit_created_tab_for_agent(
            app,
            browser_session_id,
            &tab_token,
            &target_id,
            &url,
            false,
            authorization,
            &creation_id,
            Some(retained),
            || {
                ensure_hosted_caller_epoch_live(
                    browser_session_id,
                    &session_token,
                    caller_epoch.caller_pid(),
                    caller_epoch.wrapper_instance_nonce(),
                )
            },
        )? {
            return Err("popup 标签页在提交前已关闭".to_string());
        }
        let _ = app.emit(
            "browser:tabs-changed",
            json!({ "sessionId": browser_session_id, "tab": tab_token }),
        );
        self.persist_native_restore_best_effort(browser_session_id);
        Ok(tab_token)
    }

    async fn create_native_bound_tab(
        &self,
        app: &AppHandle,
        browser_session_id: &str,
        url: String,
        background: bool,
    ) -> Result<String, String> {
        let tab_token = self.native_surface.lock().generate_tab_token();
        // Always create on a private marker first. Publishing/navigating a
        // remote page before target binding would make the strict v2 host
        // mapping disappear and break every Agent tool after the user clicks
        // the toolbar "+" button.
        let marker = format!("about:blank#pinvou-tab-{tab_token}");
        let created = {
            let mut surface = self.native_surface.lock();
            surface.create_tab(app, browser_session_id, &tab_token, &marker, background)?
        };
        let Some(created) = created else {
            return Err("指定对话的原生浏览器工作区不存在".to_string());
        };
        let binding = async {
            let target_id = self
                .bind_staged_native_target(app, browser_session_id, &tab_token)
                .await?;
            if !self.native_surface.lock().bind_target(
                browser_session_id,
                &tab_token,
                &target_id,
            )? {
                return Err("新建标签页在绑定自动化 target 前已关闭".to_string());
            }
            if !self.native_surface.lock().navigate_tab_after_bind(
                Some(app),
                browser_session_id,
                &tab_token,
                &url,
            )? {
                return Err("新建标签页在导航前已关闭".to_string());
            }
            Ok::<(), String>(())
        }
        .await;
        if let Err(error) = binding {
            return match self.rollback_staged_user_tab(app, browser_session_id, &tab_token) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!("{error}; {rollback_error}")),
            };
        }
        let _ = app.emit(
            "browser:tabs-changed",
            json!({ "sessionId": browser_session_id, "tab": created }),
        );
        self.persist_native_restore_best_effort(browser_session_id);
        Ok(created)
    }

    /// 在指定对话的原生浏览器工作区中新建标签页。
    pub async fn create_tab(
        &self,
        browser_session_id: &str,
        url: String,
        background: bool,
    ) -> Result<String, String> {
        // 与 navigate 同款协议白名单：防 file:///javascript: 等本地/脚本协议被注入。
        if !is_allowed_url(&url) {
            return Err("仅支持 http/https/about:blank 协议".to_string());
        }
        let app = self
            .app
            .lock()
            .clone()
            .ok_or_else(|| "应用句柄尚未就绪".to_string())?;
        self.create_native_bound_tab(&app, browser_session_id, url, background)
            .await
    }

    /// 关闭标签页。仅当关掉的是当前激活页时才自动切到第一个剩余页——关后台
    /// 标签页不应动用户正在看的页面。
    pub async fn close_tab(
        &self,
        browser_session_id: &str,
        target_id: String,
    ) -> Result<(), String> {
        let app = self.app.lock().clone();
        if self
            .native_surface
            .lock()
            .close_tab(app.as_ref(), browser_session_id, &target_id)?
        {
            if let Some(app) = app {
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": browser_session_id }),
                );
            }
            self.persist_native_restore_best_effort(browser_session_id);
            return Ok(());
        }
        Err("指定对话或标签页的原生浏览器工作区不存在".to_string())
    }

    /// 切换激活标签页（targetId）。
    pub async fn activate_tab(
        &self,
        browser_session_id: &str,
        target_id: String,
    ) -> Result<(), String> {
        let app = self.app.lock().clone();
        if self
            .native_surface
            .lock()
            .activate_tab(app.as_ref(), browser_session_id, &target_id)?
        {
            if let Some(app) = app {
                let _ = app.emit(
                    "browser:tabs-changed",
                    json!({ "sessionId": browser_session_id, "tab": target_id }),
                );
            }
            self.persist_native_restore_best_effort(browser_session_id);
            return Ok(());
        }
        Err("指定对话或标签页的原生浏览器工作区不存在".to_string())
    }

    // -----------------------------------------------------------------------
    // 导航 / 交互
    // -----------------------------------------------------------------------

    /// 导航到指定 URL。
    pub async fn navigate(&self, browser_session_id: &str, url: String) -> Result<(), String> {
        if !is_allowed_url(&url) {
            return Err("仅支持 http/https/about:blank 协议".to_string());
        }
        let app = self.app.lock().clone();
        if self
            .native_surface
            .lock()
            .navigate(app.as_ref(), browser_session_id, &url)?
        {
            return Ok(());
        }
        Err("指定对话的原生浏览器工作区不存在".to_string())
    }

    pub async fn go_back(&self, browser_session_id: &str) -> Result<(), String> {
        self.history_step(browser_session_id, -1).await
    }

    pub async fn go_forward(&self, browser_session_id: &str) -> Result<(), String> {
        self.history_step(browser_session_id, 1).await
    }

    async fn history_step(&self, browser_session_id: &str, delta: i64) -> Result<(), String> {
        let app = self.app.lock().clone();
        if self.native_surface.lock().history_step(
            app.as_ref(),
            browser_session_id,
            delta.signum() as i8,
        )? {
            return Ok(());
        }
        Err("指定对话的原生浏览器工作区不存在".to_string())
    }

    pub async fn reload(&self, browser_session_id: &str) -> Result<(), String> {
        let app = self.app.lock().clone();
        if self
            .native_surface
            .lock()
            .reload(app.as_ref(), browser_session_id)?
        {
            return Ok(());
        }
        Err("指定对话的原生浏览器工作区不存在".to_string())
    }
}

// ---------------------------------------------------------------------------
// 事件循环：只维护内部 target 生命周期状态。面向 UI 的导航、标题和标签事件
// 由原生宿主按 sessionId 发送，禁止从全局 CDP 连接广播无任务归属的事件。
// ---------------------------------------------------------------------------
async fn run_event_loop(app: AppHandle, mut events: tokio::sync::mpsc::Receiver<cdp::CdpEvent>) {
    use cdp::CdpEvent;
    while let Some(ev) = events.recv().await {
        match ev {
            CdpEvent::Event {
                session_id: _,
                method,
                params,
            } => match method.as_str() {
                "Target.targetCreated" | "Target.targetDestroyed" => {
                    // 协议形状差异与路由判定见 route_target_event：created 带完整
                    // targetInfo（可过滤非页面 target），destroyed 只有 { targetId }。
                    match route_target_event(&method, &params) {
                        TargetEventRoute::Ignore => continue,
                        // 激活页被（MCP/页面脚本）销毁时先自愈切换，再通知前端刷新。
                        TargetEventRoute::Destroy(tid) => {
                            app.state::<BrowserManager>()
                                .on_target_destroyed(&tid)
                                .await;
                        }
                        // 全部标签页关闭后模型新建标签页：自动补激活新页。
                        TargetEventRoute::Create(tid) => {
                            app.state::<BrowserManager>().on_target_created(&tid).await;
                        }
                    }
                }
                _ => {}
            },
        }
    }
}

// ---------------------------------------------------------------------------
// 内部工具（free functions，便于无 &self 时调用）
// ---------------------------------------------------------------------------

fn reconcile_browser_session_file_dirs(
    active_tokens: &HashSet<String>,
    directories: &[PathBuf],
    startup_cutoff: SystemTime,
) -> Result<(), String> {
    let mut errors = Vec::new();
    for directory in directories {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                errors.push(format!("读取 {} 失败: {error}", directory.display()));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    errors.push(format!("枚举 {} 失败: {error}", directory.display()));
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Some(token) = path.file_stem().and_then(|value| value.to_str()) else {
                errors.push(format!(
                    "浏览器会话文件名不是有效 UTF-8: {}",
                    path.display()
                ));
                continue;
            };
            if active_tokens.contains(token) {
                continue;
            }
            let is_removable_file = entry
                .file_type()
                .map(|kind| kind.is_file() || kind.is_symlink())
                .unwrap_or(false);
            if !is_removable_file {
                continue;
            }
            let modified =
                match std::fs::symlink_metadata(&path).and_then(|metadata| metadata.modified()) {
                    Ok(modified) => modified,
                    Err(error) => {
                        // 无法证明是上进程遗留文件时 fail-safe 保留，下一启动重试。
                        errors.push(format!(
                            "读取浏览器文件时间 {} 失败: {error}",
                            path.display()
                        ));
                        continue;
                    }
                };
            if modified >= startup_cutoff {
                // 当前进程可能在 active-session 静态快照之后刚创建该文件；绝不能
                // 让后台 reconciliation 用旧 token 集误删新任务状态。
                continue;
            }
            if let Err(error) = std::fs::remove_file(&path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    errors.push(format!(
                        "删除孤儿浏览器文件 {} 失败: {error}",
                        path.display()
                    ));
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// browser 级 Target 事件的路由判定（纯函数，便于单测协议形状）。
///
/// 协议形状差异（实测/协议文档）：`Target.targetCreated` 的 params 携带完整
/// `targetInfo`（可按 type 过滤掉 iframe/worker 等非页面 target，避免把内部状态机
/// 暴露给无关 target 的事件风暴）；
/// `Target.targetDestroyed` 的 params **只有 `{ targetId }`**，没有 targetInfo——
/// 对它按 targetInfo.type 过滤会把全部销毁事件丢弃（type 恒取不到），激活页销毁
/// 自愈（on_target_destroyed）即成为死代码。销毁事件不做类型过滤：
/// on_target_destroyed 内部（page_sessions 删除 + active_target 比对）本身幂等，
/// 非页面 target 传入无害。
#[derive(Debug, PartialEq, Eq)]
enum TargetEventRoute {
    Create(String),
    Destroy(String),
    Ignore,
}

fn route_target_event(method: &str, params: &Value) -> TargetEventRoute {
    match method {
        "Target.targetCreated" => {
            let info = params.get("targetInfo");
            if info.and_then(|i| i.get("type")).and_then(Value::as_str) != Some("page") {
                return TargetEventRoute::Ignore;
            }
            match info.and_then(|i| i.get("targetId")).and_then(Value::as_str) {
                Some(tid) => TargetEventRoute::Create(tid.to_string()),
                None => TargetEventRoute::Ignore,
            }
        }
        "Target.targetDestroyed" => match params.get("targetId").and_then(Value::as_str) {
            Some(tid) => TargetEventRoute::Destroy(tid.to_string()),
            None => TargetEventRoute::Ignore,
        },
        _ => TargetEventRoute::Ignore,
    }
}

/// 导航/新建标签页的 URL 协议白名单（UI 与宿主 WebView 回调共用）：http/https/about:blank，
/// 大小写不敏感（与前端地址栏预检 `/^https?:\/\//i` 同口径）；file:/
/// javascript:/data:/chrome: 等本地/脚本协议一律拒绝（fail-closed）。
/// 已发布和未发布的原生页面都会经过同一回调校验，因此 Tauri release/dev
/// 特权 origin 同样不能由 MCP 导航绕过。
fn is_allowed_url(url: &str) -> bool {
    let Ok(parsed) = tauri::Url::parse(url) else {
        return false;
    };
    match parsed.scheme() {
        // Tauri serves the privileged application UI from `tauri.localhost`
        // in Windows release builds and from the configured Vite origin while
        // developing. A remote browser tab must never turn itself into a
        // second local application UI. The capability manifest separately
        // scopes IPC to named application webviews as a defense in depth.
        "http" | "https" => parsed.host_str().is_some_and(|host| {
            let reserved_release_origin = host.eq_ignore_ascii_case("tauri.localhost");
            let reserved_dev_host = host.eq_ignore_ascii_case("localhost")
                || host == "127.0.0.1"
                || host == "[::1]"
                || host == "::1";
            let reserved_dev_origin =
                reserved_dev_host && parsed.port_or_known_default() == Some(1420);
            !reserved_release_origin && !reserved_dev_origin
        }),
        // WebView 标签使用 fragment 保存本机随机标记；它仍然是 about:blank，
        // 但 about:config/about:srcdoc 等其他内部页面必须保持拒绝。
        "about" => parsed.path() == "blank" && parsed.query().is_none(),
        _ => false,
    }
}

/// attach 指定页面 target，复用缓存中已有的 flatten session。CDP 对同一 target
/// 的每次 attach 都产生独立 session 且不自动释放——无缓存时高频枚举（每次
/// tabs-changed 触发前端刷新）会无界泄漏 Chrome 侧 session。
async fn attach_page_cached(
    session: &CdpSession,
    pages: &PageSessions,
    target_id: &str,
) -> Result<String, String> {
    if let Some(sid) = pages.lock().get(target_id) {
        return Ok(sid.clone());
    }
    let sid = session
        .call(
            None,
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
        )
        .await
        .map_err(|e| format!("attach 失败: {e}"))?
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if sid.is_empty() {
        return Err("attachToTarget 未返回 sessionId".to_string());
    }
    pages.lock().insert(target_id.to_string(), sid.clone());
    Ok(sid)
}

async fn attach_first_page_cached(
    session: &CdpSession,
    pages: &PageSessions,
) -> Result<(String, String), String> {
    let targets = session
        .call(None, "Target.getTargets", json!({}))
        .await
        .map_err(|e| format!("Target.getTargets 失败: {e}"))?;
    let mut page_id: Option<String> = None;
    if let Some(infos) = targets.get("targetInfos").and_then(Value::as_array) {
        for info in infos {
            if info.get("type").and_then(Value::as_str) == Some("page") {
                page_id = info
                    .get("targetId")
                    .and_then(Value::as_str)
                    .map(String::from);
                break;
            }
        }
    }
    let target_id = match page_id {
        Some(id) => id,
        None => {
            let v = session
                .call(None, "Target.createTarget", json!({ "url": "about:blank" }))
                .await
                .map_err(|e| format!("Target.createTarget 失败: {e}"))?;
            v.get("targetId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        }
    };
    let sid = attach_page_cached(session, pages, &target_id).await?;
    Ok((target_id, sid))
}

/// 将宿主刚创建、尚未导航的 about:blank WebView 精确绑定到 CDP target。
/// token 只存在于一次性的内部空白页：优先匹配 URL fragment；WebView2 若从
/// Target 列表省略 fragment，则只在 about:blank 候选上读取宿主初始化脚本写入的
/// bootstrap 标记。任何 http(s) 页面都不会被当作绑定来源。
async fn discover_native_target(port: u16, tab_token: &str) -> Result<String, String> {
    let connected = cdp::connect(port)
        .await
        .map_err(|error| format!("连接原生页面自动化端点失败: {error:#}"))?;
    let session = connected.session;
    let result = async {
        let targets = session
            .call(None, "Target.getTargets", json!({}))
            .await
            .map_err(|error| format!("枚举原生页面 target 失败: {error}"))?;
        let expected_session_marker = format!("#pinvou-session-{tab_token}");
        let expected_tab_marker = format!("#pinvou-tab-{tab_token}");
        let mut matches = Vec::new();
        for info in targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if info.get("type").and_then(Value::as_str) != Some("page") {
                continue;
            }
            let Some(target_id) = info.get("targetId").and_then(Value::as_str) else {
                continue;
            };
            let url = info.get("url").and_then(Value::as_str).unwrap_or("");
            if url.contains(&expected_session_marker) || url.contains(&expected_tab_marker) {
                matches.push(target_id.to_string());
                continue;
            }
            // 远程页面即使主动定义同名 global 也不能伪造归属。
            if url != "about:blank" && !url.starts_with("about:blank#") {
                continue;
            }
            let attached = session
                .call(
                    None,
                    "Target.attachToTarget",
                    json!({ "targetId": target_id, "flatten": true }),
                )
                .await
                .map_err(|error| format!("绑定原生页面 target 失败: {error}"))?;
            let sid = attached
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or_else(|| "绑定原生页面 target 未返回 sessionId".to_string())?;
            let marker = session
                .call(
                    Some(sid),
                    "Runtime.evaluate",
                    json!({
                        "expression": "globalThis.__PINVOU_BROWSER_BOOTSTRAP_TOKEN__ || null",
                        "returnByValue": true,
                    }),
                )
                .await
                .map_err(|error| format!("读取原生页面 bootstrap 标记失败: {error}"))?;
            if marker.pointer("/result/value").and_then(Value::as_str) == Some(tab_token) {
                matches.push(target_id.to_string());
            }
        }
        matches.sort();
        matches.dedup();
        match matches.as_slice() {
            [target_id] => Ok(target_id.clone()),
            [] => Err("未找到宿主新建页面对应的唯一自动化 target".to_string()),
            _ => Err("宿主页面 bootstrap 标记对应多个自动化 target，已拒绝绑定".to_string()),
        }
    }
    .await;
    let _ = session.close().await;
    connected.reader_task.abort();
    result
}

/// 端口文件有效且 CDP 存活时返回端口（live 探测）。
async fn live_port() -> Option<u16> {
    let raw = std::fs::read_to_string(paths::browser_cdp_port_json()).ok()?;
    let p = parse_host_owned_port_json(&raw)?;
    probe_cdp(p, Duration::from_millis(800)).await.then_some(p)
}

async fn list_page_tabs(
    session: &CdpSession,
    pages: &PageSessions,
) -> Result<Vec<TabInfo>, String> {
    let targets = session
        .call(None, "Target.getTargets", json!({}))
        .await
        .map_err(|e| format!("Target.getTargets 失败: {e}"))?;
    let mut tabs = Vec::new();
    if let Some(infos) = targets.get("targetInfos").and_then(Value::as_array) {
        for info in infos {
            if info.get("type").and_then(Value::as_str) != Some("page") {
                continue;
            }
            let target_id = info
                .get("targetId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            // attach 复用缓存：枚举高频发生（每次标签页增删），不缓存会每次都
            // 新建 flatten session（CDP 不自动释放，无界泄漏）。
            if attach_page_cached(session, pages, &target_id)
                .await
                .is_err()
            {
                continue;
            }
            tabs.push(TabInfo {
                target_id,
                page_id: None,
                title: info
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                url: info
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            });
        }
    }
    Ok(tabs)
}

/// 切换 Agent 自动化所使用的 flatten session。用户画面由同一页面的原生 WebView
/// 直接呈现，这里只启用页面协议域，不启动任何连续截图流。
async fn switch_active_session_locked(inner: &mut Inner, sid: &str) -> Result<(), String> {
    let session = inner
        .session
        .as_ref()
        .ok_or_else(|| "浏览器未启动".to_string())?;
    session
        .call(Some(sid), "Page.enable", json!({}))
        .await
        .map_err(|e| format!("Page.enable 失败: {e}"))?;
    inner.active_session = Some(sid.to_string());
    Ok(())
}

// ---------------------------------------------------------------------------
// 原生宿主自动化端点协调
// ---------------------------------------------------------------------------

fn valid_host_token(value: &str) -> bool {
    value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_host_request_id(value: &str) -> bool {
    (1..=160).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_wrapper_instance_nonce(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn validate_hosted_caller_identity(
    caller_pid: u32,
    wrapper_instance_nonce: &str,
) -> Result<(), String> {
    if caller_pid == 0 || !valid_wrapper_instance_nonce(wrapper_instance_nonce) {
        return Err("浏览器宿主调用方 epoch 身份无效".to_string());
    }
    Ok(())
}

fn valid_browser_session_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn validate_hosted_identity(
    protocol_version: u8,
    session_id: &str,
    session_token: &str,
    request_id: &str,
    idempotency_key: &str,
    path: &std::path::Path,
    extension: &str,
) -> Result<(), String> {
    if protocol_version != 3 {
        return Err("浏览器宿主协议版本不受支持".to_string());
    }
    if !valid_browser_session_id(session_id)
        || !valid_host_token(session_token)
        || paths::browser_session_token(session_id) != session_token
    {
        return Err("浏览器宿主请求的会话身份校验失败".to_string());
    }
    if !valid_host_request_id(request_id) {
        return Err("浏览器宿主 request_id 无效".to_string());
    }
    if idempotency_key != format!("{session_token}/{request_id}") {
        return Err("浏览器宿主 idempotency_key 无效".to_string());
    }
    let expected_name = format!("{session_token}-{request_id}.{extension}");
    if path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
        return Err("浏览器宿主请求文件名与请求身份不匹配".to_string());
    }
    Ok(())
}

fn validate_hosted_request(
    request: &HostedBrowserRequest,
    path: &std::path::Path,
) -> Result<(), String> {
    validate_hosted_identity(
        request.protocol_version,
        &request.session_id,
        &request.session_token,
        &request.request_id,
        &request.idempotency_key,
        path,
        "json",
    )?;
    if request.operation.as_str().is_empty() {
        return Err("浏览器宿主操作无效".to_string());
    }
    validate_hosted_caller_identity(request.caller_pid, &request.wrapper_instance_nonce)?;
    let now_ms = hosted_protocol_now_ms()?;
    validate_hosted_request_freshness_at(request, now_ms)?;
    Ok(())
}

fn hosted_protocol_now_ms() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| "系统时间早于 Unix epoch，拒绝浏览器宿主请求".to_string())?
        .as_millis()
        .try_into()
        .map_err(|_| "系统时间超出浏览器宿主协议范围".to_string())
}

fn validate_hosted_request_freshness_at(
    request: &HostedBrowserRequest,
    now_ms: u64,
) -> Result<(), String> {
    const CLOCK_SKEW_TOLERANCE_MS: u64 = 5_000;
    if request.requested_at == 0
        || request.requested_at > now_ms.saturating_add(CLOCK_SKEW_TOLERANCE_MS)
    {
        return Err("浏览器宿主请求时间戳无效".to_string());
    }
    if now_ms.saturating_sub(request.requested_at) > request.operation.maximum_artifact_age_ms() {
        return Err("browser/host-request-expired: caller is no longer live".to_string());
    }
    Ok(())
}

fn hosted_caller_heartbeat_path_for(session_token: &str, wrapper_instance_nonce: &str) -> PathBuf {
    paths::browser_host_requests_dir().join(format!(
        "{}-{}.heartbeat",
        session_token, wrapper_instance_nonce
    ))
}

fn hosted_caller_heartbeat_path(request: &HostedBrowserRequest) -> PathBuf {
    hosted_caller_heartbeat_path_for(&request.session_token, &request.wrapper_instance_nonce)
}

fn hosted_cancellation_path(request: &HostedBrowserRequest) -> PathBuf {
    paths::browser_host_requests_dir().join(format!(
        "{}-{}.cancelled",
        request.session_token, request.request_id
    ))
}

fn hosted_prepare_journal_dir() -> PathBuf {
    paths::browser_home().join("prepare-journal")
}

fn hosted_prepare_journal_path_for(session_token: &str) -> PathBuf {
    hosted_prepare_journal_dir().join(format!("{session_token}.json"))
}

fn hosted_prepare_journal_path(request: &HostedBrowserRequest) -> PathBuf {
    hosted_prepare_journal_path_for(&request.session_token)
}

impl HostedPrepareCompensation {
    fn from_request(request: &HostedBrowserRequest, rollback_kind: &str) -> Self {
        Self {
            protocol_version: request.protocol_version,
            kind: "host_prepare_compensation".to_string(),
            request_id: request.request_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            session_id: request.session_id.clone(),
            session_token: request.session_token.clone(),
            caller_pid: request.caller_pid,
            wrapper_instance_nonce: request.wrapper_instance_nonce.clone(),
            rollback_kind: rollback_kind.to_string(),
            revision: None,
        }
    }

    fn rollback_value(&self) -> Option<Value> {
        let revision = self.revision?;
        matches!(
            self.rollback_kind.as_str(),
            "prepared_session" | "restored_session"
        )
        .then(|| {
            json!({
                "kind": self.rollback_kind,
                "session_id": self.session_id,
                "request_id": self.request_id,
                "revision": revision,
            })
        })
    }
}

fn new_hosted_prepare_journal(
    request: &HostedBrowserRequest,
    rollback_kind: &str,
    now_ms: u64,
) -> HostedPrepareJournal {
    HostedPrepareJournal {
        protocol_version: request.protocol_version,
        kind: "host_prepare_journal".to_string(),
        phase: HostedPreparePhase::Pending,
        compensation: HostedPrepareCompensation::from_request(request, rollback_kind),
        requested_at: request.requested_at,
        updated_at: now_ms,
        response: None,
    }
}

fn write_hosted_prepare_journal(journal: &HostedPrepareJournal) -> Result<(), String> {
    validate_hosted_prepare_journal(
        journal,
        &hosted_prepare_journal_path_for(&journal.compensation.session_token),
    )?;
    let path = hosted_prepare_journal_path_for(&journal.compensation.session_token);
    let parent = path
        .parent()
        .ok_or_else(|| "浏览器 Prepare 持久日志缺少父目录".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "创建浏览器 Prepare 持久日志目录 {} 失败: {error}",
            parent.display()
        )
    })?;
    crate::platform::os::make_private_dir(parent);
    let encoded = serde_json::to_vec(journal)
        .map_err(|error| format!("编码浏览器 Prepare 持久日志失败: {error}"))?;
    crate::platform::filesystem::atomic_write_private(&path, &encoded).map_err(|error| {
        format!(
            "写入浏览器 Prepare 持久日志 {} 失败: {error}",
            path.display()
        )
    })
}

fn read_hosted_prepare_journal(path: &Path) -> Result<HostedPrepareJournal, String> {
    let raw = std::fs::read_to_string(path).map_err(|error| {
        format!(
            "读取浏览器 Prepare 持久日志 {} 失败: {error}",
            path.display()
        )
    })?;
    let journal: HostedPrepareJournal = serde_json::from_str(&raw)
        .map_err(|error| format!("浏览器 Prepare 持久日志格式无效: {error}"))?;
    validate_hosted_prepare_journal(&journal, path)?;
    Ok(journal)
}

fn remove_hosted_prepare_journal(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "删除浏览器 Prepare 持久日志 {} 失败: {error}",
                path.display()
            ))
        }
    }
    if path.exists() {
        return Err(format!(
            "删除浏览器 Prepare 持久日志 {} 后仍可见",
            path.display()
        ));
    }
    Ok(())
}

fn remove_hosted_prepare_journal_for_session(session_id: &str) -> Result<(), String> {
    let session_token = paths::browser_session_token(session_id);
    let path = hosted_prepare_journal_path_for(&session_token);
    if !path.exists() {
        return Ok(());
    }
    let journal = read_hosted_prepare_journal(&path)?;
    if journal.compensation.session_id != session_id
        || journal.compensation.session_token != session_token
    {
        return Err("拒绝删除其他任务的 Prepare 持久日志".to_string());
    }
    remove_hosted_prepare_journal(&path)
}

fn remove_matching_hosted_prepare_journal(
    cancellation: &HostedBrowserCancellation,
) -> Result<(), String> {
    if matching_hosted_prepare_journal_for_cancellation(cancellation)?.is_none() {
        return Ok(());
    }
    remove_hosted_prepare_journal(&hosted_prepare_journal_path_for(
        &cancellation.session_token,
    ))
}

fn matching_hosted_prepare_journal_for_cancellation(
    cancellation: &HostedBrowserCancellation,
) -> Result<Option<HostedPrepareJournal>, String> {
    let path = hosted_prepare_journal_path_for(&cancellation.session_token);
    if !path.exists() {
        return Ok(None);
    }
    let journal = read_hosted_prepare_journal(&path)?;
    let compensation = &journal.compensation;
    if compensation.request_id != cancellation.request_id
        || compensation.idempotency_key != cancellation.idempotency_key
        || compensation.session_id != cancellation.session_id
        || compensation.session_token != cancellation.session_token
        || compensation.caller_pid != cancellation.caller_pid
        || compensation.wrapper_instance_nonce != cancellation.wrapper_instance_nonce
    {
        // A distinct newer Prepare atomically supersedes the old per-session
        // WAL. Its generation must remain untouched, while the validated late
        // cancellation is an idempotent no-op that can still be acknowledged.
        return Ok(None);
    }
    Ok(Some(journal))
}

/// Read the cancellation path reserved for one durable Prepare generation and
/// require its full caller epoch to match. Merely observing a file at that path
/// is not authority to compensate a committed workspace.
fn matching_hosted_cancellation_for_compensation(
    compensation: &HostedPrepareCompensation,
) -> Result<bool, String> {
    let cancellation_path = paths::browser_host_requests_dir().join(format!(
        "{}-{}.cancelled",
        compensation.session_token, compensation.request_id
    ));
    if !cancellation_path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(&cancellation_path).map_err(|error| {
        format!(
            "读取 Prepare 取消记录 {} 失败: {error}",
            cancellation_path.display()
        )
    })?;
    let cancellation: HostedBrowserCancellation =
        serde_json::from_str(&raw).map_err(|error| format!("Prepare 取消记录格式无效: {error}"))?;
    validate_hosted_cancellation(&cancellation, &cancellation_path)?;
    if cancellation.request_id != compensation.request_id
        || cancellation.idempotency_key != compensation.idempotency_key
        || cancellation.session_id != compensation.session_id
        || cancellation.session_token != compensation.session_token
        || cancellation.caller_pid != compensation.caller_pid
        || cancellation.wrapper_instance_nonce != compensation.wrapper_instance_nonce
    {
        return Err("Prepare 取消记录与持久 generation 不一致".to_string());
    }
    Ok(true)
}

fn remove_matching_hosted_prepare_journal_for_request(
    request: &HostedBrowserRequest,
) -> Result<(), String> {
    let path = hosted_prepare_journal_path(request);
    if !path.exists() {
        return Ok(());
    }
    let journal = read_hosted_prepare_journal(&path)?;
    let compensation = &journal.compensation;
    if compensation.request_id != request.request_id
        || compensation.idempotency_key != request.idempotency_key
        || compensation.session_id != request.session_id
        || compensation.session_token != request.session_token
        || compensation.caller_pid != request.caller_pid
        || compensation.wrapper_instance_nonce != request.wrapper_instance_nonce
    {
        return Err("拒绝删除其他请求的 Prepare 持久日志".to_string());
    }
    remove_hosted_prepare_journal(&path)
}

fn validate_hosted_prepare_journal(
    journal: &HostedPrepareJournal,
    path: &Path,
) -> Result<(), String> {
    let compensation = &journal.compensation;
    if journal.protocol_version != 3
        || journal.kind != "host_prepare_journal"
        || compensation.protocol_version != journal.protocol_version
        || compensation.kind != "host_prepare_compensation"
        || journal.requested_at == 0
        || journal.updated_at == 0
    {
        return Err("浏览器 Prepare 持久日志协议无效".to_string());
    }
    validate_hosted_caller_identity(
        compensation.caller_pid,
        &compensation.wrapper_instance_nonce,
    )?;
    let synthetic_request_path = paths::browser_host_requests_dir().join(format!(
        "{}-{}.json",
        compensation.session_token, compensation.request_id
    ));
    validate_hosted_identity(
        compensation.protocol_version,
        &compensation.session_id,
        &compensation.session_token,
        &compensation.request_id,
        &compensation.idempotency_key,
        &synthetic_request_path,
        "json",
    )?;
    if path != hosted_prepare_journal_path_for(&compensation.session_token) {
        return Err("浏览器 Prepare 持久日志路径与会话身份不一致".to_string());
    }
    match compensation.rollback_kind.as_str() {
        "none" if compensation.revision.is_none() => {}
        "prepared_session" | "restored_session" => {
            if !matches!(journal.phase, HostedPreparePhase::Pending)
                && compensation.revision.unwrap_or_default() == 0
            {
                return Err("浏览器 Prepare 持久日志缺少补偿 revision".to_string());
            }
        }
        _ => return Err("浏览器 Prepare 持久日志补偿类型无效".to_string()),
    }
    match journal.phase {
        HostedPreparePhase::Committed => {
            let response = journal
                .response
                .as_ref()
                .ok_or_else(|| "已提交 Prepare 日志缺少响应".to_string())?;
            if response.get("protocol_version").and_then(Value::as_u64)
                != Some(compensation.protocol_version as u64)
                || response.get("request_id").and_then(Value::as_str)
                    != Some(compensation.request_id.as_str())
                || response.get("idempotency_key").and_then(Value::as_str)
                    != Some(compensation.idempotency_key.as_str())
                || response.get("ok").and_then(Value::as_bool) != Some(true)
            {
                return Err("已提交 Prepare 日志响应身份无效".to_string());
            }
        }
        _ if journal.response.is_some() => {
            return Err("未提交 Prepare 日志不得携带成功响应".to_string())
        }
        _ => {}
    }
    Ok(())
}

fn validate_hosted_prepare_compensation(
    compensation: &HostedPrepareCompensation,
    allow_missing_revision: bool,
) -> Result<(), String> {
    if compensation.protocol_version != 3 || compensation.kind != "host_prepare_compensation" {
        return Err("浏览器 Prepare 补偿协议无效".to_string());
    }
    validate_hosted_caller_identity(
        compensation.caller_pid,
        &compensation.wrapper_instance_nonce,
    )?;
    let synthetic_request_path = paths::browser_host_requests_dir().join(format!(
        "{}-{}.json",
        compensation.session_token, compensation.request_id
    ));
    validate_hosted_identity(
        compensation.protocol_version,
        &compensation.session_id,
        &compensation.session_token,
        &compensation.request_id,
        &compensation.idempotency_key,
        &synthetic_request_path,
        "json",
    )?;
    match compensation.rollback_kind.as_str() {
        "none" if compensation.revision.is_none() => Ok(()),
        "prepared_session" | "restored_session"
            if allow_missing_revision || compensation.revision.unwrap_or_default() > 0 =>
        {
            Ok(())
        }
        _ => Err("浏览器 Prepare 补偿 generation 无效".to_string()),
    }
}

fn reap_acknowledged_committed_prepare_journals(
    locally_committed: &mut HashSet<(String, String)>,
) -> Result<(), String> {
    let journal_dir = hosted_prepare_journal_dir();
    let entries = match std::fs::read_dir(&journal_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "读取浏览器 Prepare 持久日志目录 {} 失败: {error}",
                journal_dir.display()
            ))
        }
    };
    for entry in entries {
        let path = entry
            .map_err(|error| format!("枚举浏览器 Prepare 持久日志失败: {error}"))?
            .path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let journal = read_hosted_prepare_journal(&path)?;
        if journal.phase != HostedPreparePhase::Committed {
            continue;
        }
        let compensation = &journal.compensation;
        let local_key = (
            compensation.session_token.clone(),
            compensation.request_id.clone(),
        );
        if !locally_committed.contains(&local_key) {
            // A process restart removes transient request/response artifacts
            // before the still-live wrapper necessarily reaches its timeout.
            // Only a commit produced by this process may interpret absence as
            // acknowledgement; recovered commits wait for a matching cancel or
            // an explicit newer Prepare to supersede them.
            continue;
        }
        let artifact = |extension: &str| {
            paths::browser_host_requests_dir().join(format!(
                "{}-{}.{}",
                compensation.session_token, compensation.request_id, extension
            ))
        };
        if artifact("cancelled").exists() {
            continue;
        }
        if !artifact("json").exists() && !artifact("response").exists() {
            remove_hosted_prepare_journal(&path)?;
            locally_committed.remove(&local_key);
        }
    }
    Ok(())
}

fn hosted_internal_cancellation_value(
    request: &HostedBrowserRequest,
    cancelled_at: u64,
    prepare_compensation: Option<&HostedPrepareCompensation>,
) -> Value {
    json!({
        "protocol_version": request.protocol_version,
        "kind": "host_request_cancelled",
        "request_id": request.request_id,
        "idempotency_key": request.idempotency_key,
        "session_id": request.session_id,
        "session_token": request.session_token,
        "caller_pid": request.caller_pid,
        "wrapper_instance_nonce": request.wrapper_instance_nonce,
        "reason": "host-caller-epoch-lost-before-prepare-publication",
        "cancelled_at": cancelled_at,
        "prepare_compensation": prepare_compensation,
    })
}

fn validate_hosted_caller_heartbeat_at(
    session_id: &str,
    session_token: &str,
    caller_pid: u32,
    wrapper_instance_nonce: &str,
    heartbeat: &HostedCallerHeartbeat,
    now_ms: u64,
    caller_process_alive: bool,
) -> Result<(), String> {
    const HEARTBEAT_TTL_MS: u64 = 5_000;
    const CLOCK_SKEW_TOLERANCE_MS: u64 = 5_000;
    if heartbeat.protocol_version != 3 || heartbeat.kind != "host_caller_heartbeat" {
        return Err("browser/host-caller-not-live: heartbeat protocol is invalid".to_string());
    }
    if heartbeat.session_id != session_id
        || heartbeat.session_token != session_token
        || heartbeat.caller_pid != caller_pid
        || heartbeat.wrapper_instance_nonce != wrapper_instance_nonce
    {
        return Err(
            "browser/host-caller-not-live: heartbeat epoch does not match request".to_string(),
        );
    }
    if heartbeat.heartbeat_at == 0
        || heartbeat.heartbeat_at > now_ms.saturating_add(CLOCK_SKEW_TOLERANCE_MS)
        || now_ms.saturating_sub(heartbeat.heartbeat_at) > HEARTBEAT_TTL_MS
    {
        return Err("browser/host-caller-not-live: heartbeat is stale".to_string());
    }
    if !caller_process_alive {
        return Err("browser/host-caller-not-live: wrapper process has exited".to_string());
    }
    Ok(())
}

fn ensure_hosted_caller_epoch_live_at(
    session_id: &str,
    session_token: &str,
    caller_pid: u32,
    wrapper_instance_nonce: &str,
    now_ms: u64,
) -> Result<(), String> {
    validate_hosted_caller_identity(caller_pid, wrapper_instance_nonce)?;
    let heartbeat_path = hosted_caller_heartbeat_path_for(session_token, wrapper_instance_nonce);
    let raw = std::fs::read_to_string(&heartbeat_path).map_err(|error| {
        format!(
            "browser/host-caller-not-live: cannot read {}: {error}",
            heartbeat_path.display()
        )
    })?;
    let heartbeat: HostedCallerHeartbeat = serde_json::from_str(&raw)
        .map_err(|error| format!("browser/host-caller-not-live: heartbeat is invalid: {error}"))?;
    validate_hosted_caller_heartbeat_at(
        session_id,
        session_token,
        caller_pid,
        wrapper_instance_nonce,
        &heartbeat,
        now_ms,
        crate::platform::os::platform::process_alive(caller_pid),
    )
}

fn ensure_hosted_caller_live_at(request: &HostedBrowserRequest, now_ms: u64) -> Result<(), String> {
    if !request.operation.requires_live_caller() {
        return Ok(());
    }
    ensure_hosted_caller_epoch_live_at(
        &request.session_id,
        &request.session_token,
        request.caller_pid,
        &request.wrapper_instance_nonce,
        now_ms,
    )
}

fn ensure_hosted_caller_epoch_live(
    session_id: &str,
    session_token: &str,
    caller_pid: u32,
    wrapper_instance_nonce: &str,
) -> Result<(), String> {
    ensure_hosted_caller_epoch_live_at(
        session_id,
        session_token,
        caller_pid,
        wrapper_instance_nonce,
        hosted_protocol_now_ms()?,
    )
}

fn ensure_hosted_caller_live(request: &HostedBrowserRequest) -> Result<(), String> {
    // This narrows abandoned-artifact execution but cannot make a distributed
    // native mutation exactly-once: the wrapper can still be SIGKILLed after
    // this final check and before the platform commit. That residual window
    // remains an acknowledgement-unknown outcome handled by the existing
    // idempotency/tombstone/compensation protocol, not proof of at-most-once.
    ensure_hosted_caller_live_at(request, hosted_protocol_now_ms()?)
}

fn validate_hosted_cancellation(
    cancellation: &HostedBrowserCancellation,
    path: &std::path::Path,
) -> Result<(), String> {
    if cancellation.kind != "host_request_cancelled" {
        return Err("浏览器宿主取消记录类型无效".to_string());
    }
    validate_hosted_caller_identity(
        cancellation.caller_pid,
        &cancellation.wrapper_instance_nonce,
    )?;
    validate_hosted_identity(
        cancellation.protocol_version,
        &cancellation.session_id,
        &cancellation.session_token,
        &cancellation.request_id,
        &cancellation.idempotency_key,
        path,
        "cancelled",
    )?;
    if let Some(compensation) = cancellation.prepare_compensation.as_ref() {
        validate_hosted_prepare_compensation(compensation, false)?;
        if compensation.protocol_version != cancellation.protocol_version
            || compensation.request_id != cancellation.request_id
            || compensation.idempotency_key != cancellation.idempotency_key
            || compensation.session_id != cancellation.session_id
            || compensation.session_token != cancellation.session_token
            || compensation.caller_pid != cancellation.caller_pid
            || compensation.wrapper_instance_nonce != cancellation.wrapper_instance_nonce
        {
            return Err("浏览器宿主取消记录的 Prepare 补偿身份不一致".to_string());
        }
    }
    Ok(())
}

fn hosted_response(request: &HostedBrowserRequest, result: Result<Value, String>) -> Value {
    match result {
        Ok(result) => json!({
            "protocol_version": request.protocol_version,
            "request_id": request.request_id,
            "idempotency_key": request.idempotency_key,
            "ok": true,
            "result": result,
        }),
        Err(error) => json!({
            "protocol_version": request.protocol_version,
            "request_id": request.request_id,
            "idempotency_key": request.idempotency_key,
            "ok": false,
            "error": error,
        }),
    }
}

fn browser_core_tool_result(text: String, structured: Option<Value>) -> Value {
    let mut result = json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false,
    });
    if let Some(structured) = structured {
        result["structuredContent"] = structured;
    }
    result
}

fn should_reuse_browser_core_initial_tab(
    browser_session_id: &str,
    active_tab_token: &str,
    tabs: &[TabInfo],
    background: bool,
) -> bool {
    let initial_tab_token = paths::browser_session_token(browser_session_id);
    !background
        && tabs.len() == 1
        && active_tab_token == initial_tab_token
        && tabs[0].target_id == initial_tab_token
        && tabs[0].url == "about:blank"
}

fn write_hosted_response(request_path: &std::path::Path, response: &Value) -> Result<(), String> {
    let response_path = request_path.with_extension("response");
    if let Some(parent) = response_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建浏览器宿主响应目录失败: {error}"))?;
    }
    let encoded =
        serde_json::to_vec(response).map_err(|error| format!("浏览器宿主响应编码失败: {error}"))?;
    crate::platform::filesystem::atomic_write(&response_path, &encoded)
        .map_err(|error| format!("写入浏览器宿主响应失败: {error}"))
}

fn remove_hosted_request_artifacts(path: &std::path::Path) -> Result<(), String> {
    let mut errors = Vec::new();
    for artifact in [
        path.with_extension("response"),
        path.with_extension("json"),
        path.with_extension("cancelled"),
    ] {
        if let Err(error) = std::fs::remove_file(&artifact) {
            if error.kind() != std::io::ErrorKind::NotFound {
                errors.push(format!("删除 {} 失败: {error}", artifact.display()));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn native_lease_from_request(request: &HostedBrowserRequest) -> Result<NativeTabLease, String> {
    NativeTabLease::from_assertion(
        request.session_id.clone(),
        request
            .tab_token
            .clone()
            .ok_or_else(|| "浏览器宿主 lease 缺少 tab_token".to_string())?,
        request
            .target_id
            .clone()
            .ok_or_else(|| "浏览器宿主 lease 缺少 target_id".to_string())?,
        request
            .revision
            .ok_or_else(|| "浏览器宿主 lease 缺少 revision".to_string())?,
        request
            .lease
            .clone()
            .ok_or_else(|| "浏览器宿主 lease 缺少能力令牌".to_string())?,
    )
}

fn native_mutation_lease_from_request(
    request: &HostedBrowserRequest,
) -> Result<NativeTabLease, String> {
    NativeTabLease::from_assertion(
        request.session_id.clone(),
        request
            .authorization_tab_token
            .clone()
            .ok_or_else(|| "浏览器宿主 mutation lease 缺少 authorization_tab_token".to_string())?,
        request
            .target_id
            .clone()
            .ok_or_else(|| "浏览器宿主 mutation lease 缺少 target_id".to_string())?,
        request
            .revision
            .ok_or_else(|| "浏览器宿主 mutation lease 缺少 revision".to_string())?,
        request
            .lease
            .clone()
            .ok_or_else(|| "浏览器宿主 mutation lease 缺少能力令牌".to_string())?,
    )
}

fn native_lease_from_value(value: &Value) -> Result<NativeTabLease, String> {
    NativeTabLease::from_assertion(
        value
            .get("session_id")
            .or_else(|| value.get("sessionId"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        value
            .get("tab_token")
            .or_else(|| value.get("tabToken"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        value
            .get("target_id")
            .or_else(|| value.get("targetId"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        value.get("revision").and_then(Value::as_u64).unwrap_or(0),
        value
            .get("lease")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
}

async fn probe_cdp(port: u16, timeout: Duration) -> bool {
    let url = format!("http://127.0.0.1:{port}/json/version");
    // 回环探测不走系统代理：reqwest 默认 auto_sys_proxy，设了 HTTP_PROXY 且未配
    // NO_PROXY 的用户会把 127.0.0.1 请求发往代理而失败。探测频次低，一次性 client 即可。
    let Ok(client) = reqwest::Client::builder().no_proxy().build() else {
        return false;
    };
    tokio::time::timeout(timeout, client.get(&url).send())
        .await
        .ok()
        .and_then(|r| r.ok())
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn pick_free_port() -> Result<u16, String> {
    use rand::RngExt;
    let base = 9222 + rand::rng().random_range(0..3000);
    for port in base..(base + 200) {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
        {
            return Ok(port);
        }
    }
    // 候选区间全占用：直接报错而不是回落到已知被占的 base。
    Err(format!(
        "端口区间 {base}..{} 全部被占用，无法创建原生浏览器自动化端点",
        base + 200
    ))
}

/// Wait until the wrapper publishes the durable cancellation artifact for a
/// claimed host request. The surrounding `tokio::select!` revokes the platform
/// lease immediately, then keeps polling any already-dispatched DOM/WebDriver
/// work to its bounded settlement so a later mutation cannot overtake it.
async fn wait_for_hosted_cancellation(path: &Path) {
    while !path.exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 解析端口文件内容。显式校验合法端口范围：损坏/他人写入的值（如 65536+k）经
/// `as u16` 会静默回绕到任意端口，探测错误端点（多耗 ~10s 后才走 stale 清理）。
fn parse_port_json(raw: &str) -> Option<u16> {
    let v: Value = serde_json::from_str(raw).ok()?;
    v.get("port")
        .and_then(Value::as_u64)
        .filter(|p| (1..=65535).contains(p))
        .map(|p| p as u16)
}

/// 只有当前应用原生宿主发布的端口才能进入自动化连接路径。旧 wrapper/外部 Chrome
/// 写入的 `owner=mcp` 或带 `browser_pid` 文件即使端口存活也必须拒绝，避免原生页面
/// 不可用时悄悄改变浏览器身份和交互语义。
fn parse_host_owned_port_json(raw: &str) -> Option<u16> {
    let v: Value = serde_json::from_str(raw).ok()?;
    if v.get("owner").and_then(Value::as_str) != Some("app")
        || v.get("browser_pid").is_some_and(|value| !value.is_null())
    {
        return None;
    }
    parse_port_json(raw)
}

fn write_port_file(port: u16, owner: &str, browser_pid: Option<u32>) -> Result<(), String> {
    let path = paths::browser_cdp_port_json();
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let mut data = json!({
        "port": port,
        "pid": std::process::id(),
        "owner": owner,
        "started_at": chrono::Utc::now().timestamp_millis(),
    });
    if let Some(browser_pid) = browser_pid {
        data["browser_pid"] = json!(browser_pid);
    }
    let encoded = serde_json::to_vec_pretty(&data).map_err(|e| e.to_string())?;
    // CDP 无鉴权：临时文件创建即收紧 0600，并通过跨平台替换状态机覆盖旧文件。
    // Windows 的普通 rename 不能覆盖崩溃残留的 cdp-port.json，会让重启恢复失败。
    crate::platform::filesystem::atomic_write_private(&path, &encoded)
        .map_err(|e| format!("写端口文件失败: {e}"))
}

fn remove_file_and_verify_absent(path: &Path, description: &str) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("{description} {} 失败: {error}", path.display())),
    }
    if path.exists() {
        return Err(format!("{description} {} 后文件仍可见", path.display()));
    }
    Ok(())
}

/// Recover host-owned Prepare journals before the transient request directory
/// is reset and before any status/restore path can publish a stale manifest.
/// Native WebViews cannot survive an application-process restart, so recovery
/// only has to reconcile the durable restore/runtime mapping files.
fn recover_hosted_prepare_journals_for_process_start() -> Result<(), String> {
    let journal_dir = hosted_prepare_journal_dir();
    let entries = match std::fs::read_dir(&journal_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "读取浏览器 Prepare 持久日志目录 {} 失败: {error}",
                journal_dir.display()
            ))
        }
    };
    let mut journal_paths = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("枚举浏览器 Prepare 持久日志失败: {error}"))?
            .path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            journal_paths.push(path);
        }
    }
    journal_paths.sort();
    let mut errors = Vec::new();
    for path in journal_paths {
        let result = (|| {
            let journal = read_hosted_prepare_journal(&path)?;
            let compensation = &journal.compensation;
            let matching_cancellation =
                matching_hosted_cancellation_for_compensation(compensation)?;
            let committed_without_cancellation =
                journal.phase == HostedPreparePhase::Committed && !matching_cancellation;
            // Runtime labels/targets are process-local in every phase. Remove
            // them even when the committed manifest/WAL must remain available
            // for a still-live wrapper's late cancellation.
            remove_file_and_verify_absent(
                &paths::browser_workspace_state_json(&compensation.session_token),
                "删除失效浏览器运行期映射",
            )?;
            if committed_without_cancellation {
                // The wrapper can still be alive and reach its timeout after
                // this process starts. Keep the host-owned compensation record
                // across transient request-directory reset so that late cancel
                // remains authoritative. This recovered WAL is never reaped by
                // transient artifact absence; only a matching tombstone or a
                // newer Prepare may settle/supersede it.
                return Ok(());
            }
            if compensation.rollback_kind == "prepared_session" {
                remove_file_and_verify_absent(
                    &paths::browser_workspace_restore_json(&compensation.session_token),
                    "删除未提交 Prepare 恢复清单",
                )?;
            }
            // Re-read the CreatedBlank delete boundary before removing the WAL.
            // A SIGKILL after the manifest deletion therefore leaves the WAL
            // behind and the next process repeats the idempotent verification.
            if compensation.rollback_kind == "prepared_session"
                && paths::browser_workspace_restore_json(&compensation.session_token).exists()
            {
                return Err("未提交 Prepare 恢复清单仍存在".to_string());
            }
            remove_hosted_prepare_journal(&path)
        })();
        if let Err(error) = result {
            errors.push(format!("{}: {error}", path.display()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// 为当前 app process 建立全新的 transient host-request 目录。整个旧目录先原子
/// rename 到 watcher 永不监听的同级 quarantine，再创建空目录；因此旧进程遗留请求
/// 不会与 watcher 注册窗口里的本进程新请求混在一起。
fn reset_host_request_directory_for_process_start(
    request_dir: &std::path::Path,
) -> Result<(), String> {
    let parent = request_dir
        .parent()
        .ok_or_else(|| "浏览器宿主请求目录缺少父目录".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("创建浏览器宿主协调目录失败: {error}"))?;

    let mut quarantined = None;
    if request_dir.exists() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let stem = request_dir
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("host-requests");
        let mut last_error = None;
        for attempt in 0..8_u8 {
            let candidate = parent.join(format!(
                ".{stem}.stale-{}-{nonce}-{attempt}",
                std::process::id()
            ));
            match std::fs::rename(request_dir, &candidate) {
                Ok(()) => {
                    quarantined = Some(candidate);
                    last_error = None;
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    last_error = None;
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    last_error = Some(error);
                }
                Err(error) => {
                    return Err(format!("原子隔离 {} 失败: {error}", request_dir.display()));
                }
            }
        }
        if let Some(error) = last_error {
            return Err(format!(
                "无法为 {} 分配隔离目录: {error}",
                request_dir.display()
            ));
        }
    }

    std::fs::create_dir(request_dir)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        })
        .map_err(|error| format!("创建当前进程浏览器宿主请求目录失败: {error}"))?;

    if let Some(quarantine) = quarantined {
        let cleanup = match std::fs::symlink_metadata(&quarantine) {
            Ok(metadata) if metadata.file_type().is_dir() => std::fs::remove_dir_all(&quarantine),
            Ok(_) => std::fs::remove_file(&quarantine),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        };
        if let Err(error) = cleanup {
            // 旧请求已经与 watcher 物理隔离，清理失败不能重新暴露或重放它；下次
            // 启动仍只监听标准目录。保留诊断，避免牺牲当前进程浏览器可用性。
            eprintln!(
                "[browser] 删除已隔离的旧宿主请求目录 {} 失败: {error}",
                quarantine.display()
            );
        }
    }
    Ok(())
}

fn clear_host_request_files() {
    let Ok(entries) = std::fs::read_dir(paths::browser_host_requests_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PinvouHomeGuard(Option<std::ffi::OsString>);

    impl PinvouHomeGuard {
        fn install(path: &Path) -> Self {
            let previous = std::env::var_os("PINVOU3_HOME");
            std::env::set_var("PINVOU3_HOME", path);
            Self(previous)
        }
    }

    impl Drop for PinvouHomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(previous) => std::env::set_var("PINVOU3_HOME", previous),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
        }
    }

    fn hosted_request_at(
        operation: HostedBrowserOperation,
        requested_at: u64,
    ) -> HostedBrowserRequest {
        HostedBrowserRequest {
            protocol_version: 3,
            request_id: "request-a".to_string(),
            idempotency_key: "unused-in-freshness-test".to_string(),
            session_id: "session-a".to_string(),
            session_token: "0123456789abcdef".to_string(),
            caller_pid: 42,
            wrapper_instance_nonce: "0123456789abcdef0123456789abcdef".to_string(),
            operation,
            requested_at,
            tab_token: None,
            authorization_tab_token: None,
            creation_id: None,
            url: None,
            target_id: None,
            revision: None,
            lease: None,
            tool_name: None,
            tool_arguments: None,
            background: false,
            emits_trusted_input: false,
        }
    }

    fn valid_hosted_prepare_request(now_ms: u64) -> HostedBrowserRequest {
        let session_id = "prepare-journal-session".to_string();
        let session_token = paths::browser_session_token(&session_id);
        let request_id = "prepare.request-a".to_string();
        HostedBrowserRequest {
            protocol_version: 3,
            idempotency_key: format!("{session_token}/{request_id}"),
            request_id,
            session_id,
            session_token,
            caller_pid: 42,
            wrapper_instance_nonce: "0123456789abcdef0123456789abcdef".to_string(),
            operation: HostedBrowserOperation::Prepare,
            requested_at: now_ms,
            tab_token: None,
            authorization_tab_token: None,
            creation_id: None,
            url: None,
            target_id: None,
            revision: None,
            lease: None,
            tool_name: None,
            tool_arguments: None,
            background: false,
            emits_trusted_input: false,
        }
    }

    fn hosted_caller_heartbeat_at(heartbeat_at: u64) -> HostedCallerHeartbeat {
        HostedCallerHeartbeat {
            protocol_version: 3,
            kind: "host_caller_heartbeat".to_string(),
            session_id: "session-a".to_string(),
            session_token: "0123456789abcdef".to_string(),
            caller_pid: 42,
            wrapper_instance_nonce: "0123456789abcdef0123456789abcdef".to_string(),
            heartbeat_at,
        }
    }

    fn validate_test_heartbeat(
        request: &HostedBrowserRequest,
        heartbeat: &HostedCallerHeartbeat,
        now_ms: u64,
        caller_process_alive: bool,
    ) -> Result<(), String> {
        validate_hosted_caller_heartbeat_at(
            &request.session_id,
            &request.session_token,
            request.caller_pid,
            &request.wrapper_instance_nonce,
            heartbeat,
            now_ms,
            caller_process_alive,
        )
    }

    #[test]
    fn hosted_request_artifacts_expire_with_their_live_caller_budget() {
        let now = 100_000;
        assert!(validate_hosted_request_freshness_at(
            &hosted_request_at(HostedBrowserOperation::CoreTool, now - 25_000),
            now,
        )
        .is_ok());
        assert!(validate_hosted_request_freshness_at(
            &hosted_request_at(HostedBrowserOperation::CoreTool, now - 25_001),
            now,
        )
        .is_err());
        assert!(validate_hosted_request_freshness_at(
            &hosted_request_at(HostedBrowserOperation::CreateTab, now - 12_001),
            now,
        )
        .is_err());
        assert!(validate_hosted_request_freshness_at(
            &hosted_request_at(HostedBrowserOperation::EndAgentOperation, now - 12_000),
            now,
        )
        .is_ok());
        assert!(validate_hosted_request_freshness_at(
            &hosted_request_at(HostedBrowserOperation::Prepare, now + 5_001),
            now,
        )
        .is_err());
        assert!(validate_hosted_request_freshness_at(
            &hosted_request_at(HostedBrowserOperation::Prepare, 0),
            now,
        )
        .is_err());
    }

    #[test]
    fn authority_bearing_host_requests_require_a_matching_live_epoch() {
        let now = 100_000;
        let request = hosted_request_at(HostedBrowserOperation::CreateTab, now);
        let heartbeat = hosted_caller_heartbeat_at(now - 5_000);
        assert!(validate_test_heartbeat(&request, &heartbeat, now, true).is_ok());

        let stale = hosted_caller_heartbeat_at(now - 5_001);
        assert!(validate_test_heartbeat(&request, &stale, now, true).is_err());
        let future = hosted_caller_heartbeat_at(now + 5_001);
        assert!(validate_test_heartbeat(&request, &future, now, true).is_err());
        assert!(validate_test_heartbeat(&request, &heartbeat, now, false).is_err());

        let mut wrong_epoch = hosted_caller_heartbeat_at(now);
        wrong_epoch.wrapper_instance_nonce = "fedcba9876543210fedcba9876543210".to_string();
        assert!(validate_test_heartbeat(&request, &wrong_epoch, now, true).is_err());
        let mut wrong_pid = hosted_caller_heartbeat_at(now);
        wrong_pid.caller_pid += 1;
        assert!(validate_test_heartbeat(&request, &wrong_pid, now, true).is_err());
        let mut wrong_protocol = hosted_caller_heartbeat_at(now);
        wrong_protocol.protocol_version = 2;
        assert!(validate_test_heartbeat(&request, &wrong_protocol, now, true).is_err());
        let mut wrong_session = hosted_caller_heartbeat_at(now);
        wrong_session.session_id = "session-b".to_string();
        assert!(validate_test_heartbeat(&request, &wrong_session, now, true).is_err());
        let mut wrong_token = hosted_caller_heartbeat_at(now);
        wrong_token.session_token = "fedcba9876543210".to_string();
        assert!(validate_test_heartbeat(&request, &wrong_token, now, true).is_err());
    }

    #[test]
    fn wrapper_epoch_identity_and_artifact_name_are_canonical() {
        assert!(valid_wrapper_instance_nonce(
            "0123456789abcdef0123456789abcdef"
        ));
        assert!(!valid_wrapper_instance_nonce(
            "0123456789ABCDEF0123456789ABCDEF"
        ));
        assert!(!valid_wrapper_instance_nonce("0123456789abcdef"));
        assert!(validate_hosted_caller_identity(42, "0123456789abcdef0123456789abcdef").is_ok());
        assert!(validate_hosted_caller_identity(0, "0123456789abcdef0123456789abcdef").is_err());

        let request = hosted_request_at(HostedBrowserOperation::Prepare, 1);
        assert_eq!(
            hosted_caller_heartbeat_path(&request)
                .file_name()
                .and_then(|name| name.to_str()),
            Some("0123456789abcdef-0123456789abcdef0123456789abcdef.heartbeat")
        );
    }

    #[test]
    fn cleanup_requests_do_not_depend_on_a_live_wrapper_epoch() {
        for operation in [
            HostedBrowserOperation::EndAgentOperation,
            HostedBrowserOperation::RollbackCreatedTab,
        ] {
            let request = hosted_request_at(operation, 100_000);
            assert!(!operation.requires_live_caller());
            // No heartbeat artifact is present. Cleanup must still pass the
            // epoch gate so it can release authority/resources after SIGKILL.
            assert!(ensure_hosted_caller_live_at(&request, 100_000).is_ok());
        }
        for operation in [
            HostedBrowserOperation::Prepare,
            HostedBrowserOperation::CreateTab,
            HostedBrowserOperation::ActivateTab,
            HostedBrowserOperation::CloseTab,
            HostedBrowserOperation::AssertHostLease,
            HostedBrowserOperation::BeginAgentOperation,
            HostedBrowserOperation::RefreshAgentOperation,
            HostedBrowserOperation::RefreshAgentInput,
            HostedBrowserOperation::CoreTool,
        ] {
            assert!(operation.requires_live_caller());
        }
    }

    #[test]
    fn host_request_and_cancellation_require_wrapper_epoch_fields() {
        let request_without_epoch = json!({
            "protocol_version": 3,
            "request_id": "request-a",
            "idempotency_key": "0123456789abcdef/request-a",
            "session_id": "session-a",
            "session_token": "0123456789abcdef",
            "operation": "prepare",
            "requested_at": 100_000,
        });
        assert!(serde_json::from_value::<HostedBrowserRequest>(request_without_epoch).is_err());

        let cancellation_without_epoch = json!({
            "protocol_version": 3,
            "kind": "host_request_cancelled",
            "request_id": "request-a",
            "idempotency_key": "0123456789abcdef/request-a",
            "session_id": "session-a",
            "session_token": "0123456789abcdef",
        });
        assert!(
            serde_json::from_value::<HostedBrowserCancellation>(cancellation_without_epoch)
                .is_err()
        );
    }

    #[test]
    fn dead_prepare_failure_retains_exact_retryable_compensation_metadata() {
        let request = hosted_request_at(HostedBrowserOperation::Prepare, 100_000);
        let created_blank = hosted_prepare_rollback_record(
            PreparedWorkspaceDisposition::CreatedBlank,
            &request.session_id,
            &request.request_id,
            7,
        )
        .unwrap();
        assert_eq!(created_blank["kind"], "prepared_session");
        assert_eq!(created_blank["revision"], 7);

        let restored = hosted_prepare_rollback_record(
            PreparedWorkspaceDisposition::RestoredExisting,
            &request.session_id,
            &request.request_id,
            8,
        )
        .unwrap();
        assert_eq!(restored["kind"], "restored_session");
        assert_eq!(restored["revision"], 8);
        assert!(hosted_prepare_rollback_record(
            PreparedWorkspaceDisposition::Existing,
            &request.session_id,
            &request.request_id,
            9,
        )
        .is_none());

        let failed = HostedBrowserOutcome::failed_with_rollback(
            "browser/host-caller-not-live".to_string(),
            created_blank.clone(),
        );
        assert_eq!(
            failed.error.as_deref(),
            Some("browser/host-caller-not-live")
        );
        assert_eq!(failed.rollback, created_blank);

        let tombstone = hosted_internal_cancellation_value(&request, 100_001, None);
        let decoded: HostedBrowserCancellation = serde_json::from_value(tombstone).unwrap();
        assert_eq!(decoded.request_id, request.request_id);
        assert_eq!(decoded.caller_pid, request.caller_pid);
        assert_eq!(
            decoded.wrapper_instance_nonce,
            request.wrapper_instance_nonce
        );
        assert_eq!(
            hosted_cancellation_path(&request)
                .file_name()
                .and_then(|name| name.to_str()),
            Some("0123456789abcdef-request-a.cancelled")
        );
    }

    #[test]
    fn prepare_journal_is_host_owned_outside_the_transient_request_directory() {
        let request = valid_hosted_prepare_request(100_000);
        let journal_path = hosted_prepare_journal_path(&request);
        let expected_name = format!("{}.json", request.session_token);
        assert!(journal_path.starts_with(paths::browser_home()));
        assert!(!journal_path.starts_with(paths::browser_host_requests_dir()));
        assert_eq!(
            journal_path.file_name().and_then(|name| name.to_str()),
            Some(expected_name.as_str())
        );
    }

    #[test]
    fn first_prepare_journal_write_creates_private_durable_directory() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);

        assert!(!hosted_prepare_journal_dir().exists());
        write_hosted_prepare_journal(&journal).unwrap();
        assert!(hosted_prepare_journal_dir().is_dir());
        assert!(hosted_prepare_journal_path(&request).is_file());
    }

    #[test]
    fn successful_session_stop_removes_exact_prepare_wal_idempotently() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);
        write_hosted_prepare_journal(&journal).unwrap();

        remove_hosted_prepare_journal_for_session(&request.session_id).unwrap();
        remove_hosted_prepare_journal_for_session(&request.session_id).unwrap();
        assert!(!hosted_prepare_journal_path(&request).exists());
    }

    #[test]
    fn failed_session_stop_wal_delete_remains_retryable() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let path = hosted_prepare_journal_path(&request);
        std::fs::create_dir_all(&path).unwrap();

        assert!(remove_hosted_prepare_journal_for_session(&request.session_id).is_err());
        assert!(path.exists());
    }

    #[test]
    fn startup_pending_created_blank_recovery_deletes_manifest_before_journal() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        let runtime_path = paths::browser_workspace_state_json(&request.session_token);
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
        std::fs::write(&restore_path, b"created-blank").unwrap();
        std::fs::write(&runtime_path, b"runtime-mapping").unwrap();
        write_hosted_prepare_journal(&journal).unwrap();

        recover_hosted_prepare_journals_for_process_start().unwrap();

        assert!(!restore_path.exists());
        assert!(!runtime_path.exists());
        assert!(!hosted_prepare_journal_path(&request).exists());
    }

    #[test]
    fn startup_pending_restored_recovery_preserves_original_manifest() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let journal = new_hosted_prepare_journal(&request, "restored_session", 100_001);
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::write(&restore_path, b"original-restore").unwrap();
        write_hosted_prepare_journal(&journal).unwrap();

        recover_hosted_prepare_journals_for_process_start().unwrap();

        assert_eq!(std::fs::read(&restore_path).unwrap(), b"original-restore");
        assert!(!hosted_prepare_journal_path(&request).exists());
    }

    #[test]
    fn startup_before_wrapper_timeout_retains_committed_wal_for_late_cancel() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let mut journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);
        journal.phase = HostedPreparePhase::Committed;
        journal.compensation.revision = Some(7);
        journal.response = Some(hosted_response(
            &request,
            Ok(json!({ "sessionId": request.session_id })),
        ));
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        let runtime_path = paths::browser_workspace_state_json(&request.session_token);
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(runtime_path.parent().unwrap()).unwrap();
        std::fs::write(&restore_path, b"committed-restore").unwrap();
        std::fs::write(&runtime_path, b"stale-process-local-targets").unwrap();
        write_hosted_prepare_journal(&journal).unwrap();
        // The host crashed after Committed but before publishing a response.
        // The still-live wrapper has not reached its timeout yet.

        recover_hosted_prepare_journals_for_process_start().unwrap();

        assert_eq!(std::fs::read(&restore_path).unwrap(), b"committed-restore");
        assert!(!runtime_path.exists());
        assert!(hosted_prepare_journal_path(&request).exists());

        // Reset-induced artifact absence is not a wrapper acknowledgement for
        // a commit recovered from another host process.
        reset_host_request_directory_for_process_start(&paths::browser_host_requests_dir())
            .unwrap();
        let mut locally_committed = HashSet::new();
        reap_acknowledged_committed_prepare_journals(&mut locally_committed).unwrap();
        assert!(hosted_prepare_journal_path(&request).exists());

        // The old wrapper reaches its timeout after the new host is already up.
        // The retained WAL still supplies exact compensation metadata.
        let cancellation_path = hosted_cancellation_path(&request);
        std::fs::write(
            &cancellation_path,
            serde_json::to_vec(&hosted_internal_cancellation_value(&request, 100_002, None))
                .unwrap(),
        )
        .unwrap();
        recover_hosted_prepare_journals_for_process_start().unwrap();
        assert!(!restore_path.exists());
        assert!(!hosted_prepare_journal_path(&request).exists());
    }

    #[test]
    fn startup_matching_cancel_overrides_committed_prepare_before_transient_reset() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let mut journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);
        journal.phase = HostedPreparePhase::Committed;
        journal.compensation.revision = Some(7);
        journal.response = Some(hosted_response(
            &request,
            Ok(json!({ "sessionId": request.session_id })),
        ));
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::write(&restore_path, b"must-be-rolled-back").unwrap();
        write_hosted_prepare_journal(&journal).unwrap();
        let cancellation_path = hosted_cancellation_path(&request);
        std::fs::create_dir_all(cancellation_path.parent().unwrap()).unwrap();
        std::fs::write(
            &cancellation_path,
            serde_json::to_vec(&hosted_internal_cancellation_value(
                &request,
                100_002,
                Some(&journal.compensation),
            ))
            .unwrap(),
        )
        .unwrap();

        // This is the exact spawn_watch ordering: durable recovery consumes
        // the old tombstone before reset quarantines transient artifacts.
        recover_hosted_prepare_journals_for_process_start().unwrap();
        reset_host_request_directory_for_process_start(&paths::browser_host_requests_dir())
            .unwrap();

        assert!(!restore_path.exists());
        assert!(!hosted_prepare_journal_path(&request).exists());
        assert_eq!(
            std::fs::read_dir(paths::browser_host_requests_dir())
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn committed_journal_is_removed_only_after_response_artifacts_are_consumed() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let mut journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);
        journal.phase = HostedPreparePhase::Committed;
        journal.compensation.revision = Some(7);
        let response = hosted_response(&request, Ok(json!({ "sessionId": request.session_id })));
        journal.response = Some(response.clone());
        write_hosted_prepare_journal(&journal).unwrap();
        let request_path = paths::browser_host_requests_dir().join(format!(
            "{}-{}.json",
            request.session_token, request.request_id
        ));
        std::fs::create_dir_all(request_path.parent().unwrap()).unwrap();
        std::fs::write(&request_path, b"request").unwrap();
        write_hosted_response(&request_path, &response).unwrap();
        let key = (request.session_token.clone(), request.request_id.clone());
        let mut locally_committed = HashSet::from([key.clone()]);

        reap_acknowledged_committed_prepare_journals(&mut locally_committed).unwrap();
        assert!(hosted_prepare_journal_path(&request).exists());
        assert!(locally_committed.contains(&key));

        std::fs::remove_file(request_path.with_extension("response")).unwrap();
        std::fs::remove_file(&request_path).unwrap();
        reap_acknowledged_committed_prepare_journals(&mut locally_committed).unwrap();
        assert!(!hosted_prepare_journal_path(&request).exists());
        assert!(!locally_committed.contains(&key));
    }

    #[test]
    fn distinct_prepare_supersedes_late_old_cancel_without_deleting_new_wal() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let old_request = valid_hosted_prepare_request(100_000);
        let mut new_request = valid_hosted_prepare_request(100_010);
        new_request.request_id = "request-b".to_string();
        new_request.idempotency_key = format!("{}/request-b", new_request.session_token);
        new_request.wrapper_instance_nonce = "fedcba9876543210fedcba9876543210".to_string();
        let new_journal = new_hosted_prepare_journal(&new_request, "restored_session", 100_011);
        write_hosted_prepare_journal(&new_journal).unwrap();

        let old_cancellation_path = hosted_cancellation_path(&old_request);
        std::fs::create_dir_all(old_cancellation_path.parent().unwrap()).unwrap();
        std::fs::write(
            &old_cancellation_path,
            serde_json::to_vec(&hosted_internal_cancellation_value(
                &old_request,
                100_012,
                None,
            ))
            .unwrap(),
        )
        .unwrap();
        let cancellation: HostedBrowserCancellation =
            serde_json::from_slice(&std::fs::read(&old_cancellation_path).unwrap()).unwrap();
        validate_hosted_cancellation(&cancellation, &old_cancellation_path).unwrap();

        assert!(
            matching_hosted_prepare_journal_for_cancellation(&cancellation)
                .unwrap()
                .is_none()
        );
        remove_matching_hosted_prepare_journal(&cancellation).unwrap();
        remove_hosted_request_artifacts(&old_cancellation_path).unwrap();

        assert!(!old_cancellation_path.exists());
        let retained =
            read_hosted_prepare_journal(&hosted_prepare_journal_path(&new_request)).unwrap();
        assert_eq!(retained.compensation.request_id, new_request.request_id);
        assert_eq!(
            retained.compensation.wrapper_instance_nonce,
            new_request.wrapper_instance_nonce
        );
    }

    #[test]
    fn failed_created_blank_delete_retains_wal_and_retries_after_repair() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let request = valid_hosted_prepare_request(100_000);
        let journal = new_hosted_prepare_journal(&request, "prepared_session", 100_001);
        let restore_path = paths::browser_workspace_restore_json(&request.session_token);
        std::fs::create_dir_all(&restore_path).unwrap();
        write_hosted_prepare_journal(&journal).unwrap();

        assert!(recover_hosted_prepare_journals_for_process_start().is_err());
        assert!(hosted_prepare_journal_path(&request).exists());

        std::fs::remove_dir(&restore_path).unwrap();
        recover_hosted_prepare_journals_for_process_start().unwrap();
        assert!(!hosted_prepare_journal_path(&request).exists());
    }

    #[test]
    fn startup_journal_directory_io_error_fails_closed() {
        let _env_lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let _home = PinvouHomeGuard::install(temp.path());
        let journal_dir = hosted_prepare_journal_dir();
        std::fs::create_dir_all(journal_dir.parent().unwrap()).unwrap();
        std::fs::write(&journal_dir, b"not-a-directory").unwrap();

        assert!(recover_hosted_prepare_journals_for_process_start().is_err());
    }

    #[test]
    fn startup_reconciliation_removes_only_orphan_browser_json_files() {
        let temp = tempfile::tempdir().unwrap();
        let directories = [
            temp.path().join("restore"),
            temp.path().join("workspaces"),
            temp.path().join("mcp-sessions"),
            temp.path().join("prepare-journal"),
        ];
        for directory in &directories {
            std::fs::create_dir_all(directory).unwrap();
            std::fs::write(directory.join("active-token.json"), b"active").unwrap();
            std::fs::write(directory.join("orphan-token.json"), b"orphan").unwrap();
            std::fs::write(directory.join("orphan-token.tmp"), b"temporary").unwrap();
        }
        let active = HashSet::from(["active-token".to_string()]);

        let cutoff_after_fixture = SystemTime::now() + Duration::from_secs(1);
        reconcile_browser_session_file_dirs(&active, &directories, cutoff_after_fixture).unwrap();
        // Idempotent retry after a partial/crash cleanup must also succeed.
        reconcile_browser_session_file_dirs(&active, &directories, cutoff_after_fixture).unwrap();

        for directory in &directories {
            assert!(directory.join("active-token.json").exists());
            assert!(!directory.join("orphan-token.json").exists());
            assert!(directory.join("orphan-token.tmp").exists());
        }
    }

    #[test]
    fn startup_reconciliation_never_deletes_files_newer_than_process_cutoff() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("restore");
        std::fs::create_dir_all(&directory).unwrap();
        let new_file = directory.join("new-session-token.json");
        std::fs::write(&new_file, b"new").unwrap();

        reconcile_browser_session_file_dirs(&HashSet::new(), &[directory], SystemTime::UNIX_EPOCH)
            .unwrap();
        assert!(new_file.exists());
    }

    #[test]
    fn process_start_quarantines_stale_host_requests_before_accepting_new_ones() {
        let temp = tempfile::tempdir().unwrap();
        let request_dir = temp.path().join("host-requests");
        std::fs::create_dir_all(&request_dir).unwrap();
        std::fs::write(request_dir.join("old-create.json"), b"stale").unwrap();
        std::fs::write(request_dir.join("old-close.cancelled"), b"stale").unwrap();

        reset_host_request_directory_for_process_start(&request_dir).unwrap();
        assert!(request_dir.is_dir());
        assert_eq!(std::fs::read_dir(&request_dir).unwrap().count(), 0);

        let current_request = request_dir.join("current.json");
        std::fs::write(&current_request, b"current").unwrap();
        assert!(current_request.exists());
    }

    #[test]
    fn native_visibility_rejects_old_renderer_and_out_of_order_sequence() {
        let mut clock = SurfaceVisibilityClock::default();
        let first = clock.begin_generation();
        assert_eq!(first, 1);
        assert!(clock.claim(first, 1));
        assert!(!clock.claim(first, 1));
        assert!(clock.claim(first, 10));
        assert!(!clock.claim(first, 9));

        let reloaded = clock.begin_generation();
        assert_eq!(reloaded, 2);
        assert!(!clock.claim(first, 100));
        assert!(clock.claim(reloaded, 1));
        assert_eq!(
            clock,
            SurfaceVisibilityClock {
                generation: reloaded,
                sequence: 1,
            }
        );
    }

    #[test]
    fn canceled_prepare_preserves_a_restored_manifest_but_deletes_a_new_blank() {
        assert_eq!(
            PreparedWorkspaceDisposition::RestoredExisting.rollback_kind(),
            Some("restored_session")
        );
        assert_eq!(
            PreparedWorkspaceDisposition::CreatedBlank.rollback_kind(),
            Some("prepared_session")
        );
        assert_eq!(PreparedWorkspaceDisposition::Existing.rollback_kind(), None);
    }

    #[test]
    fn lease_heartbeats_and_end_use_the_independent_control_plane() {
        for operation in [
            HostedBrowserOperation::AssertHostLease,
            HostedBrowserOperation::BeginAgentOperation,
            HostedBrowserOperation::RefreshAgentOperation,
            HostedBrowserOperation::RefreshAgentInput,
            HostedBrowserOperation::EndAgentOperation,
        ] {
            assert!(operation.is_control_plane());
        }
        for operation in [
            HostedBrowserOperation::Prepare,
            HostedBrowserOperation::CreateTab,
            HostedBrowserOperation::ActivateTab,
            HostedBrowserOperation::CloseTab,
            HostedBrowserOperation::RollbackCreatedTab,
            HostedBrowserOperation::CoreTool,
        ] {
            assert!(!operation.is_control_plane());
        }
    }

    #[test]
    fn restore_requires_a_real_cdp_or_browser_core_backend() {
        let webkit_surface = platform::NativeSurfaceCapabilities::new(true, true, false);
        assert!(!has_restore_automation_backend(webkit_surface, false));
        assert!(has_restore_automation_backend(webkit_surface, true));

        let windows_surface = platform::NativeSurfaceCapabilities::new(true, true, true);
        assert!(has_restore_automation_backend(windows_surface, false));
    }

    #[test]
    fn browser_core_reuses_only_the_productized_unused_initial_blank() {
        let session_id = "session-a";
        let initial = paths::browser_session_token(session_id);
        let blank = TabInfo {
            target_id: initial.clone(),
            page_id: Some(0),
            title: "about:blank".to_string(),
            url: "about:blank".to_string(),
        };

        assert!(should_reuse_browser_core_initial_tab(
            session_id,
            &initial,
            std::slice::from_ref(&blank),
            false,
        ));
        assert!(!should_reuse_browser_core_initial_tab(
            session_id,
            &initial,
            std::slice::from_ref(&blank),
            true,
        ));

        let navigated = TabInfo {
            url: "https://example.com".to_string(),
            ..blank.clone()
        };
        assert!(!should_reuse_browser_core_initial_tab(
            session_id,
            &initial,
            &[navigated],
            false,
        ));
        let restored_blank = TabInfo {
            target_id: "fedcba9876543210".to_string(),
            ..blank.clone()
        };
        assert!(!should_reuse_browser_core_initial_tab(
            session_id,
            "fedcba9876543210",
            &[restored_blank],
            false,
        ));
        assert!(!should_reuse_browser_core_initial_tab(
            session_id,
            &initial,
            &[blank.clone(), blank],
            false,
        ));
    }

    #[test]
    fn session_validator_rejects_crash_orphans_and_delete_tombstone_wins() {
        let manager = BrowserManager::new();
        manager.bind_session_validator(Arc::new(|session_id| session_id == "active-session"));
        assert!(manager
            .ensure_browser_session_allowed("active-session")
            .is_ok());
        assert!(manager
            .ensure_browser_session_allowed("orphan-from-previous-process")
            .is_err());

        manager.mark_session_deleted("active-session");
        assert!(manager
            .ensure_browser_session_allowed("active-session")
            .is_err());
    }

    #[test]
    fn host_consumer_startup_failure_blocks_browser_entry_points_immediately() {
        let manager = BrowserManager::new();
        *manager.prepare_recovery_error.lock() = Some(
            "browser/host-consumer-unavailable: injected request-dir reset failure".to_string(),
        );

        let error = manager
            .ensure_browser_session_allowed("session-a")
            .unwrap_err();
        assert!(error.contains("browser/host-consumer-unavailable"));
        assert!(error.contains("injected request-dir reset failure"));
    }

    // --- 按对话停止的生命周期路由 ---

    #[test]
    fn scoped_stop_closes_a_registered_native_session() {
        assert_eq!(
            scoped_stop_action(true, true),
            ScopedStopAction::CloseNativeSession
        );
    }

    #[test]
    fn scoped_stop_ignores_an_unknown_session_while_native_workspaces_exist() {
        assert_eq!(
            scoped_stop_action(false, true),
            ScopedStopAction::IgnoreUnknownNativeSession
        );
    }

    #[test]
    fn scoped_stop_cleans_managed_runtime_without_native_workspaces() {
        assert_eq!(
            scoped_stop_action(false, false),
            ScopedStopAction::StopManagedRuntime
        );
    }

    // --- browser 级 Target 事件路由（targetDestroyed 的 params 只有 targetId） ---

    #[test]
    fn route_target_created_page_creates() {
        let params = json!({ "targetInfo": { "targetId": "T1", "type": "page" } });
        assert_eq!(
            route_target_event("Target.targetCreated", &params),
            TargetEventRoute::Create("T1".to_string())
        );
    }

    #[test]
    fn route_target_created_non_page_ignored() {
        // iframe/worker 等非页面 target 不触发枚举/通知（事件风暴防护）。
        for ty in ["worker", "service_worker", "iframe", "other"] {
            let params = json!({ "targetInfo": { "targetId": "T1", "type": ty } });
            assert_eq!(
                route_target_event("Target.targetCreated", &params),
                TargetEventRoute::Ignore
            );
        }
    }

    #[test]
    fn route_target_destroyed_uses_top_level_target_id() {
        // 协议形状（此前漏掉的场景）：targetDestroyed 的 params 仅 { targetId }，
        // 无 targetInfo——旧实现按 targetInfo.type 过滤会把销毁事件全部丢弃。
        let params = json!({ "targetId": "T9" });
        assert_eq!(
            route_target_event("Target.targetDestroyed", &params),
            TargetEventRoute::Destroy("T9".to_string())
        );
    }

    #[test]
    fn route_target_destroyed_without_target_id_ignored() {
        assert_eq!(
            route_target_event("Target.targetDestroyed", &json!({})),
            TargetEventRoute::Ignore
        );
        // 损坏形状：targetId 出现在 targetInfo 里（旧实现的错误假设）不应路由。
        let params = json!({ "targetInfo": { "targetId": "T9", "type": "page" } });
        assert_eq!(
            route_target_event("Target.targetDestroyed", &params),
            TargetEventRoute::Ignore
        );
    }

    // --- 导航/新建标签页的 URL 协议白名单 ---

    #[test]
    fn is_allowed_url_accepts_http_https_about_blank() {
        assert!(is_allowed_url("http://example.com"));
        assert!(is_allowed_url("https://example.com/path?q=1"));
        assert!(is_allowed_url("HTTP://EXAMPLE.COM"));
        assert!(is_allowed_url("Https://example.com"));
        assert!(is_allowed_url("about:blank"));
        assert!(is_allowed_url("about:blank#pinvou-tab-0123456789abcdef"));
    }

    #[test]
    fn is_allowed_url_rejects_local_and_script_schemes() {
        assert!(!is_allowed_url("file:///etc/passwd"));
        assert!(!is_allowed_url("javascript:alert(1)"));
        assert!(!is_allowed_url("data:text/html,<script></script>"));
        assert!(!is_allowed_url("chrome://settings"));
        assert!(!is_allowed_url("http://tauri.localhost/"));
        assert!(!is_allowed_url("https://TAURI.LOCALHOST/index.html"));
        assert!(is_allowed_url("https://tauri.localhost.example.com/"));
        assert!(!is_allowed_url("http://127.0.0.1:1420/"));
        assert!(!is_allowed_url("http://LOCALHOST:1420/src/app/main.jsx"));
        assert!(!is_allowed_url("http://[::1]:1420/"));
        assert!(is_allowed_url("http://127.0.0.1:1421/"));
        assert!(!is_allowed_url(""));
        // 超短串（get(..7)/get(..8) 为 None，不得 panic）
        assert!(!is_allowed_url("http:"));
        assert!(!is_allowed_url("ht"));
        // 非 ASCII 前缀（get 切片遇非字符边界返回 None，不得 panic）
        assert!(!is_allowed_url("ｈｔｔｐ://example.com"));
    }

    // --- 端口文件解析（范围校验防 as u16 回绕） ---

    #[test]
    fn parse_port_json_accepts_valid_ports() {
        assert_eq!(parse_port_json(r#"{"port": 9222}"#), Some(9222));
        assert_eq!(parse_port_json(r#"{"port": 1}"#), Some(1));
        assert_eq!(parse_port_json(r#"{"port": 65535}"#), Some(65535));
    }

    #[test]
    fn parse_port_json_rejects_out_of_range_and_garbage() {
        assert_eq!(parse_port_json(r#"{"port": 0}"#), None);
        assert_eq!(parse_port_json(r#"{"port": 65536}"#), None);
        assert_eq!(parse_port_json(r#"{"port": 70000}"#), None);
        // 非数字、负数、缺字段、非法 JSON
        assert_eq!(parse_port_json(r#"{"port": "9222"}"#), None);
        assert_eq!(parse_port_json(r#"{"port": -1}"#), None);
        assert_eq!(parse_port_json(r#"{"pid": 123}"#), None);
        assert_eq!(parse_port_json("not json"), None);
    }

    #[test]
    fn host_owned_port_rejects_external_browser_endpoints() {
        assert_eq!(
            parse_host_owned_port_json(r#"{"port":9222,"owner":"app"}"#),
            Some(9222)
        );
        assert_eq!(
            parse_host_owned_port_json(r#"{"port":9222,"owner":"mcp"}"#),
            None
        );
        assert_eq!(
            parse_host_owned_port_json(r#"{"port":9222,"owner":"app","browser_pid":1234}"#),
            None
        );
        assert_eq!(parse_host_owned_port_json(r#"{"port":9222}"#), None);
    }
}
