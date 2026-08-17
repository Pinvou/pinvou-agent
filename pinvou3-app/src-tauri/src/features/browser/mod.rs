//! 浏览器功能模块：管理"专用有头 Chrome"实例，提供 CDP 截图流（实时显示给用户）、
//! 用户交互转发（点击/滚动/键盘）、导航与多标签页控制。
//!
//! 与 MCP wrapper（`bundle/mcp-servers/browser-wrapper.mjs`）通过
//! `~/.pinvou3/browser/cdp-port.json` + 独占锁幂等协调同一 Chrome 实例：
//! - 谁先启动谁写端口文件；另一方检测到端口有效则直接复用（Chrome 同一
//!   `--user-data-dir` 只允许一个实例，协调必须可靠）；
//! - wrapper 退出时只清理自己启动的实例；本模块 stop() 经 CDP `Browser.close`
//!   优雅关闭（对 wrapper 启动的实例同样有效），品悟退出时兜底清理（主进程语义）。
//!
//! 截图流：`Page.startScreencast`（JPEG 帧）→ 事件 `Page.screencastFrame` → 每帧
//! `screencastFrameAck`（帧号原样回传，防止帧堆积）→ 转发给前端（emit
//! `browser:frame`）。交互坐标以帧 metadata 的 viewport CSS 像素为基准。
//!
//! 端范围：**本期仅桌面端**。`browser:*` 事件仅本地 `emit`，不转发远端 WebUI
//! （relay 的 `access-policy.json` 白名单不含任何 `browser:*` 事件/命令，
//! 转发只会被拒绝并刷日志）——web/移动端暂不提供浏览器 Tab 与交互
//! （"三端共享"为后续迭代项，勿在文档中宣称已支持）。

mod cdp;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::platform::paths;

pub use cdp::CdpSession;

/// 标签页身份（targetId）→ flatten sessionId 缓存。Arc 共享给事件循环做
/// session→target 反查（导航/帧事件以 target 身份携带给前端）。
type PageSessions = Arc<parking_lot::Mutex<HashMap<String, String>>>;

/// 单个页面标签页。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TabInfo {
    pub target_id: String,
    pub title: String,
    pub url: String,
}

#[derive(Default)]
struct Inner {
    port: Option<u16>,
    /// 本模块启动的 Chrome 子进程（wrapper 启动的我们拿不到句柄）。
    child: Option<Child>,
    /// browser 级 CDP 会话（一条连接管所有标签页）。
    session: Option<Arc<CdpSession>>,
    /// 当前激活（正在 screencast）标签页的 sessionId。
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
    /// 启动临界区互斥：串行化整个启动序列（协调 Chrome → CDP 连接 → attach →
    /// startScreencast → 事件循环），避免 watch 轮询与 Tauri 命令并发进入产生
    /// 双事件循环/双截图流/句柄丢失（single-flight）。stop() 也参与本锁，
    /// 保证 stop 不会在启动序列中途"看到空状态提前返回"而被启动方随后覆盖。
    start_mtx: tokio::sync::Mutex<()>,
    /// 停止代际计数：stop() 每次 +1；ensure_started 启动前记录、完成后核对，
    /// 启动期间被 stop 打断时丢弃本次启动结果（避免 stop 被吞、浏览器残留）。
    stop_gen: std::sync::atomic::AtomicU64,
    /// "已向前端 emit 过 browser:activated"标记（watch 与 stop 共享）：
    /// stop()/崩溃路径置 false，保证再次接入时必重新 emit（前端 Tab 重现）。
    activated: std::sync::atomic::AtomicBool,
    /// 前端浏览器视图是否正在查看（BrowserView 挂载时置 true、卸载置 false）：
    /// 截图流只在有人看时推——否则模型用过浏览器后 JPEG 编码 + base64 + IPC
    /// 在后台空转（用户可能从未点开浏览器 Tab）。
    streaming: std::sync::atomic::AtomicBool,
    /// 主进程退出标记：shutdown_on_exit 置位后 watch 退出、ensure_started 拒绝——
    /// 防止退出瞬间 watch 重新拉起 Chrome 成为无人回收的孤儿进程。
    shutting_down: std::sync::atomic::AtomicBool,
    /// target_id → flatten sessionId 缓存：同一标签页复用 attach。CDP 对同一
    /// target 的每次 attach 都产生独立 session 且不自动释放，无缓存会在高频
    /// 枚举/切换下无界泄漏 Chrome 侧 session。
    page_sessions: PageSessions,
    app: parking_lot::Mutex<Option<AppHandle>>,
}

impl BrowserManager {
    pub fn new() -> Self {
        Self {
            inner: tokio::sync::Mutex::new(Inner::default()),
            start_mtx: tokio::sync::Mutex::new(()),
            stop_gen: std::sync::atomic::AtomicU64::new(0),
            activated: std::sync::atomic::AtomicBool::new(false),
            streaming: std::sync::atomic::AtomicBool::new(false),
            shutting_down: std::sync::atomic::AtomicBool::new(false),
            page_sessions: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            app: parking_lot::Mutex::new(None),
        }
    }

    /// 绑定 AppHandle（setup 时调用一次）。
    pub fn bind_app(&self, app: AppHandle) {
        *self.app.lock() = Some(app);
    }

    /// 监听 `cdp-port.json`：检测到有效端口（MCP wrapper 或本模块启动的 Chrome）且品悟
    /// 尚未接入时，自动 `ensure_started` 并 emit `browser:activated` —— 前端据此在
    /// "工作模式 + 模型实际调用浏览器能力"时显示浏览器 Tab（不调用则永不出现/加载）。
    ///
    /// 另承担崩溃恢复：已接入但 CDP 失联（Chrome 崩溃/被杀）时重置状态并 emit
    /// `browser:stopped`，让前端隐藏 Tab、下次调用自动重新拉起。
    pub fn spawn_watch(app: AppHandle) {
        // 必须走 tauri::async_runtime：setup 闭包在 wry 事件循环主线程同步调用，
        // 无 tokio runtime 上下文，裸 tokio::spawn 会 panic（there is no reactor
        // running）导致应用启动即崩。
        tauri::async_runtime::spawn(async move {
            let mut fail_count = 0u32;
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let mgr = app.state::<BrowserManager>();
                // 主进程退出后不再重启/接入浏览器（防退出瞬间拉起孤儿 Chrome）。
                if mgr.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                // 1) 已接入但 Chrome 失联（崩溃/被杀）→ 重置状态并通知前端。
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
                        eprintln!("[browser] Chrome 失联（端口 {port}），重置浏览器状态");
                        if let Some(task) = inner.loop_task.take() {
                            task.abort();
                        }
                        if let Some(task) = inner.reader_task.take() {
                            task.abort();
                        }
                        if let Some(session) = inner.session.take() {
                            // 兜底关 WS（Browser.close 已在 stop/崩溃前失败场景下截断推流）。
                            let _ = session.close().await;
                        }
                        let had_child = inner.child.is_some();
                        if let Some(mut child) = inner.child.take() {
                            let _ = child.kill();
                            let _ = child.wait();
                        }
                        inner.port = None;
                        inner.active_session = None;
                        inner.active_target = None;
                        mgr.page_sessions.lock().clear();
                        mgr.activated
                            .store(false, std::sync::atomic::Ordering::SeqCst);
                        let _ = app.emit("browser:stopped", json!({}));
                        // 端口文件**仅在 kill 了本模块自启 Chrome 时删除**：接入的是
                        // wrapper 实例时，Chrome 可能只是 wedged 而仍健康（wrapper
                        // 只在自身启动时写一次端口文件，被误删后无人重写 → 下一个
                        // 启动者对同一 profile 双启 Chrome，撞单实例锁 15s 失败循环）。
                        // wrapper 的 Chrome 真死时其 chromeChild exit 兜底会自清。
                        if had_child {
                            let _ = std::fs::remove_file(paths::browser_cdp_port_json());
                        }
                        continue;
                    }
                }
                // 2) 未接入：端口文件有效则接入并激活 Tab。
                let Some(port) = port_file() else {
                    fail_count = 0;
                    continue;
                };
                if !probe_cdp(port, Duration::from_millis(800)).await {
                    // 端口文件存在但 Chrome 已死：连续失败后清掉 stale 文件，
                    // 避免永久空转探测（wrapper 崩溃残留/异常退出场景）。
                    fail_count += 1;
                    if fail_count >= 5 {
                        eprintln!("[browser] 端口文件 stale（端口 {port}），清理后重试");
                        let _ = std::fs::remove_file(paths::browser_cdp_port_json());
                        fail_count = 0;
                    }
                    continue;
                }
                fail_count = 0;
                if mgr.ensure_started().await.is_ok() {
                    if !mgr
                        .activated
                        .swap(true, std::sync::atomic::Ordering::SeqCst)
                    {
                        let _ = app.emit("browser:activated", json!({}));
                    }
                } else {
                    // 接入失败（如 Chrome 恰好在退出）静默重试：端口文件仍有效时下次再试。
                    eprintln!("[browser] 接入 Chrome 失败，稍后重试");
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
        let streaming = self.streaming.load(std::sync::atomic::Ordering::SeqCst);
        switch_screencast_locked(&mut inner, &sid, streaming).await?;
        self.page_sessions.lock().insert(target_id.clone(), sid);
        inner.active_target = Some(target_id);
        Ok(())
    }

    /// 确保专用 Chrome 已启动并接入 CDP 截图流。幂等：已连接则直接复用。
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
                // 不持 inner 锁做网络 await（attach 走 CDP 调用）；之后重新拿锁恢复截图流。
                drop(inner);
                return self.reattach_existing(session, gen).await;
            }
        }

        // single-flight：整个启动序列持 start_mtx，并发调用者在此等待后复用
        // 已完成的状态，而不是各自再启动一遍（双事件循环/双截图流/句柄丢失）。
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

        // 1) 协调启动 Chrome（复用端口文件或自启）
        let (port, mut spawned_child) = self.acquire_or_start_chrome().await?;

        // 2-5) 连接 CDP / attach / 启域 / 截图流 / 事件循环。任一步失败时清理
        //     自启的 Chrome（若有），避免孤儿进程占住 profile 单实例锁。
        // session/reader 句柄提到闭包外：失败路径需要关闭 WS / 中止读循环——
        // 复用 wrapper 实例时 Chrome 仍活着，泄漏的读循环会永久持有连接与事件循环。
        let mut boot_session: Option<Arc<cdp::CdpSession>> = None;
        let mut boot_reader: Option<tokio::task::JoinHandle<()>> = None;
        let boot: Result<(), String> = async {
            let connected = cdp::connect(port)
                .await
                .map_err(|e| format!("CDP 连接失败: {e:#}"))?;
            let session = connected.session;
            boot_session = Some(Arc::clone(&session));
            boot_reader = Some(connected.reader_task);

            // 开启 Target 发现：不开则 browser 级 Target.targetCreated/targetDestroyed
            // 事件永不送达（实测）——browser:tabs-changed 会成为断头路：模型经 MCP
            // 增删标签页时前端不刷新，激活页被 MCP 关闭后截图流冻结且无人自愈。
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
            // 截图流仅在前端浏览器视图查看时推流（streaming 门控，见
            // set_streaming）：视图未打开时不启 JPEG 编码/推流，避免后台空转。
            if self.streaming.load(std::sync::atomic::Ordering::SeqCst) {
                session
                    .call(
                        Some(&session_id),
                        "Page.startScreencast",
                        screencast_params(),
                    )
                    .await
                    .map_err(|e| format!("Page.startScreencast 失败: {e}"))?;
            }

            let app = self
                .app
                .lock()
                .clone()
                .ok_or_else(|| "BrowserManager 未绑定 AppHandle".to_string())?;
            let loop_task = tokio::spawn(run_event_loop(
                app,
                Arc::clone(&self.page_sessions),
                connected.events,
            ));

            // 启动期间被 stop() 打断（代际已变）：丢弃本次结果，避免 stop 被吞、
            // 浏览器以无 UI 状态残留（watch 视 session alive 而不再重置）。
            // WS 关闭与读循环中止统一由下方失败路径的 boot_session/boot_reader 完成。
            if self.stop_gen.load(std::sync::atomic::Ordering::SeqCst) != gen_at_start {
                return Err("浏览器启动期间已被停止".to_string());
            }

            let mut inner = self.inner.lock().await;
            // 锁内再核对一次代际与退出标记：上方 gen 检查到拿到 inner 锁之间存在
            // 窗口，期间 stop()/shutdown_on_exit（也 bump 代际）可能已完成；等锁
            // 期间被停止/退出时若照样提交 child/session，退出后这只 Chrome 无人
            // 回收（孤儿进程）。丢弃走统一失败清理（boot_session/boot_reader 关闭
            // 中止 + spawned_child kill）。
            if self.stop_gen.load(std::sync::atomic::Ordering::SeqCst) != gen_at_start
                || self.shutting_down.load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err("浏览器启动期间已被停止或应用正在退出".to_string());
            }
            inner.port = Some(port);
            inner.child = spawned_child.take();
            inner.session = Some(session);
            inner.active_session = Some(session_id);
            inner.active_target = Some(target_id);
            inner.loop_task = Some(loop_task);
            inner.reader_task = boot_reader.take();
            Ok(())
        }
        .await;

        if let Err(e) = &boot {
            // 关闭本次启动建立的 WS 并中止读循环：复用 wrapper 实例的路径失败时
            // Chrome 仍活着，不清理会让每次重试（watch 2s 轮询）净泄漏一条连接
            // 与一个读循环任务。close 幂等（gen 中断路径已关过也无害）。
            if let Some(session) = boot_session.take() {
                let _ = session.close().await;
            }
            if let Some(task) = boot_reader.take() {
                task.abort();
            }
            // 启动失败：kill 自启的 Chrome。仅当 Chrome 是本模块自启时才清端口文件
            // （复用 wrapper 实例的路径失败时其端口文件仍然健康，删除会丢协调文件）。
            let spawned_by_us = spawned_child.is_some();
            if let Some(mut child) = spawned_child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            // 仅自启实例失败时清端口文件（避免误删 wrapper 的健康协调文件）。
            if spawned_by_us {
                let _ = std::fs::remove_file(paths::browser_cdp_port_json());
            }
            return Err(e.clone());
        }
        // 成功接入：清除历史失败记录，避免 24h 内向模型注入陈旧的「浏览器不可用」
        // 原因（wrapper 侧成功路径同样清除；本模块自启路径不经 wrapper，需自理）。
        let _ = std::fs::remove_file(paths::browser_last_error_json());
        Ok(())
    }

    /// 停止浏览器：停 screencast、优雅关闭 Chrome（CDP `Browser.close` 对 wrapper
    /// 启动的实例同样有效；失败则回退 kill 自启子进程）、清理协调文件并通知前端
    /// （emit `browser:stopped`，前端据此隐藏浏览器 Tab）。
    ///
    /// 与 `ensure_started` 共享 `start_mtx`（同序：先 start_mtx 再 inner）：stop 不会
    /// 在启动序列中途"看到空状态提前返回"而被随后完成的启动覆盖；代际 +1 让进行中的
    /// 启动在完成后自弃结果。
    pub async fn stop(&self) -> Result<(), String> {
        // 先参与 single-flight（与 ensure_started 同序获取，无死锁），保证 stop 与
        // 启动序列串行；再 +1 代际，让已被本 stop 打断的启动完成后自弃。
        let _start_guard = self.start_mtx.lock().await;
        self.stop_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let was_activated = self
            .activated
            .swap(false, std::sync::atomic::Ordering::SeqCst);

        let mut inner = self.inner.lock().await;
        let had_session = inner.session.is_some();
        if let (Some(session), Some(sid)) = (inner.session.as_ref(), inner.active_session.as_ref())
        {
            let _ = session
                .call(Some(sid), "Page.stopScreencast", json!({}))
                .await;
        }
        // 优先经 CDP 优雅关闭整个 Chrome（browser 级 Browser.close）——对 wrapper
        // 启动的实例（无子进程句柄）也生效；CDP 不可用时回退 kill 自启子进程。
        let closed_via_cdp = match inner.session.as_ref() {
            Some(s) => s.call(None, "Browser.close", json!({})).await.is_ok(),
            None => false,
        };
        let had_child = inner.child.is_some();
        if let Some(mut child) = inner.child.take() {
            if !closed_via_cdp {
                let _ = child.kill();
            }
            // wait 必须限期：此处同时持有 start_mtx 与 inner，`Browser.close` 被接受
            // 后进程退出仍可能任意缓慢（unload 处理/崩溃清理），无界同步 wait 会
            // 冻结整个 BrowserManager（命令/watch/后续 stop 全部排队，代际也救不了）。
            // 超时后强 kill 再等一次；仍不退（内核级卡死）则放弃等待——句柄随
            // Child drop 关闭，进程留待系统回收，锁先释放。
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while child.try_wait().map(|s| s.is_none()).unwrap_or(false) {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
        // Browser.close 失败（wedged）且无子进程句柄可 kill（wrapper 启动的实例）时，
        // 至少关闭 WS 截断读循环与帧推流，避免资源永久残留。
        if let Some(session) = inner.session.take() {
            let _ = session.close().await;
        }
        // 端口文件**只在本模块确已关掉 Chrome 时删除**（closed_via_cdp 或 kill 了
        // 自启 child，二者至少其一）。以下路径不得删：从未接入（session == None，
        // wrapper 的 Chrome 可能健康运行——`RunEvent::Exit` 会无条件走到这里）；
        // Browser.close 失败且无 child 句柄（Chrome wedged 但活着）。误删会让下一个
        // 启动者对同一 profile 双启 Chrome（死在单实例锁上，15s 探测失败循环）；
        // wrapper 侧 chromeChild exit 兜底也会清，无需本模块代劳。
        // start.lock 同理**只删 stale 残留**：活跃持有者（wrapper 正在启动中）的锁
        // 不可删，持有者正常启动完成后自删，崩溃残留由 60s stale 判定兜底。
        if closed_via_cdp || had_child {
            let _ = std::fs::remove_file(paths::browser_cdp_port_json());
        }
        if lock_file_stale(&paths::browser_start_lock()) {
            let _ = std::fs::remove_file(paths::browser_start_lock());
        }
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
        // 通知前端隐藏浏览器 Tab（main.jsx / BrowserView 监听 browser:stopped）。
        // 仅在浏览器确实运行过/激活过时通知：从未启动的 stop（如 RunEvent::Exit
        // 兜底路径）不产生伪事件。
        if had_session || was_activated {
            if let Some(app) = self.app.lock().clone() {
                let _ = app.emit("browser:stopped", json!({}));
            }
        }
        Ok(())
    }

    /// 前端浏览器视图的截图流门控：BrowserView 挂载时 `true`（开始/恢复推流）、
    /// 卸载时 `false`（停流）。模型用过浏览器但用户从未打开 Tab 的场景下，
    /// 不推流可避免 JPEG 编码 + base64 + IPC 在后台以显示帧率空转。
    pub async fn set_streaming(&self, enabled: bool) -> Result<(), String> {
        let was = self
            .streaming
            .swap(enabled, std::sync::atomic::Ordering::SeqCst);
        if enabled == was {
            return Ok(());
        }
        let inner = self.inner.lock().await;
        if let (Some(session), Some(sid)) = (inner.session.as_ref(), inner.active_session.as_ref())
        {
            if enabled {
                session
                    .call(Some(sid), "Page.startScreencast", screencast_params())
                    .await
                    .map_err(|e| format!("Page.startScreencast 失败: {e}"))?;
            } else {
                let _ = session
                    .call(Some(sid), "Page.stopScreencast", json!({}))
                    .await;
            }
        }
        Ok(())
    }

    /// `Target.targetCreated` 补激活（由事件循环调用）：全部标签页被关闭后
    /// （active 为空）模型经 MCP 新建标签页时，自动把截图流接到新页——否则
    /// 前端标签条刷新了但画面永远空白，且无人触发重附着。
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
        let streaming = self.streaming.load(std::sync::atomic::Ordering::SeqCst);
        if switch_screencast_locked(&mut inner, &sid, streaming)
            .await
            .is_ok()
        {
            inner.active_target = Some(target_id.to_string());
        }
    }

    /// `Target.targetDestroyed` 自愈（由事件循环调用）：激活标签页被 MCP/页面/// 脚本关闭时，切到剩余页并维持截图流；无剩余页则清空 active——下次
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
                    let streaming = self.streaming.load(std::sync::atomic::Ordering::SeqCst);
                    if switch_screencast_locked(&mut inner, &sid, streaming)
                        .await
                        .is_ok()
                    {
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

    /// 主进程退出时的同步兜底清理：**不依赖 async runtime**，直接 kill 本模块
    /// 自启的 Chrome 并清理协调文件。`RunEvent::Exit` 时 async `spawn` 的 stop()
    /// 与 teardown 竞态、几乎不会执行到（两次 CDP 调用各至多 30s），必须在此
    /// 同步截断自启进程；wrapper 启动的实例由 wrapper 自身的 chromeChild exit
    /// 兜底清理（`cleanup()` SIGTERM + 清端口文件），本方法对 wrapper 实例无句柄
    /// 可 kill，靠 Chrome 单实例 profile 锁与下次启动的端口探测/复用自愈。
    ///
    /// 锁竞争：若启动/停止序列正持 inner 锁（罕见，退出瞬间），try_lock 失败则
    /// 放弃同步清理，交由 spawn 的 stop() 尽力而为。
    pub fn shutdown_on_exit(&self) {
        // 先置退出标记：watch 下一轮退出、ensure_started 拒绝新启动——退出瞬间
        // 进行中的 watch 探测不得再拉起 Chrome（无主的孤儿进程）。
        self.shutting_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
        // 同时 bump stop 代际：进行中的启动序列（网络 await 阶段不持 inner 锁，
        // 下方 try_lock 会成功空转）在提交点前核对该代际，不等则丢弃结果并走
        // 失败清理——否则退出瞬间在飞的启动会把 Chrome 提交进已被清空的 inner，
        // 主进程退出后无人回收。
        self.stop_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let Ok(mut inner) = self.inner.try_lock() else {
            eprintln!("[browser] 退出时 inner 锁被占用，跳过同步清理");
            return;
        };
        // 同步 kill 自启 Chrome（std-only，无 await）：与 stop() 的 CDP 优雅路径
        // 不同，这里不依赖事件循环仍在运行。
        let had_child = inner.child.is_some();
        if let Some(mut child) = inner.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(task) = inner.loop_task.take() {
            task.abort();
        }
        if let Some(task) = inner.reader_task.take() {
            task.abort();
        }
        if let Some(session) = inner.session.take() {
            // 尽力关闭 WS 截断读循环（close 无超时兜底，此处同步环境只做尽力）。
            let session = Arc::clone(&session);
            tauri::async_runtime::spawn(async move { session.close().await });
        }
        inner.port = None;
        inner.active_session = None;
        inner.active_target = None;
        self.page_sessions.lock().clear();
        self.activated
            .store(false, std::sync::atomic::Ordering::SeqCst);
        // 协调文件：仅当本模块 kill 了自启 Chrome（端口文件随之失效）时才删。
        // 从未接入 / 接入的是 wrapper 实例（无句柄可 kill）时保留——wrapper 自身
        // 的 chromeChild exit 兜底会清理，误删会让下次启动对同一 profile 双启
        // （死在单实例锁上）。start.lock 是否删除取决于持有者——残留由下次启动
        // 的 stale 判定清理，这里不强行删（可能正被 wrapper 持有）。
        if had_child {
            let _ = std::fs::remove_file(paths::browser_cdp_port_json());
        }
    }

    /// 查询状态（前端挂载/轮询用）。`activeTab` 为激活标签页的 targetId
    /// （前端标签身份统一用 targetId；sessionId 每次 attach 都不同，不可作身份）。
    pub async fn status(&self) -> Value {
        let inner = self.inner.lock().await;
        let mut status = json!({
            "running": inner.session.is_some(),
            "port": inner.port,
            "activeTab": inner.active_target,
        });
        if let (Some(session), Some(sid)) = (inner.session.as_ref(), inner.active_session.as_ref())
        {
            if let Ok(v) = session
                .call(Some(sid), "Page.getNavigationHistory", json!({}))
                .await
            {
                let entries = v
                    .get("entries")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let current = v.get("currentIndex").and_then(Value::as_u64).unwrap_or(0);
                let url = entries
                    .get(current as usize)
                    .and_then(|e| e.get("url"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                status["url"] = json!(url);
            }
        }
        status
    }

    /// 标签页列表（实时枚举 page 类型 target；attach 复用缓存，防 session 泄漏）。
    pub async fn list_tabs(&self) -> Result<Vec<TabInfo>, String> {
        let inner = self.inner.lock().await;
        let session = inner
            .session
            .as_ref()
            .ok_or_else(|| "浏览器未启动".to_string())?;
        list_page_tabs(session, &self.page_sessions).await
    }

    /// 新建标签页并激活（截图流切换到新页）。
    pub async fn create_tab(&self, url: String) -> Result<(), String> {
        // 与 navigate 同款协议白名单：防 file:///javascript: 等本地/脚本协议被注入。
        if !is_allowed_url(&url) {
            return Err("仅支持 http/https/about:blank 协议".to_string());
        }
        let mut inner = self.inner.lock().await;
        let session = inner
            .session
            .as_ref()
            .ok_or_else(|| "浏览器未启动".to_string())?;
        let v = session
            .call(None, "Target.createTarget", json!({ "url": url }))
            .await
            .map_err(|e| format!("Target.createTarget 失败: {e}"))?;
        let target_id = v
            .get("targetId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
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
        let streaming = self.streaming.load(std::sync::atomic::Ordering::SeqCst);
        switch_screencast_locked(&mut inner, &sid, streaming).await?;
        self.page_sessions.lock().insert(target_id.clone(), sid);
        inner.active_target = Some(target_id);
        Ok(())
    }

    /// 关闭标签页。仅当关掉的是当前激活页时才自动切到第一个剩余页——关后台
    /// 标签页不应动用户正在看的页面。
    pub async fn close_tab(&self, target_id: String) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        let session = inner
            .session
            .clone()
            .ok_or_else(|| "浏览器未启动".to_string())?;
        session
            .call(None, "Target.closeTarget", json!({ "targetId": target_id }))
            .await
            .map_err(|e| format!("Target.closeTarget 失败: {e}"))?;
        // 崩溃的标签页缓存条目随即清理（target 已销毁，sessionId 不再有效）。
        self.page_sessions.lock().remove(&target_id);
        let closed_active = inner.active_target.as_deref() == Some(&target_id);
        if !closed_active {
            return Ok(());
        }
        // 激活页被关：枚举剩余标签页；枚举失败时保留 active 状态（标签页实际还在，
        // 清空会让帧流冻结且无人触发重附着），返回错误让前端提示。
        let tabs = list_page_tabs(&session, &self.page_sessions)
            .await
            .map_err(|e| format!("关标签页后枚举失败: {e}"))?;
        if let Some(first) = tabs.first() {
            let sid = self
                .page_sessions
                .lock()
                .get(&first.target_id)
                .cloned()
                .ok_or_else(|| "剩余标签页缺失 attach 缓存".to_string())?;
            let streaming = self.streaming.load(std::sync::atomic::Ordering::SeqCst);
            switch_screencast_locked(&mut inner, &sid, streaming).await?;
            inner.active_target = Some(first.target_id.clone());
        } else if let Some(sid) = inner.active_session.take() {
            let _ = session
                .call(Some(&sid), "Page.stopScreencast", json!({}))
                .await;
            inner.active_target = None;
        }
        Ok(())
    }

    /// 切换激活标签页（targetId）。
    pub async fn activate_tab(&self, target_id: String) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        let sid = self
            .page_sessions
            .lock()
            .get(&target_id)
            .cloned()
            .ok_or_else(|| "标签页会话不存在".to_string())?;
        let streaming = self.streaming.load(std::sync::atomic::Ordering::SeqCst);
        switch_screencast_locked(&mut inner, &sid, streaming).await?;
        inner.active_target = Some(target_id);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // 导航 / 交互
    // -----------------------------------------------------------------------

    /// 导航到指定 URL。
    pub async fn navigate(&self, url: String) -> Result<(), String> {
        let inner = self.inner.lock().await;
        let (session, sid) = active_locked(&inner)?;
        if !is_allowed_url(&url) {
            return Err("仅支持 http/https/about:blank 协议".to_string());
        }
        session
            .call(Some(&sid), "Page.navigate", json!({ "url": url }))
            .await
            .map_err(|e| format!("Page.navigate 失败: {e}"))?;
        Ok(())
    }

    pub async fn go_back(&self) -> Result<(), String> {
        self.history_step(-1).await
    }

    pub async fn go_forward(&self) -> Result<(), String> {
        self.history_step(1).await
    }

    async fn history_step(&self, delta: i64) -> Result<(), String> {
        let inner = self.inner.lock().await;
        let (session, sid) = active_locked(&inner)?;
        let v = session
            .call(Some(&sid), "Page.getNavigationHistory", json!({}))
            .await
            .map_err(|e| format!("getNavigationHistory 失败: {e}"))?;
        let entries = v
            .get("entries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let current = v.get("currentIndex").and_then(Value::as_u64).unwrap_or(0);
        let target = current as i64 + delta;
        if target < 0 || target >= entries.len() as i64 {
            return Ok(());
        }
        let entry_id = entries[target as usize]
            .get("id")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        session
            .call(
                Some(&sid),
                "Page.navigateToHistoryEntry",
                json!({ "entryId": entry_id }),
            )
            .await
            .map_err(|e| format!("navigateToHistoryEntry 失败: {e}"))?;
        Ok(())
    }

    pub async fn reload(&self) -> Result<(), String> {
        let inner = self.inner.lock().await;
        let (session, sid) = active_locked(&inner)?;
        session
            .call(Some(&sid), "Page.reload", json!({ "ignoreCache": false }))
            .await
            .map_err(|e| format!("Page.reload 失败: {e}"))?;
        Ok(())
    }

    /// 转发用户输入事件（前端 → CDP Input 域）。
    /// payload: { type: "click"|"move"|"wheel"|"key"|"insertText", ... }
    pub async fn input_event(&self, payload: Value) -> Result<(), String> {
        let inner = self.inner.lock().await;
        let (session, sid) = active_locked(&inner)?;
        let ty = payload
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| "缺少 type".to_string())?;
        // CDP modifiers 位掩码（Alt=1, Ctrl=2, Meta=4, Shift=8）：修饰键状态影响
        // 页面快捷键、文本选择（Shift+方向键）、表单反向聚焦（Shift+Tab）等行为。
        let modifiers = payload
            .get("modifiers")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        match ty {
            "click" => {
                let x = payload.get("x").and_then(Value::as_f64).unwrap_or(0.0);
                let y = payload.get("y").and_then(Value::as_f64).unwrap_or(0.0);
                let button = payload
                    .get("button")
                    .and_then(Value::as_str)
                    .unwrap_or("left");
                // CDP `buttons` 是位掩码（left=1/right=2/middle=4），须与 `button`
                // 一致，否则右/中键的修饰状态错误（页面按 buttons 判定组合键）。
                let buttons_mask = match button {
                    "right" => 2,
                    "middle" => 4,
                    _ => 1,
                };
                let click_count = payload
                    .get("clickCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(1);
                session
                    .call(
                        Some(&sid),
                        "Input.dispatchMouseEvent",
                        json!({
                            "type": "mousePressed",
                            "x": x, "y": y,
                            "button": button,
                            "buttons": buttons_mask,
                            "clickCount": click_count,
                            "modifiers": modifiers
                        }),
                    )
                    .await
                    .map_err(|e| format!("mousePressed 失败: {e}"))?;
                session
                    .call(
                        Some(&sid),
                        "Input.dispatchMouseEvent",
                        json!({
                            "type": "mouseReleased",
                            "x": x, "y": y,
                            "button": button,
                            "buttons": 0,
                            "clickCount": click_count,
                            "modifiers": modifiers
                        }),
                    )
                    .await
                    .map_err(|e| format!("mouseReleased 失败: {e}"))?;
            }
            "move" => {
                let x = payload.get("x").and_then(Value::as_f64).unwrap_or(0.0);
                let y = payload.get("y").and_then(Value::as_f64).unwrap_or(0.0);
                session
                    .call(
                        Some(&sid),
                        "Input.dispatchMouseEvent",
                        json!({ "type": "mouseMoved", "x": x, "y": y, "modifiers": modifiers }),
                    )
                    .await
                    .map_err(|e| format!("mouseMoved 失败: {e}"))?;
            }
            "wheel" => {
                let x = payload.get("x").and_then(Value::as_f64).unwrap_or(0.0);
                let y = payload.get("y").and_then(Value::as_f64).unwrap_or(0.0);
                let dx = payload.get("deltaX").and_then(Value::as_f64).unwrap_or(0.0);
                let dy = payload.get("deltaY").and_then(Value::as_f64).unwrap_or(0.0);
                session
                    .call(
                        Some(&sid),
                        "Input.dispatchMouseEvent",
                        json!({
                            "type": "mouseWheel",
                            "x": x, "y": y,
                            "deltaX": dx, "deltaY": dy,
                            "modifiers": modifiers
                        }),
                    )
                    .await
                    .map_err(|e| format!("mouseWheel 失败: {e}"))?;
            }
            "key" => {
                let key = payload.get("key").and_then(Value::as_str).unwrap_or("");
                let code = payload.get("code").and_then(Value::as_str).unwrap_or("");
                let text = payload.get("text").and_then(Value::as_str).unwrap_or("");
                let key_code = payload.get("keyCode").and_then(Value::as_u64).unwrap_or(0);
                session
                    .call(
                        Some(&sid),
                        "Input.dispatchKeyEvent",
                        json!({
                            "type": "keyDown",
                            "key": key,
                            "code": code,
                            "text": text,
                            "windowsVirtualKeyCode": key_code,
                            "nativeVirtualKeyCode": key_code,
                            "modifiers": modifiers
                        }),
                    )
                    .await
                    .map_err(|e| format!("keyDown 失败: {e}"))?;
                session
                    .call(
                        Some(&sid),
                        "Input.dispatchKeyEvent",
                        json!({ "type": "keyUp", "key": key, "code": code, "modifiers": modifiers }),
                    )
                    .await
                    .map_err(|e| format!("keyUp 失败: {e}"))?;
            }
            "insertText" => {
                let text = payload.get("text").and_then(Value::as_str).unwrap_or("");
                session
                    .call(Some(&sid), "Input.insertText", json!({ "text": text }))
                    .await
                    .map_err(|e| format!("insertText 失败: {e}"))?;
            }
            other => return Err(format!("不支持的输入事件类型: {other}")),
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Chrome 协调启动
    // -----------------------------------------------------------------------

    async fn acquire_or_start_chrome(&self) -> Result<(u16, Option<Child>), String> {
        // 1) 端口文件复用
        if let Some(port) = live_port().await {
            return Ok((port, None));
        }

        // 2) 拿独占锁（与 wrapper 的 `openSync(lock,'wx')` 同语义）。
        //    锁文件内容首行为持有者 pid；mtime 超过 60s 视为 stale（持有者崩溃/
        //    被 kill 后残留），可抢占删除，避免永久死锁。
        std::fs::create_dir_all(paths::browser_home())
            .map_err(|e| format!("创建浏览器目录失败: {e}"))?;
        crate::platform::os::make_private_dir(&paths::browser_home());
        let lock_path = paths::browser_start_lock();
        let lock_file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(f) => f,
            Err(_) => {
                // 锁被占：等待持有者完成（最多 20s），期间若端口已可用则复用；
                // 锁文件 stale（持有者已死）时抢占删除后重试。
                let deadline = std::time::Instant::now() + Duration::from_secs(20);
                loop {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    if let Some(port) = live_port().await {
                        return Ok((port, None));
                    }
                    if std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&lock_path)
                        .is_ok()
                    {
                        break;
                    }
                    if lock_file_stale(&lock_path) {
                        eprintln!("[browser] 启动锁 stale，抢占删除");
                        let _ = std::fs::remove_file(&lock_path);
                        continue;
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err("等待浏览器启动锁超时".to_string());
                    }
                }
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&lock_path)
                    .map_err(|e| format!("打开锁文件失败: {e}"))?
            }
        };
        // 记录持有者 pid（诊断 + 供 stale 判定）。
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&lock_path)
                .map_err(|e| format!("写锁文件失败: {e}"))?;
            let _ = writeln!(f, "{}", std::process::id());
            let _ = f.flush();
        }

        // 3) 持锁：二次确认 → 自启
        let result: Result<(u16, Option<Child>), String> = async {
            if let Some(port) = live_port().await {
                return Ok((port, None));
            }
            let port = pick_free_port().await?;
            let chrome = find_chrome().ok_or_else(|| "未找到 Chrome/Chromium".to_string())?;
            let child = start_chrome(&chrome, port)?;
            if !probe_cdp(port, Duration::from_secs(15)).await {
                // Chrome 已 spawn 但 CDP 未就绪：先杀掉再报错，避免孤儿 Chrome
                // 占住 profile 单实例锁导致后续所有启动尝试反复失败（需手动杀进程
                // 才能恢复）。
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                return Err("Chrome 已启动但 CDP 未就绪".to_string());
            }
            if let Err(e) = write_port_file(port, "app") {
                // 端口文件写失败（磁盘满/权限等）：kill 已启动的 Chrome。std Child
                // drop 不终止进程，放任会留下无端口文件的孤儿 Chrome，占住 profile
                // 单实例锁让后续启动反复 15s 失败（需手动杀进程恢复）。
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
            Ok((port, Some(child)))
        }
        .await;

        drop(lock_file);
        let _ = std::fs::remove_file(&lock_path);
        result
    }
}

// ---------------------------------------------------------------------------
// 事件循环：screencast 帧 → ack + 转发；导航事件 → 转发
// ---------------------------------------------------------------------------
async fn run_event_loop(
    app: AppHandle,
    pages: PageSessions,
    mut events: tokio::sync::mpsc::Receiver<cdp::CdpEvent>,
) {
    use cdp::CdpEvent;
    // 事件以 CDP sessionId 路由，前端身份是 targetId：反查后随 payload 下发。
    let target_of = |sid: &Option<String>| -> Option<String> {
        let sid = sid.as_deref()?;
        pages
            .lock()
            .iter()
            .find_map(|(t, s)| (s == sid).then(|| t.clone()))
    };
    while let Some(ev) = events.recv().await {
        match ev {
            CdpEvent::Event {
                session_id,
                method,
                params,
            } => match method.as_str() {
                "Page.screencastFrame" => {
                    // ack 已在读循环侧完成（先 ack 再入队，防通道满丢帧导致
                    // in-flight 泄漏、截图流停摆，见 cdp.rs）——此处只负责转发。
                    let data = params.get("data").and_then(Value::as_str).unwrap_or("");
                    let metadata = params.get("metadata").cloned().unwrap_or(json!({}));
                    let payload = json!({ "data": data, "metadata": metadata, "tab": target_of(&session_id) });
                    let _ = app.emit("browser:frame", &payload);
                }
                "Page.frameNavigated" => {
                    // 只对主 frame 驱动地址栏：iframe 的 frameNavigated 不应覆盖
                    // 地址栏/导航状态（父 frame 会另发一次主 frame 事件）。
                    let is_iframe = params
                        .pointer("/frame/parentId")
                        .and_then(Value::as_str)
                        .is_some();
                    if !is_iframe {
                        let url = params
                            .pointer("/frame/url")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let payload = json!({ "url": url, "tab": target_of(&session_id) });
                        let _ = app.emit("browser:navigation", &payload);
                    }
                }
                "Target.targetCreated" | "Target.targetDestroyed" => {
                    // browser 级 Target 事件包含 iframe/worker 等非页面 target：不过滤
                    // 会让前端对每次 iframe 增删都触发一次标签页全量枚举（经 CDP
                    // 往返），密集页面下事件风暴。
                    let target_type = params
                        .pointer("/targetInfo/type")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if target_type != "page" {
                        continue;
                    }
                    // 激活页被（MCP/页面脚本）销毁时先自愈切换，再通知前端刷新——
                    // 否则截图流冻结在已销毁 target 的最后一帧且无人恢复
                    // （session 存活，watch 崩溃恢复不触发）。
                    if method == "Target.targetDestroyed" {
                        if let Some(tid) = params
                            .pointer("/targetInfo/targetId")
                            .and_then(Value::as_str)
                        {
                            app.state::<BrowserManager>().on_target_destroyed(tid).await;
                        }
                    } else {
                        // 全部标签页关闭后模型新建标签页：自动补激活新页。
                        if let Some(tid) = params
                            .pointer("/targetInfo/targetId")
                            .and_then(Value::as_str)
                        {
                            app.state::<BrowserManager>().on_target_created(tid).await;
                        }
                    }
                    let payload = json!({ "event": method, "params": params });
                    let _ = app.emit("browser:tabs-changed", &payload);
                }
                _ => {}
            },
        }
    }
}

// ---------------------------------------------------------------------------
// 内部工具（free functions，便于无 &self 时调用）
// ---------------------------------------------------------------------------

fn active_locked(inner: &Inner) -> Result<(&CdpSession, String), String> {
    let session = inner
        .session
        .as_ref()
        .ok_or_else(|| "浏览器未启动".to_string())?;
    let sid = inner
        .active_session
        .clone()
        .ok_or_else(|| "没有激活的标签页".to_string())?;
    Ok((session.as_ref(), sid))
}

/// 导航/新建标签页的 URL 协议白名单（UI 路径）：http/https/about:blank，
/// 大小写不敏感（与前端地址栏预检 `/^https?:\/\//i` 同口径）；file:/
/// javascript:/data:/chrome: 等本地/脚本协议一律拒绝（fail-closed）。
/// 注意：只覆盖 app 侧命令；模型 MCP 路径（chrome-devtools-mcp）的导航
/// 不受此限——网页内容不可信的红线在 bundle instructions 中声明。
fn is_allowed_url(url: &str) -> bool {
    url == "about:blank"
        || url
            .get(..7)
            .map(|p| p.eq_ignore_ascii_case("http://"))
            .unwrap_or(false)
        || url
            .get(..8)
            .map(|p| p.eq_ignore_ascii_case("https://"))
            .unwrap_or(false)
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

/// 端口文件有效且 CDP 存活时返回端口（live 探测）。
async fn live_port() -> Option<u16> {
    let p = port_file()?;
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

/// 切换截图流到指定 session：停旧流 → 启新流（`streaming` 为 false 时只切换
/// active 指向、不启新流——前端视图未打开不推流）。非事务边界：新流启动失败时
/// 恢复旧流（否则旧流已停、active 仍指向它，帧流静默死亡且无人自愈——
/// session 存活，watch 崩溃恢复不会触发）。
async fn switch_screencast_locked(
    inner: &mut Inner,
    sid: &str,
    streaming: bool,
) -> Result<(), String> {
    let session = inner
        .session
        .as_ref()
        .ok_or_else(|| "浏览器未启动".to_string())?;
    let old = inner.active_session.clone();
    if let Some(old) = old.as_deref() {
        if old != sid {
            let _ = session
                .call(Some(old), "Page.stopScreencast", json!({}))
                .await;
        }
    }
    let start = async {
        session
            .call(Some(sid), "Page.enable", json!({}))
            .await
            .map_err(|e| format!("Page.enable 失败: {e}"))?;
        if streaming {
            session
                .call(Some(sid), "Page.startScreencast", screencast_params())
                .await
                .map_err(|e| format!("Page.startScreencast 失败: {e}"))?;
        }
        Ok::<(), String>(())
    }
    .await;
    match start {
        Ok(()) => {
            inner.active_session = Some(sid.to_string());
            Ok(())
        }
        Err(e) => {
            // 恢复旧流：失败只影响本次切换（返回错误），不留"两停"状态。
            if streaming {
                if let Some(old) = old.as_deref() {
                    if old != sid {
                        let _ = session
                            .call(Some(old), "Page.startScreencast", screencast_params())
                            .await;
                    }
                }
            }
            Err(e)
        }
    }
}

/// screencast 参数统一出处（帧宽 1280 与 `--window-size=1280,800` 对应；
/// 前端坐标换算以帧 metadata 的 viewport CSS 像素为基准）。
fn screencast_params() -> Value {
    json!({ "format": "jpeg", "quality": 70, "everyNthFrame": 1, "maxWidth": 1280 })
}

// ---------------------------------------------------------------------------
// Chrome 探测 / 启动 / 端口协调
// ---------------------------------------------------------------------------

fn find_chrome() -> Option<PathBuf> {
    // 候选表与通用探测都在 platform::os 适配层（runtime_bundle 门控同用一份，
    // 避免双份实现漂移）。
    crate::platform::os::find_chrome()
}

async fn probe_cdp(port: u16, timeout: Duration) -> bool {
    let url = format!("http://127.0.0.1:{port}/json/version");
    tokio::time::timeout(timeout, reqwest::get(&url))
        .await
        .ok()
        .and_then(|r| r.ok())
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn pick_free_port() -> Result<u16, String> {
    use rand::Rng;
    let base = 9222 + rand::rng().random_range(0..3000);
    for port in base..(base + 200) {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
        {
            return Ok(port);
        }
    }
    Ok(base)
}

fn start_chrome(chrome: &Path, port: u16) -> Result<Child, String> {
    let profile = paths::browser_profile_dir();
    std::fs::create_dir_all(&profile).map_err(|e| format!("创建 profile 目录失败: {e}"))?;
    // profile 内含登录会话/Cookie/缓存：收紧为仅当前用户可访问（Windows 无
    // POSIX 语义为 no-op，靠用户目录 ACL）。
    crate::platform::os::make_private_dir(&profile);
    let mut cmd = Command::new(chrome);
    cmd.arg(format!("--remote-debugging-port={port}"));
    // CDP 无鉴权、可控制整个浏览器：显式绑定回环，不依赖各浏览器对
    // --remote-debugging-port 的默认绑定地址（默认虽为 127.0.0.1，显式更稳）。
    cmd.arg("--remote-debugging-address=127.0.0.1");
    cmd.arg(format!("--user-data-dir={}", profile.display()));
    cmd.args([
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-extensions",
        "--disable-component-update",
        "--disable-background-networking",
        "--disable-sync",
        "--metrics-recording-only",
        "--noerrdialogs",
        "--mute-audio",
        "--disable-features=Translate,MediaRouter",
        "--window-position=-32000,-32000", // 有头渲染但窗口在屏外（品悟 Tab 是唯一视图）
        "--window-size=1280,800",
        "about:blank",
    ]);
    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null());
    cmd.spawn().map_err(|e| format!("启动 Chrome 失败: {e}"))
}

fn port_file() -> Option<u16> {
    let raw = std::fs::read_to_string(paths::browser_cdp_port_json()).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    // 显式校验合法端口范围：损坏/他人写入的值（如 65536+k）经 `as u16` 会静默
    // 回绕到任意端口，探测错误端点（多耗 ~10s 后才走 stale 清理）。
    v.get("port")
        .and_then(Value::as_u64)
        .filter(|p| (1..=65535).contains(p))
        .map(|p| p as u16)
}

/// 启动锁是否 stale：mtime 超过 60s 即视为持有者崩溃/被杀后的残留。
/// 锁持有者正常持有不超过 ~35s（等锁 20s + CDP 探测 15s），60s 判定足够宽松，
/// 不会误抢正常持锁者；残留锁则被抢占删除，避免双方永久死锁。
fn lock_file_stale(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    modified
        .elapsed()
        .map(|age| age > Duration::from_secs(60))
        .unwrap_or(false)
}

fn write_port_file(port: u16, owner: &str) -> Result<(), String> {
    let path = paths::browser_cdp_port_json();
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let data = json!({
        "port": port,
        "pid": std::process::id(),
        "owner": owner,
        "started_at": chrono::Utc::now().timestamp_millis(),
    });
    // tmp 名带 pid：多进程（app 实例/wrapper）并发写同一端口文件时互不覆盖
    // （wrapper 侧用 `.tmp`，见 browser-wrapper.mjs）。
    let tmp = path.with_extension(format!("json.rust-tmp-{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(&data).unwrap())
        .map_err(|e| format!("写端口文件失败: {e}"))?;
    // CDP 无鉴权：收紧端口文件权限，同机其他本地用户不应能读到端口坐标
    // （与 wrapper 的 chmod 0o600 一致；平台差异在 platform::os 适配层实现）。
    crate::platform::os::make_private_file(&tmp);
    std::fs::rename(&tmp, &path).map_err(|e| format!("落盘端口文件失败: {e}"))
}
