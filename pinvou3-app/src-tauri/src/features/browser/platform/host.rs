//! 桌面系统 WebView 的公共承载层。
//!
//! 工作区、标签、布局和页面生命周期不依赖具体浏览器内核；平台实现只负责配置
//! WebView builder 以及声明可用的自动化后端。这样 macOS/Linux 可以复用真实
//! 系统 WebView，而不会被误报成支持 Chrome CDP。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, LazyLock,
};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tauri::{
    webview::{DownloadEvent, NewWindowResponse, PageLoadEvent},
    Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewBuilder, WebviewUrl,
};

use super::super::{NativeSurfaceBounds, TabInfo};
use super::state::{
    AgentCallerEpoch, ControlSnapshot, NativeControlOwner, NativeRequestCancel, NativeRequestClaim,
    NativeTabLease, RequestLedger, RetainedAgentOperation, SurfaceEntry, TabRegistry,
    WorkspaceControl,
};
use super::{NativeSurfaceCapabilities, NativeWorkspaceRestore};

const BROWSER_CORE_RUNTIME: &str =
    include_str!("../../../../resources/common/bundle/mcp-servers/browser-core-runtime.js");
use crate::platform::paths;

const WEBVIEW_LABEL_PREFIX: &str = "agent-browser-";
const USER_TAKEOVER_SCHEME: &str = "pinvou-user-takeover";
const USER_CONTROL_IDLE_TIMEOUT: Duration = Duration::from_secs(3);
const WORKSPACE_RESTORE_VERSION: u8 = 1;
/// 单个任务浏览器可持有的已发布标签与隐藏 staging 总数。恢复清单与运行时创建
/// 共用同一上限，避免页面通过 window.open 或并发 new_page 无界创建 child WebView。
const MAX_WORKSPACE_TABS: usize = 64;
const MAX_RESTORE_URL_LEN: usize = 16 * 1024;
const MAX_SAFE_PAGE_ID: u64 = (1_u64 << 53) - 1;
const NATIVE_PAGE_ID_SEQUENCE_BITS: u32 = 21;
const NATIVE_PAGE_ID_SEQUENCE_LIMIT: u64 = 1_u64 << NATIVE_PAGE_ID_SEQUENCE_BITS;
const NATIVE_PAGE_ID_INCARNATION_LIMIT: u64 = 1_u64 << (53 - NATIVE_PAGE_ID_SEQUENCE_BITS);
const ACTION_COMMIT_UNKNOWN_TAB_NAVIGATION: &str =
    "browser/action-commit-unknown-after-tab-navigation";
static NATIVE_PAGE_ID_INCARNATION: LazyLock<u64> =
    LazyLock::new(|| rand::random::<u64>() % NATIVE_PAGE_ID_INCARNATION_LIMIT);
static NEXT_NATIVE_PAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct WebviewBuildError {
    message: String,
    /// `add_child` succeeded, but the mandatory initial hide and the compensating
    /// close both failed.  The WebView is still a real native resource and must
    /// be transferred into the host registry instead of being orphaned.
    survivor: Option<SurfaceEntry>,
}

impl WebviewBuildError {
    fn new(message: String) -> Self {
        Self {
            message,
            survivor: None,
        }
    }

    fn with_survivor(message: String, survivor: SurfaceEntry) -> Self {
        Self {
            message,
            survivor: Some(survivor),
        }
    }
}

fn next_native_page_id() -> Result<u64, String> {
    let sequence = NEXT_NATIVE_PAGE_SEQUENCE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            (current < NATIVE_PAGE_ID_SEQUENCE_LIMIT).then(|| current + 1)
        })
        .map_err(|_| "浏览器 pageId 空间已耗尽，请重启应用".to_string())?;
    compose_native_page_id(*NATIVE_PAGE_ID_INCARNATION, sequence)
        .ok_or_else(|| "浏览器 pageId 空间已耗尽，请重启应用".to_string())
}

fn compose_native_page_id(incarnation: u64, sequence: u64) -> Option<u64> {
    if incarnation >= NATIVE_PAGE_ID_INCARNATION_LIMIT || sequence >= NATIVE_PAGE_ID_SEQUENCE_LIMIT
    {
        return None;
    }
    let page_id = (incarnation << NATIVE_PAGE_ID_SEQUENCE_BITS) | sequence;
    (page_id <= MAX_SAFE_PAGE_ID).then_some(page_id)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceRestoreFile {
    version: u8,
    active_index: usize,
    tabs: Vec<WorkspaceRestoreTab>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceRestoreTab {
    url: String,
}

pub(crate) trait PlatformWebviewConfig: Default {
    /// 现有 BrowserManager 的 `prepare` 返回值表示“同一页面已具备 Agent 自动化”。
    /// WebKit 平台在自有工具后端接通前必须保持 false，不能让上层误连 CDP。
    const ACTIVATION_READY: bool;

    fn capabilities(&self) -> NativeSurfaceCapabilities;

    fn requires_reset(&self, automation_port: Option<u16>, data_directory: &Path) -> bool;

    fn prepare(
        &mut self,
        automation_port: Option<u16>,
        data_directory: &Path,
    ) -> Result<(), String>;

    fn configure_builder(
        &self,
        builder: WebviewBuilder<tauri::Wry>,
        data_directory: &Path,
    ) -> Result<WebviewBuilder<tauri::Wry>, String>;

    fn reset(&mut self);

    fn owns_port(&self, port: u16) -> bool;

    fn is_initialized(&self) -> bool;
}

struct Workspace {
    session_token: String,
    tabs: TabRegistry,
    active_tab: String,
    bounds: Option<NativeSurfaceBounds>,
    visible: bool,
    control: Arc<WorkspaceControl>,
    /// A host Prepare that created this process-local workspace may be
    /// compensated only while this exact request still owns the untouched
    /// preparation generation. A later UI/host prepare clears or replaces it;
    /// any user/Agent mutation advances `revision` and makes rollback a no-op.
    prepare_generation: Option<PrepareGeneration>,
}

#[derive(Debug, Clone)]
struct PrepareGeneration {
    request_id: String,
    revision: u64,
}

pub(crate) struct DesktopBrowserSurface<P: PlatformWebviewConfig> {
    platform: P,
    data_directory: Option<PathBuf>,
    workspaces: HashMap<String, Workspace>,
    /// Agent create_tab 的隐藏候选页。只有 target 发现、首航提交和最终 lease CAS
    /// 全部成功后才移入工作区；因此异步绑定失败绝不会关闭用户已接管的已发布页面。
    staged_tabs: HashMap<(String, String), SurfaceEntry>,
    /// UI/User popup 同样先在隐藏 marker 上完成自动化绑定。这里仅记录其最终发布时
    /// 是否应保持后台；候选 WebView 本体仍由 staged_tabs 统一持有和清理。
    staged_user_tabs: HashMap<(String, String), bool>,
    /// Native children whose initial hide and compensating close both failed.
    /// They are owned for cleanup/capacity only and are never eligible for
    /// binding, activation, or publication as a normal workspace/staged tab.
    quarantined_tabs: HashMap<(String, String), SurfaceEntry>,
    active_session: Option<String>,
    requests: RequestLedger,
}

impl<P: PlatformWebviewConfig> Default for DesktopBrowserSurface<P> {
    fn default() -> Self {
        Self {
            platform: P::default(),
            data_directory: None,
            workspaces: HashMap::new(),
            staged_tabs: HashMap::new(),
            staged_user_tabs: HashMap::new(),
            quarantined_tabs: HashMap::new(),
            active_session: None,
            requests: RequestLedger::default(),
        }
    }
}

impl<P: PlatformWebviewConfig> DesktopBrowserSurface<P> {
    pub fn capabilities(&self) -> NativeSurfaceCapabilities {
        self.platform.capabilities()
    }

    /// 对 requestId 做幂等 claim。只有 `Execute` 允许创建资源。
    pub fn claim_request(
        &mut self,
        session_id: &str,
        request_id: &str,
    ) -> Result<NativeRequestClaim, String> {
        self.requests.claim(session_id, request_id)
    }

    /// 提交请求结果。返回 false 时 cancel tombstone 已先到达，调用方必须回滚资源。
    pub fn complete_request(
        &mut self,
        session_id: &str,
        request_id: &str,
        result: serde_json::Value,
    ) -> Result<bool, String> {
        self.requests.complete(session_id, request_id, result)
    }

    /// 取消请求；`AlreadyCompleted` 携带调用方执行补偿回滚所需的原结果。
    pub fn cancel_request(
        &mut self,
        session_id: &str,
        request_id: &str,
    ) -> Result<NativeRequestCancel, String> {
        self.requests.cancel(session_id, request_id)
    }

    pub fn acknowledge_request_cancellation(
        &mut self,
        session_id: &str,
        request_id: &str,
    ) -> Result<(), String> {
        self.requests
            .acknowledge_cancellation(session_id, request_id)
    }

    /// 兼容现有 BrowserManager 的准备入口。
    ///
    /// macOS/Linux 已具备真实页面承载能力，但在自有 Agent 工具后端接通前，不能
    /// 返回 true 让 chrome-devtools-mcp 尝试连接 WebKit。因此这些平台通过
    /// [`Self::prepare_display_only`] 提供可编译、可独立验证的显示层，现有运行路径
    /// 仍安全回退。
    pub fn prepare(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        session_token: &str,
        port: u16,
        data_directory: &Path,
    ) -> Result<bool, String> {
        if !P::ACTIVATION_READY {
            return Ok(false);
        }
        self.prepare_surface(
            app,
            session_id,
            session_token,
            Some(port),
            data_directory,
            NativeControlOwner::Agent,
        )
    }

    /// 用户从普通模式主动展开浏览器时按需创建空白工作区。仅“打开侧栏”不等于
    /// 用户已经接管页面，因此保持 Unclaimed；随后用户真实交互或 Agent 首个工具
    /// 会以现有控制权协议认领该工作区。
    pub fn prepare_unclaimed(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        session_token: &str,
        port: u16,
        data_directory: &Path,
    ) -> Result<bool, String> {
        if !P::ACTIVATION_READY {
            return Ok(false);
        }
        self.prepare_surface(
            app,
            session_id,
            session_token,
            Some(port),
            data_directory,
            NativeControlOwner::Unclaimed,
        )
    }

    /// 创建真实系统 WebView 工作区，但不对 Agent 自动化能力作任何承诺。
    pub fn prepare_display_only(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        session_token: &str,
        data_directory: &Path,
    ) -> Result<bool, String> {
        self.prepare_surface(
            app,
            session_id,
            session_token,
            None,
            data_directory,
            NativeControlOwner::Unclaimed,
        )
    }

    /// 读取可跨应用进程恢复的最小页面清单。该清单与 MCP 使用的运行期 target
    /// 映射完全分离：它不包含 session/tab token、targetId、lease 或控制权状态。
    pub fn read_restore_workspace(
        session_id: &str,
    ) -> Result<Option<NativeWorkspaceRestore>, String> {
        let session_token = paths::browser_session_token(session_id);
        let path = paths::browser_workspace_restore_json(&session_token);
        let encoded = match std::fs::read(&path) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!("读取浏览器恢复清单失败: {error}"));
            }
        };
        parse_restore_workspace(&encoded).map(Some)
    }

    /// 恢复失败时把调用前读取的清单原样写回，避免构建期间的 about:blank 导航事件
    /// 覆盖最后一个可用快照。
    pub fn write_restore_workspace(
        session_id: &str,
        restore: &NativeWorkspaceRestore,
    ) -> Result<(), String> {
        write_restore_workspace_file(
            &paths::browser_workspace_restore_json(&paths::browser_session_token(session_id)),
            restore,
        )
    }

    /// 从恢复清单创建一组全新的原生 WebView。任何声明 Agent 自动化的后端都先
    /// 停留在私有 marker；调用方必须为每个新 tab 绑定本进程的新 target、完成首航，
    /// 最后一个标签完成后才会整体发布。display-only 平台才可直接加载 URL。
    pub fn prepare_restored_surface(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        session_token: &str,
        automation_port: Option<u16>,
        data_directory: &Path,
        restore: &NativeWorkspaceRestore,
    ) -> Result<Vec<String>, String> {
        if session_id.is_empty() || !is_valid_token(session_token) {
            return Err("浏览器会话身份无效".to_string());
        }
        if restore.urls.is_empty()
            || restore.urls.len() > MAX_WORKSPACE_TABS
            || restore.active_index >= restore.urls.len()
        {
            return Err("浏览器恢复清单无效".to_string());
        }
        if automation_port.is_some() && !P::ACTIVATION_READY {
            return Err("当前平台没有可用的浏览器自动化后端".to_string());
        }
        self.reap_quarantined_for_session(app, session_id)?;
        if self.workspaces.contains_key(session_id) {
            return Ok(self
                .workspaces
                .get(session_id)
                .expect("工作区已检查")
                .tabs
                .iter()
                .map(|tab| tab.token.clone())
                .collect());
        }
        if self
            .platform
            .requires_reset(automation_port, data_directory)
            || self
                .data_directory
                .as_deref()
                .is_some_and(|current| current != data_directory)
        {
            self.close(Some(app))?;
        }

        std::fs::create_dir_all(data_directory)
            .map_err(|error| format!("创建浏览器数据目录失败: {error}"))?;
        crate::platform::os::make_private_dir(data_directory);
        self.platform.prepare(automation_port, data_directory)?;
        self.data_directory = Some(data_directory.to_path_buf());

        // 恢复清单不持久化控制权。重启本身既不是用户接管，也不是 Agent 授权；
        // 恢复页保持中立，随后真实发生的 UI/可信输入或 Agent lease 认领决定 owner。
        let control = Arc::new(WorkspaceControl::new(1, NativeControlOwner::Unclaimed));
        let mut entries: Vec<SurfaceEntry> = Vec::with_capacity(restore.urls.len());
        let mut tab_tokens = Vec::with_capacity(restore.urls.len());
        let requires_automation_binding =
            automation_port.is_some() || self.platform.capabilities().agent_automation;
        for url in &restore.urls {
            let tab_token = self.fresh_tab_token(&tab_tokens);
            let initial_url = if requires_automation_binding {
                format!("about:blank#pinvou-tab-{tab_token}")
            } else {
                marked_blank_url(url, &tab_token)
            };
            let entry = match build_webview(
                &self.platform,
                app,
                session_id,
                &tab_token,
                data_directory,
                &initial_url,
                Arc::clone(&control),
                false,
            ) {
                Ok(entry) => entry,
                Err(mut error) => {
                    if let Some(survivor) = error.survivor.take() {
                        entries.push(survivor);
                    }
                    let cleanup = self.quarantine_and_reconcile_entries(app, session_id, entries);
                    return match cleanup {
                        Ok(()) => Err(error.message),
                        Err(cleanup_error) => Err(format!(
                            "{}; 恢复构建失败后的表面对账尚未完成: {cleanup_error}",
                            error.message
                        )),
                    };
                }
            };
            entries.push(entry);
            tab_tokens.push(tab_token);
        }

        let mut entries = entries.into_iter();
        let first = entries.next().expect("恢复清单至少包含一个标签");
        let mut tabs = TabRegistry::from_entry(first);
        for entry in entries {
            tabs.insert(entry)?;
        }
        if !requires_automation_binding {
            for entry in tabs.iter() {
                entry.publish();
            }
        }
        let active_tab = tab_tokens[restore.active_index].clone();
        self.workspaces.insert(
            session_id.to_string(),
            Workspace {
                session_token: session_token.to_string(),
                tabs,
                active_tab: active_tab.clone(),
                bounds: None,
                visible: false,
                control,
                prepare_generation: None,
            },
        );
        if requires_automation_binding {
            // 旧 target 映射只属于上一个进程。新 WebView 全部完成 bind + navigate 前
            // 不写半成品映射，也不发布 control/tabs 事件。
            let _ = std::fs::remove_file(paths::browser_workspace_state_json(session_token));
        } else {
            if let Err(error) = self.persist_workspace(session_id) {
                if let Some(mut workspace) = self.workspaces.remove(session_id) {
                    if let Err(close_error) = close_workspace(app, &mut workspace) {
                        if !workspace.tabs.is_empty() {
                            self.workspaces.insert(session_id.to_string(), workspace);
                        }
                        return Err(format!("{error}; 回滚恢复工作区失败: {close_error}"));
                    }
                }
                return Err(error);
            }
            if let Some(state) = self.control_state(session_id) {
                emit_control_changed(app, session_id, &active_tab, state);
            }
        }
        Ok(tab_tokens)
    }

    fn fresh_tab_token(&self, pending: &[String]) -> String {
        loop {
            let candidate = format!("{:016x}", rand::random::<u64>());
            if pending.iter().any(|token| token == &candidate) {
                continue;
            }
            if self
                .workspaces
                .values()
                .any(|workspace| workspace.tabs.by_token(&candidate).is_some())
            {
                continue;
            }
            if self
                .staged_tabs
                .keys()
                .any(|(_, tab_token)| tab_token == &candidate)
            {
                continue;
            }
            if self
                .quarantined_tabs
                .keys()
                .any(|(_, tab_token)| tab_token == &candidate)
            {
                continue;
            }
            return candidate;
        }
    }

    /// Transfer native surfaces that could not be closed into a cleanup-only
    /// registry. Quarantined entries never masquerade as a usable workspace or
    /// as a legitimate in-flight create-tab candidate.
    fn quarantine_and_reconcile_entries(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        entries: Vec<SurfaceEntry>,
    ) -> Result<(), String> {
        self.quarantine_entries(session_id, entries);
        self.reap_quarantined_for_session(app, session_id)
    }

    fn quarantine_entries(&mut self, session_id: &str, entries: Vec<SurfaceEntry>) {
        for mut entry in entries {
            entry.unpublish();
            entry.created_by_request_id = None;
            self.quarantined_tabs
                .insert((session_id.to_string(), entry.token.clone()), entry);
        }
    }

    /// Remove a failed restore transaction from every business-visible lookup
    /// before retrying native close. The restore manifest remains authoritative;
    /// any close survivor is cleanup-only and cannot be mistaken for an
    /// `Existing` workspace by a later Prepare.
    fn quarantine_workspace_for_failed_restore(&mut self, session_id: &str) {
        let Some(mut workspace) = self.workspaces.remove(session_id) else {
            return;
        };
        let tokens = workspace
            .tabs
            .iter()
            .map(|entry| entry.token.clone())
            .collect::<Vec<_>>();
        let entries = tokens
            .into_iter()
            .filter_map(|token| workspace.tabs.remove_token(&token).map(|(_, entry)| entry))
            .collect::<Vec<_>>();
        let _ = std::fs::remove_file(paths::browser_workspace_state_json(
            &workspace.session_token,
        ));
        if self.active_session.as_deref() == Some(session_id) {
            self.active_session = None;
        }
        self.quarantine_entries(session_id, entries);
    }

    fn reap_quarantined_for_session(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
    ) -> Result<(), String> {
        reconcile_quarantined_close(&mut self.quarantined_tabs, session_id, |entry| {
            if let Some(webview) = app.get_webview(&entry.label) {
                webview
                    .close()
                    .map_err(|error| format!("关闭隔离系统 WebView 失败: {error}"))?;
            }
            Ok(())
        })
    }

    pub fn generate_tab_token(&self) -> String {
        self.fresh_tab_token(&[])
    }

    fn ensure_tab_capacity(&self, session_id: &str) -> Result<(), String> {
        let published = self
            .workspaces
            .get(session_id)
            .map(|workspace| workspace.tabs.len())
            .unwrap_or(0);
        let staged = self
            .staged_tabs
            .keys()
            .filter(|(owner_session, _)| owner_session == session_id)
            .count();
        let quarantined = self
            .quarantined_tabs
            .keys()
            .filter(|(owner_session, _)| owner_session == session_id)
            .count();
        if published.saturating_add(staged).saturating_add(quarantined) >= MAX_WORKSPACE_TABS {
            return Err(format!(
                "单个任务最多打开 {MAX_WORKSPACE_TABS} 个浏览器标签，请先关闭不再使用的标签"
            ));
        }
        Ok(())
    }

    fn prepare_surface(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        session_token: &str,
        automation_port: Option<u16>,
        data_directory: &Path,
        initial_owner: NativeControlOwner,
    ) -> Result<bool, String> {
        if session_id.is_empty() || !is_valid_token(session_token) {
            return Err("浏览器会话身份无效".to_string());
        }
        self.reap_quarantined_for_session(app, session_id)?;
        if self
            .platform
            .requires_reset(automation_port, data_directory)
            || self
                .data_directory
                .as_deref()
                .is_some_and(|current| current != data_directory)
        {
            self.close(Some(app))?;
        }
        if let Some(workspace) = self.workspaces.get(session_id) {
            if workspace.session_token != session_token {
                return Err("浏览器会话 token 与现有工作区不一致".to_string());
            }
            if !workspace.tabs.is_empty() {
                return Ok(true);
            }
        }
        if self.workspaces.iter().any(|(owner_session, workspace)| {
            owner_session != session_id && workspace.tabs.by_token(session_token).is_some()
        }) {
            return Err("浏览器标签 token 已属于其他对话".to_string());
        }

        std::fs::create_dir_all(data_directory)
            .map_err(|e| format!("创建浏览器数据目录失败: {e}"))?;
        crate::platform::os::make_private_dir(data_directory);
        self.platform.prepare(automation_port, data_directory)?;
        self.data_directory = Some(data_directory.to_path_buf());

        let control = Arc::new(WorkspaceControl::new(1, initial_owner));
        let entry = match build_webview(
            &self.platform,
            app,
            session_id,
            session_token,
            data_directory,
            &format!("about:blank#pinvou-session-{session_token}"),
            Arc::clone(&control),
            false,
        ) {
            Ok(entry) => entry,
            Err(mut error) => {
                let entries = error.survivor.take().into_iter().collect();
                let cleanup = self.quarantine_and_reconcile_entries(app, session_id, entries);
                return match cleanup {
                    Ok(()) => Err(error.message),
                    Err(cleanup_error) => Err(format!(
                        "{}; 初始化失败后的表面对账尚未完成: {cleanup_error}",
                        error.message
                    )),
                };
            }
        };
        entry.publish();
        self.workspaces.insert(
            session_id.to_string(),
            Workspace {
                session_token: session_token.to_string(),
                active_tab: session_token.to_string(),
                tabs: TabRegistry::from_entry(entry),
                bounds: None,
                visible: false,
                control,
                prepare_generation: None,
            },
        );
        if let Err(error) = self.persist_workspace(session_id) {
            if let Some(mut workspace) = self.workspaces.remove(session_id) {
                if let Err(close_error) = close_workspace(app, &mut workspace) {
                    if !workspace.tabs.is_empty() {
                        self.workspaces.insert(session_id.to_string(), workspace);
                    }
                    return Err(format!("{error}; 回滚浏览器工作区失败: {close_error}"));
                }
            }
            return Err(error);
        }
        if let Err(error) = self.persist_restore_workspace(app, session_id) {
            if let Some(mut workspace) = self.workspaces.remove(session_id) {
                if let Err(close_error) = close_workspace(app, &mut workspace) {
                    if !workspace.tabs.is_empty() {
                        self.workspaces.insert(session_id.to_string(), workspace);
                    }
                    return Err(format!("{error}; 回滚浏览器工作区失败: {close_error}"));
                }
            }
            let _ = std::fs::remove_file(paths::browser_workspace_state_json(session_token));
            return Err(error);
        }
        if let Some(state) = self.control_state(session_id) {
            emit_control_changed(app, session_id, session_token, state);
        }
        Ok(true)
    }

    pub fn create_tab(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        tab_token: &str,
        url: &str,
        background: bool,
    ) -> Result<Option<String>, String> {
        self.create_user_tab(app, session_id, tab_token, url, background)
    }

    /// wrapper 创建标签必须携带当前可见页的同一宿主 lease。WebView 可以先在隐藏
    /// 状态 staging，但注册到工作区前必须在控制锁内 CAS 提交；用户接管先提交时
    /// 立即关闭 staging WebView，绝不覆盖 User owner。
    pub fn create_tab_for_agent(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        tab_token: &str,
        url: &str,
        background: bool,
        authorization: &NativeTabLease,
        creation_id: &str,
    ) -> Result<Option<String>, String> {
        if !is_valid_token(tab_token) || creation_id.is_empty() {
            return Err("浏览器标签或创建 generation 身份无效".to_string());
        }
        self.reap_quarantined_for_session(app, session_id)?;
        if self
            .workspaces
            .values()
            .any(|workspace| workspace.tabs.by_token(tab_token).is_some())
            || self
                .staged_tabs
                .keys()
                .any(|(_, staged_token)| staged_token == tab_token)
            || self
                .quarantined_tabs
                .keys()
                .any(|(_, quarantined_token)| quarantined_token == tab_token)
        {
            return Err("浏览器标签 token 已被其他创建请求占用".to_string());
        }
        self.ensure_tab_capacity(session_id)?;
        let workspace = match self.workspaces.get(session_id) {
            Some(workspace) => workspace,
            None => return Ok(None),
        };
        validate_agent_mutation(workspace, session_id, authorization, None)?;
        if !workspace
            .control
            .assert_agent_lease(authorization.revision, &authorization.lease)
        {
            return Err("Agent mutation lease 已失效；用户可能已接管浏览器".to_string());
        }
        if !self.platform.is_initialized() {
            return Err("浏览器系统 WebView 尚未就绪".to_string());
        }
        let data_directory = self
            .data_directory
            .clone()
            .ok_or_else(|| "浏览器数据目录尚未就绪".to_string())?;
        let mut entry = match build_webview(
            &self.platform,
            app,
            session_id,
            tab_token,
            &data_directory,
            &marked_blank_url(url, tab_token),
            Arc::clone(&workspace.control),
            false,
        ) {
            Ok(entry) => entry,
            Err(mut error) => {
                if let Some(survivor) = error.survivor.take() {
                    self.quarantined_tabs
                        .insert((session_id.to_string(), tab_token.to_string()), survivor);
                }
                return Err(error.message);
            }
        };
        entry.created_by_request_id = Some(creation_id.to_string());
        self.staged_tabs
            .insert((session_id.to_string(), tab_token.to_string()), entry);
        let _ = background; // publishing/activation is decided at the final CAS.
        Ok(Some(tab_token.to_string()))
    }

    /// Discover 后由 BrowserManager 调用：先让隐藏页首航，再在同一宿主 lease 的
    /// CAS 临界区内写入权威映射、发布页面并按 background 决定是否激活。
    #[allow(clippy::too_many_arguments)]
    pub fn commit_created_tab_for_agent<F>(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        tab_token: &str,
        automation_target: &str,
        requested_url: &str,
        background: bool,
        authorization: &NativeTabLease,
        creation_id: &str,
        retained_popup: Option<&RetainedAgentOperation>,
        mut caller_guard: F,
    ) -> Result<bool, String>
    where
        F: FnMut() -> Result<(), String>,
    {
        let result = self.commit_created_tab_for_agent_inner(
            app,
            session_id,
            tab_token,
            automation_target,
            requested_url,
            background,
            authorization,
            creation_id,
            retained_popup,
            &mut caller_guard,
        );
        match result {
            Ok(true) => Ok(true),
            Ok(false) => match self.rollback_staged_agent_creation(
                Some(app),
                session_id,
                tab_token,
                creation_id,
            ) {
                Ok(_) => Ok(false),
                Err(rollback_error) => Err(format!(
                    "新建标签未提交；精确回滚隐藏候选失败: {rollback_error}"
                )),
            },
            Err(error) => match self.rollback_staged_agent_creation(
                Some(app),
                session_id,
                tab_token,
                creation_id,
            ) {
                Ok(_) => Err(error),
                Err(rollback_error) => {
                    Err(format!("{error}; 精确回滚隐藏候选失败: {rollback_error}"))
                }
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_created_tab_for_agent_inner<F>(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        tab_token: &str,
        automation_target: &str,
        requested_url: &str,
        background: bool,
        authorization: &NativeTabLease,
        creation_id: &str,
        retained_popup: Option<&RetainedAgentOperation>,
        caller_guard: &mut F,
    ) -> Result<bool, String>
    where
        F: FnMut() -> Result<(), String>,
    {
        if automation_target.is_empty() || automation_target.len() > 512 {
            return Err("浏览器自动化 targetId 无效".to_string());
        }
        if self
            .workspaces
            .values()
            .any(|workspace| workspace.tabs.token_for_target(automation_target).is_some())
        {
            return Err("浏览器自动化 target 已属于其他标签".to_string());
        }
        let key = (session_id.to_string(), tab_token.to_string());
        let mut entry = self
            .staged_tabs
            .get(&key)
            .cloned()
            .ok_or_else(|| "待提交的浏览器标签不存在".to_string())?;
        if entry.created_by_request_id.as_deref() != Some(creation_id) {
            return Err("创建 generation 与待提交标签不匹配".to_string());
        }
        let workspace = self
            .workspaces
            .get_mut(session_id)
            .ok_or_else(|| "浏览器工作区不存在".to_string())?;
        validate_agent_mutation(workspace, session_id, authorization, None)?;

        let webview = app
            .get_webview(&entry.label)
            .ok_or_else(|| "待提交的浏览器标签页表面不存在".to_string())?;
        let requested_url = requested_url
            .parse::<tauri::Url>()
            .map_err(|error| format!("浏览器首航地址无效: {error}"))?;
        let control = Arc::clone(&workspace.control);
        caller_guard()?;
        if retained_popup
            .is_some_and(|retained| !control.authorize_retained_agent_operation(retained))
        {
            return Err("popup Agent operation holder 已失效".to_string());
        }
        if !control.authorize_agent_dispatch(authorization) {
            return Err("Agent mutation lease 已失效；用户可能已接管浏览器".to_string());
        }
        // entry.published=false，首航及其同步回调不会发布 UI 事件或改变控制权。
        webview.navigate(requested_url).map_err(|error| {
            format!("{ACTION_COMMIT_UNKNOWN_TAB_NAVIGATION}: 浏览器隐藏标签首航响应不确定: {error}")
        })?;
        caller_guard().map_err(|error| {
            format!(
                "{ACTION_COMMIT_UNKNOWN_TAB_NAVIGATION}: 隐藏标签首航已派发，但调用方 epoch 已失效: {error}"
            )
        })?;
        if retained_popup
            .is_some_and(|retained| !control.authorize_retained_agent_operation(retained))
        {
            return Err(format!(
                "{ACTION_COMMIT_UNKNOWN_TAB_NAVIGATION}: 隐藏标签首航已派发，但 popup holder 已失效"
            ));
        }
        entry.automation_target = Some(automation_target.to_string());
        entry.created_at_revision = Some(authorization.revision.saturating_add(1));

        let previous_active = workspace.active_tab.clone();
        let staged_publication = Arc::clone(&entry.published);
        let committed = control.commit_agent_mutation(authorization, || {
            workspace.tabs.insert(entry.clone())?;
            if !background {
                workspace.active_tab = tab_token.to_string();
            }
            let committed_revision = authorization.revision.saturating_add(1);
            if let Err(error) = persist_workspace_snapshot(workspace, committed_revision) {
                let _ = workspace.tabs.remove_token(tab_token);
                workspace.active_tab = previous_active.clone();
                return Err(error);
            }
            if !background && workspace.visible {
                if let Err(error) = show_active_workspace(app, workspace) {
                    let _ = workspace.tabs.remove_token(tab_token);
                    workspace.active_tab = previous_active.clone();
                    if let Err(restore_error) =
                        persist_workspace_snapshot(workspace, authorization.revision)
                    {
                        eprintln!("[browser] 回滚新标签映射失败: {restore_error}");
                    }
                    if let Err(restore_error) = show_active_workspace(app, workspace) {
                        eprintln!("[browser] 回滚新标签显示失败: {restore_error}");
                    }
                    return Err(error);
                }
            }
            staged_publication.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });
        if !matches!(committed, Ok(Some(_))) {
            return match committed {
                Ok(None) => Err(format!(
                    "{ACTION_COMMIT_UNKNOWN_TAB_NAVIGATION}: 首航已派发，但 Agent mutation lease 在发布前失效"
                )),
                Err(error) => Err(format!(
                    "{ACTION_COMMIT_UNKNOWN_TAB_NAVIGATION}: 首航已派发，但宿主发布失败: {error}"
                )),
                Ok(Some(_)) => unreachable!(),
            };
        }
        self.staged_tabs.remove(&key);
        let snapshot = control.snapshot();
        emit_control_changed(app, session_id, &workspace.active_tab, snapshot);
        if let Err(error) = self.persist_restore_workspace(app, session_id) {
            eprintln!("[browser] 新建标签已提交，但恢复清单刷新失败: {error}");
        }
        Ok(true)
    }

    fn create_user_tab(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        tab_token: &str,
        url: &str,
        background: bool,
    ) -> Result<Option<String>, String> {
        if !is_valid_token(tab_token) {
            return Err("浏览器标签身份无效".to_string());
        }
        self.reap_quarantined_for_session(app, session_id)?;
        if self.workspaces.iter().any(|(owner_session, workspace)| {
            owner_session != session_id && workspace.tabs.by_token(tab_token).is_some()
        }) || self
            .staged_tabs
            .keys()
            .any(|(_, staged_token)| staged_token == tab_token)
            || self
                .quarantined_tabs
                .keys()
                .any(|(_, quarantined_token)| quarantined_token == tab_token)
        {
            return Err("浏览器标签 token 已属于其他对话".to_string());
        }
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(None);
        };
        if workspace.tabs.by_token(tab_token).is_some() {
            return Ok(Some(tab_token.to_string()));
        }
        self.ensure_tab_capacity(session_id)?;
        let workspace = self
            .workspaces
            .get(session_id)
            .expect("工作区已在容量检查前确认存在");
        if !self.platform.is_initialized() {
            return Err("浏览器系统 WebView 尚未就绪".to_string());
        }
        let data_directory = self
            .data_directory
            .clone()
            .ok_or_else(|| "浏览器数据目录尚未就绪".to_string())?;
        let control = Arc::clone(&workspace.control);
        let initial_url = marked_blank_url(url, tab_token);
        let mut entry = match build_webview(
            &self.platform,
            app,
            session_id,
            tab_token,
            &data_directory,
            &initial_url,
            Arc::clone(&control),
            false,
        ) {
            Ok(entry) => entry,
            Err(mut error) => {
                if let Some(survivor) = error.survivor.take() {
                    self.quarantined_tabs
                        .insert((session_id.to_string(), tab_token.to_string()), survivor);
                }
                return Err(error.message);
            }
        };
        entry.created_by_request_id = None;
        if has_internal_marker_for_token(&initial_url, tab_token) {
            let key = (session_id.to_string(), tab_token.to_string());
            self.staged_tabs.insert(key.clone(), entry);
            self.staged_user_tabs.insert(key, background);
            return Ok(Some(tab_token.to_string()));
        }
        let created_label = entry.label.clone();
        let created_publication = Arc::clone(&entry.published);

        let workspace = self
            .workspaces
            .get_mut(session_id)
            .expect("工作区已在上方检查");
        if let Err(error) = workspace.tabs.insert(entry) {
            if let Some(webview) = app.get_webview(&created_label) {
                let _ = webview.close();
            }
            return Err(error);
        }
        let previous_active = workspace.active_tab.clone();
        if !background {
            workspace.active_tab = tab_token.to_string();
        }
        control.bump(Some(NativeControlOwner::User));
        if !background && workspace.visible {
            if let Err(error) = show_active_workspace(app, workspace) {
                let _ = workspace.tabs.remove_token(tab_token);
                workspace.active_tab = previous_active;
                let rollback = control.bump(Some(NativeControlOwner::User));
                if let Some(webview) = app.get_webview(&created_label) {
                    let _ = webview.close();
                }
                if let Err(restore_error) = persist_workspace_snapshot(workspace, rollback.revision)
                {
                    eprintln!("[browser] 回滚用户新建标签映射失败: {restore_error}");
                }
                if let Err(restore_error) = show_active_workspace(app, workspace) {
                    eprintln!("[browser] 回滚用户新建标签显示失败: {restore_error}");
                }
                emit_control_changed(app, session_id, &workspace.active_tab, rollback);
                return Err(error);
            }
        }
        created_publication.store(true, std::sync::atomic::Ordering::SeqCst);
        // CAS 后用户可能已经再次接管；读取当前状态再发事件，不能用旧 Agent
        // snapshot 覆盖更新后的 UI。
        let snapshot = control.snapshot();
        emit_control_changed(app, session_id, &workspace.active_tab, snapshot);
        if let Err(error) = self.persist_workspace(session_id) {
            eprintln!("[browser] 用户新建标签后的映射持久化失败: {error}");
        }
        if let Err(error) = self.persist_restore_workspace(app, session_id) {
            eprintln!("[browser] 用户新建标签后的恢复清单刷新失败: {error}");
        }
        Ok(Some(tab_token.to_string()))
    }

    /// 将 wrapper 发现的底层 target 绑定到宿主 tab。该双射跨全部对话强制唯一。
    pub fn bind_target(
        &mut self,
        session_id: &str,
        tab_token: &str,
        automation_target: &str,
    ) -> Result<bool, String> {
        if automation_target.is_empty() || automation_target.len() > 512 {
            return Err("浏览器自动化 targetId 无效".to_string());
        }
        if self.workspaces.iter().any(|(owner_session, workspace)| {
            workspace
                .tabs
                .token_for_target(automation_target)
                .is_some_and(|bound_tab| owner_session != session_id || bound_tab != tab_token)
        }) || self
            .staged_tabs
            .iter()
            .any(|((owner_session, owner_tab), entry)| {
                entry.automation_target.as_deref() == Some(automation_target)
                    && (owner_session != session_id || owner_tab != tab_token)
            })
        {
            return Err("浏览器自动化 target 已属于其他标签".to_string());
        }
        let staged_key = (session_id.to_string(), tab_token.to_string());
        if self.staged_user_tabs.contains_key(&staged_key) {
            let entry = self
                .staged_tabs
                .get_mut(&staged_key)
                .ok_or_else(|| "用户待绑定标签状态不完整".to_string())?;
            if entry.automation_target.as_deref() == Some(automation_target) {
                return Ok(true);
            }
            entry.automation_target = Some(automation_target.to_string());
            return Ok(true);
        }
        let Some(workspace) = self.workspaces.get_mut(session_id) else {
            return Ok(false);
        };
        if workspace.tabs.target_for_token(tab_token) == Some(automation_target) {
            return Ok(true);
        }
        let restoring_unpublished_workspace =
            workspace.tabs.iter().any(|entry| !entry.is_published());
        workspace.tabs.bind_target(tab_token, automation_target)?;
        if restoring_unpublished_workspace {
            return Ok(true);
        }
        workspace.control.bump(None);
        self.persist_workspace(session_id)?;
        Ok(true)
    }

    pub fn target_for_tab(&self, session_id: &str, tab_token: &str) -> Option<String> {
        self.workspaces
            .get(session_id)?
            .tabs
            .target_for_token(tab_token)
            .map(ToOwned::to_owned)
    }

    /// BrowserManager 只需对这些标签执行首次 CDP marker 发现，不必读取页面主世界归属。
    pub fn unbound_tabs(&self, session_id: &str) -> Vec<String> {
        self.workspaces
            .get(session_id)
            .map(|workspace| {
                workspace
                    .tabs
                    .iter()
                    .filter(|entry| entry.automation_target.is_none())
                    .map(|entry| entry.token.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn control_state(&self, session_id: &str) -> Option<ControlSnapshot> {
        self.workspaces
            .get(session_id)
            .map(|workspace| workspace.control.snapshot())
    }

    pub fn tab_for_target(&self, automation_target: &str) -> Option<(String, String)> {
        self.workspaces.iter().find_map(|(session_id, workspace)| {
            workspace
                .tabs
                .token_for_target(automation_target)
                .map(|tab_token| (session_id.clone(), tab_token.to_string()))
        })
    }

    pub fn show(
        &mut self,
        window: &tauri::Window,
        session_id: &str,
        bounds: NativeSurfaceBounds,
    ) -> Result<bool, String> {
        if !self
            .workspaces
            .get(session_id)
            .and_then(active_entry)
            .is_some_and(SurfaceEntry::is_published)
        {
            return Ok(false);
        }
        let previous_active = self.active_session.clone();
        let previous_bounds = self
            .workspaces
            .get(session_id)
            .and_then(|workspace| workspace.bounds);
        hide_all(window.app_handle(), &self.workspaces);
        set_exclusive_workspace_visibility(&mut self.workspaces, session_id);
        {
            let workspace = self
                .workspaces
                .get_mut(session_id)
                .expect("工作区已在上方检查");
            workspace.bounds = Some(bounds);
        }
        let show_result = show_active_workspace(
            window.app_handle(),
            self.workspaces.get(session_id).expect("工作区已在上方检查"),
        );
        if let Err(error) = show_result {
            if let Some(workspace) = self.workspaces.get_mut(session_id) {
                workspace.visible = false;
                workspace.bounds = previous_bounds;
            }
            self.active_session = None;
            if let Some(previous_session) = previous_active.as_deref() {
                if let Some(previous) = self.workspaces.get_mut(previous_session) {
                    previous.visible = true;
                }
                if let Some(previous) = self.workspaces.get(previous_session) {
                    match show_active_workspace(window.app_handle(), previous) {
                        Ok(()) => self.active_session = Some(previous_session.to_string()),
                        Err(restore_error) => {
                            eprintln!("[browser] 回滚原生工作区显示失败: {restore_error}");
                            if let Some(previous) = self.workspaces.get_mut(previous_session) {
                                previous.visible = false;
                            }
                        }
                    }
                }
            }
            return Err(error);
        }
        self.active_session = Some(session_id.to_string());
        Ok(true)
    }

    pub fn hide(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: Option<&str>,
    ) -> Result<(), String> {
        if let Some(app) = app {
            match session_id {
                Some(session_id) => {
                    if let Some(workspace) = self.workspaces.get(session_id) {
                        hide_workspace(app, workspace);
                    }
                }
                None => hide_all(app, &self.workspaces),
            }
        }
        match session_id {
            Some(session_id) => {
                if let Some(workspace) = self.workspaces.get_mut(session_id) {
                    workspace.visible = false;
                }
                if self.active_session.as_deref() == Some(session_id) {
                    self.active_session = None;
                }
            }
            None => {
                for workspace in self.workspaces.values_mut() {
                    workspace.visible = false;
                }
                self.active_session = None;
            }
        }
        Ok(())
    }

    pub fn activate_tab(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
    ) -> Result<bool, String> {
        Ok(self
            .activate_tab_as(app, session_id, tab_token, NativeControlOwner::User, false)?
            .is_some())
    }

    pub fn rollback_agent_activation(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        activated_tab: &str,
        previous_tab: &str,
        expected_revision: u64,
        previous_owner: NativeControlOwner,
    ) -> Result<bool, String> {
        let session_owns_visible_surface = self.active_session.as_deref() == Some(session_id);
        let Some(workspace) = self.workspaces.get_mut(session_id) else {
            return Ok(false);
        };
        if workspace.active_tab != activated_tab || workspace.tabs.by_token(previous_tab).is_none()
        {
            return Ok(false);
        }
        let visible_app = if workspace_may_present_native_surface(
            session_owns_visible_surface,
            workspace.visible,
        ) {
            Some(app.ok_or_else(|| "应用句柄尚未就绪".to_string())?)
        } else {
            None
        };
        let control = Arc::clone(&workspace.control);
        let committed = control.rollback_agent_activation(
            expected_revision,
            previous_owner,
            |rollback_revision| {
                workspace.active_tab = previous_tab.to_string();
                if let Err(error) = persist_workspace_snapshot(workspace, rollback_revision) {
                    workspace.active_tab = activated_tab.to_string();
                    return Err(error);
                }
                if let Some(app) = visible_app {
                    if let Err(error) = show_active_workspace(app, workspace) {
                        workspace.active_tab = activated_tab.to_string();
                        if let Err(restore_error) =
                            persist_workspace_snapshot(workspace, expected_revision)
                        {
                            eprintln!("[browser] 回滚取消标签激活失败: {restore_error}");
                        }
                        let _ = show_active_workspace(app, workspace);
                        return Err(error);
                    }
                }
                Ok(())
            },
        )?;
        let Some((snapshot, ())) = committed else {
            return Ok(false);
        };
        if let Some(app) = app {
            emit_control_changed(app, session_id, previous_tab, snapshot);
        }
        Ok(true)
    }

    /// Agent 激活标签时返回短期 lease；执行任何工具前必须调用 [`Self::assert_lease`]。
    pub fn activate_tab_with_lease(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
    ) -> Result<Option<NativeTabLease>, String> {
        self.activate_tab_with_lease_authorized(app, session_id, tab_token, false)
    }

    fn activate_tab_with_lease_authorized(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
        explicit_user_handback: bool,
    ) -> Result<Option<NativeTabLease>, String> {
        let Some((snapshot, target_id, lease)) = self.activate_tab_as(
            app,
            session_id,
            tab_token,
            NativeControlOwner::Agent,
            explicit_user_handback,
        )?
        else {
            return Ok(None);
        };
        Ok(Some(NativeTabLease {
            session_id: session_id.to_string(),
            tab_token: tab_token.to_string(),
            target_id,
            revision: snapshot.revision,
            owner: NativeControlOwner::Agent,
            lease,
        }))
    }

    /// 用户在 UI 中立即把当前标签交还给 Agent。空闲自动交还不会预签发 lease；
    /// 返回的新 lease 仅供这个显式快捷入口使用，并会替换、撤销旧 lease。
    pub fn hand_back_to_agent(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
    ) -> Result<Option<NativeTabLease>, String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(None);
        };
        let tab_token = workspace.active_tab.clone();
        self.activate_tab_with_lease_authorized(app, session_id, &tab_token, true)
    }

    fn activate_tab_as(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
        owner: NativeControlOwner,
        explicit_user_handback: bool,
    ) -> Result<Option<(ControlSnapshot, String, String)>, String> {
        let session_owns_visible_surface = self.active_session.as_deref() == Some(session_id);
        let Some(workspace) = self.workspaces.get_mut(session_id) else {
            return Ok(None);
        };
        if workspace.tabs.by_token(tab_token).is_none() {
            return Err("标签页不存在或不属于当前对话".to_string());
        }
        let target_id = workspace
            .tabs
            .target_for_token(tab_token)
            .map(ToOwned::to_owned)
            .unwrap_or_default();
        if owner == NativeControlOwner::Agent && target_id.is_empty() {
            return Err("浏览器标签尚未绑定宿主权威 automation target".to_string());
        }
        // workspace.visible 只能表达该工作区自己的期望；物理表面是应用全局唯一的。
        // 只有同时持有 active_session 的前台工作区才允许 activate 触发 show，后台
        // Agent 激活只更新该工作区标签/lease，不能抢占用户正在看的其他任务。
        let visible_app = if workspace_may_present_native_surface(
            session_owns_visible_surface,
            workspace.visible,
        ) {
            Some(app.ok_or_else(|| "应用句柄尚未就绪".to_string())?)
        } else {
            None
        };
        let control = Arc::clone(&workspace.control);
        let (snapshot, lease) = if owner == NativeControlOwner::Agent {
            let previous_active = workspace.active_tab.clone();
            let previous_revision = control.snapshot().revision;
            let issued =
                control.issue_agent_lease_with(explicit_user_handback, |committed_revision| {
                    workspace.active_tab = tab_token.to_string();
                    if let Err(error) = persist_workspace_snapshot(workspace, committed_revision) {
                        workspace.active_tab = previous_active.clone();
                        return Err(error);
                    }
                    if let Some(app) = visible_app {
                        if let Err(error) = show_active_workspace(app, workspace) {
                            workspace.active_tab = previous_active.clone();
                            if let Err(restore_error) =
                                persist_workspace_snapshot(workspace, previous_revision)
                            {
                                eprintln!("[browser] 回滚标签激活映射失败: {restore_error}");
                            }
                            let _ = show_active_workspace(app, workspace);
                            return Err(error);
                        }
                    }
                    Ok(())
                })?;
            let Some((snapshot, lease, ())) = issued else {
                return Err(
                    "用户刚刚操作了浏览器；停止操作 3 秒后会自动恢复，也可点击“交还 Agent”立即恢复"
                        .to_string(),
                );
            };
            (snapshot, lease)
        } else {
            let previous_active = workspace.active_tab.clone();
            workspace.active_tab = tab_token.to_string();
            let snapshot = control.bump(Some(owner));
            if let Some(app) = visible_app {
                if let Err(error) = show_active_workspace(app, workspace) {
                    workspace.active_tab = previous_active;
                    // 不能把 control revision 倒退到失败前；用一个新的 User mutation
                    // 发布回滚结果，让任何观察到失败激活的旧 lease/事件都失效。
                    let rollback = control.bump(Some(NativeControlOwner::User));
                    if let Err(restore_error) =
                        persist_workspace_snapshot(workspace, rollback.revision)
                    {
                        eprintln!("[browser] 回滚用户标签激活映射失败: {restore_error}");
                    }
                    if let Err(restore_error) = show_active_workspace(app, workspace) {
                        eprintln!("[browser] 回滚用户标签激活显示失败: {restore_error}");
                    }
                    emit_control_changed(app, session_id, &workspace.active_tab, rollback);
                    return Err(error);
                }
            }
            (snapshot, String::new())
        };
        if let Some(app) = app {
            // 用户可在 Agent 临界区释放后立即接管；发当前快照避免迟到的 Agent
            // 事件覆盖 UI 中更新后的 User owner。
            emit_control_changed(app, session_id, tab_token, control.snapshot());
        }
        if owner != NativeControlOwner::Agent {
            if let Err(error) = self.persist_workspace(session_id) {
                eprintln!("[browser] 用户切换标签后的映射持久化失败: {error}");
            }
        }
        if let Some(app) = app {
            if let Err(error) = self.persist_restore_workspace(app, session_id) {
                eprintln!("[browser] 标签激活后的恢复清单刷新失败: {error}");
            }
        }
        Ok(Some((snapshot, target_id, lease)))
    }

    /// 同时复核 session、tab、target、revision、owner 与宿主不透明能力令牌。
    pub fn assert_lease(&self, lease: &NativeTabLease) -> Result<bool, String> {
        let Some(workspace) = self.workspaces.get(&lease.session_id) else {
            return Ok(false);
        };
        if workspace.active_tab != lease.tab_token
            || workspace.tabs.target_for_token(&lease.tab_token) != Some(lease.target_id.as_str())
        {
            return Ok(false);
        }
        Ok(lease.owner == NativeControlOwner::Agent
            && workspace
                .control
                .assert_agent_lease(lease.revision, &lease.lease))
    }

    /// 在每个工具 dispatch 前原子复核 lease；输入类工具还会打开短时 trusted-event 抑制窗。
    pub fn begin_agent_operation(
        &self,
        lease: &NativeTabLease,
        emits_trusted_input: bool,
        caller_pid: u32,
        wrapper_instance_nonce: &str,
    ) -> Result<bool, String> {
        if !self.assert_lease(lease)? {
            return Ok(false);
        }
        let caller_epoch = AgentCallerEpoch::new(caller_pid, wrapper_instance_nonce.to_string())?;
        Ok(self
            .workspaces
            .get(&lease.session_id)
            .is_some_and(|workspace| {
                workspace.control.begin_agent_operation_for_caller(
                    lease,
                    emits_trusted_input,
                    caller_epoch,
                )
            }))
    }

    /// 仅续期当前这一个已 begin 的 trusted-input dispatch。`assert_lease` 先复核宿主
    /// workspace 的 session/tab/target/revision/opaque lease；WorkspaceControl 再在同一锁内
    /// 要求 active_agent_operation 与整个 lease 完全相等。迟到 heartbeat 因此不能
    /// 续期已结束操作、新操作或用户接管后的旧授权。
    pub fn refresh_agent_input(&self, lease: &NativeTabLease) -> Result<bool, String> {
        if !self.assert_lease(lease)? {
            return Ok(false);
        }
        Ok(self
            .workspaces
            .get(&lease.session_id)
            .is_some_and(|workspace| workspace.control.refresh_agent_input_window(lease)))
    }

    /// Renew only the bounded liveness of an exact begun operation. This is
    /// used by long non-input upstream calls and intentionally does not open
    /// the trusted-input provenance window.
    pub fn refresh_agent_operation(&self, lease: &NativeTabLease) -> Result<bool, String> {
        if !self.assert_lease(lease)? {
            return Ok(false);
        }
        Ok(self
            .workspaces
            .get(&lease.session_id)
            .is_some_and(|workspace| workspace.control.refresh_agent_operation(lease)))
    }

    /// dispatch 完成后立即结束 active operation。已派发的 WebKit 事件可能在下一个
    /// run-loop 回合才触发 takeover delegate，因此仅保留至多 100ms 的 callback grace；
    /// 显式 UI 接管仍会直接 bump revision 并立即清除该窗口。
    pub fn end_agent_operation(&self, lease: &NativeTabLease) {
        if let Some(workspace) = self.workspaces.get(&lease.session_id) {
            workspace.control.end_agent_operation(lease);
        }
    }

    /// Release the exact holder retained by the WebView popup callback. The
    /// holder carries the original caller epoch, so an async cleanup from an
    /// older wrapper incarnation cannot consume current operation state.
    pub fn release_popup_agent_operation(&self, retained: &RetainedAgentOperation) {
        if let Some(workspace) = self.workspaces.get(&retained.authorization().session_id) {
            workspace.control.release_retained_agent_operation(retained);
        }
    }

    /// Validate that this exact retained popup holder is still registered;
    /// checking only its shared lease would let a released popup borrow a
    /// sibling holder from the same upstream operation.
    pub fn authorize_popup_agent_operation(&self, retained: &RetainedAgentOperation) -> bool {
        self.workspaces
            .get(&retained.authorization().session_id)
            .is_some_and(|workspace| {
                workspace
                    .control
                    .authorize_retained_agent_operation(retained)
            })
    }

    /// Revoke a claimed BrowserCore request as soon as a durable cancellation
    /// tombstone wins. The caller still polls any already-dispatched platform
    /// future to real settlement; this method blocks later sub-dispatches and
    /// rolls back only tabs carrying this request's exact creation id.
    pub fn cancel_in_flight_core_request(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        request_id: &str,
    ) -> Result<(), String> {
        if let Some(workspace) = self.workspaces.get(session_id) {
            workspace
                .control
                .cancel_agent_operation_for_session(session_id);
        }

        let mut created_tokens = self
            .staged_tabs
            .iter()
            .filter_map(|((owner_session, token), entry)| {
                (owner_session == session_id
                    && entry.created_by_request_id.as_deref() == Some(request_id))
                .then(|| token.clone())
            })
            .collect::<Vec<_>>();
        if let Some(workspace) = self.workspaces.get(session_id) {
            created_tokens.extend(workspace.tabs.iter().filter_map(|entry| {
                (entry.created_by_request_id.as_deref() == Some(request_id))
                    .then(|| entry.token.clone())
            }));
        }
        created_tokens.sort();
        created_tokens.dedup();

        let mut errors = Vec::new();
        for token in created_tokens {
            if let Err(error) = self.rollback_created_tab(app, session_id, &token, request_id) {
                errors.push(format!("{token}: {error}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "browser/core-cancellation-compensation-failed: {}",
                errors.join("; ")
            ))
        }
    }

    pub fn close_tab(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
    ) -> Result<bool, String> {
        self.close_tab_as(
            app,
            session_id,
            tab_token,
            Some(NativeControlOwner::User),
            None,
        )
    }

    pub fn close_tab_for_agent(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
        authorization: &NativeTabLease,
    ) -> Result<bool, String> {
        self.close_tab_as(
            app,
            session_id,
            tab_token,
            Some(NativeControlOwner::Agent),
            Some(authorization),
        )
    }

    fn rollback_staged_agent_creation(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
        creation_id: &str,
    ) -> Result<bool, String> {
        let staged_key = (session_id.to_string(), tab_token.to_string());
        let Some(entry) = self.staged_tabs.get(&staged_key) else {
            return Ok(false);
        };
        if entry.created_by_request_id.as_deref() != Some(creation_id) {
            return Err("创建补偿 generation 与待提交标签不匹配".to_string());
        }
        let label = entry.label.clone();
        if let Some(webview) = app.and_then(|app| app.get_webview(&label)) {
            webview
                .close()
                .map_err(|error| format!("关闭待提交浏览器标签失败: {error}"))?;
        }
        super::unregister_browser_core_webview_binding(&label);
        self.staged_tabs.remove(&staged_key);
        self.staged_user_tabs.remove(&staged_key);
        Ok(true)
    }

    /// 仅供 request tombstone 回滚本请求刚创建的标签；不改变现有控制权 owner。
    pub fn rollback_created_tab(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
        creation_id: &str,
    ) -> Result<bool, String> {
        if self.rollback_staged_agent_creation(app, session_id, tab_token, creation_id)? {
            return Ok(true);
        }
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let Some(entry) = workspace.tabs.by_token(tab_token) else {
            return Ok(false);
        };
        if entry.created_by_request_id.as_deref() != Some(creation_id) {
            return Err("创建补偿 generation 与当前标签不匹配".to_string());
        }
        let expected_revision = entry
            .created_at_revision
            .ok_or_else(|| "创建补偿缺少提交 generation".to_string())?;
        let current = workspace.control.snapshot();
        if current.owner != NativeControlOwner::Agent || current.revision != expected_revision {
            // 用户接管或任意后续 mutation 已把这个 creation generation 安全取代。
            // 晚到 tombstone 只能 ACK 并保留页面，不能无限重试，更不能误关用户页面。
            return Ok(false);
        }
        if workspace.tabs.len() <= 1 {
            return Err("至少保留一个浏览器标签页".to_string());
        }
        let entry = entry.clone();
        let control = Arc::clone(&workspace.control);
        let workspace = self
            .workspaces
            .get_mut(session_id)
            .expect("工作区已在上方检查");
        let committed = control.commit_agent_generation_rollback(expected_revision, || {
            if let Some(webview) = app.and_then(|app| app.get_webview(&entry.label)) {
                webview
                    .close()
                    .map_err(|error| format!("关闭浏览器标签页失败: {error}"))?;
            }
            remove_tab_from_workspace(workspace, tab_token);
            if workspace.visible {
                let app = app.ok_or_else(|| "应用句柄尚未就绪".to_string())?;
                if let Err(error) = show_active_workspace(app, workspace) {
                    eprintln!("[browser] 创建补偿后显示回退页失败: {error}");
                }
            }
            Ok(())
        })?;
        if committed.is_none() {
            return Ok(false);
        }
        super::unregister_browser_core_webview_binding(&entry.label);
        if let Err(error) = self.persist_workspace(session_id) {
            eprintln!("[browser] 创建补偿后的映射持久化失败: {error}");
        }
        if let Some(app) = app {
            if let Err(error) = self.persist_restore_workspace(app, session_id) {
                eprintln!("[browser] 创建补偿后的恢复清单刷新失败: {error}");
            }
            let workspace = self
                .workspaces
                .get(session_id)
                .expect("补偿提交后工作区仍存在");
            emit_control_changed(app, session_id, &workspace.active_tab, control.snapshot());
        }
        Ok(true)
    }

    /// UI/用户 popup 创建失败的精确补偿；只能作用于没有 Agent creation
    /// generation 的标签，避免误删并发 Agent 创建的新资源。
    pub fn rollback_user_created_tab(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
    ) -> Result<bool, String> {
        let staged_key = (session_id.to_string(), tab_token.to_string());
        if self.staged_user_tabs.contains_key(&staged_key) {
            let label = self
                .staged_tabs
                .get(&staged_key)
                .map(|entry| entry.label.clone())
                .ok_or_else(|| "用户待发布标签状态不完整".to_string())?;
            if let Some(webview) = app.and_then(|app| app.get_webview(&label)) {
                webview
                    .close()
                    .map_err(|error| format!("关闭用户待发布浏览器标签失败: {error}"))?;
            }
            super::unregister_browser_core_webview_binding(&label);
            self.staged_tabs.remove(&staged_key);
            self.staged_user_tabs.remove(&staged_key);
            return Ok(true);
        }
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let Some(entry) = workspace.tabs.by_token(tab_token) else {
            return Ok(false);
        };
        if entry.created_by_request_id.is_some() {
            return Err("用户创建补偿不能删除 Agent generation 标签".to_string());
        }
        self.close_tab_as(app, session_id, tab_token, None, None)
    }

    fn close_tab_as(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
        owner: Option<NativeControlOwner>,
        authorization: Option<&NativeTabLease>,
    ) -> Result<bool, String> {
        let Some(workspace) = self.workspaces.get_mut(session_id) else {
            return Ok(false);
        };
        if workspace.tabs.len() <= 1 {
            return Err("至少保留一个浏览器标签页".to_string());
        }
        let entry = workspace
            .tabs
            .by_token(tab_token)
            .cloned()
            .ok_or_else(|| "标签页不存在或不属于当前对话".to_string())?;
        let app = app.ok_or_else(|| "应用句柄尚未就绪，无法关闭浏览器标签".to_string())?;
        let agent_committed = owner == Some(NativeControlOwner::Agent);
        if agent_committed {
            let authorization =
                authorization.ok_or_else(|| "Agent 关闭标签缺少宿主 mutation lease".to_string())?;
            validate_agent_mutation(workspace, session_id, authorization, Some(tab_token))?;
            let control = Arc::clone(&workspace.control);
            let committed = control.commit_agent_mutation(authorization, || {
                if let Some(webview) = app.get_webview(&entry.label) {
                    webview
                        .close()
                        .map_err(|error| {
                            format!(
                                "browser/action-commit-unknown-after-tab-close: 原生标签关闭响应不确定: {error}"
                            )
                        })?;
                }
                remove_tab_from_workspace(workspace, tab_token);
                if workspace.visible {
                    // 页面已关闭后显示 fallback 失败不能逆转 close；记录错误但仍
                    // 提交注册表/control，避免宿主状态声称已关闭页仍然存在。
                    if let Err(error) = show_active_workspace(app, workspace) {
                        eprintln!("[browser] Agent 关闭标签后显示回退页失败: {error}");
                    }
                }
                Ok(())
            })?;
            if committed.is_none() {
                return Err("Agent mutation lease 已失效；用户可能已接管浏览器".to_string());
            }
        } else {
            if let Some(webview) = app.get_webview(&entry.label) {
                webview.close().map_err(|error| {
                    format!(
                        "browser/action-commit-unknown-after-tab-close: 原生标签关闭响应不确定: {error}"
                    )
                })?;
            }
            remove_tab_from_workspace(workspace, tab_token);
            workspace.control.bump(owner);
            if workspace.visible {
                // WebView close 已经物理提交；fallback show 失败不能把成功的 close
                // 伪装成失败，否则调用方重试只会得到“标签不存在”。
                if let Err(error) = show_active_workspace(app, workspace) {
                    eprintln!("[browser] 用户关闭标签后显示回退页失败: {error}");
                }
            }
        }
        super::unregister_browser_core_webview_binding(&entry.label);
        let snapshot = workspace.control.snapshot();
        emit_control_changed(app, session_id, &workspace.active_tab, snapshot);
        // WebView close 已是不可逆物理提交；后续快照失败不得把成功操作伪装成失败，
        // 否则调用方重试会得到“标签不存在”。保持成功并让后续状态刷新继续修复。
        if let Err(error) = self.persist_workspace(session_id) {
            eprintln!("[browser] 关闭标签后的映射持久化失败: {error}");
        }
        if let Err(error) = self.persist_restore_workspace(app, session_id) {
            eprintln!("[browser] 关闭标签后的恢复清单刷新失败: {error}");
        }
        Ok(true)
    }

    pub fn close(&mut self, app: Option<&tauri::AppHandle>) -> Result<(), String> {
        self.close_impl(app, true)
    }

    /// 应用进程退出只销毁当次 WebView/target 映射，保留已经原子写入的 URL 清单，
    /// 供下次按对话惰性重建。显式“停止浏览器”仍走 [`Self::close`] 删除清单。
    pub fn close_preserving_restore(
        &mut self,
        app: Option<&tauri::AppHandle>,
    ) -> Result<(), String> {
        self.close_impl(app, false)
    }

    fn close_impl(
        &mut self,
        app: Option<&tauri::AppHandle>,
        delete_restore: bool,
    ) -> Result<(), String> {
        if app.is_none() && self.has_sessions() {
            return Err("应用句柄尚未就绪，无法关闭浏览器".to_string());
        }
        let mut session_ids = self.workspaces.keys().cloned().collect::<Vec<_>>();
        for (session_id, _) in self.staged_tabs.keys() {
            if !session_ids.contains(session_id) {
                session_ids.push(session_id.clone());
            }
        }
        for (session_id, _) in self.quarantined_tabs.keys() {
            if !session_ids.contains(session_id) {
                session_ids.push(session_id.clone());
            }
        }
        session_ids.sort();
        let mut errors = Vec::new();
        for session_id in session_ids {
            if let Err(error) = self.close_session_impl(app, &session_id, delete_restore) {
                errors.push(format!("{session_id}: {error}"));
            }
        }
        if !self.has_sessions() {
            self.active_session = None;
            self.data_directory = None;
            self.platform.reset();
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    pub fn close_session(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
    ) -> Result<bool, String> {
        self.close_session_impl(app, session_id, true)
    }

    /// 恢复过程中部分 WebView 创建失败时只回滚当次原生资源，保留原始恢复清单，
    /// 下一次状态查询仍可重试。
    pub fn close_session_preserving_restore(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
    ) -> Result<bool, String> {
        self.close_session_impl(app, session_id, false)
    }

    /// Roll back an uncommitted restore transaction. Unlike an ordinary stop,
    /// this first removes the provisional workspace from business state, then
    /// reconciles its native children through the cleanup-only quarantine. A
    /// failed close therefore remains owned and retryable without making
    /// `has_session` return true.
    pub fn quarantine_failed_restore(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
    ) -> Result<bool, String> {
        let owns_workspace = self.workspaces.contains_key(session_id);
        let owns_quarantine = self
            .quarantined_tabs
            .keys()
            .any(|(owner_session, _)| owner_session == session_id);
        if !owns_workspace && !owns_quarantine {
            return Ok(self.has_sessions());
        }
        let app = app.ok_or_else(|| "应用句柄尚未就绪，无法对账失败的浏览器恢复".to_string())?;
        self.quarantine_workspace_for_failed_restore(session_id);
        self.reap_quarantined_for_session(app, session_id)?;
        Ok(self.has_sessions())
    }

    fn close_session_impl(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        delete_restore: bool,
    ) -> Result<bool, String> {
        let has_workspace = self.workspaces.contains_key(session_id);
        let has_staging = self
            .staged_tabs
            .keys()
            .any(|(owner_session, _)| owner_session == session_id);
        let has_quarantine = self
            .quarantined_tabs
            .keys()
            .any(|(owner_session, _)| owner_session == session_id);
        if !has_workspace && !has_staging && !has_quarantine {
            if delete_restore {
                remove_restore_file(&paths::browser_session_token(session_id))?;
            }
            return Ok(self.has_sessions());
        }

        let app = app.ok_or_else(|| "应用句柄尚未就绪，无法关闭对话浏览器".to_string())?;
        let mut errors = Vec::new();
        if let Some(workspace) = self.workspaces.get_mut(session_id) {
            if let Err(error) = close_workspace(app, workspace) {
                errors.push(error);
            }
        }
        if let Err(error) = self.close_staged_for_session(Some(app), session_id) {
            errors.push(error);
        }
        if let Err(error) = self.reap_quarantined_for_session(app, session_id) {
            errors.push(error);
        }

        let workspace_empty = self
            .workspaces
            .get(session_id)
            .is_some_and(|workspace| workspace.tabs.is_empty());
        if workspace_empty {
            let workspace = self
                .workspaces
                .remove(session_id)
                .expect("空工作区在关闭期间由同一互斥锁保护");
            let _ = std::fs::remove_file(paths::browser_workspace_state_json(
                &workspace.session_token,
            ));
            if delete_restore {
                if let Err(error) = remove_restore_file(&workspace.session_token) {
                    errors.push(error);
                }
            }
        } else if let Some(workspace) = self.workspaces.get(session_id) {
            // WebView close 是逐页不可逆提交。失败时注册表只保留真实 survivor；
            // 已发布 survivor 同步成为新的恢复真相，未发布 restore staging 则保留
            // 调用前清单，供清理成功后重新执行完整恢复。
            if let Err(error) = self.persist_workspace(session_id) {
                errors.push(format!("保存部分关闭后的浏览器状态失败: {error}"));
            }
            if workspace.tabs.iter().any(SurfaceEntry::is_published) {
                if let Err(error) = self.persist_restore_workspace(app, session_id) {
                    errors.push(format!("保存部分关闭后的恢复清单失败: {error}"));
                }
            }
        } else if delete_restore {
            // 仅有 staging 的异常状态没有可恢复的用户页面；停止操作仍应删除旧清单，
            // staging 本身保留在内存中，下一次幂等 stop 会继续关闭它。
            if let Err(error) = remove_restore_file(&paths::browser_session_token(session_id)) {
                errors.push(error);
            }
        }

        if self.active_session.as_deref() == Some(session_id)
            && !self.workspaces.contains_key(session_id)
        {
            self.active_session = None;
        }
        let has_remaining = self.has_sessions();
        if errors.is_empty() {
            Ok(has_remaining)
        } else {
            Err(errors.join("; "))
        }
    }

    fn close_staged_for_session(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
    ) -> Result<(), String> {
        let has_entries = self
            .staged_tabs
            .keys()
            .any(|(owner_session, _)| owner_session == session_id);
        if !has_entries {
            self.staged_user_tabs
                .retain(|(owner_session, _), _| owner_session != session_id);
            return Ok(());
        }
        let app = app.ok_or_else(|| "应用句柄尚未就绪，无法关闭待提交浏览器标签".to_string())?;
        reconcile_staged_close(
            &mut self.staged_tabs,
            &mut self.staged_user_tabs,
            session_id,
            |entry| {
                if let Some(webview) = app.get_webview(&entry.label) {
                    webview
                        .close()
                        .map_err(|error| format!("关闭系统 WebView 失败: {error}"))?;
                }
                Ok(())
            },
        )
    }

    pub fn delete_restore_workspace(session_id: &str) -> Result<(), String> {
        remove_restore_file(&paths::browser_session_token(session_id))
    }

    pub fn session_state(
        &self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
    ) -> Option<(String, String)> {
        let workspace = self.workspaces.get(session_id)?;
        let entry = active_entry(workspace)?;
        if !entry.is_published() {
            return None;
        }
        let url = app?.get_webview(&entry.label)?.url().ok()?.to_string();
        Some((entry.token.clone(), sanitize_marker_url(url, &entry.token)))
    }

    pub fn list_tabs(
        &self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
    ) -> Option<Vec<TabInfo>> {
        let workspace = self.workspaces.get(session_id)?;
        let app = app?;
        Some(
            workspace
                .tabs
                .iter()
                .filter(|entry| entry.is_published())
                .filter_map(|entry| {
                    let url = sanitize_marker_url(
                        app.get_webview(&entry.label)?.url().ok()?.to_string(),
                        &entry.token,
                    );
                    let title = url
                        .parse::<tauri::Url>()
                        .ok()
                        .and_then(|url| url.host_str().map(ToOwned::to_owned))
                        .unwrap_or_else(|| "about:blank".to_string());
                    Some(TabInfo {
                        target_id: entry.token.clone(),
                        page_id: Some(entry.page_id),
                        title,
                        url,
                    })
                })
                .collect(),
        )
    }

    pub fn tab_token_for_page_id(&self, session_id: &str, page_id: u64) -> Option<String> {
        self.workspaces
            .get(session_id)?
            .tabs
            .token_for_page_id(page_id)
            .map(ToOwned::to_owned)
    }

    pub fn navigate(
        &self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        url: &str,
    ) -> Result<bool, String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let entry = active_entry(workspace).ok_or_else(|| "当前标签页不存在".to_string())?;
        let app = app.ok_or_else(|| "应用句柄尚未就绪".to_string())?;
        let webview = app
            .get_webview(&entry.label)
            .ok_or_else(|| "对话浏览器表面不存在".to_string())?;
        self.mark_user_control(app, session_id, &entry.token)?;
        let target_url = marked_blank_url(url, &entry.token);
        webview
            .navigate(
                target_url
                    .parse()
                    .map_err(|e| format!("浏览器地址无效: {e}"))?,
            )
            .map_err(|e| format!("浏览器导航失败: {e}"))?;
        Ok(true)
    }

    /// A newly created tab starts on an internal about:blank marker so the
    /// host can bind its authoritative CDP target before any remote page is
    /// exposed.  UI-created tabs use this narrow post-bind navigation path;
    /// it deliberately does not issue another control revision because
    /// `create_tab` has already transferred control to the user.
    pub fn navigate_tab_after_bind(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        tab_token: &str,
        url: &str,
    ) -> Result<bool, String> {
        let app = app.ok_or_else(|| "应用句柄尚未就绪".to_string())?;
        let staged_key = (session_id.to_string(), tab_token.to_string());
        if let Some(background) = self.staged_user_tabs.get(&staged_key).copied() {
            let entry = self
                .staged_tabs
                .get(&staged_key)
                .cloned()
                .ok_or_else(|| "用户待发布标签状态不完整".to_string())?;
            if entry.automation_target.is_none() {
                return Err("标签页尚未绑定宿主权威 automation target".to_string());
            }
            let webview = app
                .get_webview(&entry.label)
                .ok_or_else(|| "待发布浏览器表面不存在".to_string())?;
            let target_url = marked_blank_url(url, tab_token);
            webview
                .navigate(
                    target_url
                        .parse()
                        .map_err(|error| format!("浏览器地址无效: {error}"))?,
                )
                .map_err(|error| format!("浏览器隐藏标签首航失败: {error}"))?;

            let (snapshot, active_tab) = {
                let workspace = self
                    .workspaces
                    .get_mut(session_id)
                    .ok_or_else(|| "浏览器工作区不存在".to_string())?;
                let control = Arc::clone(&workspace.control);
                let previous_active = workspace.active_tab.clone();
                workspace.tabs.insert(entry.clone())?;
                if !background {
                    workspace.active_tab = tab_token.to_string();
                }
                let snapshot = control.bump(Some(NativeControlOwner::User));
                if let Err(error) = persist_workspace_snapshot(workspace, snapshot.revision) {
                    let _ = workspace.tabs.remove_token(tab_token);
                    workspace.active_tab = previous_active.clone();
                    let rollback = control.bump(Some(NativeControlOwner::User));
                    if let Err(restore_error) =
                        persist_workspace_snapshot(workspace, rollback.revision)
                    {
                        eprintln!("[browser] 回滚用户待发布标签映射失败: {restore_error}");
                    }
                    emit_control_changed(app, session_id, &workspace.active_tab, rollback);
                    return Err(error);
                }
                if !background && workspace.visible {
                    if let Err(error) = show_active_workspace(app, workspace) {
                        let _ = workspace.tabs.remove_token(tab_token);
                        workspace.active_tab = previous_active;
                        let rollback = control.bump(Some(NativeControlOwner::User));
                        if let Err(restore_error) =
                            persist_workspace_snapshot(workspace, rollback.revision)
                        {
                            eprintln!("[browser] 回滚用户待发布标签显示映射失败: {restore_error}");
                        }
                        if let Err(restore_error) = show_active_workspace(app, workspace) {
                            eprintln!("[browser] 回滚用户待发布标签物理表面失败: {restore_error}");
                        }
                        emit_control_changed(app, session_id, &workspace.active_tab, rollback);
                        return Err(error);
                    }
                }
                (snapshot, workspace.active_tab.clone())
            };

            entry.publish();
            self.staged_tabs.remove(&staged_key);
            self.staged_user_tabs.remove(&staged_key);
            emit_control_changed(app, session_id, &active_tab, snapshot);
            if let Err(error) = self.persist_restore_workspace(app, session_id) {
                eprintln!("[browser] 用户标签已发布，但恢复清单刷新失败: {error}");
            }
            return Ok(true);
        }

        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let entry = workspace
            .tabs
            .by_token(tab_token)
            .ok_or_else(|| "标签页不存在或不属于当前对话".to_string())?;
        if entry.automation_target.is_none() {
            return Err("标签页尚未绑定宿主权威 automation target".to_string());
        }
        let webview = app
            .get_webview(&entry.label)
            .ok_or_else(|| "对话浏览器表面不存在".to_string())?;
        let target_url = marked_blank_url(url, tab_token);
        webview
            .navigate(
                target_url
                    .parse()
                    .map_err(|error| format!("浏览器地址无效: {error}"))?,
            )
            .map_err(|error| format!("浏览器导航失败: {error}"))?;

        // 恢复工作区的全部 WebView 都保持 unpublished，直到最后一个标签已经取得
        // 当前进程 target 且完成首航。这样 list/status/popup 永远看不到半恢复状态。
        let should_publish_restore = workspace
            .tabs
            .iter()
            .all(|candidate| candidate.automation_target.is_some())
            && workspace
                .tabs
                .iter()
                .any(|candidate| !candidate.is_published());
        if should_publish_restore {
            for candidate in workspace.tabs.iter() {
                candidate.publish();
            }
            if let Err(error) = self.persist_workspace(session_id) {
                for candidate in workspace.tabs.iter() {
                    candidate.unpublish();
                }
                return Err(error);
            }
            if let Some(state) = self.control_state(session_id) {
                emit_control_changed(app, session_id, &workspace.active_tab, state);
            }
        }
        Ok(true)
    }

    pub fn history_step(
        &self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        delta: i8,
    ) -> Result<bool, String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let entry = active_entry(workspace).ok_or_else(|| "当前标签页不存在".to_string())?;
        let app = app.ok_or_else(|| "应用句柄尚未就绪".to_string())?;
        let webview = app
            .get_webview(&entry.label)
            .ok_or_else(|| "对话浏览器表面不存在".to_string())?;
        self.mark_user_control(app, session_id, &entry.token)?;
        webview
            .eval(if delta < 0 {
                "history.back()"
            } else {
                "history.forward()"
            })
            .map_err(|e| format!("浏览器历史导航失败: {e}"))?;
        Ok(true)
    }

    pub fn reload(&self, app: Option<&tauri::AppHandle>, session_id: &str) -> Result<bool, String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let entry = active_entry(workspace).ok_or_else(|| "当前标签页不存在".to_string())?;
        let app = app.ok_or_else(|| "应用句柄尚未就绪".to_string())?;
        let webview = app
            .get_webview(&entry.label)
            .ok_or_else(|| "对话浏览器表面不存在".to_string())?;
        self.mark_user_control(app, session_id, &entry.token)?;
        webview
            .reload()
            .map_err(|e| format!("刷新浏览器页面失败: {e}"))?;
        Ok(true)
    }

    pub fn has_session(&self, session_id: &str) -> bool {
        self.workspaces.contains_key(session_id)
            || self
                .staged_tabs
                .keys()
                .any(|(owner_session, _)| owner_session == session_id)
    }

    /// Resource-lifecycle ownership is deliberately broader than the business
    /// `has_session` predicate: cleanup-only quarantine must be stoppable but
    /// must never make prepare/restore treat a failed build as a live session.
    pub fn owns_session_resources(&self, session_id: &str) -> bool {
        self.has_session(session_id)
            || self
                .quarantined_tabs
                .keys()
                .any(|(owner_session, _)| owner_session == session_id)
    }

    /// Record (or supersede) the only Prepare request allowed to compensate a
    /// process-local workspace. Existing workspaces deliberately receive no
    /// generation: a later prepare means the previous request no longer owns
    /// teardown, even if its response/tombstone arrives late.
    pub fn record_prepare_generation(
        &mut self,
        session_id: &str,
        request_id: Option<&str>,
    ) -> Result<(), String> {
        let workspace = self
            .workspaces
            .get_mut(session_id)
            .ok_or_else(|| "浏览器工作区不存在".to_string())?;
        workspace.prepare_generation = request_id.map(|request_id| PrepareGeneration {
            request_id: request_id.to_string(),
            revision: workspace.control.snapshot().revision,
        });
        Ok(())
    }

    pub fn prepare_generation_revision(&self, session_id: &str, request_id: &str) -> Option<u64> {
        let workspace = self.workspaces.get(session_id)?;
        let generation = workspace.prepare_generation.as_ref()?;
        (generation.request_id == request_id).then_some(generation.revision)
    }

    /// Compensate a lost Prepare acknowledgement only while the exact request
    /// still owns an untouched workspace generation. A newer prepare, user
    /// takeover/navigation, tab mutation, or Agent activation makes this a
    /// successful no-op rather than deleting current user state.
    pub fn rollback_prepare_generation(
        &mut self,
        app: Option<&tauri::AppHandle>,
        session_id: &str,
        request_id: &str,
        expected_revision: u64,
        preserve_restore: bool,
    ) -> Result<Option<bool>, String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            // A previous compensation attempt may already have closed and
            // removed the runtime workspace before the durable restore delete
            // failed. Retrying the same CreatedBlank tombstone must finish
            // that durable delete instead of ACKing a manifest that can revive
            // the cancelled workspace on the next launch. RestoredExisting
            // intentionally keeps the last complete manifest.
            return complete_absent_prepare_rollback(preserve_restore, self.has_sessions(), || {
                remove_restore_file(&paths::browser_session_token(session_id))
            });
        };
        let matches = workspace
            .prepare_generation
            .as_ref()
            .is_some_and(|generation| {
                generation.request_id == request_id
                    && generation.revision == expected_revision
                    && workspace.control.snapshot().revision == expected_revision
            });
        if !matches {
            return Ok(None);
        }
        self.close_session_impl(app, session_id, !preserve_restore)
            .map(Some)
    }

    pub fn has_published_session(&self, session_id: &str) -> bool {
        self.workspaces
            .get(session_id)
            .is_some_and(|workspace| workspace.tabs.iter().any(SurfaceEntry::is_published))
    }

    pub fn has_tab(&self, session_id: &str, tab_token: &str) -> bool {
        self.workspaces
            .get(session_id)
            .is_some_and(|workspace| workspace.tabs.by_token(tab_token).is_some())
    }

    /// Resolve a task-owned published or exact hidden-staging tab to its Tauri child-WebView
    /// label. BrowserManager holds the random tab token and uses this only after ownership/
    /// generation validation; page code never receives the label or a Tauri bridge.
    pub fn webview_label_for_tab(&self, session_id: &str, tab_token: &str) -> Option<String> {
        self.workspaces
            .get(session_id)
            .and_then(|workspace| workspace.tabs.by_token(tab_token))
            .or_else(|| {
                self.staged_tabs
                    .get(&(session_id.to_string(), tab_token.to_string()))
            })
            .map(|entry| entry.label.clone())
    }

    pub fn active_tab_token(&self, session_id: &str) -> Option<String> {
        self.workspaces
            .get(session_id)
            .map(|workspace| workspace.active_tab.clone())
    }

    pub fn has_sessions(&self) -> bool {
        !self.workspaces.is_empty()
            || !self.staged_tabs.is_empty()
            || !self.quarantined_tabs.is_empty()
    }

    pub fn session_ids(&self) -> Vec<String> {
        let mut ids = self.workspaces.keys().cloned().collect::<Vec<_>>();
        for (session_id, _) in self.staged_tabs.keys() {
            if !ids.contains(session_id) {
                ids.push(session_id.clone());
            }
        }
        for (session_id, _) in self.quarantined_tabs.keys() {
            if !ids.contains(session_id) {
                ids.push(session_id.clone());
            }
        }
        ids.sort();
        ids
    }

    pub fn is_initialized(&self) -> bool {
        self.platform.is_initialized()
    }

    pub fn owns_port(&self, port: u16) -> bool {
        self.platform.owns_port(port)
    }

    fn mark_user_control(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
        tab_token: &str,
    ) -> Result<(), String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(());
        };
        let state = workspace.control.bump(Some(NativeControlOwner::User));
        emit_control_changed(app, session_id, tab_token, state);
        if let Err(error) = self.persist_workspace(session_id) {
            eprintln!("[browser] 用户接管后的映射持久化失败: {error}");
        }
        Ok(())
    }

    /// 只在定时器仍对应最后一次用户动作时自动释放控制权。该提交和 revision
    /// 校验共用 WorkspaceControl 锁，因此迟到定时器不能覆盖后续操作。
    pub fn release_user_control_if_idle(
        &mut self,
        app: &tauri::AppHandle,
        session_id: &str,
        expected_revision: u64,
    ) -> Result<bool, String> {
        let Some(workspace) = self.workspaces.get(session_id) else {
            return Ok(false);
        };
        let Some(snapshot) = workspace
            .control
            .release_user_control_if_unchanged(expected_revision)
        else {
            return Ok(false);
        };
        let active_tab = workspace.active_tab.clone();
        emit_control_changed(app, session_id, &active_tab, snapshot);
        if let Err(error) = self.persist_workspace(session_id) {
            eprintln!("[browser] 自动交还控制权后的映射持久化失败: {error}");
        }
        Ok(true)
    }

    /// 原子保存用户可恢复的最小页面清单。URL 来自宿主 WebView 本身，不接受前端
    /// 或 MCP 提交的 target 映射；内部 marker 会收敛为 about:blank。
    pub fn persist_restore_workspace(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
    ) -> Result<(), String> {
        let workspace = self
            .workspaces
            .get(session_id)
            .ok_or_else(|| "浏览器工作区不存在".to_string())?;
        // A restored workspace remains an unpublished transaction until every
        // fresh WebView has been bound and navigated. Navigation callbacks and
        // process exit may race that transaction; preserve the last complete
        // manifest instead of replacing it with temporary marker pages.
        if !workspace_restore_ready(workspace) {
            return Ok(());
        }
        let active_index = workspace
            .tabs
            .iter()
            .position(|tab| tab.token == workspace.active_tab)
            .ok_or_else(|| "浏览器当前标签不在工作区内".to_string())?;
        let mut tabs = Vec::with_capacity(workspace.tabs.len());
        for entry in workspace.tabs.iter() {
            let webview = app
                .get_webview(&entry.label)
                .ok_or_else(|| "浏览器标签页表面不存在".to_string())?;
            let url = sanitize_marker_url(
                webview
                    .url()
                    .map_err(|error| format!("读取浏览器标签页地址失败: {error}"))?
                    .to_string(),
                &entry.token,
            );
            if url.len() > MAX_RESTORE_URL_LEN || !super::super::is_allowed_url(&url) {
                return Err("浏览器标签页地址不能写入恢复清单".to_string());
            }
            tabs.push(json!({ "url": url }));
        }
        if tabs.is_empty() || tabs.len() > MAX_WORKSPACE_TABS {
            return Err("浏览器恢复标签数量无效".to_string());
        }
        let restore = NativeWorkspaceRestore {
            urls: tabs
                .into_iter()
                .filter_map(|tab| tab.get("url")?.as_str().map(ToOwned::to_owned))
                .collect(),
            active_index,
        };
        write_restore_workspace_file(
            &paths::browser_workspace_restore_json(&workspace.session_token),
            &restore,
        )
    }

    pub fn persist_all_restore(&self, app: &tauri::AppHandle) -> Result<(), String> {
        for session_id in self.workspaces.keys() {
            self.persist_restore_workspace(app, session_id)?;
        }
        Ok(())
    }

    pub fn persist_navigation_state(
        &self,
        app: &tauri::AppHandle,
        session_id: &str,
    ) -> Result<(), String> {
        self.persist_workspace(session_id)?;
        self.persist_restore_workspace(app, session_id)
    }

    fn persist_workspace(&self, session_id: &str) -> Result<(), String> {
        let workspace = self
            .workspaces
            .get(session_id)
            .ok_or_else(|| "浏览器工作区不存在".to_string())?;
        persist_workspace_snapshot(workspace, workspace.control.snapshot().revision)
    }
}

fn workspace_restore_ready(workspace: &Workspace) -> bool {
    !workspace.tabs.is_empty() && workspace.tabs.iter().all(SurfaceEntry::is_published)
}

fn persist_workspace_snapshot(workspace: &Workspace, revision: u64) -> Result<(), String> {
    let path = paths::browser_workspace_state_json(&workspace.session_token);
    // 对外只发布完整、可严格解析的 v2 权威映射。prepare/create 到 bind_target
    // 之间删除旧快照，避免 wrapper 把 null 或陈旧 target 当成可执行状态。
    if workspace
        .tabs
        .iter()
        .any(|tab| tab.automation_target.is_none())
    {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("清理未完成的浏览器工作区状态失败: {error}"));
            }
        }
        return Ok(());
    }
    std::fs::create_dir_all(path.parent().expect("工作区状态文件应有父目录"))
        .map_err(|e| format!("创建浏览器工作区状态目录失败: {e}"))?;
    let value = workspace_state_value_with_revision(workspace, revision);
    let encoded = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    crate::platform::filesystem::atomic_write(&path, &encoded)
        .map_err(|e| format!("写入浏览器工作区状态失败: {e}"))
}

fn workspace_state_value(workspace: &Workspace) -> serde_json::Value {
    workspace_state_value_with_revision(workspace, workspace.control.snapshot().revision)
}

fn workspace_state_value_with_revision(workspace: &Workspace, revision: u64) -> serde_json::Value {
    debug_assert!(workspace
        .tabs
        .iter()
        .all(|tab| tab.automation_target.is_some()));
    json!({
        "version": 2,
        "mapping_authority": "host",
        "revision": revision,
        "session_token": workspace.session_token,
        "active_tab": workspace.active_tab,
        "tabs": workspace.tabs.iter().map(|tab| json!({
            "token": tab.token,
            "target_id": tab.automation_target,
        })).collect::<Vec<_>>(),
    })
}

fn parse_restore_workspace(encoded: &[u8]) -> Result<NativeWorkspaceRestore, String> {
    let decoded: WorkspaceRestoreFile = serde_json::from_slice(encoded)
        .map_err(|error| format!("解析浏览器恢复清单失败: {error}"))?;
    if decoded.version != WORKSPACE_RESTORE_VERSION {
        return Err(format!("不支持的浏览器恢复清单版本: {}", decoded.version));
    }
    if decoded.tabs.is_empty()
        || decoded.tabs.len() > MAX_WORKSPACE_TABS
        || decoded.active_index >= decoded.tabs.len()
    {
        return Err("浏览器恢复清单的标签数量或当前标签无效".to_string());
    }
    let mut urls = Vec::with_capacity(decoded.tabs.len());
    for tab in decoded.tabs {
        if tab.url.len() > MAX_RESTORE_URL_LEN || !super::super::is_allowed_url(&tab.url) {
            return Err("浏览器恢复清单包含不受支持的页面地址".to_string());
        }
        urls.push(tab.url);
    }
    Ok(NativeWorkspaceRestore {
        urls,
        active_index: decoded.active_index,
    })
}

fn write_restore_workspace_file(
    path: &Path,
    restore: &NativeWorkspaceRestore,
) -> Result<(), String> {
    if restore.urls.is_empty()
        || restore.urls.len() > MAX_WORKSPACE_TABS
        || restore.active_index >= restore.urls.len()
        || restore
            .urls
            .iter()
            .any(|url| url.len() > MAX_RESTORE_URL_LEN || !super::super::is_allowed_url(url))
    {
        return Err("浏览器恢复清单无效".to_string());
    }
    let parent = path.parent().expect("浏览器恢复清单应有父目录");
    std::fs::create_dir_all(parent).map_err(|error| format!("创建浏览器恢复目录失败: {error}"))?;
    crate::platform::os::make_private_dir(parent);
    let tabs = restore
        .urls
        .iter()
        .map(|url| json!({ "url": url }))
        .collect::<Vec<_>>();
    let encoded = serde_json::to_vec(&json!({
        "version": WORKSPACE_RESTORE_VERSION,
        "active_index": restore.active_index,
        "tabs": tabs,
    }))
    .map_err(|error| format!("编码浏览器恢复清单失败: {error}"))?;
    crate::platform::filesystem::atomic_write_private(path, &encoded)
        .map_err(|error| format!("写入浏览器恢复清单失败: {error}"))
}

fn remove_restore_file(session_token: &str) -> Result<(), String> {
    match std::fs::remove_file(paths::browser_workspace_restore_json(session_token)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("删除浏览器恢复清单失败: {error}")),
    }
}

/// Finish a Prepare compensation after its process-local workspace is already
/// gone. Keeping the durable delete as a separate fallible step is important:
/// a first attempt may close every native WebView successfully and then fail
/// filesystem cleanup, so a retry must execute this step again.
fn complete_absent_prepare_rollback(
    preserve_restore: bool,
    has_remaining_sessions: bool,
    delete_restore: impl FnOnce() -> Result<(), String>,
) -> Result<Option<bool>, String> {
    if preserve_restore {
        return Ok(None);
    }
    delete_restore()?;
    Ok(Some(has_remaining_sessions))
}

fn build_webview<P: PlatformWebviewConfig>(
    platform: &P,
    app: &tauri::AppHandle,
    session_id: &str,
    tab_token: &str,
    data_directory: &Path,
    url: &str,
    control: Arc<WorkspaceControl>,
    published: bool,
) -> Result<SurfaceEntry, WebviewBuildError> {
    let window = app
        .get_window("main")
        .ok_or_else(|| WebviewBuildError::new("主窗口尚未就绪".to_string()))?;
    let parsed_url = url
        .parse()
        .map_err(|e| WebviewBuildError::new(format!("浏览器初始地址无效: {e}")))?;
    let page_id = next_native_page_id().map_err(WebviewBuildError::new)?;
    let label = format!("{WEBVIEW_LABEL_PREFIX}{tab_token}");
    if let Some(stale) = app.get_webview(&label) {
        stale
            .close()
            .map_err(|e| WebviewBuildError::new(format!("关闭失效浏览器标签页失败: {e}")))?;
    }

    let navigation_app = app.clone();
    let navigation_session_id = session_id.to_string();
    let navigation_tab_token = tab_token.to_string();
    let navigation_label = label.clone();
    let navigation_control = Arc::clone(&control);
    let publication = Arc::new(std::sync::atomic::AtomicBool::new(published));
    let navigation_publication = Arc::clone(&publication);
    let committed_navigation_app = app.clone();
    let committed_navigation_session_id = session_id.to_string();
    let committed_navigation_tab_token = tab_token.to_string();
    let committed_navigation_control = Arc::clone(&control);
    let committed_navigation_publication = Arc::clone(&publication);
    let title_app = app.clone();
    let title_session_id = session_id.to_string();
    let title_tab_token = tab_token.to_string();
    let title_publication = Arc::clone(&publication);
    let popup_app = app.clone();
    let popup_session_id = session_id.to_string();
    let popup_tab_token = tab_token.to_string();
    let popup_control = Arc::clone(&control);
    let popup_publication = Arc::clone(&publication);
    let download_app = app.clone();
    let download_session_id = session_id.to_string();
    let download_tab_token = tab_token.to_string();
    let cdp_tab_token = platform
        .capabilities()
        .chrome_devtools_protocol
        .then_some(tab_token);
    super::register_browser_core_webview_binding(&label, tab_token, &control)
        .map_err(WebviewBuildError::new)?;
    let init_script = browser_initialization_script(cdp_tab_token);
    let builder = WebviewBuilder::new(&label, WebviewUrl::External(parsed_url));
    let builder = match platform.configure_builder(builder, data_directory) {
        Ok(builder) => builder,
        Err(error) => {
            super::unregister_browser_core_webview_binding(&label);
            return Err(WebviewBuildError::new(error));
        }
    }
        // BrowserCore runs in every frame on all three desktop engines. The
        // script exposes DOM-only helpers; native input remains behind the
        // task lease in Rust and is never callable by page JavaScript.
        .initialization_script_for_all_frames(init_script)
        .on_download(move |_webview, event| {
            if let DownloadEvent::Requested { url, .. } = event {
                // URL 可能含一次性 token/查询参数；只向 UI 暴露 origin，不记录本地
                // destination。内嵌下载默认拒绝，用户可在系统浏览器自行处理。
                let source = match (url.scheme(), url.host_str()) {
                    (scheme, Some(host)) => format!("{scheme}://{host}"),
                    (scheme, None) => format!("{scheme}:"),
                };
                let _ = download_app.emit(
                    "browser:download-blocked",
                    json!({
                        "sessionId": download_session_id,
                        "tab": download_tab_token,
                        "source": source,
                    }),
                );
            }
            false
        })
        .on_navigation(move |url| {
            let binding_marker = super::classify_browser_core_binding_navigation(
                &navigation_label,
                url.as_str(),
            );
            // Agent create_tab 首航发生在隐藏 staging WebView。此时只做协议校验，
            // 不让 marker/真实 URL 回调提前改变控制权、污染恢复清单或显示 UI。
            if !navigation_publication.load(std::sync::atomic::Ordering::SeqCst) {
                return binding_marker
                    || has_internal_marker_for_token(url.as_str(), &navigation_tab_token)
                    || super::super::is_allowed_url(url.as_str());
            }
            // Linux temporarily navigates an already-published WebView to a
            // process-local marker while recovering its WebDriver handle.
            // Do not expose or persist that transient URL; the exact restored
            // real URL closes the binding window in the platform registry.
            if binding_marker {
                return true;
            }
            if let Some(interaction) = user_takeover_interaction(url) {
                // CDP/平台输入同样可能产生 isTrusted=true；只有 wrapper 在 lease 复核后
                // 显式打开的短输入窗口可以抑制这次 fail-safe 接管。
                if navigation_control.agent_input_in_progress() {
                    return false;
                }
                // 这是有意设计成低权限、单向的信号通道：远程页面即使主动导航到
                // 该保留 scheme，也只能让 Agent 暂停并把控制权交给用户，不能
                // 调用任意 Tauri command 或取得宿主数据。
                // Every real user activity advances the revision. This both
                // revokes any Agent lease and restarts the 3-second idle
                // hand-back window; timers created by earlier activity then
                // fail their revision CAS instead of stealing control back.
                let snapshot = navigation_control.bump(Some(NativeControlOwner::User));
                let _ = navigation_app.emit(
                    "browser:user-takeover",
                    json!({
                        "sessionId": navigation_session_id,
                        "tabToken": navigation_tab_token,
                        "interaction": interaction,
                        "revision": snapshot.revision,
                    }),
                );
                emit_control_changed(
                    &navigation_app,
                    &navigation_session_id,
                    &navigation_tab_token,
                    snapshot,
                );
                let persist_app = navigation_app.clone();
                let persist_session_id = navigation_session_id.clone();
                tauri::async_runtime::spawn(async move {
                    let manager = persist_app.state::<super::super::BrowserManager>();
                    if let Err(error) = manager.persist_native_restore(&persist_session_id) {
                        eprintln!("[browser] 持久化用户接管状态失败，将后台重试: {error}");
                    }
                });
                // 在 WebView 提交导航前拒绝，保留 scheme 不会进入页面历史记录。
                return false;
            }
            let internal_marker =
                has_internal_marker_for_token(url.as_str(), &navigation_tab_token);
            if !internal_marker && !super::super::is_allowed_url(url.as_str()) {
                let _ = navigation_app.emit(
                    "browser:navigation-blocked",
                    json!({
                        "sessionId": navigation_session_id,
                        "tab": navigation_tab_token,
                        "scheme": url.scheme(),
                    }),
                );
                return false;
            }
            true
        })
        .on_page_load(move |_webview, payload| {
            // Wry's navigation-policy callback includes redirects and child
            // frames on WKWebView. Only a committed main document is allowed
            // to update the address bar, control revision and restore state.
            if payload.event() != PageLoadEvent::Started
                || !committed_navigation_publication.load(std::sync::atomic::Ordering::SeqCst)
            {
                return;
            }
            let committed_url = payload.url().as_str();
            if has_internal_marker_for_token(committed_url, &committed_navigation_tab_token)
                || super::is_browser_core_binding_url(committed_url)
            {
                return;
            }
            let payload = json!({
                "sessionId": committed_navigation_session_id,
                "tab": committed_navigation_tab_token,
                "url": committed_url,
            });
            let _ = committed_navigation_app.emit("browser:navigation", &payload);
            let _ = committed_navigation_app.emit("browser:tabs-changed", &payload);
            if let Some(snapshot) = committed_navigation_control
                .bump_for_navigation_if_no_active_agent_operation()
            {
                emit_control_changed(
                    &committed_navigation_app,
                    &committed_navigation_session_id,
                    &committed_navigation_tab_token,
                    snapshot,
                );
            }
            // Page-load delegates run on the native UI thread and may still be
            // nested under WebView dispatch. Persist asynchronously to avoid
            // re-entering the native_surface lock.
            let persist_app = committed_navigation_app.clone();
            let persist_session_id = committed_navigation_session_id.clone();
            tauri::async_runtime::spawn(async move {
                let manager = persist_app.state::<super::super::BrowserManager>();
                if let Err(error) = manager.persist_native_restore(&persist_session_id) {
                    eprintln!("[browser] 持久化页面导航失败: {error}");
                }
            });
        })
        .on_document_title_changed(move |_webview, title| {
            if !title_publication.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let _ = title_app.emit(
                "browser:tab-title",
                json!({
                    "sessionId": title_session_id,
                    "tab": title_tab_token,
                    "title": title,
                }),
            );
        })
        .on_new_window(move |url, _features| {
            if !popup_publication.load(std::sync::atomic::Ordering::SeqCst) {
                return NewWindowResponse::Deny;
            }
            if !super::super::is_allowed_url(url.as_str()) {
                let _ = popup_app.emit(
                    "browser:navigation-blocked",
                    json!({
                        "sessionId": popup_session_id,
                        "scheme": url.scheme(),
                    }),
                );
                return NewWindowResponse::Deny;
            }
            // 只有已 begin 的原子 dispatch 才能从 Rust 控制状态复制完整 lease。
            // 没有有效授权的页面自发 popup 走 User；捕获到授权后则由 BrowserManager
            // 使用隐藏 staging + 最终 CAS 发布，用户接管先发生时安全拒绝晚到的新页。
            let authorization =
                popup_agent_authorization(&popup_control, &popup_session_id, &popup_tab_token);
            let app = popup_app.clone();
            let session_id = popup_session_id.clone();
            tauri::async_runtime::spawn(async move {
                let manager = app.state::<super::super::BrowserManager>();
                if let Err(error) = manager
                    .create_popup_tab(&session_id, url.to_string(), authorization)
                    .await
                {
                    eprintln!("[browser] 接管网页新窗口失败: {error}");
                }
            });
            NewWindowResponse::Deny
        });
    let webview = match window.add_child(
        builder,
        PhysicalPosition::new(0, 0),
        PhysicalSize::new(1, 1),
    ) {
        Ok(webview) => webview,
        Err(error) => {
            super::unregister_browser_core_webview_binding(&label);
            return Err(WebviewBuildError::new(format!(
                "创建系统 WebView 浏览器标签页失败: {error}"
            )));
        }
    };
    let entry = SurfaceEntry {
        label,
        token: tab_token.to_string(),
        page_id,
        automation_target: None,
        created_by_request_id: None,
        published: publication,
        created_at_revision: None,
    };
    if let Err(hide_error) = webview.hide() {
        entry.unpublish();
        return match webview.close() {
            Ok(()) => {
                super::unregister_browser_core_webview_binding(&entry.label);
                Err(WebviewBuildError::new(format!(
                    "初始化隐藏浏览器标签页失败: {hide_error}"
                )))
            }
            Err(close_error) => Err(WebviewBuildError::with_survivor(
                format!("初始化隐藏浏览器标签页失败: {hide_error}; 补偿关闭失败: {close_error}"),
                entry,
            )),
        };
    }
    if let Err(attach_error) = super::attach_native_surface(&webview) {
        entry.unpublish();
        return match webview.close() {
            Ok(()) => {
                super::unregister_browser_core_webview_binding(&entry.label);
                Err(WebviewBuildError::new(format!(
                    "初始化浏览器标签页原生容器失败: {attach_error}"
                )))
            }
            Err(close_error) => Err(WebviewBuildError::with_survivor(
                format!(
                    "初始化浏览器标签页原生容器失败: {attach_error}; 补偿关闭失败: {close_error}"
                ),
                entry,
            )),
        };
    }
    Ok(entry)
}

fn popup_agent_authorization(
    control: &WorkspaceControl,
    session_id: &str,
    source_tab_token: &str,
) -> Option<RetainedAgentOperation> {
    control.retain_agent_operation_for_popup(session_id, source_tab_token)
}

fn active_entry(workspace: &Workspace) -> Option<&SurfaceEntry> {
    workspace.tabs.by_token(&workspace.active_tab)
}

fn validate_agent_mutation(
    workspace: &Workspace,
    session_id: &str,
    authorization: &NativeTabLease,
    required_authorization_tab: Option<&str>,
) -> Result<(), String> {
    if authorization.owner != NativeControlOwner::Agent
        || authorization.session_id != session_id
        || workspace.active_tab != authorization.tab_token
        || required_authorization_tab.is_some_and(|required| required != authorization.tab_token)
        || workspace.tabs.target_for_token(&authorization.tab_token)
            != Some(authorization.target_id.as_str())
    {
        return Err("Agent mutation lease 与当前宿主标签不一致".to_string());
    }
    Ok(())
}

fn remove_tab_from_workspace(workspace: &mut Workspace, tab_token: &str) {
    let (index, _) = workspace
        .tabs
        .remove_token(tab_token)
        .expect("标签在关闭 WebView 前已检查");
    if workspace.active_tab == tab_token {
        if workspace.tabs.is_empty() {
            workspace.active_tab.clear();
            return;
        }
        let fallback = index.saturating_sub(1).min(workspace.tabs.len() - 1);
        workspace.active_tab = workspace
            .tabs
            .token_at(fallback)
            .expect("关闭后至少保留一个标签")
            .to_string();
    }
}

fn hide_workspace(app: &tauri::AppHandle, workspace: &Workspace) {
    for entry in workspace.tabs.iter() {
        if let Some(webview) = app.get_webview(&entry.label) {
            let _ = webview.hide();
        }
    }
}

fn hide_all(app: &tauri::AppHandle, workspaces: &HashMap<String, Workspace>) {
    for workspace in workspaces.values() {
        hide_workspace(app, workspace);
    }
}

/// The application owns one physical browser surface. Publishing one workspace therefore
/// revokes the visibility intent of every other workspace in the same state transition.
fn set_exclusive_workspace_visibility(
    workspaces: &mut HashMap<String, Workspace>,
    session_id: &str,
) {
    for (workspace_session_id, workspace) in workspaces.iter_mut() {
        workspace.visible = workspace_session_id == session_id;
    }
}

fn workspace_may_present_native_surface(
    session_owns_visible_surface: bool,
    workspace_visible: bool,
) -> bool {
    session_owns_visible_surface && workspace_visible
}

fn show_active_workspace(app: &tauri::AppHandle, workspace: &Workspace) -> Result<(), String> {
    hide_workspace(app, workspace);
    let entry = active_entry(workspace).ok_or_else(|| "当前标签页不存在".to_string())?;
    let webview = app
        .get_webview(&entry.label)
        .ok_or_else(|| "当前浏览器标签页表面不存在".to_string())?;
    super::show_native_surface(&webview, workspace.bounds)
}

fn reconcile_workspace_close(
    workspace: &mut Workspace,
    mut close_entry: impl FnMut(&SurfaceEntry) -> Result<(), String>,
) -> Result<(), String> {
    let tokens = workspace
        .tabs
        .iter()
        .map(|entry| entry.token.clone())
        .collect::<Vec<_>>();
    let mut errors = Vec::new();
    for token in tokens {
        let entry = workspace
            .tabs
            .by_token(&token)
            .cloned()
            .expect("关闭清单来自同一工作区快照");
        match close_entry(&entry) {
            Ok(()) => {
                super::unregister_browser_core_webview_binding(&entry.label);
                remove_tab_from_workspace(workspace, &token);
            }
            Err(error) => errors.push(format!("{}: {error}", entry.label)),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "关闭对话浏览器标签页失败，仍有 {} 个表面可重试: {}",
            workspace.tabs.len(),
            errors.join("; ")
        ))
    }
}

fn reconcile_staged_close(
    staged_tabs: &mut HashMap<(String, String), SurfaceEntry>,
    staged_user_tabs: &mut HashMap<(String, String), bool>,
    session_id: &str,
    mut close_entry: impl FnMut(&SurfaceEntry) -> Result<(), String>,
) -> Result<(), String> {
    let mut keys = staged_tabs
        .keys()
        .filter(|(owner_session, _)| owner_session == session_id)
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    let mut errors = Vec::new();
    for key in keys {
        let entry = staged_tabs
            .get(&key)
            .cloned()
            .expect("待关闭 staging 清单来自同一映射快照");
        match close_entry(&entry) {
            Ok(()) => {
                super::unregister_browser_core_webview_binding(&entry.label);
                staged_tabs.remove(&key);
                staged_user_tabs.remove(&key);
            }
            Err(error) => errors.push(format!("{}: {error}", entry.label)),
        }
    }
    staged_user_tabs.retain(|key, _| staged_tabs.contains_key(key));
    if errors.is_empty() {
        Ok(())
    } else {
        let survivors = staged_tabs
            .keys()
            .filter(|(owner_session, _)| owner_session == session_id)
            .count();
        Err(format!(
            "关闭待提交浏览器标签失败，仍有 {survivors} 个表面可重试: {}",
            errors.join("; ")
        ))
    }
}

fn reconcile_quarantined_close(
    quarantined_tabs: &mut HashMap<(String, String), SurfaceEntry>,
    session_id: &str,
    mut close_entry: impl FnMut(&SurfaceEntry) -> Result<(), String>,
) -> Result<(), String> {
    let mut keys = quarantined_tabs
        .keys()
        .filter(|(owner_session, _)| owner_session == session_id)
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    let mut errors = Vec::new();
    for key in keys {
        let entry = quarantined_tabs
            .get(&key)
            .cloned()
            .expect("隔离清单来自同一映射快照");
        match close_entry(&entry) {
            Ok(()) => {
                super::unregister_browser_core_webview_binding(&entry.label);
                quarantined_tabs.remove(&key);
            }
            Err(error) => errors.push(format!("{}: {error}", entry.label)),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        let survivors = quarantined_tabs
            .keys()
            .filter(|(owner_session, _)| owner_session == session_id)
            .count();
        Err(format!(
            "关闭隔离浏览器标签失败，仍有 {survivors} 个表面可重试: {}",
            errors.join("; ")
        ))
    }
}

fn close_workspace(app: &tauri::AppHandle, workspace: &mut Workspace) -> Result<(), String> {
    reconcile_workspace_close(workspace, |entry| {
        if let Some(webview) = app.get_webview(&entry.label) {
            webview
                .close()
                .map_err(|error| format!("关闭系统 WebView 失败: {error}"))?;
        }
        Ok(())
    })
}

fn emit_control_changed(
    app: &tauri::AppHandle,
    session_id: &str,
    tab_token: &str,
    snapshot: ControlSnapshot,
) {
    let _ = app.emit(
        "browser:control-changed",
        json!({
            "sessionId": session_id,
            "tabToken": tab_token,
            "revision": snapshot.revision,
            "owner": snapshot.owner.as_str(),
        }),
    );
    if snapshot.owner == NativeControlOwner::User {
        let release_app = app.clone();
        let release_session_id = session_id.to_string();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(USER_CONTROL_IDLE_TIMEOUT).await;
            let manager = release_app.state::<super::super::BrowserManager>();
            if let Err(error) =
                manager.release_user_control_if_idle(&release_session_id, snapshot.revision)
            {
                eprintln!("[browser] 自动交还浏览器控制权失败: {error}");
            }
        });
    }
}

fn sanitize_marker_url(url: String, expected_tab_token: &str) -> String {
    if has_internal_marker_for_token(&url, expected_tab_token) {
        "about:blank".to_string()
    } else {
        url
    }
}

fn has_internal_marker_for_token(url: &str, expected_tab_token: &str) -> bool {
    is_valid_token(expected_tab_token)
        && internal_marker_token(url).is_some_and(|token| token == expected_tab_token)
}

fn internal_marker_token(url: &str) -> Option<&str> {
    // WKWebView/NSURL serializes the fragment delimiter in opaque `about:` URLs as `%23`.
    // Keep this compatibility at the host-owned marker seam: arbitrary about URLs, suffixes,
    // queries, and markers belonging to another tab must remain untrusted.
    const PREFIXES: [&str; 4] = [
        "about:blank#pinvou-session-",
        "about:blank#pinvou-tab-",
        "about:blank%23pinvou-session-",
        "about:blank%23pinvou-tab-",
    ];
    PREFIXES
        .iter()
        .find_map(|prefix| url.strip_prefix(prefix))
        .filter(|token| is_valid_token(token))
}

fn marked_blank_url(url: &str, tab_token: &str) -> String {
    if url == "about:blank" {
        format!("about:blank#pinvou-tab-{tab_token}")
    } else {
        url.to_string()
    }
}

fn browser_initialization_script(cdp_tab_token: Option<&str>) -> String {
    let bootstrap_identity = cdp_tab_token
        .map(|tab_token| {
            debug_assert!(is_valid_token(tab_token));
            format!(
                r#"
  const bootstrapUrl = globalThis.location.href;
  if (bootstrapUrl === 'about:blank#pinvou-session-{tab_token}' ||
      bootstrapUrl === 'about:blank#pinvou-tab-{tab_token}') {{
    Object.defineProperty(globalThis, '__PINVOU_BROWSER_BOOTSTRAP_TOKEN__', {{
      value: '{tab_token}', enumerable: false, configurable: true, writable: false
    }});
  }}"#
            )
        })
        .unwrap_or_default();
    let takeover = format!(
        r#"
(() => {{
{bootstrap_identity}
  const deferSignal = globalThis.queueMicrotask.bind(globalThis);
  const signalTakeover = (event) => {{
    if (!event.isTrusted) return;
    deferSignal(() => {{
      try {{ globalThis.location.href = '{USER_TAKEOVER_SCHEME}://interaction/' + event.type; }} catch (_) {{}}
    }});
  }};
  for (const type of ['pointerdown', 'keydown', 'wheel']) {{
    globalThis.addEventListener(type, signalTakeover, {{ capture: true, passive: true }});
  }}
}})();
"#
    );
    format!("{takeover}\n{BROWSER_CORE_RUNTIME}")
}

fn user_takeover_interaction(url: &tauri::Url) -> Option<&str> {
    if url.scheme() != USER_TAKEOVER_SCHEME || url.host_str() != Some("interaction") {
        return None;
    }
    match url.path().trim_matches('/') {
        "pointerdown" => Some("pointerdown"),
        "keydown" => Some("keydown"),
        "wheel" => Some("wheel"),
        _ => None,
    }
}

fn is_valid_token(token: &str) -> bool {
    token.len() == 16 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_page_ids_are_incarnation_scoped_and_javascript_safe() {
        assert_eq!(
            compose_native_page_id(7, 0),
            Some(7_u64 << NATIVE_PAGE_ID_SEQUENCE_BITS)
        );
        assert_ne!(compose_native_page_id(7, 0), compose_native_page_id(8, 0));
        assert_eq!(
            compose_native_page_id(
                NATIVE_PAGE_ID_INCARNATION_LIMIT - 1,
                NATIVE_PAGE_ID_SEQUENCE_LIMIT - 1
            ),
            Some(MAX_SAFE_PAGE_ID)
        );
        assert_eq!(
            compose_native_page_id(NATIVE_PAGE_ID_INCARNATION_LIMIT, 0),
            None
        );
        assert_eq!(
            compose_native_page_id(7, NATIVE_PAGE_ID_SEQUENCE_LIMIT),
            None
        );
    }

    #[derive(Default)]
    struct TestPlatform {
        initialized: bool,
    }

    impl PlatformWebviewConfig for TestPlatform {
        const ACTIVATION_READY: bool = true;

        fn capabilities(&self) -> NativeSurfaceCapabilities {
            NativeSurfaceCapabilities::new(true, true, true)
        }

        fn requires_reset(&self, _automation_port: Option<u16>, _data_directory: &Path) -> bool {
            false
        }

        fn prepare(
            &mut self,
            _automation_port: Option<u16>,
            _data_directory: &Path,
        ) -> Result<(), String> {
            self.initialized = true;
            Ok(())
        }

        fn configure_builder(
            &self,
            builder: WebviewBuilder<tauri::Wry>,
            _data_directory: &Path,
        ) -> Result<WebviewBuilder<tauri::Wry>, String> {
            Ok(builder)
        }

        fn reset(&mut self) {
            self.initialized = false;
        }

        fn owns_port(&self, _port: u16) -> bool {
            self.initialized
        }

        fn is_initialized(&self) -> bool {
            self.initialized
        }
    }

    fn surface_with_workspace(session_id: &str) -> DesktopBrowserSurface<TestPlatform> {
        let token = "0123456789abcdef".to_string();
        let mut surface = DesktopBrowserSurface {
            platform: TestPlatform { initialized: true },
            data_directory: None,
            workspaces: HashMap::new(),
            staged_tabs: HashMap::new(),
            staged_user_tabs: HashMap::new(),
            quarantined_tabs: HashMap::new(),
            active_session: Some(session_id.to_string()),
            requests: RequestLedger::default(),
        };
        surface.workspaces.insert(
            session_id.to_string(),
            Workspace {
                session_token: token.clone(),
                tabs: TabRegistry::from_entry(SurfaceEntry {
                    label: "test-webview".to_string(),
                    token: token.clone(),
                    page_id: 1,
                    automation_target: None,
                    created_by_request_id: None,
                    published: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                    created_at_revision: None,
                }),
                active_tab: token,
                bounds: None,
                visible: false,
                control: Arc::new(WorkspaceControl::new(1, NativeControlOwner::Agent)),
                prepare_generation: None,
            },
        );
        surface
    }

    fn test_entry(
        token: &str,
        label: &str,
        page_id: u64,
        published: bool,
        creation_id: Option<&str>,
    ) -> SurfaceEntry {
        SurfaceEntry {
            label: label.to_string(),
            token: token.to_string(),
            page_id,
            automation_target: None,
            created_by_request_id: creation_id.map(ToOwned::to_owned),
            published: Arc::new(std::sync::atomic::AtomicBool::new(published)),
            created_at_revision: None,
        }
    }

    #[test]
    fn close_session_keeps_registration_when_app_handle_is_unavailable() {
        let mut surface = surface_with_workspace("session-a");

        let result = surface.close_session(None, "session-a");

        assert!(result.is_err());
        assert!(surface.has_session("session-a"));
        assert!(surface.has_sessions());
        assert!(surface.is_initialized());
    }

    #[test]
    fn close_unknown_session_is_idempotent_and_keeps_other_workspaces() {
        let mut surface = surface_with_workspace("session-a");

        assert_eq!(surface.close_session(None, "session-missing"), Ok(true));
        assert!(surface.has_session("session-a"));
    }

    #[test]
    fn runtime_tab_limit_counts_hidden_staging_entries() {
        let mut surface = surface_with_workspace("session-a");
        for index in 1..MAX_WORKSPACE_TABS {
            let token = format!("{index:016x}");
            surface.staged_tabs.insert(
                ("session-a".to_string(), token.clone()),
                SurfaceEntry {
                    label: format!("staged-webview-{index}"),
                    token,
                    page_id: index as u64 + 1,
                    automation_target: None,
                    created_by_request_id: Some(format!("create-{index}")),
                    published: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    created_at_revision: None,
                },
            );
        }

        assert!(surface.ensure_tab_capacity("session-a").is_err());
        surface
            .staged_tabs
            .remove(&("session-a".to_string(), format!("{:016x}", 1)));
        assert_eq!(surface.ensure_tab_capacity("session-a"), Ok(()));
    }

    #[test]
    fn partial_workspace_close_keeps_only_real_survivors_and_retry_is_idempotent() {
        let mut surface = surface_with_workspace("session-a");
        let workspace = surface.workspaces.get_mut("session-a").unwrap();
        workspace
            .tabs
            .insert(test_entry(
                "1111111111111111",
                "test-webview-b",
                2,
                true,
                None,
            ))
            .unwrap();
        workspace
            .tabs
            .insert(test_entry(
                "2222222222222222",
                "test-webview-c",
                3,
                true,
                None,
            ))
            .unwrap();
        workspace.active_tab = "1111111111111111".to_string();

        let first = reconcile_workspace_close(workspace, |entry| {
            if entry.label == "test-webview-b" {
                Err("injected close failure".to_string())
            } else {
                Ok(())
            }
        });

        assert!(first.is_err());
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.active_tab, "1111111111111111");
        assert_eq!(workspace.tabs.token_at(0), Some("1111111111111111"));

        assert_eq!(reconcile_workspace_close(workspace, |_| Ok(())), Ok(()));
        assert!(workspace.tabs.is_empty());
        assert!(workspace.active_tab.is_empty());
    }

    #[test]
    fn partial_staging_close_retains_failed_generation_and_retries_only_survivor() {
        let mut surface = surface_with_workspace("session-a");
        let key_a = ("session-a".to_string(), "1111111111111111".to_string());
        let key_b = ("session-a".to_string(), "2222222222222222".to_string());
        let other = ("session-b".to_string(), "3333333333333333".to_string());
        surface.staged_tabs.insert(
            key_a.clone(),
            test_entry(&key_a.1, "staged-a", 2, false, Some("create-a")),
        );
        surface.staged_tabs.insert(
            key_b.clone(),
            test_entry(&key_b.1, "staged-b", 3, false, Some("create-b")),
        );
        surface.staged_tabs.insert(
            other.clone(),
            test_entry(&other.1, "staged-other", 4, false, Some("create-other")),
        );
        surface.staged_user_tabs.insert(key_a.clone(), false);
        surface.staged_user_tabs.insert(key_b.clone(), true);

        let first = reconcile_staged_close(
            &mut surface.staged_tabs,
            &mut surface.staged_user_tabs,
            "session-a",
            |entry| {
                if entry.label == "staged-b" {
                    Err("injected close failure".to_string())
                } else {
                    Ok(())
                }
            },
        );

        assert!(first.is_err());
        assert!(!surface.staged_tabs.contains_key(&key_a));
        assert!(surface.staged_tabs.contains_key(&key_b));
        assert!(surface.staged_tabs.contains_key(&other));
        assert!(!surface.staged_user_tabs.contains_key(&key_a));
        assert!(surface.staged_user_tabs.contains_key(&key_b));

        assert_eq!(
            reconcile_staged_close(
                &mut surface.staged_tabs,
                &mut surface.staged_user_tabs,
                "session-a",
                |_| Ok(()),
            ),
            Ok(())
        );
        assert!(!surface.staged_tabs.contains_key(&key_b));
        assert!(surface.staged_tabs.contains_key(&other));
        assert!(!surface.staged_user_tabs.contains_key(&key_b));
    }

    #[test]
    fn hide_and_compensating_close_failure_is_quarantined_and_never_becomes_a_session() {
        let mut surface = DesktopBrowserSurface::<TestPlatform>::default();
        let mut build_error = WebviewBuildError::with_survivor(
            "injected hide and close failure".to_string(),
            test_entry(
                "1111111111111111",
                "quarantined-hide-failure",
                2,
                true,
                Some("must-be-cleared"),
            ),
        );
        surface.quarantine_entries(
            "session-a",
            build_error.survivor.take().into_iter().collect(),
        );

        let key = ("session-a".to_string(), "1111111111111111".to_string());
        let survivor = &surface.quarantined_tabs[&key];
        assert!(!survivor.is_published());
        assert!(survivor.created_by_request_id.is_none());
        assert!(!surface.has_session("session-a"));
        assert!(surface.owns_session_resources("session-a"));
        assert!(surface.has_sessions());
        assert!(surface.webview_label_for_tab("session-a", &key.1).is_none());
    }

    #[test]
    fn repeated_create_gate_retries_one_quarantine_without_accumulating_staging() {
        let mut quarantined = HashMap::from([(
            ("session-a".to_string(), "1111111111111111".to_string()),
            test_entry("1111111111111111", "quarantined-create", 2, false, None),
        )]);

        for _ in 0..2 {
            let result = reconcile_quarantined_close(&mut quarantined, "session-a", |_| {
                Err("injected close failure".to_string())
            });
            assert!(result.is_err());
            assert_eq!(quarantined.len(), 1);
        }
        assert_eq!(
            reconcile_quarantined_close(&mut quarantined, "session-a", |_| Ok(())),
            Ok(())
        );
        assert!(quarantined.is_empty());
    }

    #[test]
    fn post_build_restore_close_failure_moves_survivors_out_of_business_state() {
        let mut surface = surface_with_workspace("session-a");
        surface
            .workspaces
            .get_mut("session-a")
            .unwrap()
            .tabs
            .insert(test_entry(
                "1111111111111111",
                "restored-second",
                2,
                true,
                None,
            ))
            .unwrap();

        // Model a bind/navigation/persist failure after all restore WebViews
        // were built (and possibly tentatively published).
        surface.quarantine_workspace_for_failed_restore("session-a");
        assert!(!surface.has_session("session-a"));
        assert!(surface.owns_session_resources("session-a"));
        assert!(surface
            .webview_label_for_tab("session-a", "0123456789abcdef")
            .is_none());
        assert!(surface
            .webview_label_for_tab("session-a", "1111111111111111")
            .is_none());
        assert!(surface
            .quarantined_tabs
            .values()
            .all(|entry| !entry.is_published()));

        let first =
            reconcile_quarantined_close(&mut surface.quarantined_tabs, "session-a", |entry| {
                if entry.label == "restored-second" {
                    Err("injected post-build close failure".to_string())
                } else {
                    Ok(())
                }
            });
        assert!(first.is_err());
        assert_eq!(surface.quarantined_tabs.len(), 1);
        assert!(!surface.has_session("session-a"));
        assert!(surface.owns_session_resources("session-a"));

        assert_eq!(
            reconcile_quarantined_close(&mut surface.quarantined_tabs, "session-a", |_| Ok(())),
            Ok(())
        );
        assert!(!surface.owns_session_resources("session-a"));
    }

    #[test]
    fn exact_agent_staging_rollback_releases_capacity_and_checks_generation() {
        let mut surface = surface_with_workspace("session-a");
        let key = ("session-a".to_string(), "1111111111111111".to_string());
        surface.staged_tabs.insert(
            key.clone(),
            test_entry(&key.1, "staged-agent", 2, false, Some("create-a")),
        );

        assert!(surface
            .rollback_staged_agent_creation(None, "session-a", &key.1, "wrong-generation")
            .is_err());
        assert!(surface.staged_tabs.contains_key(&key));
        assert_eq!(
            surface.rollback_staged_agent_creation(None, "session-a", &key.1, "create-a"),
            Ok(true)
        );
        assert!(!surface.staged_tabs.contains_key(&key));
        assert_eq!(surface.ensure_tab_capacity("session-a"), Ok(()));
    }

    #[test]
    fn hosted_core_cancellation_revokes_operation_and_exact_staging_generation() {
        let mut surface = surface_with_workspace("session-a");
        let key = ("session-a".to_string(), "1111111111111111".to_string());
        let other = ("session-a".to_string(), "2222222222222222".to_string());
        surface.staged_tabs.insert(
            key.clone(),
            test_entry(&key.1, "staged-cancelled", 2, false, Some("request-a")),
        );
        surface.staged_tabs.insert(
            other.clone(),
            test_entry(&other.1, "staged-other", 3, false, Some("request-b")),
        );

        let control = Arc::clone(&surface.workspaces["session-a"].control);
        let (snapshot, opaque_lease) = control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();
        assert!(control.begin_agent_operation(&authorization, true));

        surface
            .cancel_in_flight_core_request(None, "session-a", "request-a")
            .unwrap();

        assert!(control.active_agent_operation().is_none());
        assert!(!control.agent_input_in_progress());
        assert!(!surface.staged_tabs.contains_key(&key));
        assert!(surface.staged_tabs.contains_key(&other));
    }

    #[test]
    fn unpublished_restore_transaction_is_not_eligible_for_snapshot() {
        let surface = surface_with_workspace("session-a");
        let workspace = &surface.workspaces["session-a"];
        assert!(workspace_restore_ready(workspace));

        workspace
            .tabs
            .by_token("0123456789abcdef")
            .unwrap()
            .unpublish();
        assert!(!workspace_restore_ready(workspace));
    }

    #[test]
    fn prepare_rollback_generation_is_invalidated_by_takeover_or_later_prepare() {
        let mut surface = surface_with_workspace("session-a");
        surface
            .record_prepare_generation("session-a", Some("prepare-a"))
            .unwrap();
        assert_eq!(
            surface.prepare_generation_revision("session-a", "prepare-a"),
            Some(1)
        );

        surface
            .workspaces
            .get("session-a")
            .unwrap()
            .control
            .bump(Some(NativeControlOwner::User));
        assert_eq!(
            surface
                .rollback_prepare_generation(None, "session-a", "prepare-a", 1, false,)
                .unwrap(),
            None
        );
        assert!(surface.has_session("session-a"));

        surface
            .record_prepare_generation("session-a", Some("prepare-b"))
            .unwrap();
        surface
            .record_prepare_generation("session-a", None)
            .unwrap();
        assert_eq!(
            surface.prepare_generation_revision("session-a", "prepare-b"),
            None
        );
    }

    #[test]
    fn newer_prepare_supersedes_late_old_tombstone_even_at_same_revision() {
        let mut surface = surface_with_workspace("session-a");
        surface
            .record_prepare_generation("session-a", Some("prepare-old"))
            .unwrap();
        let old_revision = surface
            .prepare_generation_revision("session-a", "prepare-old")
            .unwrap();

        // Existing-workspace Prepare does not need to mutate control state, so
        // request identity—not revision alone—must make the old CAS fail.
        surface
            .record_prepare_generation("session-a", Some("prepare-new"))
            .unwrap();
        assert_eq!(
            surface
                .rollback_prepare_generation(None, "session-a", "prepare-old", old_revision, false,)
                .unwrap(),
            None
        );
        assert!(surface.has_session("session-a"));
        assert_eq!(
            surface.prepare_generation_revision("session-a", "prepare-new"),
            Some(old_revision)
        );
    }

    #[test]
    fn created_blank_prepare_rollback_retry_finishes_durable_delete_after_runtime_is_gone() {
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_home = std::env::var_os("PINVOU3_HOME");
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("PINVOU3_HOME", temp.path());

        let session_id = format!(
            "missing-created-blank-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        );
        let restore_path =
            paths::browser_workspace_restore_json(&paths::browser_session_token(&session_id));
        std::fs::create_dir_all(restore_path.parent().unwrap()).unwrap();
        std::fs::write(&restore_path, b"stale-created-blank").unwrap();

        let mut surface = surface_with_workspace(&session_id);
        // Model the first rollback after native close has already succeeded:
        // the runtime workspace is gone, but its durable manifest deletion
        // fails. This boundary must remain retryable instead of ACKing early.
        surface.workspaces.remove(&session_id);
        surface.active_session = None;
        assert_eq!(
            complete_absent_prepare_rollback(false, surface.has_sessions(), || Err(
                "injected durable restore deletion failure".to_string()
            )),
            Err("injected durable restore deletion failure".to_string())
        );
        assert!(!surface.has_session(&session_id));
        assert!(restore_path.exists());

        // The repeated CreatedBlank compensation sees no runtime workspace,
        // retries only the durable deletion, and completes successfully.
        assert_eq!(
            surface
                .rollback_prepare_generation(None, &session_id, "prepare-a", 1, false)
                .unwrap(),
            Some(false)
        );
        assert!(!restore_path.exists());

        std::fs::write(&restore_path, b"restored-existing").unwrap();
        let delete_called = std::cell::Cell::new(false);
        assert_eq!(
            complete_absent_prepare_rollback(true, surface.has_sessions(), || {
                delete_called.set(true);
                Ok(())
            }),
            Ok(None)
        );
        assert!(!delete_called.get());
        assert_eq!(
            surface
                .rollback_prepare_generation(None, &session_id, "prepare-b", 1, true)
                .unwrap(),
            None
        );
        assert!(restore_path.exists());

        match previous_home {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }

    #[test]
    fn selecting_visible_workspace_clears_every_other_visibility_flag() {
        let mut surface = surface_with_workspace("session-a");
        surface.workspaces.get_mut("session-a").unwrap().visible = true;
        let token = "fedcba9876543210".to_string();
        surface.workspaces.insert(
            "session-b".to_string(),
            Workspace {
                session_token: token.clone(),
                tabs: TabRegistry::from_entry(SurfaceEntry {
                    label: "test-webview-b".to_string(),
                    token: token.clone(),
                    page_id: 2,
                    automation_target: None,
                    created_by_request_id: None,
                    published: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                    created_at_revision: None,
                }),
                active_tab: token,
                bounds: None,
                // Reproduce a stale state left by an older show implementation.
                visible: true,
                control: Arc::new(WorkspaceControl::new(1, NativeControlOwner::Agent)),
                prepare_generation: None,
            },
        );

        set_exclusive_workspace_visibility(&mut surface.workspaces, "session-b");

        assert!(!surface.workspaces["session-a"].visible);
        assert!(surface.workspaces["session-b"].visible);
        assert_eq!(
            surface
                .workspaces
                .values()
                .filter(|workspace| workspace.visible)
                .count(),
            1
        );
    }

    #[test]
    fn background_workspace_is_not_eligible_for_physical_activation() {
        assert!(!workspace_may_present_native_surface(false, true));
        assert!(!workspace_may_present_native_surface(true, false));
        assert!(workspace_may_present_native_surface(true, true));
    }

    #[test]
    fn blank_page_keeps_a_hidden_workspace_marker() {
        assert_eq!(
            marked_blank_url("about:blank", "0123456789abcdef"),
            "about:blank#pinvou-tab-0123456789abcdef"
        );
        assert_eq!(
            sanitize_marker_url(
                "about:blank#pinvou-tab-0123456789abcdef".to_string(),
                "0123456789abcdef",
            ),
            "about:blank"
        );
    }

    #[test]
    fn wkwebview_percent_encoded_marker_is_canonicalized_for_its_exact_tab() {
        for marker in [
            "about:blank%23pinvou-session-0123456789abcdef",
            "about:blank%23pinvou-tab-0123456789abcdef",
        ] {
            assert!(has_internal_marker_for_token(marker, "0123456789abcdef"));
            assert_eq!(
                sanitize_marker_url(marker.to_string(), "0123456789abcdef"),
                "about:blank"
            );
        }
    }

    #[test]
    fn wkwebview_marker_alias_cannot_claim_another_tab_or_remote_url() {
        let marker = "about:blank%23pinvou-tab-0123456789abcdef";
        assert!(!has_internal_marker_for_token(marker, "fedcba9876543210"));
        assert_eq!(
            sanitize_marker_url(marker.to_string(), "fedcba9876543210"),
            marker
        );
        for untrusted in [
            "about:blank%23pinvou-tab-0123456789abcdef-extra",
            "about:blank%23pinvou-tab-0123456789abcdef?query=1",
            "https://example.com/%23pinvou-tab-0123456789abcdef",
        ] {
            assert!(!has_internal_marker_for_token(
                untrusted,
                "0123456789abcdef"
            ));
        }
    }

    #[test]
    fn normal_page_url_is_not_modified() {
        assert_eq!(
            marked_blank_url("https://example.com/path#section", "0123456789abcdef"),
            "https://example.com/path#section"
        );
    }

    #[test]
    fn takeover_script_exposes_only_a_trusted_low_privilege_signal() {
        let script = browser_initialization_script(Some("0123456789abcdef"));
        assert!(script.contains("event.isTrusted"));
        assert!(script.contains("queueMicrotask.bind"));
        assert!(script.contains("pinvou-user-takeover://interaction/"));
        let global_debounce_marker = ["last", "SignalAt"].concat();
        assert!(!script.contains(&global_debounce_marker));
        assert!(script.contains("about:blank#pinvou-session-0123456789abcdef"));
        assert!(script.contains("about:blank#pinvou-tab-0123456789abcdef"));
        assert!(script.contains("Object.defineProperty"));
        assert!(script.contains("enumerable: false"));
        assert!(!script.contains("globalThis.__PINVOU_BROWSER_BOOTSTRAP_TOKEN__ ="));
        assert!(!script.contains("PINVOU_BROWSER_TAB_TOKEN"));
        assert!(!script.contains("__TAURI__"));
        assert!(!script.contains("invoke("));
    }

    #[test]
    fn browser_core_page_script_contains_no_task_or_tab_identity() {
        let script = browser_initialization_script(None);
        assert!(!script.contains("0123456789abcdef"));
        assert!(!script.contains("PINVOU_BROWSER_BOOTSTRAP_TOKEN"));
        assert!(!script.contains("session_token"));
        assert!(!script.contains("tab_token"));
        // BrowserCore's documentation may name the host-side lease boundary, but the
        // injected page runtime must never contain a lease field or page-visible handle.
        assert!(!script.contains("__PINVOU_BROWSER_LEASE"));
        assert!(!script.contains("\"lease\":"));
    }

    #[test]
    fn only_exact_about_blank_markers_are_internal() {
        for marker in [
            "about:blank#pinvou-session-0123456789abcdef",
            "about:blank#pinvou-tab-0123456789abcdef",
            "about:blank%23pinvou-session-0123456789abcdef",
            "about:blank%23pinvou-tab-0123456789abcdef",
        ] {
            assert!(has_internal_marker_for_token(marker, "0123456789abcdef"));
        }
        assert!(!has_internal_marker_for_token(
            "https://example.com/#pinvou-tab-0123456789abcdef",
            "0123456789abcdef"
        ));
        assert!(!has_internal_marker_for_token(
            "about:blank#pinvou-tab-0123456789abcdef-extra",
            "0123456789abcdef"
        ));
        assert!(!super::super::super::is_allowed_url(
            "about:blank%23pinvou-tab-0123456789abcdef"
        ));
        assert!(parse_restore_workspace(
            br#"{"version":1,"active_index":0,"tabs":[{"url":"about:blank%23pinvou-tab-0123456789abcdef"}]}"#,
        )
        .is_err());
    }

    #[test]
    fn restore_manifest_accepts_only_urls_order_and_active_index() {
        let restore = parse_restore_workspace(
            br#"{"version":1,"active_index":1,"tabs":[{"url":"https://example.com/a"},{"url":"about:blank"}]}"#,
        )
        .unwrap();
        assert_eq!(
            restore,
            NativeWorkspaceRestore {
                urls: vec![
                    "https://example.com/a".to_string(),
                    "about:blank".to_string()
                ],
                active_index: 1,
            }
        );
        assert!(parse_restore_workspace(
            br#"{"version":1,"active_index":0,"target_id":"old","tabs":[{"url":"https://example.com"}]}"#,
        )
        .is_err());
        assert!(parse_restore_workspace(
            br#"{"version":1,"active_index":0,"tabs":[{"url":"file:///secret"}]}"#,
        )
        .is_err());
    }

    #[test]
    fn restore_writer_never_serializes_runtime_identity() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("restore.json");
        write_restore_workspace_file(
            &path,
            &NativeWorkspaceRestore {
                urls: vec!["https://example.com/path?q=1".to_string()],
                active_index: 0,
            },
        )
        .unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(value["version"], WORKSPACE_RESTORE_VERSION);
        assert_eq!(value["active_index"], 0);
        assert_eq!(value["tabs"][0]["url"], "https://example.com/path?q=1");
        let encoded = value.to_string();
        for forbidden in ["target_id", "lease", "session_token", "tab_token"] {
            assert!(!encoded.contains(forbidden));
        }
    }

    #[test]
    fn takeover_navigation_accepts_only_the_reserved_scheme_and_route() {
        let pointer = "pinvou-user-takeover://interaction/pointerdown"
            .parse::<tauri::Url>()
            .unwrap();
        let unrelated = "https://example.com/interaction/pointerdown"
            .parse::<tauri::Url>()
            .unwrap();
        let unknown = "pinvou-user-takeover://interaction/click"
            .parse::<tauri::Url>()
            .unwrap();
        assert_eq!(user_takeover_interaction(&pointer), Some("pointerdown"));
        assert_eq!(user_takeover_interaction(&unrelated), None);
        assert_eq!(user_takeover_interaction(&unknown), None);
    }

    #[test]
    fn assert_lease_checks_host_target_revision_and_owner() {
        let mut surface = surface_with_workspace("session-a");
        let workspace = surface.workspaces.get_mut("session-a").unwrap();
        workspace
            .tabs
            .bind_target("0123456789abcdef", "target-a")
            .unwrap();
        let (snapshot, opaque_lease) = workspace.control.issue_agent_lease();
        let lease = NativeTabLease {
            session_id: "session-a".to_string(),
            tab_token: "0123456789abcdef".to_string(),
            target_id: "target-a".to_string(),
            revision: snapshot.revision,
            owner: NativeControlOwner::Agent,
            lease: opaque_lease,
        };

        assert!(surface.assert_lease(&lease).unwrap());
        let mut wrong_target = lease.clone();
        wrong_target.target_id = "target-b".to_string();
        assert!(!surface.assert_lease(&wrong_target).unwrap());
        let mut forged = lease.clone();
        forged.lease = "00000000000000000000000000000000".to_string();
        assert!(!surface.assert_lease(&forged).unwrap());
        surface
            .workspaces
            .get("session-a")
            .unwrap()
            .control
            .bump(Some(NativeControlOwner::User));
        assert!(!surface.assert_lease(&lease).unwrap());
    }

    #[test]
    fn popup_inside_begun_agent_dispatch_captures_complete_lease() {
        let mut surface = surface_with_workspace("session-a");
        let workspace = surface.workspaces.get_mut("session-a").unwrap();
        workspace
            .tabs
            .bind_target("0123456789abcdef", "target-a")
            .unwrap();
        let (snapshot, opaque_lease) = workspace.control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();
        let epoch = AgentCallerEpoch::new(41, "0123456789abcdef0123456789abcdef").unwrap();
        assert!(workspace.control.begin_agent_operation_for_caller(
            &authorization,
            false,
            epoch.clone()
        ));

        let retained =
            popup_agent_authorization(&workspace.control, "session-a", "0123456789abcdef")
                .expect("popup must retain the begun operation");
        assert_eq!(retained.authorization(), &authorization);
        assert_eq!(retained.caller_epoch(), &epoch);
        workspace
            .control
            .release_retained_agent_operation(&retained);
        workspace.control.end_agent_operation(&authorization);
    }

    #[test]
    fn popup_without_valid_dispatch_is_user_owned() {
        let mut surface = surface_with_workspace("session-a");
        let workspace = surface.workspaces.get_mut("session-a").unwrap();
        workspace
            .tabs
            .bind_target("0123456789abcdef", "target-a")
            .unwrap();
        let (snapshot, opaque_lease) = workspace.control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .unwrap();

        assert!(
            popup_agent_authorization(&workspace.control, "session-a", "0123456789abcdef")
                .is_none()
        );
        assert!(workspace.control.begin_agent_operation_for_caller(
            &authorization,
            true,
            AgentCallerEpoch::new(41, "0123456789abcdef0123456789abcdef").unwrap()
        ));
        workspace.control.bump(Some(NativeControlOwner::User));
        assert!(
            popup_agent_authorization(&workspace.control, "session-a", "0123456789abcdef")
                .is_none()
        );
    }

    #[test]
    fn target_binding_is_unique_across_workspaces() {
        let mut surface = surface_with_workspace("session-a");
        surface
            .workspaces
            .get_mut("session-a")
            .unwrap()
            .tabs
            .bind_target("0123456789abcdef", "target-a")
            .unwrap();
        let token = "fedcba9876543210".to_string();
        surface.workspaces.insert(
            "session-b".to_string(),
            Workspace {
                session_token: token.clone(),
                tabs: TabRegistry::from_entry(SurfaceEntry {
                    label: "test-webview-b".to_string(),
                    token: token.clone(),
                    page_id: 2,
                    automation_target: None,
                    created_by_request_id: None,
                    published: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                    created_at_revision: None,
                }),
                active_tab: token,
                bounds: None,
                visible: false,
                control: Arc::new(WorkspaceControl::new(1, NativeControlOwner::Agent)),
                prepare_generation: None,
            },
        );

        assert_eq!(
            surface.tab_for_target("target-a"),
            Some(("session-a".to_string(), "0123456789abcdef".to_string()))
        );
        assert!(surface
            .bind_target("session-b", "fedcba9876543210", "target-a")
            .is_err());
    }

    #[test]
    fn workspace_schema_persists_revision_and_authoritative_target_binding() {
        let mut surface = surface_with_workspace("session-a");
        let workspace = surface.workspaces.get_mut("session-a").unwrap();
        workspace
            .tabs
            .bind_target("0123456789abcdef", "target-a")
            .unwrap();
        workspace.control.bump(Some(NativeControlOwner::Agent));

        let value = workspace_state_value(workspace);
        assert_eq!(value["version"], 2);
        assert_eq!(value["mapping_authority"], "host");
        assert_eq!(value["revision"], 2);
        assert_eq!(value["tabs"][0]["token"], "0123456789abcdef");
        assert_eq!(value["tabs"][0]["target_id"], "target-a");
    }
}
