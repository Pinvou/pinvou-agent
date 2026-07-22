//! 多 session 并发的 engine 池。
//!
//! 旧模型:整个进程一个 Engine,切 session 靠 `Op::SyncSession` 整体替换内部状态
//! → 同一时刻只能服务一个 session,且切走正在跑的 session 会串台。
//!
//! 新模型:**每个 session 一个独立 Engine**(底座 `spawn_engine` 是独立工厂,见
//! [`AppEngine::spawn_for_session`])。本池按 `session_id` 管理这些 engine 的生命周期:
//!  - **lazy spawn**:首次给某 session 发消息时才 spawn(带该 session 专属 workspace +
//!    instructions);已有磁盘历史的 session 在 spawn 后用一次性 `SyncSession` 注水。
//!  - **keep-alive**:spawn 后常驻,切 session 不销毁(后台 session 继续跑各自的 turn)。
//!  - **evict**:删 session 时回收(cancel 在跑的 turn + Shutdown engine + abort forwarder)。
//!
//! 池本身是 Tauri State;`commands.rs` 里的 chat / cancel / submit_user_input 等都带
//! `session_id` 路由到对应 engine。
//!
//! 并发说明:`entries` 用 `tokio::Mutex`,`get_or_spawn` 全程持锁(spawn 很快,只建
//! channel + spawn task,无网络),从根上避免「同 session 并发 spawn 两个 engine」的
//! TOCTOU。不同 session 的发送只在各自首次 spawn 的瞬间串行,spawn 完即各自并发跑。

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use anyhow::{bail, Context, Result};
use deepseek_tui::core::events::TurnOutcomeStatus;
use deepseek_tui::core::ops::Op;
use deepseek_tui::models::{ContentBlock, Message};
use deepseek_tui::tools::shell::{
    new_shared_shell_manager, SharedShellManager, ShellJobSnapshot, ShellResult,
};
use deepseek_tui::tools::spec::ToolSpec;
use deepseek_tui::tools::user_input::UserInputResponse;
use deepseek_tui::tui::app::AppMode;
use parking_lot::Mutex as SyncMutex;
use tauri::async_runtime::JoinHandle;
use tauri::AppHandle;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::platform::prefs::{SavedModel, UserPrefs};
use crate::features::sessions::{ScheduledRunProfile, SessionStore};
use crate::features::assistant::platform::bridge::Pinvou3Bridge;
use crate::features::assistant::engine::{AppEngine, EngineTurnSignal, TurnLifecycle};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduledTurnCompletion {
    pub turn_id: String,
    pub status: TurnOutcomeStatus,
    pub error: Option<String>,
    pub cancel_requested: bool,
}

struct ScheduledUnattendedGuard(Arc<AtomicBool>);

impl ScheduledUnattendedGuard {
    fn enter(flag: Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::Release);
        Self(flag)
    }
}

impl Drop for ScheduledUnattendedGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Clone, Default)]
struct SessionTurnLocks {
    locks: Arc<Mutex<HashMap<String, Weak<Mutex<()>>>>>,
}

impl SessionTurnLocks {
    async fn for_session(&self, session_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().await;
        locks.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = locks.get(session_id).and_then(Weak::upgrade) {
            return gate;
        }

        let gate = Arc::new(Mutex::new(()));
        locks.insert(session_id.to_string(), Arc::downgrade(&gate));
        gate
    }
}

/// Per-session turn state survives Engine entry removal, allowing cancel and
/// abnormal forwarder shutdown to converge on one authoritative terminal.
#[derive(Clone, Default)]
struct SessionTurnLifecycles {
    states: Arc<SyncMutex<HashMap<String, Arc<TurnLifecycle>>>>,
}

impl SessionTurnLifecycles {
    fn for_session(&self, session_id: &str) -> Arc<TurnLifecycle> {
        let mut states = self.states.lock();
        states
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(TurnLifecycle::default()))
            .clone()
    }

    fn get(&self, session_id: &str) -> Option<Arc<TurnLifecycle>> {
        self.states.lock().get(session_id).cloned()
    }

    fn remove(&self, session_id: &str) {
        self.states.lock().remove(session_id);
    }
}

#[derive(Clone, Default)]
struct SessionShellManagers {
    managers: Arc<SyncMutex<HashMap<String, SharedShellManager>>>,
}

impl SessionShellManagers {
    fn for_session(&self, session_id: &str, workspace: std::path::PathBuf) -> SharedShellManager {
        let mut managers = self.managers.lock();
        managers
            .entry(session_id.to_string())
            .or_insert_with(|| new_shared_shell_manager(workspace))
            .clone()
    }

    fn get(&self, session_id: &str) -> Option<SharedShellManager> {
        self.managers.lock().get(session_id).cloned()
    }

    fn remove(&self, session_id: &str) {
        self.managers.lock().remove(session_id);
    }
}

fn scheduled_profile_after_turn_gate(
    store: &SessionStore,
    session_id: &str,
    expected_task_id: &str,
) -> Result<ScheduledRunProfile> {
    let profile = store.scheduled_profile(session_id).with_context(|| {
        format!("Scheduled session '{session_id}' was deleted before the follow-up could start")
    })?;
    if profile.task_id != expected_task_id {
        bail!(
            "Scheduled session '{session_id}' changed owner from '{expected_task_id}' to '{}'",
            profile.task_id
        );
    }
    if !store.scheduled_session_exists(session_id) {
        bail!("Scheduled session '{session_id}' no longer exists");
    }
    Ok(profile)
}

async fn delete_scheduled_run_with_gate<F, Fut>(
    turn_locks: &SessionTurnLocks,
    store: &SessionStore,
    session_id: &str,
    expected_task_id: &str,
    evict_locked: F,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ()>,
{
    let turn_lock = turn_locks.for_session(session_id).await;
    let _turn = turn_lock.lock().await;
    evict_locked().await;
    store.delete_scheduled_run(session_id, expected_task_id)
}

/// 池里一个 session 的常驻条目:engine + 它专属的 event forwarder task。
struct EngineEntry {
    engine: AppEngine,
    /// 该 engine 的 event forwarder,evict 时 abort,避免僵尸 task 继续 emit。
    forwarder: JoinHandle<()>,
}

pub type EngineToolFactory =
    Arc<dyn Fn(&AppHandle, &str) -> Vec<Arc<dyn ToolSpec>> + Send + Sync + 'static>;
pub type ToolPolicy = Arc<dyn Fn(&AppHandle) -> Vec<String> + Send + Sync + 'static>;

fn should_sync_session(is_scheduled: bool, has_messages: bool) -> bool {
    is_scheduled || has_messages
}

/// 多 session engine 池。Tauri State 持有,`Clone` 廉价(内部全是 Arc)。
#[derive(Clone)]
pub struct EnginePool {
    entries: Arc<Mutex<HashMap<String, EngineEntry>>>,
    turn_locks: SessionTurnLocks,
    turn_lifecycles: SessionTurnLifecycles,
    shell_managers: SessionShellManagers,
    app: AppHandle,
    store: SessionStore,
    tool_factory: EngineToolFactory,
    tool_policy: ToolPolicy,
    /// 所有 session 共享一份已 boot 的 bridge(boot 会写盘 / 设 env,只能一次)。
    /// commands 读 model / workspace 也走这里。
    pub bridge: Pinvou3Bridge,
}

impl EnginePool {
    /// boot bridge(一次)并建空池。不预热任何 engine(lazy)。
    pub fn new(app: AppHandle, store: SessionStore) -> Result<Self> {
        Self::new_with_dependencies(
            app,
            store,
            Arc::new(|_, _| Vec::new()),
            Arc::new(|_| Vec::new()),
        )
    }

    pub fn new_with_dependencies(
        app: AppHandle,
        store: SessionStore,
        tool_factory: EngineToolFactory,
        tool_policy: ToolPolicy,
    ) -> Result<Self> {
        let bridge = Pinvou3Bridge::boot()?;
        Ok(Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            turn_locks: SessionTurnLocks::default(),
            turn_lifecycles: SessionTurnLifecycles::default(),
            shell_managers: SessionShellManagers::default(),
            app,
            store,
            tool_factory,
            tool_policy,
            bridge,
        })
    }

    pub fn compute_disallowed_tools(&self) -> Vec<String> {
        (self.tool_policy)(&self.app)
    }

    pub async fn refresh_disallowed_tools(&self) -> Vec<String> {
        let tools = self.compute_disallowed_tools();
        self.set_disallowed_all(tools.clone()).await;
        tools
    }

    /// 为 spawn 构造该 session 的 bridge:从 disk 读最新 prefs(模型列表/默认可能刚被
    /// GUI 改过),再按该 session 的显式 model_id 注入 session_model(没绑定则回退全局
    /// active)。绑定指向已删模型时 `model_by_id` 返回 None,自然回退 active。
    /// 这是「热切换不重启」的落点:改模型只写 disk + evict,下次 spawn 经此读到新配置。
    pub(crate) async fn fresh_bridge_for(&self, session_id: &str) -> Result<Pinvou3Bridge> {
        self.fresh_bridge_for_policy(session_id, false).await
    }

    async fn fresh_bridge_for_policy(
        &self,
        session_id: &str,
        scheduled_unattended: bool,
    ) -> Result<Pinvou3Bridge> {
        let started_at = std::time::Instant::now();
        log::info!("[engine_pool] fresh_bridge_for start sid={}", session_id);
        let mut b = self.bridge.clone();
        log::info!("[engine_pool] fresh_bridge_for bridge cloned sid={}", session_id);
        b.prefs = UserPrefs::load();
        log::info!("[engine_pool] fresh_bridge_for prefs loaded sid={}", session_id);
        let scheduled_profile = self.store.scheduled_profile(session_id);
        let interactive_model_override = self.store.session_model_override(session_id);
        let pins_scheduled_model = scheduled_profile.is_some()
            && (scheduled_unattended || interactive_model_override.is_none());
        b.session_model = resolve_spawn_model(
            &b.prefs.advanced.saved_models,
            scheduled_profile.as_ref(),
            interactive_model_override.as_deref(),
            scheduled_unattended,
        )?;
        log::info!(
            "[engine_pool] fresh_bridge_for session model resolved sid={} has_model={}",
            session_id,
            b.session_model.is_some()
        );
        let use_builtin_llmapi = b
            .effective_model_owned()
            .as_ref()
            .is_some_and(|model| model.is_builtin_llmapi());
        log::info!(
            "[engine_pool] fresh_bridge_for effective model checked sid={} use_builtin_llmapi={}",
            session_id,
            use_builtin_llmapi
        );
        if use_builtin_llmapi {
            log::info!(
                "[engine_pool] fresh_bridge_for ready_saved_model_local start sid={}",
                session_id
            );
            match crate::features::llmapi_hub::provisioning::ready_saved_model_from_local_binding_system() {
                Ok(model) => {
                    log::info!(
                        "[engine_pool] fresh_bridge_for ready_saved_model_local ok sid={} elapsed_ms={}",
                        session_id,
                        started_at.elapsed().as_millis()
                    );
                    b.session_model = Some(model);
                }
                Err(err) => {
                    log::warn!(
                        "[engine_pool] fresh_bridge_for ready_saved_model_local failed sid={} code={:?} elapsed_ms={} message={}",
                        session_id,
                        err.code,
                        started_at.elapsed().as_millis(),
                        err.message
                    );
                    eprintln!(
                        "[pinvou3-app] LLM API Hub model injection skipped: {}",
                        err.to_tauri_error()
                    );
                }
            }
        }
        // 本地 vLLM:发请求的 model 名以 vLLM 实际 served name 为准(探测 /v1/models),
        // 免去写死 qwen36_35b_256k 与 --served-model-name 不一致的 model_not_found。
        // 探测失败(vLLM 没起)保持配置值;云端 provider 不探测。
        if b.provider() == "vllm" {
            log::info!("[engine_pool] fresh_bridge_for vllm probe start sid={}", session_id);
            let (served, max_len) = crate::features::monitor::probe_vllm_model_info(&b.base_url()).await;
            log::info!(
                "[engine_pool] fresh_bridge_for vllm probe done sid={} served={:?} max_len={:?}",
                session_id,
                served,
                max_len
            );
            if let Some(served) = served.filter(|_| !pins_scheduled_model) {
                if let Some(mut m) = b.effective_model_owned() {
                    if m.model != served {
                        m.model = served;
                        b.session_model = Some(m);
                    }
                }
            }
            // 窗口探测:填给 bridge,build_engine_config 据此填 active_route_limits.context_tokens
            // + 按真实窗口推导压缩阈值。探测失败保持 None → 名字 hint 老路。
            b.probed_context_tokens = max_len;
        }
        log::info!(
            "[engine_pool] fresh_bridge_for done sid={} elapsed_ms={}",
            session_id,
            started_at.elapsed().as_millis()
        );
        Ok(b)
    }

    /// 取该 session 的 engine,没有就 spawn 一个。spawn 后若该 session 有磁盘历史
    /// 则一次性 `SyncSession` 把历史 messages 注水进新 engine(冷启动 / app 重启后
    /// 打开旧会话再发消息的场景)。
    pub async fn get_or_spawn(&self, session_id: &str) -> Result<AppEngine> {
        self.get_or_spawn_with_policy(session_id, false).await
    }

    /// Spawn policy for an unattended automation turn is deliberately distinct
    /// from an interactive continuation: the task profile remains authoritative
    /// even if the user temporarily selected another model while viewing it.
    async fn get_or_spawn_with_policy(
        &self,
        session_id: &str,
        scheduled_unattended: bool,
    ) -> Result<AppEngine> {
        let started_at = std::time::Instant::now();
        log::info!("[engine_pool] get_or_spawn start sid={}", session_id);
        log::info!("[engine_pool] get_or_spawn lock wait start sid={}", session_id);
        let mut entries = self.entries.lock().await;
        log::info!(
            "[engine_pool] get_or_spawn lock acquired sid={} elapsed_ms={}",
            session_id,
            started_at.elapsed().as_millis()
        );
        if let Some(entry) = entries.get(session_id) {
            log::info!(
                "[engine_pool] get_or_spawn reuse existing engine sid={} elapsed_ms={}",
                session_id,
                started_at.elapsed().as_millis()
            );
            return Ok(entry.engine.clone());
        }

        let is_scheduled = self.store.scheduled_profile(session_id).is_some();
        log::info!("[engine_pool] get_or_spawn spawn bridge start sid={}", session_id);
        let bridge = self
            .fresh_bridge_for_policy(session_id, scheduled_unattended)
            .await?;
        log::info!(
            "[engine_pool] get_or_spawn spawn_for_session start sid={} elapsed_ms={}",
            session_id,
            started_at.elapsed().as_millis()
        );
        let shell_workspace = self
            .store
            .scheduled_profile(session_id)
            .map(|profile| profile.workspace)
            .unwrap_or_else(|| bridge.session_workspace(session_id));
        let shell_manager = self.shell_managers.for_session(session_id, shell_workspace);
        let (engine, forwarder) = AppEngine::spawn_for_session(
            self.app.clone(),
            self.store.clone(),
            bridge,
            session_id,
            (self.tool_factory)(&self.app, session_id),
            self.compute_disallowed_tools(),
            self.turn_lifecycles.for_session(session_id),
            shell_manager,
        )
        .await?;
        log::info!(
            "[engine_pool] get_or_spawn spawn_for_session ok sid={} elapsed_ms={}",
            session_id,
            started_at.elapsed().as_millis()
        );

        // 普通新会话没有历史时可跳过；scheduled session 即使为空也必须先同步，
        // 让底座 Engine 的内部 session id 与预创建的持久化会话一致。
        log::info!("[engine_pool] get_or_spawn load history start sid={}", session_id);
        match self.store.load(session_id) {
            Ok(saved) => {
                log::info!(
                    "[engine_pool] get_or_spawn load history ok sid={} messages={}",
                    session_id,
                    saved.messages.len()
                );
                if should_sync_session(is_scheduled, !saved.messages.is_empty()) {
                log::info!("[engine_pool] get_or_spawn sync history start sid={}", session_id);
                    if let Err(error) = engine
                    .sync_session(session_id.to_string(), saved.messages)
                    .await
                {
                        if is_scheduled {
                            let _ = engine.handle.send(Op::Shutdown).await;
                            forwarder.abort();
                            return Err(error).with_context(|| {
                                format!("sync scheduled session {session_id} before its first turn")
                            });
                        }
                        eprintln!("[engine_pool] sync history for {session_id} failed: {error:?}");
                    log::warn!(
                        "[engine_pool] get_or_spawn sync history failed sid={} error={:?}",
                        session_id,
                            error
                    );
                }
                log::info!("[engine_pool] get_or_spawn sync history done sid={}", session_id);
            }
            }
            Err(error) if is_scheduled => {
                let _ = engine.handle.send(Op::Shutdown).await;
                forwarder.abort();
                return Err(error).with_context(|| {
                    format!("load scheduled session {session_id} before its first turn")
                });
            }
            Err(error) => {
                log::warn!(
                    "[engine_pool] get_or_spawn load history failed sid={} error={:#}",
                    session_id,
                    error
                );
            }
        }

        entries.insert(
            session_id.to_string(),
            EngineEntry {
                engine: engine.clone(),
                forwarder,
            },
        );
        log::info!(
            "[engine_pool] get_or_spawn inserted engine sid={} elapsed_ms={}",
            session_id,
            started_at.elapsed().as_millis()
        );
        Ok(engine)
    }

    /// 取已存在的 engine(不 spawn)。cancel / submit_user_input 等用:engine 没起
    /// 说明该 session 没在跑,这些操作天然是 no-op。
    pub async fn handle_for(&self, session_id: &str) -> Option<AppEngine> {
        self.entries
            .lock()
            .await
            .get(session_id)
            .map(|entry| entry.engine.clone())
    }

    /// 回收某 session 的 engine:cancel 在跑的 turn → Shutdown engine → abort forwarder。
    /// 删除 session 时调。
    pub async fn evict(&self, session_id: &str) {
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        self.evict_locked(session_id).await;
    }

    /// Atomically closes the live engine and removes a scheduled session under
    /// the same per-session turn gate used by initial and follow-up turns.
    /// A follow-up already queued on that gate observes the deletion and fails
    /// instead of lazily respawning the id as an ordinary chat.
    pub(crate) async fn delete_scheduled_run(
        &self,
        session_id: &str,
        expected_task_id: &str,
    ) -> Result<()> {
        let result = delete_scheduled_run_with_gate(
            &self.turn_locks,
            &self.store,
            session_id,
            expected_task_id,
            || self.evict_locked(session_id),
        )
        .await;
        if result.is_ok() {
            self.forget_session(session_id);
        }
        result
    }

    async fn evict_locked(&self, session_id: &str) {
        if let Some(entry) = self.entries.lock().await.remove(session_id) {
            if entry
                .engine
                .finish_reclaimed_turn(&self.app, session_id)
            {
                log::warn!(
                    "[engine_pool] emitted interrupted terminal before reclaim sid={}",
                    session_id
                );
                crate::features::assistant::timing::finish_turn(session_id, "Interrupted", None);
            }
            entry.engine.cancel_current();
            entry.forwarder.abort();
            if let Err(e) = entry.engine.handle.send(Op::Shutdown).await {
                eprintln!("[engine_pool] shutdown {session_id} failed: {e:?}");
            }
        }
    }

    pub(crate) fn forget_session(&self, session_id: &str) {
        self.turn_lifecycles.remove(session_id);
        self.shell_managers.remove(session_id);
    }

    pub async fn list_shell_tasks(&self, session_id: &str) -> Result<Vec<ShellJobSnapshot>> {
        let Some(manager) = self.shell_managers.get(session_id) else {
            return Ok(Vec::new());
        };
        tauri::async_runtime::spawn_blocking(move || {
            let mut manager = manager
                .lock()
                .map_err(|_| anyhow::anyhow!("Shell manager lock poisoned"))?;
            Ok(manager.list_jobs())
        })
        .await
        .map_err(|error| anyhow::anyhow!("list shell tasks join failed: {error}"))?
    }

    pub async fn cancel_shell_task(&self, session_id: &str, task_id: &str) -> Result<ShellResult> {
        let manager = self
            .shell_managers
            .get(session_id)
            .with_context(|| format!("No shell runtime for session '{session_id}'"))?;
        let task_id = task_id.to_string();
        tauri::async_runtime::spawn_blocking(move || {
            let mut manager = manager
                .lock()
                .map_err(|_| anyhow::anyhow!("Shell manager lock poisoned"))?;
            manager.kill(&task_id)
        })
        .await
        .map_err(|error| anyhow::anyhow!("cancel shell task join failed: {error}"))?
    }

    // ── 模型热切换(commands.rs 调用)──────────────────────────────

    /// 新建会话用的默认模型:取全局 active model 的(model 名, id)。从 disk 读最新
    /// (GUI 可能刚改过默认),失败回退 boot 快照。
    pub fn default_model_for_new_session(&self) -> (String, Option<String>) {
        let prefs = UserPrefs::load();
        match prefs.active_model() {
            Some(m) => (m.model.clone(), Some(m.id.clone())),
            None => (self.bridge.model(), None),
        }
    }

    /// 切某 session 的模型(聊天 chip 热切):写 per-session 绑定 + evict 该 session
    /// engine。下次发消息 get_or_spawn 用新模型重建(跨 provider 重建 client;历史靠
    /// SyncSession 注水)。`model_id = None` = 清除绑定回退全局默认。
    pub async fn switch_session_model(
        &self,
        session_id: &str,
        model_id: Option<String>,
    ) -> Result<()> {
        self.store.set_session_model_id(session_id, model_id)?;
        self.evict(session_id).await;
        Ok(())
    }

    // ── 高层路由(commands.rs 调用)─────────────────────────────────

    /// 发用户消息给指定 session 的 engine(没起则 lazy spawn)。
    pub async fn send_user_message(
        &self,
        session_id: &str,
        content: String,
        mode: AppMode,
        restrict_tools_for_turn: bool,
    ) -> Result<()> {
        let scheduled_profile = self.store.scheduled_profile(session_id);
        if scheduled_profile.is_none() && session_id.starts_with("sched-") {
            bail!("Scheduled session '{session_id}' no longer exists");
        }
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        if let Some(profile) = scheduled_profile {
            scheduled_profile_after_turn_gate(&self.store, session_id, &profile.task_id)?;
        }
        // Side B 卡片池: 该 session 加持了专家面具时,每 turn 注入轻锚点(短)维持身份。
        // 完整 body 已在加持首条消息一次性注入(commands::chat take_pending_persona_body)。
        // 在 pool 层解析,所有上层调用(chat / accept_plan)自动带上锚点。
        // 同一张卡派生两样每-turn 状态: ① 轻锚点(粘性身份) ② 是否清空工具表
        // (纯对话元卡如卡牌制造专家 → 本轮零工具,防它误写文件)。每 turn 实时读 active
        // persona,戴上即限 / 卸下即恢复 / 换卡按新卡走,无持久状态、无需 equip/unequip 同步。
        let active_card = self
            .store
            .active_persona_id(session_id)
            .and_then(|pid| crate::features::personas::get(&pid));
        let persona_reminder = active_card.as_ref().map(crate::features::personas::equip_anchor);
        let restrict_tools = active_card
            .as_ref()
            .map_or(false, |c| c.conversational_only);
        let restrict_tools = restrict_tools || restrict_tools_for_turn;
        self.get_or_spawn(session_id)
            .await?
            .send_user_message(content, mode, persona_reminder, restrict_tools)
            .await
    }

    /// Execute the initial turn for a pre-created scheduled session and wait
    /// for the authoritative terminal event produced by the existing engine
    /// forwarder. The engine is evicted afterwards, while the session itself
    /// remains durable and can later be opened or continued by the user.
    pub(crate) async fn run_scheduled_turn<F, Fut>(
        &self,
        session_id: &str,
        content: String,
        cancel: CancellationToken,
        mut on_started: F,
    ) -> Result<ScheduledTurnCompletion>
    where
        F: FnMut(&str) -> Fut + Send,
        Fut: Future<Output = Result<()>> + Send,
    {
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        let result = async {
            let profile = self
                .store
                .scheduled_profile(session_id)
                .with_context(|| format!("Scheduled session '{session_id}' has no profile"))?;
            // A user may have opened this conversation since the previous run.
            // Scheduled execution must rebuild from the latest task profile and
            // global model/provider settings instead of reusing that old client.
            self.evict_locked(session_id).await;
            let engine = match self.get_or_spawn_with_policy(session_id, true).await {
                Ok(engine) => engine,
                Err(engine_error) => {
                    if let Err(seed_error) = persist_scheduled_prompt(
                        self.store.clone(),
                        session_id.to_string(),
                        content.clone(),
                    )
                    .await
                    {
                        bail!(
                            "{engine_error:#}; additionally failed to preserve the scheduled prompt: {seed_error:#}"
                        );
                    }
                    return Err(engine_error);
                }
            };
            let _unattended =
                ScheduledUnattendedGuard::enter(engine.scheduled_unattended.clone());
            let mut turn_events = engine.subscribe_turns();
            persist_scheduled_prompt(
                self.store.clone(),
                session_id.to_string(),
                content.clone(),
            )
            .await?;
            if cancel.is_cancelled() {
                return Ok(ScheduledTurnCompletion {
                    turn_id: String::new(),
                    status: TurnOutcomeStatus::Interrupted,
                    error: None,
                    cancel_requested: true,
                });
            }
            engine.send_scheduled_message(content, &profile).await?;
            wait_for_scheduled_terminal(
                &mut turn_events,
                &engine,
                cancel,
                &mut on_started,
            )
            .await
        }
        .await;

        self.evict_locked(session_id).await;
        result
    }

    /// 取消指定 session 正在生成的回复。Engine 已不存在时，仅在池仍确认存在活动
    /// turn 时补发 Interrupted 权威终态；空闲会话取消保持 no-op。
    pub async fn cancel(&self, session_id: &str) {
        if let Some(engine) = self.handle_for(session_id).await {
            engine.cancel_current();
        } else if let Some(lifecycle) = self.turn_lifecycles.get(session_id) {
            let emitted = lifecycle.emit_terminal_once(
                &self.app,
                session_id,
                TurnOutcomeStatus::Interrupted,
                None,
            );
            if emitted.is_none() {
                log::info!(
                    "[engine_pool] cancel ignored for idle session without engine sid={}",
                    session_id
                );
                return;
            }
            log::warn!(
                "[engine_pool] cancel recovered active turn without engine sid={}",
                session_id
            );
            crate::features::assistant::timing::finish_turn(session_id, "Interrupted", None);
        } else {
            log::info!(
                "[engine_pool] cancel ignored for unknown idle session sid={}",
                session_id
            );
        }
    }

    /// pinvou3 工具开关(全局持久):把"被禁用的工具全名"(模型可见全名,小写)广播给
    /// **所有在跑的 session engine** → 写入各自 config.disallowed_tools,下一轮即隐藏。
    /// 没起的会话下次 spawn 时从持久列表读初值(build_engine_config),所以新窗口/新对话
    /// 都继承同一份禁用状态。
    pub async fn set_disallowed_all(&self, tools: Vec<String>) {
        let entries = self.entries.lock().await;
        for (sid, entry) in entries.iter() {
            if let Err(e) = entry
                .engine
                .handle
                .send(Op::SetDisallowedTools {
                    tools: tools.clone(),
                })
                .await
            {
                eprintln!("[engine_pool] set_disallowed_all {sid} failed: {e:?}");
            }
        }
    }

    /// 编辑/重发指定 session 最后一轮 user 消息。
    pub async fn edit_last_turn(&self, session_id: &str, new_message: String) -> Result<()> {
        let scheduled_profile = self.store.scheduled_profile(session_id);
        if scheduled_profile.is_none() && session_id.starts_with("sched-") {
            bail!("Scheduled session '{session_id}' no longer exists");
        }
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        if let Some(profile) = scheduled_profile {
            scheduled_profile_after_turn_gate(&self.store, session_id, &profile.task_id)?;
        }
        self.get_or_spawn(session_id)
            .await?
            .edit_last_turn(new_message)
            .await
    }

    /// 手动压缩指定 session 上下文。engine 没起则 no-op(无上下文可压)。
    pub async fn compact_now(&self, session_id: &str) -> Result<()> {
        if let Some(engine) = self.handle_for(session_id).await {
            engine.compact_now().await?;
        }
        Ok(())
    }

    /// 提交指定 session 的 request_user_input 选择。
    pub async fn submit_user_input(
        &self,
        session_id: &str,
        tool_call_id: String,
        response: UserInputResponse,
    ) -> Result<()> {
        if let Some(engine) = self.handle_for(session_id).await {
            engine.submit_user_input(tool_call_id, response).await?;
        }
        Ok(())
    }

    /// 取消指定 session 的 request_user_input。
    pub async fn cancel_user_input(&self, session_id: &str, tool_call_id: String) -> Result<()> {
        if let Some(engine) = self.handle_for(session_id).await {
            engine.cancel_user_input(tool_call_id).await?;
        }
        Ok(())
    }

    /// super permission 改动后调用。**无需热刷静态 prompt**——sudo 的开/关状态
    /// 已改由 `build_send_message_op` 每 turn 注入 `<system-reminder>`
    /// (见 `super_permission::turn_reminder`),`is_enabled()` 每次实时读 disk,
    /// 所以切开关下一 turn 自动生效。静态 prompt 里只剩一句中性指引(指向
    /// per-turn reminder),过不过时都不影响行为。
    ///
    /// 本函数保留为 no-op:调用点(set_super_permission)语义上"通知一下",
    /// 但实际生效靠 per-turn 注入,不依赖这里。
    pub async fn refresh_all_instructions(&self) {
        let live_count = self.entries.lock().await.len();
        eprintln!(
            "[engine_pool] sudo permission changed; {live_count} live session(s) — \
             new state takes effect next turn via per-turn system-reminder"
        );
    }
}

async fn persist_scheduled_prompt(
    store: SessionStore,
    session_id: String,
    prompt: String,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let saved = store.load(&session_id)?;
        if !saved.messages.is_empty() {
            bail!(
                "Scheduled initial session '{}' already contains messages",
                session_id
            );
        }
        store.update_messages(
            &session_id,
            vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: prompt,
                    cache_control: None,
                }],
            }],
        )
    })
    .await
    .context("Scheduled prompt persistence task failed")??;
    Ok(())
}

async fn wait_for_scheduled_terminal<F, Fut>(
    receiver: &mut tokio::sync::broadcast::Receiver<EngineTurnSignal>,
    engine: &AppEngine,
    cancel: CancellationToken,
    on_started: &mut F,
) -> Result<ScheduledTurnCompletion>
where
    F: FnMut(&str) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let mut active_turn_id: Option<String> = None;
    let mut cancel_requested = false;
    let mut cancel_deadline: Option<tokio::time::Instant> = None;
    loop {
        let cancel_timeout = async {
            match cancel_deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            signal = receiver.recv() => match signal {
                Ok(EngineTurnSignal::Started { turn_id }) => {
                    if let Some(active) = active_turn_id.as_deref() {
                        bail!("Engine started overlapping scheduled turns '{active}' and '{turn_id}'");
                    }
                    active_turn_id = Some(turn_id.clone());
                    on_started(&turn_id).await?;
                    if cancel_requested {
                        engine.handle.cancel_with_reason(
                            deepseek_tui::core::engine::CancelReason::External,
                        );
                    }
                }
                Ok(EngineTurnSignal::Terminal {
                    turn_id,
                    status,
                    error,
                }) if active_turn_id.as_deref() == Some(turn_id.as_str()) => {
                    return Ok(ScheduledTurnCompletion {
                        turn_id,
                        status,
                        error,
                        cancel_requested,
                    });
                }
                Ok(EngineTurnSignal::Terminal { .. }) => {}
                Ok(EngineTurnSignal::ForwarderStopped { error }) => bail!(error),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    bail!("Engine event stream closed before the scheduled turn completed")
                }
            },
            _ = cancel.cancelled(), if !cancel_requested => {
                cancel_requested = true;
                cancel_deadline = Some(
                    tokio::time::Instant::now() + std::time::Duration::from_secs(30),
                );
                if active_turn_id.is_some() {
                    engine.handle.cancel_with_reason(
                        deepseek_tui::core::engine::CancelReason::External,
                    );
                }
            }
            _ = cancel_timeout, if cancel_requested => {
                bail!("Timed out waiting for the scheduled turn to stop");
            }
        }
    }
}

fn resolve_scheduled_model(
    models: &[SavedModel],
    profile: &ScheduledRunProfile,
) -> Result<SavedModel> {
    if let Some(model_id) = profile.model_id.as_deref() {
        let selected = models
            .iter()
            .find(|model| model.id == model_id)
            .with_context(|| {
                format!("此任务绑定的 AI 模型配置已失效，请重新选择 AI 模型并保存任务。缺失配置：{model_id}")
            })?;
        if selected.model != profile.model {
            bail!(
                "此任务绑定的 AI 模型配置已变更，请重新选择 AI 模型并保存任务。配置 {model_id} 从 '{}' 变为 '{}'",
                profile.model,
                selected.model
            );
        }
        return Ok(selected.clone());
    }

    let mut matches = models.iter().filter(|model| model.model == profile.model);
    let selected = matches.next().with_context(|| {
        format!(
            "此任务绑定的 AI 模型已不可用，请重新选择 AI 模型并保存任务。模型：{}",
            profile.model
        )
    })?;
    if matches.next().is_some() {
        bail!(
            "此任务绑定的 AI 模型配置不唯一，请重新选择 AI 模型并保存任务。模型：{}",
            profile.model
        );
    }
    Ok(selected.clone())
}

fn resolve_spawn_model(
    models: &[SavedModel],
    scheduled_profile: Option<&ScheduledRunProfile>,
    interactive_model_override: Option<&str>,
    scheduled_unattended: bool,
) -> Result<Option<SavedModel>> {
    if scheduled_unattended {
        return scheduled_profile
            .map(|profile| resolve_scheduled_model(models, profile))
            .transpose();
    }
    if let Some(model_id) = interactive_model_override {
        return Ok(models.iter().find(|model| model.id == model_id).cloned());
    }
    scheduled_profile
        .map(|profile| resolve_scheduled_model(models, profile))
        .transpose()
}

#[cfg(test)]
mod scheduled_model_tests {
    use super::{
        delete_scheduled_run_with_gate, resolve_scheduled_model, resolve_spawn_model,
        scheduled_profile_after_turn_gate, should_sync_session, ScheduledUnattendedGuard,
        SessionShellManagers, SessionTurnLifecycles, SessionTurnLocks,
    };
    use crate::platform::prefs::{ModelPreset, SavedModel};
    use crate::features::sessions::{ScheduledRunMode, ScheduledRunProfile, SessionStore};
    use crate::platform::credential_store::{CredentialEditAction, CredentialState};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn model(id: &str, wire_name: &str) -> SavedModel {
        SavedModel {
            id: id.to_string(),
            name: id.to_string(),
            preset: ModelPreset::OpenaiCompatible,
            context_window_tokens: None,
            max_output_tokens: None,
            model: wire_name.to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            api_key: String::new(),
            credential_ref: None,
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None::<CredentialEditAction>,
        }
    }

    fn profile(model_id: Option<&str>, wire_name: &str) -> ScheduledRunProfile {
        ScheduledRunProfile {
            task_id: "task-1".to_string(),
            model: wire_name.to_string(),
            model_id: model_id.map(str::to_string),
            workspace: PathBuf::from("D:/workspace"),
            mode: ScheduledRunMode::Yolo,
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
        }
    }

    #[test]
    fn configured_model_is_resolved_by_stable_id_and_wire_name() {
        let models = vec![model("other", "other-model"), model("wanted", "wire-model")];
        let selected = resolve_scheduled_model(&models, &profile(Some("wanted"), "wire-model"))
            .expect("configured model");
        assert_eq!(selected.id, "wanted");
    }

    #[test]
    fn unattended_spawn_uses_task_model_despite_interactive_override() {
        let models = vec![
            model("task-model", "task-wire"),
            model("interactive-model", "interactive-wire"),
        ];
        let scheduled = profile(Some("task-model"), "task-wire");
        let unattended =
            resolve_spawn_model(&models, Some(&scheduled), Some("interactive-model"), true)
                .expect("unattended model")
                .expect("selected unattended model");
        assert_eq!(unattended.id, "task-model");

        let interactive =
            resolve_spawn_model(&models, Some(&scheduled), Some("interactive-model"), false)
                .expect("interactive model")
                .expect("selected interactive model");
        assert_eq!(interactive.id, "interactive-model");
    }

    #[test]
    fn deleted_or_changed_configured_model_never_falls_back_to_active() {
        let models = vec![
            model("active", "active-model"),
            model("wanted", "renamed-model"),
        ];
        assert!(resolve_scheduled_model(&models, &profile(Some("missing"), "wire-model")).is_err());
        assert!(resolve_scheduled_model(&models, &profile(Some("wanted"), "wire-model")).is_err());
    }

    #[test]
    fn legacy_profile_without_id_requires_one_unambiguous_wire_name() {
        let one = vec![model("one", "wire-model")];
        assert_eq!(
            resolve_scheduled_model(&one, &profile(None, "wire-model"))
                .expect("unique model")
                .id,
            "one"
        );
        let duplicates = vec![model("one", "wire-model"), model("two", "wire-model")];
        assert!(resolve_scheduled_model(&duplicates, &profile(None, "wire-model")).is_err());
    }

    #[test]
    fn scheduled_empty_session_is_synchronized_before_its_first_turn() {
        assert!(should_sync_session(true, false));
        assert!(should_sync_session(true, true));
        assert!(should_sync_session(false, true));
        assert!(!should_sync_session(false, false));
    }

    #[test]
    fn unattended_policy_is_scoped_to_the_executor_turn() {
        let flag = Arc::new(AtomicBool::new(false));
        {
            let _guard = ScheduledUnattendedGuard::enter(flag.clone());
            assert!(flag.load(Ordering::Acquire));
        }
        assert!(!flag.load(Ordering::Acquire));
    }

    #[test]
    fn session_shell_manager_is_reused_across_engine_rebuilds() {
        let managers = SessionShellManagers::default();
        let first = managers.for_session("session-1", PathBuf::from("D:/workspace-a"));
        let rebuilt = managers.for_session("session-1", PathBuf::from("D:/workspace-b"));
        assert!(Arc::ptr_eq(&first, &rebuilt));
        drop(first);
        assert!(
            Arc::strong_count(&rebuilt) >= 2,
            "the session registry must keep detached jobs alive after an Engine entry drops"
        );
        managers.remove("session-1");
        assert!(managers.get("session-1").is_none());
    }

    #[test]
    fn session_turn_lifecycle_survives_engine_entry_removal_without_faking_idle_cancel() {
        let lifecycles = SessionTurnLifecycles::default();
        let engine_lifecycle = lifecycles.for_session("session-1");
        engine_lifecycle.on_submitted();
        drop(engine_lifecycle);

        let pool_lifecycle = lifecycles
            .get("session-1")
            .expect("pool retains active lifecycle independently");
        assert!(pool_lifecycle.finish_once(|| {}).is_some());
        assert_eq!(
            pool_lifecycle.finish_once(|| panic!("must stay idle")),
            None
        );
        assert!(lifecycles.get("unknown-session").is_none());
        lifecycles.remove("session-1");
        assert!(lifecycles.get("session-1").is_none());
    }

    #[tokio::test]
    async fn scheduled_close_and_concurrent_followup_share_one_session_gate() {
        let locks = SessionTurnLocks::default();
        let scheduled_gate = locks.for_session("scheduled-session").await;
        let followup_gate = locks.for_session("scheduled-session").await;
        assert!(Arc::ptr_eq(&scheduled_gate, &followup_gate));

        let scheduled_guard = scheduled_gate.lock().await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), followup_gate.lock())
                .await
                .is_err()
        );
        drop(scheduled_guard);
        let _followup_guard =
            tokio::time::timeout(std::time::Duration::from_secs(1), followup_gate.lock())
                .await
                .expect("follow-up acquires only after scheduled close");
    }

    #[tokio::test]
    async fn scheduled_delete_wins_over_waiting_followup_without_resurrecting_state() {
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = std::env::temp_dir().join(format!(
            "pinvou3-engine-pool-delete-race-{}",
            std::process::id()
        ));
        let previous_home = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&home);
        std::env::set_var("PINVOU3_HOME", &home);

        let store = SessionStore::boot().expect("session store");
        let session_id = store
            .create_scheduled_run(ScheduledRunProfile {
                task_id: "task-delete-race".to_string(),
                model: "wire-model".to_string(),
                model_id: None,
                workspace: home.join("workspace"),
                mode: ScheduledRunMode::Agent,
                allow_shell: false,
                trust_mode: false,
                auto_approve: false,
            })
            .expect("scheduled session")
            .metadata
            .id;
        let locks = SessionTurnLocks::default();
        let gate = locks.for_session(&session_id).await;
        let blocker = gate.lock().await;
        let fake_engine_present = Arc::new(AtomicBool::new(true));

        let delete_locks = locks.clone();
        let delete_store = store.clone();
        let delete_id = session_id.clone();
        let delete_engine = fake_engine_present.clone();
        let delete = tokio::spawn(async move {
            delete_scheduled_run_with_gate(
                &delete_locks,
                &delete_store,
                &delete_id,
                "task-delete-race",
                || async move {
                    delete_engine.store(false, Ordering::Release);
                },
            )
            .await
        });
        tokio::task::yield_now().await;

        let followup_locks = locks.clone();
        let followup_store = store.clone();
        let followup_id = session_id.clone();
        let followup = tokio::spawn(async move {
            let gate = followup_locks.for_session(&followup_id).await;
            let _turn = gate.lock().await;
            scheduled_profile_after_turn_gate(&followup_store, &followup_id, "task-delete-race")
        });
        tokio::task::yield_now().await;
        drop(blocker);
        drop(gate);

        delete
            .await
            .expect("delete task joins")
            .expect("delete run");
        assert!(
            followup.await.expect("follow-up task joins").is_err(),
            "a follow-up already waiting on the gate must fail after deletion"
        );
        assert!(!fake_engine_present.load(Ordering::Acquire));
        assert!(!store.scheduled_session_exists(&session_id));
        assert!(store.scheduled_profile(&session_id).is_none());
        let probe = locks.for_session("turn-lock-prune-probe").await;
        assert!(!locks.locks.lock().await.contains_key(&session_id));
        drop(probe);

        match previous_home {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(home);
    }

    #[tokio::test]
    async fn one_shot_turn_locks_do_not_accumulate() {
        let locks = SessionTurnLocks::default();

        for index in 0..128 {
            let gate = locks.for_session(&format!("one-shot-{index}")).await;
            let _guard = gate.lock().await;
        }

        assert!(
            locks.locks.lock().await.len() <= 1,
            "dead per-session turn gates must be reclaimed"
        );
    }
}
