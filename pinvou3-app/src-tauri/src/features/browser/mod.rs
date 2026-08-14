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

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::platform::paths;

pub use cdp::CdpSession;

/// 单个页面标签页（flatten session）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TabInfo {
    pub target_id: String,
    pub session_id: String,
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
    app: parking_lot::Mutex<Option<AppHandle>>,
}

impl BrowserManager {
    pub fn new() -> Self {
        Self {
            inner: tokio::sync::Mutex::new(Inner::default()),
            start_mtx: tokio::sync::Mutex::new(()),
            stop_gen: std::sync::atomic::AtomicU64::new(0),
            activated: std::sync::atomic::AtomicBool::new(false),
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
        tokio::spawn(async move {
            let mut fail_count = 0u32;
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let mgr = app.state::<BrowserManager>();
                // 1) 已接入但 Chrome 失联（崩溃/被杀）→ 重置状态并通知前端。
                {
                    let mut inner = mgr.inner.lock().await;
                    if inner.session.is_some() {
                        let port = inner
                            .port
                            .or_else(|| inner.session.as_ref().map(|s| s.port()))
                            .unwrap_or(0);
                        if !probe_cdp(port, Duration::from_millis(800)).await {
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
                            if let Some(mut child) = inner.child.take() {
                                let _ = child.kill();
                                let _ = child.wait();
                            }
                            inner.port = None;
                            inner.active_session = None;
                            mgr.activated
                                .store(false, std::sync::atomic::Ordering::SeqCst);
                            let _ = app.emit("browser:stopped", json!({}));
                            // Chrome 已死：端口文件失效，清掉让下次启动干净重建。
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

    /// 确保专用 Chrome 已启动并接入 CDP 截图流。幂等：已连接则直接复用。
    pub async fn ensure_started(&self) -> Result<(), String> {
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
                // 不持 inner 锁做网络 await（attach 走 CDP 调用）；之后重新拿锁恢复截图流。
                drop(inner);
                let sid = attach_first_page(&session).await?;
                let mut inner = self.inner.lock().await;
                switch_screencast_locked(&mut inner, &sid).await?;
                return Ok(());
            }
        }

        // single-flight：整个启动序列持 start_mtx，并发调用者在此等待后复用
        // 已完成的状态，而不是各自再启动一遍（双事件循环/双截图流/句柄丢失）。
        let _start_guard = self.start_mtx.lock().await;
        // stop 代际快照：启动期间若 stop() 执行（代际 +1），完成后丢弃本次结果。
        let gen_at_start = self.stop_gen.load(std::sync::atomic::Ordering::SeqCst);
        {
            let inner = self.inner.lock().await;
            if inner.session.is_some() && inner.active_session.is_some() {
                return Ok(());
            }
        }

        // 1) 协调启动 Chrome（复用端口文件或自启）
        let (port, mut spawned_child) = self.acquire_or_start_chrome().await?;

        // 2-5) 连接 CDP / attach / 启域 / 截图流 / 事件循环。任一步失败时清理
        //     自启的 Chrome（若有），避免孤儿进程占住 profile 单实例锁。
        let boot: Result<(), String> = async {
            let connected = cdp::connect(port)
                .await
                .map_err(|e| format!("CDP 连接失败: {e:#}"))?;
            let session = connected.session;

            let session_id = attach_first_page(&session).await?;

            session
                .call(Some(&session_id), "Page.enable", json!({}))
                .await
                .map_err(|e| format!("Page.enable 失败: {e}"))?;
            session
                .call(
                    Some(&session_id),
                    "Page.startScreencast",
                    json!({
                        "format": "jpeg",
                        "quality": 70,
                        "everyNthFrame": 1,
                        "maxWidth": 1280
                    }),
                )
                .await
                .map_err(|e| format!("Page.startScreencast 失败: {e}"))?;

            let app = self
                .app
                .lock()
                .clone()
                .ok_or_else(|| "BrowserManager 未绑定 AppHandle".to_string())?;
            let loop_task =
                tokio::spawn(run_event_loop(app, Arc::clone(&session), connected.events));

            // 启动期间被 stop() 打断（代际已变）：丢弃本次结果，避免 stop 被吞、
            // 浏览器以无 UI 状态残留（watch 视 session alive 而不再重置）。
            if self.stop_gen.load(std::sync::atomic::Ordering::SeqCst) != gen_at_start {
                let _ = session.close().await;
                return Err("浏览器启动期间已被停止".to_string());
            }

            let mut inner = self.inner.lock().await;
            inner.port = Some(port);
            inner.child = spawned_child.take();
            inner.session = Some(session);
            inner.active_session = Some(session_id);
            inner.loop_task = Some(loop_task);
            inner.reader_task = Some(connected.reader_task);
            Ok(())
        }
        .await;

        if let Err(e) = &boot {
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
        self.activated
            .store(false, std::sync::atomic::Ordering::SeqCst);

        let mut inner = self.inner.lock().await;
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
        if let Some(mut child) = inner.child.take() {
            if !closed_via_cdp {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        // Browser.close 失败（wedged）且无子进程句柄可 kill（wrapper 启动的实例）时，
        // 至少关闭 WS 截断读循环与帧推流，避免资源永久残留。
        if let Some(session) = inner.session.take() {
            let _ = session.close().await;
        }
        // Chrome 已关（close/kill 至少一条路径生效）：删端口文件（wrapper 的
        // chromeChild exit 兜底也会清）。start.lock **只删 stale 残留**：活跃持有者
        // （wrapper 正在启动中）的锁不可删，否则第三方启动者可并发进入、对同一
        // profile 双启 Chrome（一个死在单实例锁上，15s 探测失败）。持有者正常
        // 启动完成后会自删锁；崩溃残留由 60s stale 判定兜底。
        let _ = std::fs::remove_file(paths::browser_cdp_port_json());
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
        // 通知前端隐藏浏览器 Tab（main.jsx / BrowserView 监听 browser:stopped）。
        if let Some(app) = self.app.lock().clone() {
            let _ = app.emit("browser:stopped", json!({}));
        }
        Ok(())
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
        let Ok(mut inner) = self.inner.try_lock() else {
            eprintln!("[browser] 退出时 inner 锁被占用，跳过同步清理");
            return;
        };
        // 同步 kill 自启 Chrome（std-only，无 await）：与 stop() 的 CDP 优雅路径
        // 不同，这里不依赖事件循环仍在运行。
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
        self.activated
            .store(false, std::sync::atomic::Ordering::SeqCst);
        // 协调文件：本模块自启实例已 kill（端口文件失效）；start.lock 是否删除
        // 取决于持有者——残留由下次启动的 stale 判定清理，这里不强行删（可能
        // 正被 wrapper 持有）。
        let _ = std::fs::remove_file(paths::browser_cdp_port_json());
    }

    /// 查询状态（前端挂载/轮询用）。
    pub async fn status(&self) -> Value {
        // 锁内只取快照（running/port/activeTab + clone 会话/sid），随后释放锁再做
        // CDP 调用：getNavigationHistory 经网络往返，最多 30s；持锁期间 stop() 被阻塞、
        // shutdown_on_exit 的 try_lock 直接放弃同步清理。
        let (mut status, active) = {
            let inner = self.inner.lock().await;
            let status = json!({
                "running": inner.session.is_some(),
                "port": inner.port,
                "activeTab": inner.active_session,
            });
            let active = match active_arc(&inner) {
                Ok(tuple) => Some(tuple),
                Err(_) => None,
            };
            (status, active)
        };
        if let Some((session, sid)) = active {
            if let Ok(v) = session
                .call(Some(&sid), "Page.getNavigationHistory", json!({}))
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

    /// 标签页列表（实时枚举 page 类型 target 并 attach）。
    pub async fn list_tabs(&self) -> Result<Vec<TabInfo>, String> {
        let inner = self.inner.lock().await;
        let session = inner
            .session
            .as_ref()
            .ok_or_else(|| "浏览器未启动".to_string())?;
        list_page_tabs(session).await
    }

    /// 新建标签页并激活（截图流切换到新页）。
    pub async fn create_tab(&self, url: String) -> Result<(), String> {
        // 与 navigate 同款协议白名单：防 file:///javascript: 等本地/脚本协议被注入。
        if !url.starts_with("http://") && !url.starts_with("https://") && url != "about:blank" {
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
        switch_screencast_locked(&mut inner, &sid).await?;
        Ok(())
    }

    /// 关闭标签页（若关的是激活页，自动切回第一个剩余页）。
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
        let tabs = list_page_tabs(&session).await.unwrap_or_default();
        if let Some(first) = tabs.first() {
            if inner.active_session.as_deref() != Some(&first.session_id) {
                switch_screencast_locked(&mut inner, &first.session_id).await?;
            }
        } else if let Some(sid) = inner.active_session.take() {
            let _ = session
                .call(Some(&sid), "Page.stopScreencast", json!({}))
                .await;
        }
        Ok(())
    }

    /// 切换激活标签页。
    pub async fn activate_tab(&self, session_id: String) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        switch_screencast_locked(&mut inner, &session_id).await
    }

    // -----------------------------------------------------------------------
    // 导航 / 交互
    // -----------------------------------------------------------------------

    /// 导航到指定 URL。
    pub async fn navigate(&self, url: String) -> Result<(), String> {
        if !url.starts_with("http://") && !url.starts_with("https://") && url != "about:blank" {
            return Err("仅支持 http/https/about:blank 协议".to_string());
        }
        let (session, sid) = {
            let inner = self.inner.lock().await;
            active_arc(&inner)?
        };
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
        let (session, sid) = {
            let inner = self.inner.lock().await;
            active_arc(&inner)?
        };
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
        let (session, sid) = {
            let inner = self.inner.lock().await;
            active_arc(&inner)?
        };
        session
            .call(Some(&sid), "Page.reload", json!({ "ignoreCache": false }))
            .await
            .map_err(|e| format!("Page.reload 失败: {e}"))?;
        Ok(())
    }

    /// 转发用户输入事件（前端 → CDP Input 域）。
    /// payload: { type: "click"|"move"|"wheel"|"key"|"insertText", ... }
    pub async fn input_event(&self, payload: Value) -> Result<(), String> {
        let (session, sid) = {
            let inner = self.inner.lock().await;
            active_arc(&inner)?
        };
        let ty = payload
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| "缺少 type".to_string())?;
        match ty {
            "click" => {
                let x = payload.get("x").and_then(Value::as_f64).unwrap_or(0.0);
                let y = payload.get("y").and_then(Value::as_f64).unwrap_or(0.0);
                let button = payload
                    .get("button")
                    .and_then(Value::as_str)
                    .unwrap_or("left");
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
                            "buttons": 1,
                            "clickCount": click_count
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
                            "clickCount": click_count
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
                        json!({ "type": "mouseMoved", "x": x, "y": y }),
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
                            "deltaX": dx, "deltaY": dy
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
                            "nativeVirtualKeyCode": key_code
                        }),
                    )
                    .await
                    .map_err(|e| format!("keyDown 失败: {e}"))?;
                session
                    .call(
                        Some(&sid),
                        "Input.dispatchKeyEvent",
                        json!({ "type": "keyUp", "key": key, "code": code }),
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
            write_port_file(port, "app")?;
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
    session: Arc<CdpSession>,
    mut events: tokio::sync::mpsc::Receiver<cdp::CdpEvent>,
) {
    use cdp::CdpEvent;
    while let Some(ev) = events.recv().await {
        match ev {
            CdpEvent::Event {
                session_id,
                method,
                params,
            } => match method.as_str() {
                "Page.screencastFrame" => {
                    // CDP 帧号是 integer（`Page.screencastFrame` 事件 params.sessionId）。
                    // 必须按数字原样回传 `Page.screencastFrameAck`，否则 Chrome 参数
                    // 校验失败、截图流握手失效（帧堆积后停止推流）。
                    let frame_sid = params.get("sessionId").and_then(Value::as_u64).unwrap_or(0);
                    let _ = session
                        .call(
                            session_id.as_deref(),
                            "Page.screencastFrameAck",
                            json!({ "sessionId": frame_sid }),
                        )
                        .await;
                    let data = params.get("data").and_then(Value::as_str).unwrap_or("");
                    let metadata = params.get("metadata").cloned().unwrap_or(json!({}));
                    let payload = json!({ "data": data, "metadata": metadata, "tab": session_id });
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
                        let payload = json!({ "url": url, "tab": session_id });
                        let _ = app.emit("browser:navigation", &payload);
                    }
                }
                "Target.targetCreated" | "Target.targetDestroyed" => {
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

/// 取激活会话的 `Arc` 与 sid（clone，非借用 Inner）。
/// 供需要跨 `.await` 调用 CDP 的只读命令使用：在锁内 clone 会话与 sid 后立即释放锁，
/// 避免卡住的 Chrome（单次 call 最多 30s）长时间持 inner 锁，进而阻塞 stop()/
/// shutdown_on_exit（后者为 try_lock，持锁期间直接放弃同步清理）。
fn active_arc(inner: &Inner) -> Result<(Arc<CdpSession>, String), String> {
    let session = inner
        .session
        .clone()
        .ok_or_else(|| "浏览器未启动".to_string())?;
    let sid = inner
        .active_session
        .clone()
        .ok_or_else(|| "没有激活的标签页".to_string())?;
    Ok((session, sid))
}

async fn attach_first_page(session: &CdpSession) -> Result<String, String> {
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
    Ok(sid)
}

/// 端口文件有效且 CDP 存活时返回端口（live 探测）。
async fn live_port() -> Option<u16> {
    let p = port_file()?;
    probe_cdp(p, Duration::from_millis(800)).await.then_some(p)
}

async fn list_page_tabs(session: &CdpSession) -> Result<Vec<TabInfo>, String> {
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
            let sid = match session
                .call(
                    None,
                    "Target.attachToTarget",
                    json!({ "targetId": target_id, "flatten": true }),
                )
                .await
            {
                Ok(v) => v
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                Err(_) => continue,
            };
            tabs.push(TabInfo {
                target_id,
                session_id: sid,
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

/// 切换截图流到指定 session：停旧流 → 启新流。
async fn switch_screencast_locked(inner: &mut Inner, sid: &str) -> Result<(), String> {
    let session = inner
        .session
        .as_ref()
        .ok_or_else(|| "浏览器未启动".to_string())?;
    if let Some(old) = inner.active_session.as_deref() {
        if old != sid {
            let _ = session
                .call(Some(old), "Page.stopScreencast", json!({}))
                .await;
        }
    }
    // 新会话的 enable + startScreencast 任一失败时，active_session 仍指向旧会话，
    // 但旧会话的截图流刚被 stop——此时 ensure_started 快速路径（session 与
    // active_session 均 Some）会直接返回 Ok 而无画面，前端冻结且无自愈触发。
    // 失败时重启旧会话截图流，保持「active_session 指向的会话必有运行中截图流」
    // 的不变量；仅当 Chrome 整体 wedged（旧会话也重启失败）时才退化为崩溃恢复场景。
    let switched = async {
        session
            .call(Some(sid), "Page.enable", json!({}))
            .await
            .map_err(|e| format!("Page.enable 失败: {e}"))?;
        session
            .call(
                Some(sid),
                "Page.startScreencast",
                json!({ "format": "jpeg", "quality": 70, "everyNthFrame": 1, "maxWidth": 1280 }),
            )
            .await
            .map_err(|e| format!("Page.startScreencast 失败: {e}"))?;
        Ok::<(), String>(())
    }
    .await;
    match switched {
        Ok(()) => {
            inner.active_session = Some(sid.to_string());
            Ok(())
        }
        Err(e) => {
            if let Some(old) = inner.active_session.as_deref() {
                if old != sid {
                    let _ = session
                        .call(
                            Some(old),
                            "Page.startScreencast",
                            json!({ "format": "jpeg", "quality": 70, "everyNthFrame": 1, "maxWidth": 1280 }),
                        )
                        .await;
                }
            }
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------
// Chrome 探测 / 启动 / 端口协调
// ---------------------------------------------------------------------------

fn find_chrome() -> Option<PathBuf> {
    // 平台候选表下沉到 platform::os 适配层（macos/linux/windows/unsupported 各一份），
    // 此处只做通用探测：绝对路径直接判存在，命令名经 PATH 解析。
    for c in crate::platform::os::chrome_candidates() {
        let p = Path::new(&c);
        if p.is_absolute() && p.exists() {
            return Some(p.to_path_buf());
        }
        if let Ok(paths) = std::env::var("PATH") {
            for dir in std::env::split_paths(&paths) {
                let cand = dir.join(&c);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
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
    v.get("port").and_then(Value::as_u64).map(|p| p as u16)
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
