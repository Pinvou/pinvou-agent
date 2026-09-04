//! Pinvou Agent Tauri 后端入口。

mod app;
mod core;
pub mod features;
pub mod platform;

// The Windows unit-test harness imports TaskDialogIndirect. Link the same
// Common Controls v6 manifest resource as the Tauri executable so the loader
// can start the harness before any Rust test code runs.
#[cfg(all(test, target_os = "windows", target_env = "msvc"))]
#[link(
    name = "pinvou3_lib_test_resource",
    kind = "static",
    modifiers = "+whole-archive"
)]
unsafe extern "C" {}

pub use features::assistant::attachments::{
    build_message_with_attachments, stage_file_in_workspace,
};

use tauri::Manager;

#[cfg(feature = "benchmark-hooks")]
pub use features::assistant::product_runtime::{agentic_task, headless_bridge};

use crate::app::commands;
use crate::features::{
    assistant::{engine_pool::EnginePool, platform::bridge},
    connectors::connector_cli,
    files::file_watcher,
    knowledge,
    monitor::MonitorState,
    pet::{pet_window, selected_pet},
    remote_control::RemoteControlManager,
    remote_knowledge::RemoteKnowledgeService,
    scheduled::tasks as scheduled_tasks,
    sessions::SessionStore,
};
use crate::platform::{notifications, startup};

const RELEASE_ENV_DEFAULTS: &[(&str, &str)] = &[
    // —— vLLM 后端：BASE_URL/MODEL/API_KEY 已在 bridge/mod.rs 有默认常量，
    // 这里只补 run-dev.sh 额外注入但 Rust 没默认的 ——
    // ⚠️ 不再注入 DEEPSEEK_PROVIDER：它会被 bridge.provider() 当成 env 覆盖
    //   （env 优先级高于 preset），在「添加模型」多 provider 方案下钉死路由——
    //   切到 kimi/openai/qwen 等仍被当 vllm，且设置页误报「环境变量已锁定 provider」。
    //   provider 现由 active_model.preset 决定（LocalVllm→vllm 默认仍成立）。
    ("DEEPSEEK_ALLOW_INSECURE_HTTP", "1"),
    ("DEEPSEEK_FORCE_HTTP1", "1"),
    // 不再注入 DEEPSEEK_MAX_OUTPUT_TOKENS：它会把所有模型（含云端）的输出上限
    // 钉死在 24576。底座对 ≥500K 窗口模型默认 64K（API_MAX_OUTPUT_TOKENS），
    // 云端模型应落到底座兜底；本地 vLLM 的 24K 预算由 route_limits_for_model
    // 的 is_local_vllm 分支显式携带，不依赖该 env。
    // 与 CodeWhale 的 stream_chunk_timeout 默认值保持一致。
    ("DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS", "300"),
    // SSE 首响应头超时(open timeout):底座只认 env,默认 45s 是为云端调的。
    // 本地 GB10 大上下文 SubAgent 请求首 token TTFT 偶发 >45s → 误杀子 agent。
    // 280s 与
    // ~/.deepseek config 的 subagent api_timeout=300 对齐(步级超时须更大)。
    ("DEEPSEEK_STREAM_OPEN_TIMEOUT_SECS", "280"),
];

/// 为 release 安装包（.deb 双击启动场景）注入 run-dev.sh 里集中处理的运行时 env。
/// dev 启动走 run-dev.sh 已经 export 过的不会被覆盖（var_os().is_none() 守门）。
fn ensure_release_env() {
    use std::env;
    crate::platform::ui_cache::configure_runtime_environment();
    for (k, v) in RELEASE_ENV_DEFAULTS {
        if env::var_os(k).is_none() {
            // SAFETY: ensure_release_env is only called by the first line of
            // run() and by the headless benchmark host startup sequence
            // (headless_bridge.rs run_headless_host, before the tokio runtime
            // is built); at both call sites the process has only the main
            // thread — no concurrent env readers (audit conclusion: the first
            // thread spawn happens in the lib.rs setup phase, after this
            // function); calls inside tests run under ENV_LOCK serialization.
            unsafe { env::set_var(k, v) };
        }
    }

    {
        if let Some(old) = env::var_os("PATH") {
            let mut dirs = Vec::new();
            if let Some(connector_bin) = crate::platform::paths::managed_connector_bin_dir() {
                // 旧布局（过渡期保留：未迁移的存量二进制还能按名解析）
                dirs.push(connector_bin);
            }
            // 版本化 CLI 资产目录进 PATH（按 lock 表当前版本逐个登记）
            for (name, pin) in crate::platform::connector_lock::all_artifact_pins() {
                dirs.push(crate::platform::paths::assets_cli_dir(&name, &pin.version));
            }
            if let Ok(prefix) = env::var("NPM_CONFIG_PREFIX") {
                dirs.push(std::path::Path::new(&prefix).join("bin"));
            }
            if let Some(home) = env::var_os("HOME") {
                let home = std::path::Path::new(&home);
                dirs.push(home.join(".npm-global").join("bin"));
                dirs.push(home.join(".local").join("bin"));
            }
            #[cfg(target_os = "macos")]
            {
                dirs.push(std::path::PathBuf::from("/opt/homebrew/bin"));
                dirs.push(std::path::PathBuf::from("/usr/local/bin"));
                dirs.push(std::path::PathBuf::from(
                    "/Applications/LibreOffice.app/Contents/MacOS",
                ));
            }
            dirs.extend(env::split_paths(&old));
            if let Ok(joined) = env::join_paths(dirs) {
                // SAFETY: same as above; run() single-threaded startup, no concurrent env readers.
                unsafe { env::set_var("PATH", joined) };
            }
        }
    }
}

/// 进程级选定并安装 rustls 的 CryptoProvider(aws-lc-rs)。
///
/// 幂等:`install_default` 返回 `Err` 表示已装过(本次或之前的 reqwest/tungstenite 调用),
/// 用 `drop` 吞掉即可,不会二次注册。装在 `run()` 最前,保证后续 reqwest / relay 的
/// `connect_async` 都用已确定的 provider,避开「aws-lc-rs + ring 同时启用时无参
/// ClientConfig::builder().expect() panic」(见 Cargo.toml rustls 注释)。
fn install_rustls_provider() {
    drop(rustls::crypto::aws_lc_rs::default_provider().install_default());
}

const SESSION_BROWSER_CLEANUP_INITIAL_RETRY: std::time::Duration =
    std::time::Duration::from_millis(250);
const SESSION_BROWSER_CLEANUP_MAX_RETRY: std::time::Duration = std::time::Duration::from_secs(30);
const SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY: usize = 8;
const SESSION_BROWSER_CLEANUP_ATTEMPT_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(30);

#[derive(Debug)]
struct SessionBrowserCleanupSchedule {
    ready_at: std::time::Instant,
    retry_delay: std::time::Duration,
    failed_attempts: u64,
    sequence: u64,
}

#[derive(Debug)]
struct SessionBrowserCleanupClaim {
    session_id: String,
    retry_delay: std::time::Duration,
    failed_attempts: u64,
}

#[derive(Default)]
struct SessionBrowserCleanupQueueState {
    pending: std::collections::HashMap<String, SessionBrowserCleanupSchedule>,
    in_flight: std::collections::HashSet<String>,
    rerun_requested: std::collections::HashSet<String>,
    next_sequence: u64,
}

#[derive(Default)]
struct SessionBrowserCleanupQueue {
    state: std::sync::Mutex<SessionBrowserCleanupQueueState>,
    notify: tokio::sync::Notify,
}

impl SessionBrowserCleanupQueue {
    fn enqueue(&self, session_id: &str) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.pending.contains_key(session_id) {
            return;
        }
        if state.in_flight.contains(session_id) {
            // A durable deletion notification can race with a successful
            // cleanup attempt just before it clears the browser deny marker.
            // Preserve one rerun without creating another concurrent attempt.
            state.rerun_requested.insert(session_id.to_string());
            return;
        }
        let sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.saturating_add(1);
        let inserted = state
            .pending
            .insert(
                session_id.to_string(),
                SessionBrowserCleanupSchedule {
                    ready_at: std::time::Instant::now(),
                    retry_delay: SESSION_BROWSER_CLEANUP_INITIAL_RETRY,
                    failed_attempts: 0,
                    sequence,
                },
            )
            .is_none();
        drop(state);
        if inserted {
            self.notify.notify_one();
        }
    }

    fn claim_ready_up_to(
        &self,
        now: std::time::Instant,
        limit: usize,
    ) -> Vec<SessionBrowserCleanupClaim> {
        if limit == 0 {
            return Vec::new();
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut claimed = Vec::new();
        while claimed.len() < limit {
            // Prefer never-attempted obligations over retries, then preserve
            // FIFO order within the same retry generation. A continuously
            // failing task therefore cannot starve a later unique deletion.
            let selected = state
                .pending
                .iter()
                .filter(|(_, schedule)| schedule.ready_at <= now)
                .min_by(|(left_id, left), (right_id, right)| {
                    left.failed_attempts
                        .cmp(&right.failed_attempts)
                        .then_with(|| left.ready_at.cmp(&right.ready_at))
                        .then_with(|| left.sequence.cmp(&right.sequence))
                        .then_with(|| left_id.cmp(right_id))
                })
                .map(|(session_id, _)| session_id.clone());
            let Some(session_id) = selected else {
                break;
            };
            // session_id was selected from state.pending above under the same
            // lock, so the entry must still be present.
            #[allow(clippy::expect_used)]
            let schedule = state
                .pending
                .remove(&session_id)
                .expect("selected cleanup schedule must remain pending");
            let inserted = state.in_flight.insert(session_id.clone());
            debug_assert!(inserted, "cleanup queue must claim each id only once");
            claimed.push(SessionBrowserCleanupClaim {
                session_id,
                retry_delay: schedule.retry_delay,
                failed_attempts: schedule.failed_attempts,
            });
        }
        claimed
    }

    fn complete_attempt(
        &self,
        claim: SessionBrowserCleanupClaim,
        succeeded: bool,
        now: std::time::Instant,
    ) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let removed = state.in_flight.remove(&claim.session_id);
        debug_assert!(removed, "completed cleanup must own an active claim");
        let rerun_requested = state.rerun_requested.remove(&claim.session_id);
        if succeeded && !rerun_requested {
            return;
        }

        let sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.saturating_add(1);
        let schedule = if succeeded {
            SessionBrowserCleanupSchedule {
                ready_at: now,
                retry_delay: SESSION_BROWSER_CLEANUP_INITIAL_RETRY,
                failed_attempts: 0,
                sequence,
            }
        } else {
            SessionBrowserCleanupSchedule {
                ready_at: now + claim.retry_delay,
                retry_delay: next_session_browser_cleanup_retry(claim.retry_delay),
                failed_attempts: claim.failed_attempts.saturating_add(1),
                sequence,
            }
        };
        state.pending.insert(claim.session_id, schedule);
    }

    fn next_ready_delay(&self, now: std::time::Instant) -> Option<std::time::Duration> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .values()
            .map(|schedule| schedule.ready_at.saturating_duration_since(now))
            .min()
    }

    #[cfg(test)]
    fn pending_len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .len()
    }

    #[cfg(test)]
    fn in_flight_len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .in_flight
            .len()
    }
}

fn next_session_browser_cleanup_retry(current: std::time::Duration) -> std::time::Duration {
    current
        .saturating_mul(2)
        .min(SESSION_BROWSER_CLEANUP_MAX_RETRY)
}

fn session_browser_cleanup_failure_kind(error: &str) -> &'static str {
    if error == "browser manager is unavailable" {
        "browser-manager-unavailable"
    } else if error.starts_with("browser cleanup attempt timed out") {
        "timeout"
    } else {
        "cleanup-error"
    }
}

async fn cleanup_deleted_session_browser_once(
    app: tauri::AppHandle,
    claim: SessionBrowserCleanupClaim,
) -> (SessionBrowserCleanupClaim, bool) {
    let result = match tokio::time::timeout(SESSION_BROWSER_CLEANUP_ATTEMPT_TIMEOUT, async {
        match app.try_state::<features::browser::BrowserManager>() {
            Some(browser) => browser.delete_for_session(&claim.session_id).await,
            None => Err("browser manager is unavailable".to_string()),
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Err(format!(
            "browser cleanup attempt timed out after {} seconds",
            SESSION_BROWSER_CLEANUP_ATTEMPT_TIMEOUT.as_secs()
        )),
    };
    if let Err(error) = &result {
        let attempt = claim.failed_attempts.saturating_add(1);
        let failure_kind = session_browser_cleanup_failure_kind(error);
        eprintln!(
            "[sessions] browser cleanup failed for a deleted session \
             (reason {failure_kind}, attempt {attempt}); retrying in {} ms",
            claim.retry_delay.as_millis()
        );
    }
    (claim, result.is_ok())
}

async fn reconcile_orphaned_session_browser_files_until_success(
    app: tauri::AppHandle,
    store: SessionStore,
) {
    let mut retry_delay = SESSION_BROWSER_CLEANUP_INITIAL_RETRY;
    let mut attempt = 1_u64;

    loop {
        let result = store
            .list()
            .and_then(|mut sessions| {
                sessions.extend(store.list_scheduled()?);
                Ok(sessions)
            })
            .map(|sessions| {
                sessions
                    .into_iter()
                    .map(|session| session.id)
                    .collect::<std::collections::HashSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
            })
            .map_err(|error| format!("failed to read all existing task records: {error}"))
            .and_then(|active_session_ids| {
                app.try_state::<features::browser::BrowserManager>()
                    .ok_or_else(|| "browser manager is unavailable".to_string())?
                    .reconcile_session_files(&active_session_ids)
            });

        match result {
            Ok(()) => return,
            Err(error) => {
                eprintln!(
                    "[sessions] startup browser orphan reconciliation failed \
                     (attempt {attempt}); retrying in {} ms: {error}",
                    retry_delay.as_millis()
                );
                tokio::time::sleep(retry_delay).await;
                retry_delay = next_session_browser_cleanup_retry(retry_delay);
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

/// Wire the feature-neutral SessionStore lifecycle seam to browser teardown at
/// the application composition root. Sessions never depend on browser code;
/// other per-session resources can subscribe to the same seam later.
///
/// There is deliberately no process-lifetime deletion replay. Store boot can
/// prune sessions before this hook is registered, but no WebView from a prior
/// process survives; `spawn_orphaned_session_browser_reconciliation` scans and
/// retries removal of their durable browser artifacts immediately afterwards.
fn spawn_deleted_session_browser_cleanup(app: tauri::AppHandle, store: SessionStore) {
    let deletion_queue = std::sync::Arc::new(SessionBrowserCleanupQueue::default());
    let app_for_hook = app.clone();
    let queue_for_hook = deletion_queue.clone();
    store.register_session_deleted_hook(std::sync::Arc::new(move |session_id: &str| {
        // Deny late wrapper work synchronously at the durable deletion commit
        // point. WebView/file teardown is fallible and stays in the async retry
        // worker below so the session deletion path never blocks on browser I/O.
        if let Some(browser) = app_for_hook.try_state::<features::browser::BrowserManager>() {
            browser.mark_session_deleted(session_id);
        }
        // The obligation map coalesces repeated idempotent notifications.
        // Notify retains a permit across the claim-to-wait race, so no durable
        // cleanup request can be lost.
        queue_for_hook.enqueue(session_id);
    }));

    tauri::async_runtime::spawn(async move {
        use futures_util::{StreamExt, stream::FuturesUnordered};

        let mut cleanups = FuturesUnordered::new();

        loop {
            let available = SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY.saturating_sub(cleanups.len());
            for claim in deletion_queue.claim_ready_up_to(std::time::Instant::now(), available) {
                cleanups.push(cleanup_deleted_session_browser_once(app.clone(), claim));
            }

            // At capacity, notifications cannot open another slot. Await one
            // bounded attempt directly so a hot producer cannot starve
            // completions by repeatedly waking the select branch.
            if cleanups.len() >= SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY {
                if let Some((claim, succeeded)) = cleanups.next().await {
                    deletion_queue.complete_attempt(claim, succeeded, std::time::Instant::now());
                }
                continue;
            }

            let retry_delay = deletion_queue.next_ready_delay(std::time::Instant::now());
            let retry_ready = async move {
                match retry_delay {
                    Some(delay) => tokio::time::sleep(delay).await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::pin!(retry_ready);
            tokio::select! {
                _ = deletion_queue.notify.notified() => {}
                _ = &mut retry_ready => {}
                Some((claim, succeeded)) = cleanups.next(), if !cleanups.is_empty() => {
                    deletion_queue.complete_attempt(
                        claim,
                        succeeded,
                        std::time::Instant::now(),
                    );
                }
            }
        }
    });
}

fn spawn_orphaned_session_browser_reconciliation(app: tauri::AppHandle, store: SessionStore) {
    tauri::async_runtime::spawn(reconcile_orphaned_session_browser_files_until_success(
        app, store,
    ));
}

/// Funnel for process env writes inside the single-threaded startup window
/// (besides the release defaults of `ensure_release_env`).
///
/// Since edition 2024, `set_var` is unsafe and requires no concurrent env
/// readers or writers. This function is called only from the head region of
/// `run()` and from the headless benchmark host startup sequence
/// (`headless_bridge.rs run_headless_host`, before the tokio runtime is
/// built); at both call sites the process has only the main thread (the same
/// proven window as `ensure_release_env`). From the multi-threaded phase (the
/// Tauri setup onward) process env writes are forbidden entirely: MCP secrets
/// travel via keyring + in-process registry through the foundation resolver
/// hook, so the runtime no longer needs any env writes.
///
/// What it writes:
/// - `PINVOU3_SESSION_ARTIFACTS`: the binary artifacts landing dir for MCP
///   stdio subprocesses such as PPT / official-document tools (fixed to
///   `sessions/default/artifacts/`; a stdio server cannot reliably perceive
///   the current GUI session — concrete ownership is decided by tool events
///   carrying `session_id` and by frontend persistence).
/// - OS-specific startup writes (`platform::os::startup_platform_env`): on
///   Windows, configure the ONNX Runtime dylib path and prepend LibreOffice
///   to `PATH`; no-op on other platforms.
pub(crate) fn startup_process_env() {
    // SAFETY: single-threaded startup window (see the function docs); no
    // concurrent env readers.
    unsafe {
        std::env::set_var(
            "PINVOU3_SESSION_ARTIFACTS",
            crate::platform::paths::default_session_artifacts_dir(),
        )
    };
    crate::platform::os::startup_platform_env();
}

/// `iframe[srcdoc]` 在 WebKitGTK 中会作为宿主 WebView 的 `about:srcdoc` 导航
/// 进入 Wry 的 navigation handler。这里只放行浏览器内部的两个空文档地址；
/// 主窗口和 iframe 的任意外部来源仍走初始 origin 限制。
#[cfg(any(target_os = "linux", test))]
fn allow_embedded_document_navigation(url: &tauri::Url, main_origin_initialized: bool) -> bool {
    main_origin_initialized && matches!(url.as_str(), "about:blank" | "about:srcdoc")
}

/// Non-blocking synchronous fallback for `RunEvent::Exit`. Explicit `app.restart()`
/// bypasses this event and uses `prepare_app_restart()` to await the complete
/// `shutdown_before_restart()` barrier.
fn shutdown_browser_before_process_end(app: &tauri::AppHandle) {
    if let Some(browser) = app.try_state::<features::browser::BrowserManager>() {
        browser.shutdown_on_exit();
    }
}

/// Shared asynchronous child-process harvesting before exit and restart. Managed
/// state is not guaranteed to be dropped (kill_on_drop runs only when Child drops),
/// so explicitly stop ACP/connector children to prevent orphans. Each closure is
/// idempotent; even shutdown(), the highest timeout risk, only sends a oneshot and
/// kills without a long wait.
pub(crate) async fn harvest_child_processes(app: &tauri::AppHandle) {
    if let Some(acp_pool) = app.try_state::<crate::features::codex_acp::AcpPool>() {
        acp_pool.shutdown_all().await;
    }
    if let Some(connector_conn) =
        app.try_state::<crate::features::connectors::connector_cli::ConnectorConn>()
    {
        connector_conn.kill_all_pids();
    }
}

/// Every `app.restart()` call must pass through this funnel. The browser first saves
/// restore manifests and destroys native surfaces, then asynchronous children are
/// harvested. The caller invokes the non-returning restart immediately afterward.
pub(crate) async fn prepare_app_restart(app: &tauri::AppHandle) {
    if let Some(browser) = app.try_state::<features::browser::BrowserManager>() {
        browser.shutdown_before_restart().await;
    }
    harvest_child_processes(app).await;
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// 全 crate 唯一的 `generate_context!()` 展开点。
///
/// macOS 的 embed_plist 用 `#[no_mangle] static _EMBED_INFO_PLIST` 让重复
/// 展开成为链接错误;GUI(`run`)与 headless 评测宿主(benchmark-hooks)必须
/// 共用这里的单一 Context 构造,不得在别处再展开该宏。
// The generate_context! macro expansion internally calls process::exit (a
// missing context means the app cannot start); this is Tauri macro behavior,
// not a code path of this repo, so it is exempted.
#[allow(clippy::exit)]
pub fn build_tauri_context() -> tauri::Context {
    tauri::generate_context!()
}

pub fn run() {
    // WebKitGTK reads its RemoteInspector endpoint while constructing the first
    // WebContext, so the browser feature must reserve it before Tauri starts.
    features::browser::prepare_process_environment();
    // 必须最先执行:进程级选定 rustls CryptoProvider。
    // 见 Cargo.toml 的 rustls/reqwest 注释——reqwest 0.13 自带 aws-lc-rs 但只「借用」
    // provider,不写入默认槽;而 oauth2(经 reqwest 0.12)把 rustls 0.23 的 `ring` feature
    // 也拉进图,使 rustls 同时启用 aws-lc-rs + ring 两个 provider。tokio-tungstenite 0.30
    // 的 TLS 连接器走无参 ClientConfig::builder(),两 provider 共存时会 panic。此处选定
    // aws-lc-rs(FIPS 可候选、性能优于 ring),在任何 TLS 连接之前装好。
    install_rustls_provider();
    ensure_release_env();
    startup_process_env();
    startup::init();
    startup::mark("environment:ready");
    // 必须早于 Tauri Builder/WebView 创建：避免升级后 WebKit 复用旧 index.html，
    // 却在新包内找不到旧 CSS，退化成裸 HTML 页面。
    crate::platform::ui_cache::migrate_before_webview();
    let initial_navigation_reported =
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let main_navigation_origin = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let builder = tauri::Builder::default()
        // These no-op probe plugins bracket Tauri's own plugin initialization.
        // The main window is created by Tauri before the application setup hook,
        // so their lifecycle hooks expose time that was previously one opaque gap.
        .plugin({
            let initial_navigation_reported = initial_navigation_reported.clone();
            let main_navigation_origin = main_navigation_origin.clone();
            tauri::plugin::Builder::<_, ()>::new("startup-probe-runtime")
                .setup(|_app, _api| {
                    startup::mark("tauri:runtime_created");
                    Ok(())
                })
                .on_window_ready(|window| {
                    if window.label() == "main" {
                        startup::mark("tauri:main_window_ready");
                    }
                })
                .on_webview_ready(|webview| {
                    if webview.label() == "main" {
                        startup::mark("tauri:main_webview_ready");
                    }
                })
                .on_navigation(move |webview, url| {
                    use std::sync::atomic::Ordering;

                    if webview.label() != "main" {
                        return true;
                    }
                    #[cfg(target_os = "linux")]
                    if matches!(url.as_str(), "about:blank" | "about:srcdoc") {
                        let main_origin_initialized = main_navigation_origin
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .is_some();
                        return allow_embedded_document_navigation(url, main_origin_initialized);
                    }
                    let origin = format!(
                        "{}://{}:{}",
                        url.scheme(),
                        url.host_str().unwrap_or_default(),
                        url.port_or_known_default()
                            .map_or_else(|| "-".to_string(), |port| port.to_string())
                    );
                    if !initial_navigation_reported.swap(true, Ordering::Relaxed) {
                        startup::mark_with_detail(
                            "rust",
                            "tauri:main_navigation",
                            &format!("scheme={}", url.scheme()),
                        );
                    }
                    let mut initial_origin = main_navigation_origin
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match initial_origin.as_ref() {
                        None => {
                            *initial_origin = Some(origin);
                            true
                        }
                        Some(allowed) if allowed == &origin => true,
                        Some(_) => {
                            startup::mark_with_detail(
                                "rust",
                                "tauri:main_navigation_blocked",
                                &format!(
                                    "scheme={} host={} port={}",
                                    url.scheme(),
                                    url.host_str().unwrap_or_default(),
                                    url.port_or_known_default()
                                        .map_or_else(|| "-".to_string(), |port| port.to_string())
                                ),
                            );
                            false
                        }
                    }
                })
                .build()
        })
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                crate::platform::window_startup::activate_main_window(window);
            }
        }))
        .plugin(
            tauri::plugin::Builder::<_, ()>::new("startup-probe-single-instance")
                .setup(|_app, _api| {
                    startup::mark("tauri:plugin_single_instance_ready");
                    Ok(())
                })
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .plugin(
            tauri::plugin::Builder::<_, ()>::new("startup-probe-notification")
                .setup(|_app, _api| {
                    startup::mark("tauri:plugin_notification_ready");
                    Ok(())
                })
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri::plugin::Builder::<_, ()>::new("startup-probe-dialog")
                .setup(|_app, _api| {
                    startup::mark("tauri:plugin_dialog_ready");
                    Ok(())
                })
                .build(),
        )
        .on_page_load(|webview, payload| {
            startup::mark_with_detail(
                "rust",
                "webview:page_load",
                &format!("label={} event={:?}", webview.label(), payload.event()),
            );
        })
        .setup(|app| {
            startup::mark("setup:start");
            if let Ok(resource_dir) = app.path().resource_dir() {
                crate::platform::paths::set_runtime_resource_dir(resource_dir);
            }
            let _ = std::thread::Builder::new()
                .name("draft-attachment-sweep".to_string())
                .spawn(|| {
                    features::files::attachment_upload::sweep_stale_draft_attachments();
                });
            features::browser::install_automation_context(app);
            #[cfg(target_os = "macos")]
            features::updater::cleanup_stale_backup();
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            startup::mark("setup:plugins_ready");
            crate::platform::window_startup::arm_hidden_main_window_fallback(app.handle());

            // Linux webview(webkit2gtk)默认拒绝 getUserMedia,语音输入点麦克风会被拒。
            // 给 main 窗口 webview 挂 permission-request:只放行 UserMedia(麦克风/摄像头)
            // 请求,定位/通知等其余权限仍按默认拒绝。Windows/macOS 的 WebView2/WKWebView
            // 自带系统级麦克风授权,不走这条。
            #[cfg(target_os = "linux")]
            {
                use tauri::Manager;
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.with_webview(|webview| {
                        use webkit2gtk::glib::prelude::ObjectExt;
                        use webkit2gtk::{PermissionRequestExt, WebViewExt};
                        let wv = webview.inner();
                        wv.connect_permission_request(|_wv, req| {
                            if req.type_().name() == "WebKitUserMediaPermissionRequest" {
                                req.allow();
                                true
                            } else {
                                false
                            }
                        });
                    });
                }
            }

            // 必须早于 SessionStore boot：归档退役宿主并清除绑定，避免内存态与磁盘态打架。
            startup::mark("retired_features:start");
            if let Err(error) = crate::features::retirement::retire_removed_features() {
                eprintln!("[pinvou3-app] retired feature cleanup failed (will retry next boot): {error}");
                startup::mark_with_detail("rust", "retired_features:error", &error);
            }
            startup::mark("retired_features:done");

            // 多对话历史 store：用 ~/.pinvou3/sessions/ 隔离 deepseek-tui 全局目录。
            // 必须先 boot 这个，engine forwarder 需要它跟踪 active session 的 mode_state
            // 以便 TurnComplete 时判定是否 emit chat:plan_ready。
            startup::mark("session_store:start");
            let session_store = match SessionStore::boot_for_process_startup() {
                Ok(store) => {
                    // sidecar(per-session 模型 / 置顶 / 隐藏)已由 SessionStore::boot_inner
                    // 在 boot 时统一加载,setup 不再重复读同一批文件。
                    eprintln!("[pinvou3-app] session store ready");
                    Some(store)
                }
                Err(e) => {
                    eprintln!("[pinvou3-app] session store boot failed: {e:#}");
                    None
                }
            };
            startup::mark("session_store:done");
            if let Some(store) = session_store.clone() {
                app.handle().manage(store);
            }
            let remote_control_manager = RemoteControlManager::new(app.handle().clone());
            let remote_event_transport = remote_control_manager.clone();
            let remote_event_endpoint = remote_control_manager.clone();
            app.handle().manage(platform::app_events::AppEventBus::new(
                move |event, payload| remote_event_transport.forward_local_event(event, payload),
                move || remote_event_endpoint.has_active_web_transport(),
            ));
            app.handle().manage(remote_control_manager.clone());
            app.handle()
                .manage(features::behavior_telemetry::BehaviorTelemetry::new());
            // 多 session 并发:存 EnginePool(lazy spawn,首条消息才为该 session 起 engine)。
            // boot bridge 在 pool::new 里做一次(写盘 / 设 env 只能一次)。
            let handle = app.handle().clone();
            let store_for_engine = session_store.unwrap_or_else(|| {
                // store boot 失败时退化用一份临时 store（让 engine 至少能起来）；
                // 实际使用 session 相关命令会失败,但聊天能跑
                //
                // If the second boot still fails (e.g. read-only disk), no
                // usable store can be constructed and there is no available
                // failure path on the engine surface — the app is unusable.
                // Failing fast via panic beats running degraded, so expect
                // is kept here.
                #[allow(clippy::expect_used)]
                || -> SessionStore {
                    SessionStore::boot_for_process_startup().expect("session store boot fallback")
                }()
            });
            // Install the browser transient-protocol startup barrier before any Engine,
            // scheduler, or remote producer. spawn_watch synchronously isolates the
            // previous process's host-request directory before accepting current requests.
            // Installing it after engine_pool could misclassify a boot-time scheduled
            // task's valid request as a crash artifact and remove it.
            {
                let mgr = features::browser::BrowserManager::new();
                mgr.bind_app(app.handle().clone());
                let browser_session_store = store_for_engine.clone();
                mgr.bind_session_validator(std::sync::Arc::new(move |session_id| {
                    browser_session_store.load(session_id).is_ok()
                }));
                app.manage(mgr);
                features::browser::BrowserManager::spawn_watch(app.handle().clone());
                spawn_deleted_session_browser_cleanup(
                    app.handle().clone(),
                    store_for_engine.clone(),
                );
                spawn_orphaned_session_browser_reconciliation(
                    app.handle().clone(),
                    store_for_engine.clone(),
                );
            }
            // 原生代码会话的执行根解析需要共享 AcpPool 持有的 SessionAgentStore
            // （多实例各自读盘，只有这份 clone 与 AcpPool 同一份 Arc）。
            let (code_session_agents, acp_pool_for_capabilities) =
                match crate::features::codex_acp::AcpPool::new(handle.clone(), store_for_engine.clone())
                {
                    Ok(pool) => {
                        let agents = pool.agents().clone();
                        let capability_pool = pool.clone();
                        // 空闲回收巡检：ACP 会话是活的子进程，空闲超阈值回收
                        // （回到 lazy spawn 语义，下次使用重新拉起）。
                        pool.start_idle_reaper();
                        handle.manage(pool);
                        eprintln!("[pinvou3-app] Codex ACP pool ready (lazy spawn per session)");
                        (agents, capability_pool)
                    }
                    Err(error) => {
                        panic!("failed to init Codex ACP pool: {error:#}");
                    }
                };
            startup::mark("engine_pool:start");
            let tool_factory: crate::features::assistant::engine_pool::EngineToolFactory =
                std::sync::Arc::new(|app, session_id| {
                    vec![
                        std::sync::Arc::new(knowledge::KbSearchTool::new(
                            app.clone(),
                            session_id.to_string(),
                        )),
                        std::sync::Arc::new(knowledge::KbOpenSourceTool::new(
                            app.clone(),
                            session_id.to_string(),
                        )),
                    ]
                });
            let tool_policy: crate::features::assistant::engine_pool::ToolPolicy =
                std::sync::Arc::new(|app| {
                    let mut tools = crate::features::marketplace::disabled_tool_names();
                    // 语义与单一真相源见 KnowledgeService::kb_tools_usable:
                    // 只看有没有内容,不看模型在位状态(可见性随模型波动会让
                    // 自愈重载路径不可达)。
                    let kb_usable = knowledge::KnowledgeService::kb_tools_usable(
                        app.try_state::<knowledge::KnowledgeService>()
                            .map(|service| service.has_indexed_content())
                            .unwrap_or(false),
                        app.try_state::<RemoteKnowledgeService>()
                            .map(|service| service.has_connections())
                            .unwrap_or(false),
                    );
                    if !kb_usable {
                        tools.push("kb_search".to_string());
                        tools.push("kb_open_source".to_string());
                    }
                    tools
                });
            match EnginePool::new_with_dependencies(
                handle.clone(),
                store_for_engine.clone(),
                tool_factory,
                tool_policy,
            ) {
                Ok(mut pool) => {
                    // 两个根：执行根（engine cwd/shell）对绑了项目目录的原生代码会话
                    // 解析到项目目录；账本根（附件/审计/产物）恒为会话私有目录。
                    // 解析实现统一下沉在 SessionStore::session_roots，bridge 与
                    // SessionStore 注入同一份 resolver 闭包，两侧结果一致。
                    // resolver 未命中原生代码会话的项目绑定时，回退普通 chat 会话的
                    // 用户工作目录绑定（会话私有目录内的 per-session sidecar
                    // `workspace-binding.json`）——绑定会话的双根语义、AGENTS.md
                    // 注入与提示词环境段对两类会话一致。
                    let execution_root_resolver: crate::features::sessions::ExecutionRootResolver =
                        std::sync::Arc::new({
                            let agents = code_session_agents.clone();
                            let store = store_for_engine.clone();
                            move |session_id: &str| {
                                agents
                                    .code_project_workspace(session_id)
                                    .or_else(|| store.session_workspace_binding(session_id))
                            }
                        });
                    pool.bridge
                        .set_execution_root_resolver(execution_root_resolver.clone());
                    store_for_engine.set_execution_root_resolver(execution_root_resolver);
                    pool.bridge.set_code_session_predicate(std::sync::Arc::new({
                        let agents = code_session_agents.clone();
                        move |session_id: &str| agents.is_code_session(session_id)
                    }));
                    pool.bridge
                        .set_external_acp_session_predicate(std::sync::Arc::new({
                            let acp_pool = acp_pool_for_capabilities.clone();
                            move |session_id: &str| acp_pool.is_acp(session_id)
                        }));
                    // sessions feature 自己的 code 判定（mode 默认值解析 + 仅 code
                    // 持久化），与 bridge 共用同一份 SessionAgentStore 闭包。
                    store_for_engine.set_code_session_predicate(std::sync::Arc::new({
                        let agents = code_session_agents.clone();
                        move |session_id: &str| agents.is_code_session(session_id)
                    }));
                    // SessionStore::delete and deep deletion paths without
                    // an app handle (retention policy/scheduled cleanup)
                    // clear process-level per-session keys uniformly via
                    // the purge hook, matching the delete_session command's
                    // cleanup (dependency inversion, see SessionPurgedHook —
                    // sessions must not depend on assistant in reverse).
                    // MonitorState is managed late in setup while the hook
                    // only runs at deletion time, so try_state finds it in
                    // place; if unmanaged (very early deletions) the item is
                    // skipped.
                    let app_for_purge_hook = handle.clone();
                    store_for_engine
                        .register_session_purged_hook(std::sync::Arc::new(move |session_id: &str| {
                            crate::features::assistant::timing::clear_session(session_id);
                            crate::features::assistant::pending_user_input::clear_session(
                                session_id,
                            );
                            crate::features::memory::discard_turn_capture(session_id);
                            // Self-metrics accumulate per session key
                            // (warmed_sessions inserts on every TurnComplete
                            // and is never reclaimed); clear the keys on
                            // deletion.
                            if let Some(metrics) = app_for_purge_hook
                                .try_state::<crate::features::monitor::MonitorState>()
                                .map(|state| state.self_metrics())
                            {
                                metrics.drop_session(session_id);
                            }
                        }));
                    // 远程端正式支持代码会话之前，先过滤原生代码会话事件（与 Engine
                    // bridge 共用同一份 SessionAgentStore 判定）。
                    remote_control_manager.set_code_session_predicate(std::sync::Arc::new({
                        let agents = code_session_agents.clone();
                        move |session_id: &str| agents.is_code_session(session_id)
                    }));
                    let scheduled_state = tauri::async_runtime::block_on(
                        scheduled_tasks::ScheduledTaskState::boot_runtime(
                            &pool.bridge,
                            pool.clone(),
                            store_for_engine.clone(),
                        ),
                    );
                    match scheduled_state {
                        Ok(state) => {
                            handle.manage(state);
                            eprintln!("[pinvou3-app] scheduled tasks runtime ready");
                        }
                        Err(e) => {
                            eprintln!("[pinvou3-app] scheduled tasks runtime init failed: {e:#}");
                        }
                    }
                    handle.manage(pool.clone());
                    // 空闲回收巡检：engine 常驻但不再无限常驻，空闲超阈值回收
                    // （回到 lazy spawn 语义，下次发消息重建 + 注水历史）。
                    pool.start_idle_reaper();
                    eprintln!("[pinvou3-app] engine pool ready (lazy spawn per session)");
                    match remote_control_manager.resume() {
                        Ok(true) => eprintln!("[pinvou3-app] persistent Web access resumed"),
                        Ok(false) => {}
                        Err(error) => {
                            eprintln!("[pinvou3-app] persistent Web access resume failed: {error}")
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[pinvou3-app] failed to init engine pool: {e:#}");
                }
            }
            startup::mark("engine_pool:done");

            // 技能/工具开关 scope 治理(已收敛为 disabled_bundles.json):启动时
            //   1. 读一次 disabled_bundles.json——触发旧双文件迁移(disabled_connectors
            //      .json / disabled_skills.json → 包 id × SessionMode 单一禁用集);
            //   2. 退役进程级全局 DISABLED_SKILLS(过滤职责移交组合目录,组合目录
            //      空 → 整个 `## Skills` 块不渲染,路径泄露面随之封闭)。
            // 组合目录的物化在 engine spawn 时按会话进行(build_engine_config 注入
            // skills_dir 指向 ~/.pinvou3/sessions/<sid>/skills/)。
            startup::mark("disabled_skills:start");
            let _ = crate::features::assistant::skill_materialization::load_disabled_skills();
            deepseek_tui::skills::set_disabled_skills(Vec::new());
            startup::mark("disabled_skills:done");

            // Monitor 按需采样：state 只持有 session_uptime，sample 由前端调
            // get_monitor_snapshot 时触发（监控页面 1s interval，离开页面停）。
            let monitor_state = MonitorState::new();
            app.handle().manage(monitor_state);
            app.handle()
                .manage(notifications::NotificationState::default());
            app.handle()
                .manage(pet_window::PetNavigationState::default());
            app.handle().manage(pet_window::PetReplyState::default());
            app.handle().manage(selected_pet::SelectedPetStore::load());

            // CLI 连接器连接编排状态(按连接器 id 存长驻子进程 PID + 取消标志),
            // 飞书 / 企微共用,供 *_connect_begin / *_cancel 用。
            app.handle().manage(connector_cli::ConnectorConn::default());

            // File watcher: 监听 ~/.pinvou3/sessions/ 树,新文件 emit artifact:disk
            file_watcher::spawn(app.handle().clone(), bridge::paths::sessions_root());
            startup::mark("file_watcher:spawned");

            // 本地知识底座 L0:全系统元数据索引(秒搜+去重)。这里只 manage,**不自动扫**——
            // 扫描改懒触发:由前端进入文件管理页时增量扫(不进页=零扫描),不常驻 watcher/周期
            // 重扫。文件管理是低频功能,不该长期占资源。
            // embedding 模型**不再随 deb 打包**(deb 瘦 ~559MB):改按需下载到
            // ~/.pinvou3/knowledge/models/bge-m3。setup 只打开数据库并以 embedder=None 注册服务；
            // React 首帧后再调用 kb_model_load_after_first_frame，通过 spawn_blocking 后台加载。
            // 模型没装/加载失败时维持完全门控，不阻断启动；下载完成仍可热加载。
            // 语音识别引擎随平台安装包打包,容错同 bge-m3 的资源布局,
            // 注入给 voice_asr 作为 ~/.pinvou3/asr/ 之外的回退查找目录。
            if let Some(asr_res) = app.path().resource_dir().ok().and_then(|res| {
                [
                    res.join("runtime").join("asr"),
                    res.join("resources").join("runtime").join("asr"),
                ]
                .into_iter()
                .find(|d| d.join(features::voice::engine_binary_name()).exists())
            }) {
                features::voice::set_bundled_engine_dir(asr_res);
            }

            startup::mark("knowledge_service:start");
            match knowledge::KnowledgeService::new(&knowledge::default_db_path()) {
                Ok(svc) => {
                    app.handle().manage(svc);
                    eprintln!("[pinvou3-app] knowledge service ready");
                }
                Err(e) => eprintln!("[pinvou3-app] knowledge service init failed: {e:#}"),
            }
            startup::mark("knowledge_service:done");

            match RemoteKnowledgeService::load(RemoteKnowledgeService::default_path()) {
                Ok(service) => {
                    app.handle().manage(service);
                    eprintln!("[pinvou3-app] remote knowledge service ready");
                }
                Err(error) => {
                    eprintln!("[pinvou3-app] remote knowledge service init failed: {error}")
                }
            }
            // 桌宠:settings.json 里 pet.enabled 为真时随主窗口一起拉起。
            pet_window::spawn_if_enabled(app.handle());

            startup::mark("setup:done");
            Ok(())
        })
        .on_window_event(|window, event| {
            // 主窗口销毁 → 一并关掉桌宠,否则只剩宠物窗口时 app 不退出。
            if window.label() == "main" {
                if let tauri::WindowEvent::Destroyed = event {
                    pet_window::close_with_main(window.app_handle());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::chat::chat,
            commands::chat::steer_chat,
            commands::chat::withdraw_steer,
            commands::checkpoints::list_checkpoints,
            commands::checkpoints::checkpoint_diff,
            commands::checkpoints::rewind_to_turn,
            commands::checkpoints::rewind_undo_state,
            commands::checkpoints::undo_last_rewind,
            commands::behavior_telemetry::track_behavior_event,
            commands::assistant_response::export_assistant_response,
            commands::assistant_response::open_assistant_share_target,
            commands::browser::browser_stop,
            commands::browser::browser_status,
            commands::browser::browser_prepare,
            commands::browser::browser_hand_back_to_agent,
            commands::browser::browser_show_native_surface,
            commands::browser::browser_hide_native_surface,
            commands::browser::browser_begin_surface_generation,
            commands::browser::browser_navigate,
            commands::browser::browser_back,
            commands::browser::browser_forward,
            commands::browser::browser_reload,
            commands::browser::browser_list_tabs,
            commands::browser::browser_create_tab,
            commands::browser::browser_close_tab,
            commands::browser::browser_activate_tab,
            commands::startup::report_frontend_startup,
            commands::diagnostics::record_authority_sync_diagnostics,
            commands::startup::reveal_startup_window,
            commands::connectors::refresh_connector_auth_gates,
            commands::connectors::feishu_ensure_cli,
            commands::connectors::feishu_status,
            commands::connectors::feishu_connect_begin,
            commands::connectors::feishu_cancel,
            commands::connectors::feishu_logout,
            commands::connectors::feishu_apply_skills,
            commands::connectors::set_feishu_enabled,
            commands::connectors::feishu_skills_state,
            commands::connectors::wecom_ensure_cli,
            commands::connectors::wecom_status,
            commands::connectors::wecom_connect_begin,
            commands::connectors::wecom_cancel,
            commands::connectors::wecom_logout,
            commands::connectors::wecom_apply_skills,
            commands::connectors::set_wecom_enabled,
            commands::connectors::wecom_skills_state,
            commands::connectors::dingtalk_ensure_cli,
            commands::connectors::dingtalk_status,
            commands::connectors::dingtalk_connect_begin,
            commands::connectors::dingtalk_cancel,
            commands::connectors::dingtalk_logout,
            commands::connectors::dingtalk_apply_skills,
            commands::connectors::set_dingtalk_enabled,
            commands::connectors::dingtalk_skills_state,
            commands::connectors::tmeet_ensure_cli,
            commands::connectors::tmeet_status,
            commands::connectors::tmeet_connect_begin,
            commands::connectors::tmeet_cancel,
            commands::connectors::tmeet_logout,
            commands::connectors::tmeet_apply_skills,
            commands::connectors::set_tmeet_enabled,
            commands::connectors::tmeet_skills_state,
            commands::connectors::ima_status,
            commands::connectors::ima_connect,
            commands::connectors::ima_logout,
            commands::settings::get_settings,
            commands::runtime::get_platform_capabilities,
            commands::settings::submit_feedback,
            commands::settings::get_effective_model_config,
            commands::settings::update_settings,
            commands::settings::update_search_settings,
            commands::settings::save_settings_and_restart,
            commands::settings::save_search_settings_and_restart,
            commands::monitor::get_monitor_snapshot,
            commands::monitor::get_backend_status,
            commands::monitor::discover_local_vllm,
            commands::local_llm::detect_local_vllm_setup,
            commands::local_llm::bootstrap_local_vllm,
            commands::local_llm::decline_local_vllm_setup,
            commands::settings::list_models,
            commands::settings::reveal_model_api_key,
            commands::settings::probe_local_server_kind,
            commands::settings::save_model,
            commands::settings::delete_model,
            commands::settings::set_active_model,
            commands::settings::set_session_model,
            commands::settings::get_session_model_id,
            commands::settings::get_image_input_capability,
            commands::codex::get_codex_acp_status,
            commands::codex::list_acp_agents,
            commands::codex::get_acp_agent_status,
            commands::codex::prepare_codex_acp,
            commands::codex::install_codex_homebrew,
            commands::codex::install_acp_agent,
            commands::codex::login_codex_acp,
            commands::codex::login_acp_agent,
            commands::codex::switch_acp_agent_account,
            commands::codex::open_codex_login_url,
            commands::codex::open_acp_agent_login_url,
            commands::codex::submit_acp_agent_login_code,
            commands::codex::get_codex_acp_session_info,
            commands::codex::set_codex_acp_model,
            commands::codex::set_codex_acp_mode,
            commands::codex::set_codex_acp_config_option,
            commands::codex::codex_acp_prompt,
            commands::codex::cancel_codex_acp,
            commands::codex::get_codex_acp_timeline,
            commands::codex::get_codex_acp_pending_permissions,
            commands::codex::respond_codex_acp_permission,
            commands::codex::get_codex_acp_pending_elicitations,
            commands::codex::respond_codex_acp_elicitation,
            commands::acp_providers::list_acp_providers,
            commands::acp_providers::save_acp_provider,
            commands::acp_providers::delete_acp_provider,
            commands::acp_providers::switch_acp_provider,
            commands::acp_providers::switch_acp_provider_official,
            commands::acp_providers::uninstall_acp_agent,
            commands::acp_providers::cancel_acp_agent_install,
            commands::acp_providers::logout_acp_agent,
            commands::acp_providers::get_acp_provider_key,
            commands::acp_providers::export_acp_providers,
            commands::acp_providers::import_acp_providers,
            commands::acp_providers::probe_acp_agent_models,
            commands::acp_providers::set_codex_acp_session_provider,
            commands::codex::list_codex_acp_sessions,
            commands::codex::create_codex_acp_session,
            commands::codex::list_codex_workspace,
            commands::codex::search_codex_workspace,
            commands::codex::preview_codex_workspace_file,
            commands::codex::open_codex_workspace_resource,
            commands::codex::get_codex_workspace_changes,
            commands::codex::get_codex_workspace_diff,
            commands::codex::open_codex_workspace_file,
            commands::codex::reveal_codex_workspace_file,
            commands::codex::open_code_reader,
            commands::codex::take_code_reader_pending,
            commands::settings::test_model_connection,
            commands::settings::test_image_input_capability,
            commands::settings::test_search_provider,
            commands::voice::transcribe_voice_audio,
            commands::voice::reset_microphone_permission,
            commands::voice::voice_asr_status,
            commands::voice::install_voice_asr,
            commands::voice::cancel_voice_asr,
            commands::sessions::list_sessions,
            commands::sessions::create_session,
            commands::sessions::get_session_workspace_binding,
            commands::sessions::load_session,
            commands::sessions::delete_session,
            commands::sessions::rename_session,
            commands::sessions::set_session_pinned,
            commands::sessions::list_archived_sessions,
            commands::sessions::set_session_archived,
            commands::timeline::get_session_timeline,
            commands::scheduled::list_scheduled_tasks,
            commands::scheduled::read_scheduled_task,
            commands::scheduled::list_scheduled_task_runs,
            commands::scheduled::list_scheduled_runs,
            commands::scheduled::create_scheduled_task,
            commands::scheduled::update_scheduled_task,
            commands::scheduled::pause_scheduled_task,
            commands::scheduled::resume_scheduled_task,
            commands::scheduled::set_scheduled_task_pinned,
            commands::scheduled::delete_scheduled_task,
            commands::scheduled::run_scheduled_task_now,
            commands::scheduled::mark_scheduled_run_viewed,
            commands::scheduled::scheduled_task_chat_prompt,
            commands::sessions::save_session_messages,
            commands::sessions::save_session_artifacts,
            commands::sessions::save_session_pinvou_scene_events,
            commands::sessions::get_session_pinvou_scene_events,
            commands::sessions::save_session_steered_messages,
            commands::sessions::get_session_steered_messages,
            commands::sessions::list_workspace_files,
            commands::runtime::cancel_generation,
            commands::runtime::list_shell_tasks,
            commands::runtime::cancel_shell_task,
            commands::remote_control::web_access_enable,
            commands::remote_control::web_access_disable,
            commands::remote_control::web_access_status,
            commands::remote_control::web_access_rotate,
            commands::remote_control::web_access_relay_settings,
            commands::remote_control::web_access_set_relay,
            commands::remote_control::web_access_reset_relay,
            commands::remote_control::web_access_bridge_ready,
            commands::remote_control::web_access_rpc_begin,
            commands::remote_control::web_access_rpc_respond,
            commands::remote_control::web_access_publish_event,
            commands::remote_control::web_access_list_host_files,
            commands::remote_control::web_access_list_sessions,
            commands::remote_control::web_access_list_archived_sessions,
            commands::remote_control::web_access_create_session,
            commands::remote_control::web_access_create_session_and_chat,
            commands::remote_control::web_access_cancel_session_download,
            commands::remote_control::web_access_load_session_chunk,
            commands::remote_control::web_access_ingest_file,
            commands::remote_control::web_access_upload_attachment_chunk,
            commands::remote_control::web_access_abort_attachment_upload,
            commands::remote_control::web_access_discard_attachment,
            commands::remote_control::web_access_read_conversation_attachment_chunk,
            commands::remote_control::web_access_chat,
            commands::remote_control::web_access_create_codex_acp_session,
            commands::remote_control::web_access_list_codex_workspace,
            commands::remote_control::web_access_search_codex_workspace,
            commands::remote_control::web_access_preview_codex_workspace_file,
            commands::remote_control::web_access_get_codex_workspace_changes,
            commands::remote_control::web_access_get_codex_workspace_diff,
            commands::remote_control::web_access_cancel_codex_acp,
            commands::remote_control::web_access_codex_acp_prompt,
            commands::remote_control::web_access_get_codex_acp_timeline,
            commands::remote_control::web_access_get_codex_acp_session_info,
            commands::remote_control::web_access_set_codex_acp_model,
            commands::remote_control::web_access_set_codex_acp_mode,
            commands::remote_control::web_access_set_codex_acp_config_option,
            commands::remote_control::web_access_get_codex_acp_pending_permissions,
            commands::remote_control::web_access_respond_codex_acp_permission,
            commands::remote_control::web_access_get_codex_acp_pending_elicitations,
            commands::remote_control::web_access_respond_codex_acp_elicitation,
            commands::remote_control::web_access_list_codex_acp_sessions,
            commands::remote_control::web_access_list_acp_agents,
            commands::remote_control::web_access_get_acp_agent_status,
            commands::remote_control::web_access_save_session_messages_chunk,
            commands::remote_control::web_access_transcribe_voice_audio,
            commands::remote_control::web_access_read_artifact_chunk,
            commands::remote_control::web_access_update_settings,
            commands::remote_control::web_access_artifact_info,
            commands::remote_control::web_access_read_artifact_text,
            commands::remote_control::web_access_write_artifact_text,
            commands::remote_control::web_access_read_artifact_image_b64,
            commands::remote_control::web_access_read_artifact_thumbnail,
            commands::remote_control::web_access_render_artifact_visual,
            commands::connectors::set_disabled_connectors,
            commands::connectors::get_disabled_connectors,
            commands::connectors::set_bundle_visibility,
            commands::connectors::get_bundle_visibility,
            commands::connectors::set_disabled_skills,
            commands::connectors::get_disabled_skills,
            commands::connectors::set_project_skills_enabled,
            commands::connectors::get_project_skills_enabled,
            commands::memory::update_memory_profile,
            commands::memory::get_memory_overview,
            commands::memory::confirm_pending_memory,
            commands::memory::ignore_pending_memory,
            commands::memory::never_pending_memory,
            commands::memory::archive_recent_work_memory,
            commands::memory::delete_memory_preference,
            commands::memory::update_memory_preference,
            commands::memory::update_work_context_memory,
            commands::memory::delete_work_context_memory,
            commands::memory::update_timed_memory,
            commands::memory::delete_timed_memory,
            commands::memory::edit_last_turn,
            commands::artifacts::read_artifact_text,
            commands::artifacts::write_artifact_text,
            commands::artifacts::list_deliverable_index,
            commands::artifacts::artifact_info,
            commands::artifacts::render_artifact_visual,
            commands::artifacts::read_artifact_image_b64,
            commands::artifacts::read_artifact_thumbnail,
            commands::artifacts::open_in_system,
            commands::artifacts::open_containing_folder,
            commands::artifacts::reveal_session_folder,
            commands::artifacts::open_scheduled_task_folder,
            commands::artifacts::open_artifact_window,
            commands::pet::open_detached_window,
            commands::pet::begin_detach_drag,
            commands::pet::set_pet_enabled,
            commands::pet::get_pet_scale,
            commands::pet::set_pet_scale,
            commands::pet::set_pet_activity_visible,
            commands::pet::save_pet_position,
            commands::pet::save_pet_vertical_alignment,
            commands::pet::open_main_from_pet,
            commands::pet::take_pet_navigation,
            commands::pet::queue_pet_reply,
            commands::pet::take_pet_reply,
            commands::pet::get_selected_pet,
            commands::pet::set_selected_pet,
            commands::artifacts::open_external_url,
            commands::artifacts::open_user_external_url,
            commands::files::ingest_file,
            commands::files::ingest_draft_file_chunk,
            commands::files::cancel_draft_file_upload,
            commands::files::adopt_draft_attachment,
            commands::files::ingest_dropped_file_chunk,
            commands::files::cancel_dropped_file_upload,
            commands::files::discard_dropped_attachment,
            commands::files::resolve_conversation_attachment,
            commands::files::open_conversation_attachment,
            commands::files::reveal_conversation_attachment,
            commands::files::save_paste_image,
            commands::interaction::compact_now,
            commands::interaction::get_mode_state,
            commands::interaction::get_code_permission_prefs,
            commands::interaction::confirm_code_yolo,
            commands::interaction::get_mode_defaults,
            commands::interaction::set_mode_default,
            commands::interaction::set_plan_mode_next,
            commands::interaction::exit_plan_to_yolo,
            commands::interaction::set_multi_agent_mode,
            commands::interaction::accept_plan,
            commands::interaction::discard_plan,
            commands::interaction::read_skill_body,
            // 多智能体执行记录投影。
            commands::multiagent::list_subagent_transcripts,
            commands::multiagent::read_subagent_transcript,
            commands::interaction::submit_user_input,
            commands::interaction::cancel_user_input,
            commands::interaction::get_pending_user_inputs,
            commands::interaction::restart_engine,
            commands::interaction::summon_pinvou,
            commands::personas::save_session_pinvou_reviews,
            commands::personas::get_session_pinvou_reviews,
            commands::interaction::get_super_permission_status,
            commands::interaction::set_super_permission,
            commands::personas::list_personas,
            commands::personas::read_persona_body,
            commands::personas::equip_persona,
            commands::personas::unequip_persona,
            commands::personas::get_active_persona,
            commands::personas::create_persona,
            commands::personas::update_persona,
            commands::personas::delete_persona,
            commands::personas::save_session_persona_events,
            commands::personas::get_session_persona_events,
            commands::updater::get_app_version,
            commands::updater::check_for_update,
            commands::updater::download_update,
            commands::updater::install_update,
            commands::updater::restart_app,
            commands::updater::cancel_download,
            commands::updater::report_pending_update_result,
            commands::dependencies::check_dependencies,
            commands::dependencies::install_dependencies,
            commands::marketplace::list_marketplace_tools,
            commands::marketplace::install_marketplace_tool,
            commands::marketplace::get_marketplace_tool_auth_status,
            commands::marketplace::start_marketplace_tool_oauth_login,
            commands::marketplace::cancel_marketplace_tool_oauth_login,
            commands::marketplace::uninstall_marketplace_tool,
            commands::artifacts::detect_obsidian,
            commands::knowledge::kb_start_scan,
            commands::knowledge::kb_scan_status,
            commands::knowledge::kb_cancel_scan,
            commands::knowledge::kb_search,
            commands::knowledge::kb_stats,
            commands::knowledge::kb_type_counts,
            commands::knowledge::kb_collection_list,
            commands::knowledge::kb_collection_create,
            commands::knowledge::kb_collection_update,
            commands::knowledge::kb_collection_delete,
            commands::knowledge::kb_collection_add_sources,
            commands::knowledge::kb_index_status,
            commands::knowledge::kb_index_cancel,
            commands::knowledge::kb_index_failed_files,
            commands::knowledge::kb_index_resume,
            commands::knowledge::kb_index_retry_file,
            commands::knowledge::kb_documents,
            commands::knowledge::kb_remove_document,
            commands::knowledge::kb_embed_info,
            commands::knowledge::kb_model_status,
            commands::knowledge::kb_model_load_after_first_frame,
            commands::knowledge::kb_model_download,
            commands::knowledge::kb_model_cancel,
            commands::knowledge::session_mount_collection,
            commands::knowledge::session_set_mounted_collections,
            commands::knowledge::session_add_mounted_collection,
            commands::knowledge::session_set_mounted_collection_enabled,
            commands::knowledge::session_remove_mounted_collection,
            commands::knowledge::session_unmount_collection,
            commands::knowledge::session_mounted_collection,
            commands::knowledge::session_mounted_collections,
            commands::knowledge::session_mounted_collections_snapshot,
            commands::remote_knowledge::remote_kb_connections,
            commands::remote_knowledge::remote_kb_request_join,
            commands::remote_knowledge::remote_kb_probe_private_endpoint,
            commands::remote_knowledge::remote_kb_request_join_confirmed,
            commands::remote_knowledge::remote_kb_connection_identity,
            commands::remote_knowledge::remote_kb_pending_joins,
            commands::remote_knowledge::remote_kb_refresh_join,
            commands::remote_knowledge::remote_kb_cancel_join,
            commands::remote_knowledge::remote_kb_create_share,
            commands::remote_knowledge::remote_kb_shares,
            commands::remote_knowledge::remote_kb_stop_share,
            commands::remote_knowledge::remote_kb_join_requests,
            commands::remote_knowledge::remote_kb_approve_join_request,
            commands::remote_knowledge::remote_kb_reject_join_request,
            commands::remote_knowledge::remote_kb_model_status,
            commands::remote_knowledge::remote_kb_download_model,
            commands::remote_knowledge::remote_kb_devices,
            commands::remote_knowledge::remote_kb_update_device,
            commands::remote_knowledge::remote_kb_remove_device,
            commands::remote_knowledge::remote_kb_trashed_collections,
            commands::remote_knowledge::remote_kb_trashed_documents,
            commands::remote_knowledge::remote_kb_permanently_delete_collection,
            commands::remote_knowledge::remote_kb_permanently_delete_document,
            commands::shared_knowledge_host::shared_kb_host_status,
            commands::shared_knowledge_host::shared_kb_host_lan_endpoints,
            commands::shared_knowledge_host::shared_kb_discover_nearby,
            commands::shared_knowledge_host::shared_kb_host_install,
            commands::shared_knowledge_host::shared_kb_host_upgrade,
            commands::shared_knowledge_host::shared_kb_host_reconnect,
            commands::shared_knowledge_host::shared_kb_host_set_owner_device,
            commands::shared_knowledge_host::shared_kb_host_remove,
            commands::shared_knowledge_host::shared_kb_host_backup,
            commands::shared_knowledge_host::shared_kb_host_restore,
            commands::remote_knowledge::remote_kb_remove_connection,
            commands::remote_knowledge::remote_kb_collections,
            commands::remote_knowledge::remote_kb_create_collection,
            commands::remote_knowledge::remote_kb_update_collection,
            commands::remote_knowledge::remote_kb_delete_collection,
            commands::remote_knowledge::remote_kb_restore_collection,
            commands::remote_knowledge::remote_kb_documents,
            commands::remote_knowledge::remote_kb_document_statuses,
            commands::remote_knowledge::remote_kb_discover_folder_files,
            commands::remote_knowledge::remote_kb_upload_files,
            commands::remote_knowledge::remote_kb_replace_document,
            commands::remote_knowledge::remote_kb_delete_document,
            commands::remote_knowledge::remote_kb_restore_document,
            commands::remote_knowledge::remote_kb_download_document,
            commands::remote_knowledge::remote_kb_search,
            commands::remote_knowledge::session_mounted_remote_collections,
            commands::remote_knowledge::session_add_mounted_remote_collection,
            commands::remote_knowledge::session_set_mounted_remote_collection_enabled,
            commands::remote_knowledge::session_remove_mounted_remote_collection,
            commands::marketplace::list_marketplace_skills,
            commands::marketplace::install_marketplace_skill,
            commands::marketplace::update_marketplace_skill,
            commands::marketplace::update_bundle_display_meta,
            commands::marketplace::import_skill_package,
            commands::marketplace::import_skill_package_bytes,
            commands::marketplace::import_plugin_package_cmd,
            commands::marketplace::import_plugin_package_bytes_cmd,
            commands::marketplace::import_skill_md_bytes,
            commands::marketplace::uninstall_marketplace_skill,
            commands::marketplace::bundle_readiness,
            commands::marketplace::export_plugin_spec,
            commands::marketplace::list_recycled_plugins,
            commands::marketplace::restore_recycled_plugin,
            commands::marketplace::purge_recycled_plugin,
            commands::marketplace::export_recycled_plugin,
            commands::marketplace::export_installed_plugin,
        ]);

    startup::mark("tauri:builder_configured");
    startup::mark("tauri:context:start");
    let context = build_tauri_context();
    startup::mark("tauri:context:done");
    // Keep the historical marker so old and new startup runs remain comparable.
    startup::mark("tauri:run_enter");
    startup::mark("tauri:build:start");
    // Tauri template idiom: a builder.build failure means the window/event loop
    // cannot be built and the process has not entered a user session; there is
    // no degradation path — keep expect to fail fast.
    #[allow(clippy::expect_used)]
    let app = builder
        .build(context)
        .expect("error while building tauri application");
    startup::mark("tauri:build:done");
    startup::mark("tauri:event_loop:run_enter");
    let mut resumed_reported = false;
    app.run(move |app, event| match event {
        tauri::RunEvent::Ready => startup::mark("tauri:event_loop:ready"),
        tauri::RunEvent::Exit => {
            startup::mark("tauri:event_loop:exit");
            // On main-process exit, refresh browser URL restore manifests, destroy
            // in-app native WebViews, and clean runtime targets/endpoints. Preserve
            // restore manifests; there is no external Chrome process.
            shutdown_browser_before_process_end(app);
            // Exit semantics preserve the latest restore manifest. Do not asynchronously
            // call ordinary stop() here: if shutdown_on_exit skips explicit close because
            // of lock contention, a late stop could acquire the lock and delete restore
            // state with user-stop semantics. Process exit destroys remaining WebViews,
            // and shutdown_on_exit always removes coordination files.
            // 退出收割:同步执行——Exit 后进程即将结束,这是最后的清理窗口。
            // 重启(app.restart())跳过本事件,调用点在 restart 前主动调同一辅助函数。
            startup::mark("exit:cleanup:start");
            tauri::async_runtime::block_on(harvest_child_processes(app));
            startup::mark("exit:cleanup:done");
        }
        tauri::RunEvent::Resumed if !resumed_reported => {
            resumed_reported = true;
            startup::mark("tauri:event_loop:first_resumed");
        }
        _ => {}
    });
    startup::mark("process:exit");
}

#[cfg(test)]
mod tool_allowlist_contract {
    use crate::features::assistant::tool_policy::{
        PINVOU3_ALLOWED_TOOLS, PINVOU3_ALWAYS_LOADED_TOOLS, is_pinvou3_allowed,
    };

    /// Pinvou 只允许产品需要的 canonical 工具家族；动态 MCP 工具限制在标准命名空间。
    #[test]
    fn pinvou3_allowlist_uses_canonical_families_and_dynamic_mcp_namespace() {
        for core in [
            "Bash",
            "File",
            "Git",
            "Web",
            "agent",
            "load_skill",
            "todo_write",
            "tool_search",
            "request_user_input",
            "revert_turn",
            "kb_search",
            "kb_open_source",
            "mcp_weather_get_weather",
        ] {
            assert!(is_pinvou3_allowed(core), "核心工具 {core} 应在白名单");
        }

        for excluded in [
            "Run",
            "tasks",
            "automation",
            "github",
            "rlm",
            // work_update 是 v0.9.5 隐藏的 replay 别名，模型目录里只有 canonical
            // `todo_write`；别名写进白名单是死条目，且会让进度工具整体不可见。
            "work_update",
            "checklist_write",
            "update_plan",
        ] {
            assert!(
                !is_pinvou3_allowed(excluded),
                "非 Pinvou 工具家族 {excluded} 不应进入白名单"
            );
        }

        assert_eq!(
            PINVOU3_ALWAYS_LOADED_TOOLS,
            &["request_user_input", "image_analyze"]
        );
        assert!(PINVOU3_ALLOWED_TOOLS.contains(&"mcp_*"));
    }
}

#[cfg(test)]
mod navigation_policy_tests {
    #[test]
    fn embedded_srcdoc_navigation_is_allowed_without_broadening_schemes() {
        for allowed in ["about:blank", "about:srcdoc"] {
            let url = tauri::Url::parse(allowed).unwrap();
            assert!(super::allow_embedded_document_navigation(&url, true));
            assert!(
                !super::allow_embedded_document_navigation(&url, false),
                "embedded documents must not become the initial main origin"
            );
        }
        for blocked in [
            "about:config",
            "about:blank?next=https://example.com",
            "data:text/html,hello",
            "https://example.com/",
            "file:///etc/passwd",
        ] {
            let url = tauri::Url::parse(blocked).unwrap();
            assert!(
                !super::allow_embedded_document_navigation(&url, true),
                "must not classify as embedded document: {blocked}"
            );
        }
    }
}

#[cfg(test)]
mod session_browser_cleanup_retry_tests {
    use super::{
        SESSION_BROWSER_CLEANUP_INITIAL_RETRY, SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY,
        SESSION_BROWSER_CLEANUP_MAX_RETRY, SessionBrowserCleanupQueue,
        next_session_browser_cleanup_retry, session_browser_cleanup_failure_kind,
    };

    #[test]
    fn cleanup_failure_diagnostics_use_non_sensitive_categories() {
        assert_eq!(
            session_browser_cleanup_failure_kind("browser manager is unavailable"),
            "browser-manager-unavailable"
        );
        assert_eq!(
            session_browser_cleanup_failure_kind(
                "browser cleanup attempt timed out after 10 seconds"
            ),
            "timeout"
        );
        let category = session_browser_cleanup_failure_kind(
            "failed to delete session private-session-id from C:\\private\\path",
        );
        assert_eq!(category, "cleanup-error");
        assert!(!category.contains("private-session-id"));
        assert!(!category.contains("private\\path"));
    }

    #[test]
    fn pending_queue_coalesces_repeated_ids_before_worker_take() {
        let queue = SessionBrowserCleanupQueue::default();

        for _ in 0..10_000 {
            queue.enqueue("session-a");
        }
        queue.enqueue("session-b");

        assert_eq!(queue.pending_len(), 2);
        let now = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut claimed = queue.claim_ready_up_to(now, SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY);
        claimed.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        assert_eq!(
            claimed
                .iter()
                .map(|claim| claim.session_id.as_str())
                .collect::<Vec<_>>(),
            ["session-a", "session-b"]
        );
        assert_eq!(queue.pending_len(), 0);
        assert_eq!(queue.in_flight_len(), 2);
        for claim in claimed {
            queue.complete_attempt(claim, true, now);
        }
        assert_eq!(queue.in_flight_len(), 0);
    }

    #[test]
    fn unique_pending_ids_are_concurrency_bounded_without_loss() {
        let queue = SessionBrowserCleanupQueue::default();
        let expected = (0..10_000)
            .map(|index| format!("session-{index}"))
            .collect::<std::collections::HashSet<_>>();
        for session_id in &expected {
            queue.enqueue(session_id);
        }

        let mut observed = std::collections::HashSet::new();
        let now = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while queue.in_flight_len() > 0 || queue.pending_len() > 0 {
            let batch = queue.claim_ready_up_to(now, SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY);
            assert!(batch.len() <= SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY);
            assert!(queue.in_flight_len() <= SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY);
            for claim in batch {
                assert!(observed.insert(claim.session_id.clone()));
                queue.complete_attempt(claim, true, now);
            }
        }

        assert_eq!(observed, expected);
        assert_eq!(queue.pending_len(), 0);
        assert_eq!(queue.in_flight_len(), 0);
    }

    #[test]
    fn repeated_events_are_coalesced_and_success_releases_cleanup_bookkeeping() {
        let queue = SessionBrowserCleanupQueue::default();
        queue.enqueue("session-a");
        queue.enqueue("session-b");
        let now = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut first = queue.claim_ready_up_to(now, SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY);
        let session_a = first
            .iter()
            .position(|claim| claim.session_id == "session-a")
            .map(|index| first.swap_remove(index))
            .expect("session A claim");
        let session_b = first.pop().expect("session B claim");

        queue.enqueue("session-a");
        queue.enqueue("session-a");
        queue.complete_attempt(session_a, true, now);
        queue.complete_attempt(session_b, true, now);

        let rerun = queue.claim_ready_up_to(now, SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY);
        assert_eq!(rerun.len(), 1);
        assert_eq!(rerun[0].session_id, "session-a");
        queue.complete_attempt(rerun.into_iter().next().unwrap(), true, now);
        assert_eq!(queue.pending_len(), 0);
        assert_eq!(queue.in_flight_len(), 0);

        queue.enqueue("session-a");
        assert_eq!(queue.claim_ready_up_to(now, 1).len(), 1);
    }

    #[test]
    fn persistent_failures_do_not_starve_a_later_unique_cleanup() {
        let queue = SessionBrowserCleanupQueue::default();
        let failure_ids = (0..SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY)
            .map(|index| format!("failure-{index}"))
            .collect::<std::collections::HashSet<_>>();
        for index in 0..SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY {
            queue.enqueue(&format!("failure-{index}"));
        }
        queue.enqueue("eventual-success");
        let mut now = std::time::Instant::now() + std::time::Duration::from_secs(1);

        let first = queue.claim_ready_up_to(now, SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY);
        assert_eq!(first.len(), SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY);
        assert!(
            first
                .iter()
                .all(|claim| failure_ids.contains(&claim.session_id))
        );
        for claim in first {
            queue.complete_attempt(claim, false, now);
        }

        let success = queue.claim_ready_up_to(now, SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY);
        assert_eq!(success.len(), 1);
        assert_eq!(success[0].session_id, "eventual-success");
        queue.complete_attempt(success.into_iter().next().unwrap(), true, now);

        for _ in 0..3 {
            now += SESSION_BROWSER_CLEANUP_MAX_RETRY;
            let retry = queue.claim_ready_up_to(now, SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY);
            assert_eq!(retry.len(), SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY);
            assert!(
                retry
                    .iter()
                    .all(|claim| failure_ids.contains(&claim.session_id))
            );
            for claim in retry {
                queue.complete_attempt(claim, false, now);
            }
        }
        assert_eq!(queue.in_flight_len(), 0);
        assert_eq!(queue.pending_len(), SESSION_BROWSER_CLEANUP_MAX_CONCURRENCY);
    }

    #[test]
    fn cleanup_retry_uses_exponential_backoff_with_a_fixed_cap() {
        let mut delay = SESSION_BROWSER_CLEANUP_INITIAL_RETRY;
        for expected_millis in [500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000] {
            delay = next_session_browser_cleanup_retry(delay);
            assert_eq!(delay.as_millis(), expected_millis);
        }
        assert_eq!(
            next_session_browser_cleanup_retry(SESSION_BROWSER_CLEANUP_MAX_RETRY),
            SESSION_BROWSER_CLEANUP_MAX_RETRY
        );
    }
}

#[cfg(test)]
mod release_env_defaults_guard {
    /// PR #210 守卫：release/boot 不得重新注入 DEEPSEEK_MAX_OUTPUT_TOKENS。
    /// 该 env 会被底座 effective_max_output_tokens() 优先读取，一旦回归会重新把
    /// 所有模型（含云端）输出上限钉死 24576——正是本 PR 移除的根因。此守卫在
    /// CHANGES_REQUESTED 后新增：clean env 云端落底座 64K 兜底的前提，就是
    /// release 安装包启动路径（.deb 双击等）不再注入该变量。
    ///
    /// 两层检查（评审修正 2026-08-11）：
    /// 1. 常量表本身不含这两个 key（防常量里重新出现）；
    /// 2. 走**实际注入路径** `ensure_release_env`（run() 启动路径的 release env
    ///    注入函数，无写盘副作用）后断言进程 env 仍无这两个 key——即使未来有人在
    ///    注入函数里绕过常量表直接 set_var，这里也能抓到。
    ///
    /// 第三轮评审修正 2026-08-11：本测试此前直接删进程级 env 且不还原，未借 crate
    /// 级唯一 ENV_LOCK（会与 bridge.rs 等 env 写测试并发竞态），也不还原
    /// ensure_release_env 写入的 RELEASE_ENV_DEFAULTS / PATH / 平台 UI env（串行 CI
    /// 下造成后续测试顺序依赖）。现改为：先取 ENV_LOCK 再全量快照 env，退出（含
    /// panic）时按快照完整还原。bridge boot 侧的 env 注入源头由 bridge.rs
    /// `forkguard_boot_env_must_not_pin_global_output_cap` 单独守卫（boot 不直接
    /// 进单测：会 mutate PINVOU3_HOME 写盘 + 全量解包 bundle，见
    /// `engine_config_workspace_follows_bridge_field` 注释）。
    ///
    /// 进程 env 全量快照：ensure_release_env 会 set RELEASE_ENV_DEFAULTS、重写 PATH、
    /// Linux 上还会 set GDK_BACKEND 等 UI env——只有全量快照才能完整还原，且不随
    /// 各常量表增删漂移。
    ///
    /// 第四轮评审修正（2026-08-12）：此前用 `String` + `std::env::vars()`——`vars()`
    /// 对任何非 UTF-8 的合法环境变量值（POSIX 允许任意字节）会直接 panic，且 Drop 中
    /// 的二次枚举同样 panic；测试断言失败展开 panic 期间再 panic 会中止整个测试进程。
    /// 现改用 `OsString` + `vars_os()`，非 UTF-8 环境变量也能完整快照/还原。
    struct EnvSnapshot(std::collections::HashMap<std::ffi::OsString, Option<std::ffi::OsString>>);

    impl EnvSnapshot {
        fn take() -> Self {
            let mut map = std::collections::HashMap::new();
            for (k, v) in std::env::vars_os() {
                map.insert(k, Some(v));
            }
            // 显式记录"当前不存在"的 key，还原时统一按快照恢复原状（set / remove）。
            for key in ["DEEPSEEK_MAX_OUTPUT_TOKENS", "PINVOU3_MAX_OUTPUT_TOKENS"] {
                map.entry(std::ffi::OsString::from(key)).or_insert(None);
            }
            Self(map)
        }
    }

    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (k, v) in &self.0 {
                match v {
                    // SAFETY: holding platform::paths::tests::ENV_LOCK (see each
                    // use site); env writes in the test process are serialized.
                    Some(v) => unsafe { std::env::set_var(k, v) },
                    // SAFETY: same as above; restore-side removal serialized under ENV_LOCK.
                    None => unsafe { std::env::remove_var(k) },
                }
            }
            // ensure_release_env 可能 set 了快照中原本不存在的 key（PATH 分支、Linux
            // UI env 等）——全部移除，回到快照状态，杜绝后续测试的顺序依赖。
            let keep: std::collections::HashSet<&std::ffi::OsString> = self.0.keys().collect();
            let stale: Vec<std::ffi::OsString> = std::env::vars_os()
                .map(|(k, _)| k)
                .filter(|k| !keep.contains(k))
                .collect();
            for k in stale {
                // SAFETY: same as above; cleanup-side removal under ENV_LOCK serialization.
                unsafe { std::env::remove_var(&k) };
            }
        }
    }

    #[test]
    fn release_env_defaults_must_not_pin_global_output_cap() {
        // crate 级唯一 env 锁：与所有 env 写测试串行（同 bridge.rs locked_env 约定），
        // 避免与 DEEPSEEK_* / PINVOU3_HOME 写测试并发竞态。
        let _lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _snapshot = EnvSnapshot::take();

        // 第一层：常量表
        assert!(
            !super::RELEASE_ENV_DEFAULTS
                .iter()
                .any(|(k, _)| *k == "DEEPSEEK_MAX_OUTPUT_TOKENS"),
            "RELEASE_ENV_DEFAULTS 不得包含 DEEPSEEK_MAX_OUTPUT_TOKENS（PR #210 移除全局注入）"
        );
        assert!(
            !super::RELEASE_ENV_DEFAULTS
                .iter()
                .any(|(k, _)| *k == "PINVOU3_MAX_OUTPUT_TOKENS"),
            "RELEASE_ENV_DEFAULTS 不得包含 PINVOU3_MAX_OUTPUT_TOKENS（品悟侧上限仅经 prefs/route 携带）"
        );

        // 第二层：实际注入路径（ensure_release_env 是 run() 启动路径的 release env
        // 注入函数）。先清掉外部可能残留的 env，确保断言的是注入函数自身的行为。
        // SAFETY: holding platform::paths::tests::ENV_LOCK (first line of this test); env writes serialized.
        unsafe { std::env::remove_var("DEEPSEEK_MAX_OUTPUT_TOKENS") };
        // SAFETY: same as above; ENV_LOCK serialization.
        unsafe { std::env::remove_var("PINVOU3_MAX_OUTPUT_TOKENS") };
        super::ensure_release_env();
        assert!(
            std::env::var_os("DEEPSEEK_MAX_OUTPUT_TOKENS").is_none(),
            "ensure_release_env must not re-inject DEEPSEEK_MAX_OUTPUT_TOKENS (would re-pin cloud caps)"
        );
        assert!(
            std::env::var_os("PINVOU3_MAX_OUTPUT_TOKENS").is_none(),
            "ensure_release_env must not re-inject PINVOU3_MAX_OUTPUT_TOKENS (the Pinvou cap travels only via prefs/route)"
        );
        // 退出时 EnvSnapshot::drop 按快照完整还原（含 PATH / UI env / 常量表变量）。
    }

    /// `startup_process_env` is the sole injection funnel for
    /// `PINVOU3_SESSION_ARTIFACTS` (bridge boot runs in the multi-threaded
    /// phase and must not write the process env; see the dual assertion in
    /// bridge.rs `forkguard_boot_env_must_not_pin_global_output_cap`).
    /// This test locks the positive behavior: the single-threaded startup
    /// function must write the session artifacts dir into the process env for
    /// MCP stdio subprocesses such as PPT / official-document tools to
    /// inherit.
    #[test]
    fn startup_process_env_writes_session_artifacts() {
        let _lock = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _snapshot = EnvSnapshot::take();

        let root = std::env::temp_dir().join(format!(
            "pinvou3-startup-env-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::fs::create_dir_all(&root).expect("create isolated PINVOU3_HOME");
        // SAFETY: holding platform::paths::tests::ENV_LOCK (first line of this test); env writes serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
        // SAFETY: same as above; ENV_LOCK serialization.
        unsafe { std::env::remove_var("PINVOU3_SESSION_ARTIFACTS") };

        super::startup_process_env();

        let expected = crate::platform::paths::default_session_artifacts_dir();
        assert_eq!(
            std::env::var("PINVOU3_SESSION_ARTIFACTS").as_deref(),
            Ok(expected.to_str().expect("isolated home path must be UTF-8")),
            "startup_process_env must inject PINVOU3_SESSION_ARTIFACTS"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
