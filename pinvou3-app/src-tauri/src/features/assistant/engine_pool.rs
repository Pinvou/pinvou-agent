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
//! 并发说明:运行时模型准备可能访问外部凭据服务,不能占用全局 `entries` 锁。每个
//! session 先通过独立 runtime lock 串行准备/比较/rebuild,再短暂持有 `entries` 完成
//! 本地 spawn,从根上避免同 session 双引擎,也不让慢凭据服务阻塞其他 session。

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use anyhow::{bail, Context, Result};
use deepseek_tui::core::events::TurnOutcomeStatus;
use deepseek_tui::core::ops::Op;
use deepseek_tui::models::{ContentBlock, Message};
use deepseek_tui::tools::shell::{ShellJobSnapshot, ShellResult};
use deepseek_tui::tools::spec::ToolSpec;
use deepseek_tui::tools::user_input::UserInputResponse;
use deepseek_tui::tui::app::AppMode;
use parking_lot::Mutex as SyncMutex;
use tauri::async_runtime::JoinHandle;
use tauri::AppHandle;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::features::assistant::engine::{
    AppEngine, EngineTurnSignal, TranscriptOperation, TurnLifecycle, TurnReservation,
};
use crate::features::assistant::platform::bridge::Pinvou3Bridge;
use crate::features::assistant::runtime_model::{
    ModelCredentialMode, PassthroughRuntimeModelProvider, PreparedRuntimeModel,
    RuntimeModelProvider, RuntimeModelRequest,
};
use crate::features::assistant::turn_shell_tasks::{SessionShellManagers, SessionTurnShellTasks};
use crate::features::sessions::{transcript_revision, ScheduledRunProfile, SessionStore};
use crate::platform::prefs::{SavedModel, UserPrefs};

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

/// EnginePool 预备 API(含测试覆盖,待 Tauri command 层接入);在 lib 生产视角下为 dead code。
#[allow(dead_code)]
async fn delete_chat_session_with_gate<F, Fut, G>(
    turn_locks: &SessionTurnLocks,
    store: &SessionStore,
    session_id: &str,
    evict_locked: F,
    forget: G,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ()>,
    G: FnOnce(),
{
    let turn_lock = turn_locks.for_session(session_id).await;
    let _turn = turn_lock.lock().await;
    evict_locked().await;
    store.delete(session_id)?;
    forget();
    Ok(())
}

async fn quiesce_engine_before_reclaim<C, S, SFut, F, Fut, T>(
    cancel_current: C,
    stop_forwarder: S,
    finish_reclaimed: F,
) -> T
where
    C: FnOnce(),
    S: FnOnce() -> SFut,
    SFut: Future<Output = ()>,
    F: FnOnce() -> Fut,
    Fut: Future<Output = T>,
{
    cancel_current();
    stop_forwarder().await;
    finish_reclaimed().await
}

/// 池里一个 session 的常驻条目:engine + 它专属的 event forwarder task。
struct EngineEntry {
    engine: AppEngine,
    /// 该 engine 的 event forwarder,evict 时 abort,避免僵尸 task 继续 emit。
    forwarder: JoinHandle<()>,
    /// 创建该 engine 时实际使用的运行时模型、提供器版本和本地模型修订号。
    runtime_model: PreparedRuntimeState,
    /// 引擎纪元（UNIX ms）：worker ledger 上"仍在跑"的记录只有在本纪元内
    /// 有过活动才算真的活着。底座重启加载只翻内存状态、不回写落盘 running
    /// （subagent/mod.rs 的 load 路径），少了这道甄别，父会话重建引擎后
    /// 上一进程的僵尸 worker 会重新显示"工作中"并被永久轮询。
    spawned_at_ms: u64,
}

#[derive(Clone, Default)]
struct ModelUpdateRevisions {
    revisions: Arc<SyncMutex<HashMap<String, u64>>>,
}

impl ModelUpdateRevisions {
    fn current(&self, model_id: &str) -> u64 {
        self.revisions.lock().get(model_id).copied().unwrap_or(0)
    }

    fn bump(&self, model_id: &str) -> u64 {
        let mut revisions = self.revisions.lock();
        let revision = revisions.entry(model_id.to_string()).or_default();
        *revision = revision.saturating_add(1);
        *revision
    }
}

#[derive(Clone, PartialEq, Eq)]
struct PreparedRuntimeState {
    prepared: PreparedRuntimeModel,
    model_update_revision: u64,
}

impl PreparedRuntimeState {
    fn new(prepared: PreparedRuntimeModel, model_update_revision: u64) -> Self {
        Self {
            prepared,
            model_update_revision,
        }
    }

    fn requires_rebuild_from(&self, previous: &Self) -> bool {
        self != previous
    }
}

pub type EngineToolFactory =
    Arc<dyn Fn(&AppHandle, &str) -> Vec<Arc<dyn ToolSpec>> + Send + Sync + 'static>;
pub type ToolPolicy = Arc<dyn Fn(&AppHandle) -> Vec<String> + Send + Sync + 'static>;

fn should_sync_session(_is_scheduled: bool, _has_messages: bool) -> bool {
    // SyncSession carries both transcript history and the authoritative Session
    // identity. An empty ordinary Session still needs it: otherwise the freshly
    // spawned Engine keeps its generated internal id, and every SessionUpdated
    // snapshot is rejected by the outer forwarder as belonging to another
    // Session. That leaves only the admitted user fallback durable while the
    // streamed assistant reply exists in memory alone.
    true
}

/// 多 session engine 池。Tauri State 持有,`Clone` 廉价(内部全是 Arc)。
#[derive(Clone)]
pub struct EnginePool {
    entries: Arc<Mutex<HashMap<String, EngineEntry>>>,
    runtime_model_locks: SessionTurnLocks,
    model_update_revisions: ModelUpdateRevisions,
    turn_locks: SessionTurnLocks,
    turn_lifecycles: SessionTurnLifecycles,
    shell_managers: SessionShellManagers,
    turn_shell_tasks: SessionTurnShellTasks,
    app: AppHandle,
    store: SessionStore,
    tool_factory: EngineToolFactory,
    tool_policy: ToolPolicy,
    runtime_model_provider: Arc<dyn RuntimeModelProvider>,
    /// 所有 session 共享一份已 boot 的 bridge(boot 会写盘 / 设 env,只能一次)。
    /// commands 读 model / workspace 也走这里。
    pub bridge: Pinvou3Bridge,
}

impl EnginePool {
    /// boot bridge(一次)并建空池。不预热任何 engine(lazy)。
    #[allow(dead_code)]
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
        Self::new_with_runtime_model_provider(
            app,
            store,
            tool_factory,
            tool_policy,
            Arc::new(PassthroughRuntimeModelProvider),
        )
    }

    pub fn new_with_runtime_model_provider(
        app: AppHandle,
        store: SessionStore,
        tool_factory: EngineToolFactory,
        tool_policy: ToolPolicy,
        runtime_model_provider: Arc<dyn RuntimeModelProvider>,
    ) -> Result<Self> {
        let bridge = Pinvou3Bridge::boot()?;
        Ok(Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            runtime_model_locks: SessionTurnLocks::default(),
            model_update_revisions: ModelUpdateRevisions::default(),
            turn_locks: SessionTurnLocks::default(),
            turn_lifecycles: SessionTurnLifecycles::default(),
            shell_managers: SessionShellManagers::default(),
            turn_shell_tasks: SessionTurnShellTasks::default(),
            app,
            store,
            tool_factory,
            tool_policy,
            runtime_model_provider,
            bridge,
        })
    }

    pub(crate) fn credential_mode_for(
        &self,
        model: Option<&SavedModel>,
        user_api_key_required: bool,
    ) -> ModelCredentialMode {
        match model {
            Some(model) => self
                .runtime_model_provider
                .credential_mode(model, user_api_key_required),
            None if user_api_key_required => ModelCredentialMode::UserManaged,
            None => ModelCredentialMode::None,
        }
    }

    /// 模型配置或用户托管凭据保存成功后调用。只递增非敏感内存修订号；
    /// 已在生成的引擎不被立即打断，下次 turn 会在发送前安全回收并重建。
    pub(crate) fn mark_model_updated(&self, model_id: &str) {
        self.model_update_revisions.bump(model_id);
    }

    pub fn compute_disallowed_tools(&self) -> Vec<String> {
        (self.tool_policy)(&self.app)
    }

    pub async fn refresh_disallowed_tools(&self) -> Vec<String> {
        let tools = self.compute_disallowed_tools();
        self.set_disallowed_all(tools.clone()).await;
        tools
    }

    /// 该会话的项目执行根（绑项目 code 会话 = 项目目录）。解析失败 → None
    /// （项目级技能不参与组合目录，行为与未绑定一致）。
    fn project_workspace_for(&self, session_id: &str) -> Option<std::path::PathBuf> {
        self.store
            .session_roots(session_id)
            .ok()
            .map(|roots| roots.execution)
    }

    /// skill 双 scope 治理：事件驱动**增量重写**所有在线会话的组合目录
    /// （skill toggle / 安装 / 卸载命令落盘后调用，§2.3.2）。每个会话按自己的
    /// scope 计算启用集，只增删变化部分（diff 幂等）；底座每轮重扫，下一轮
    /// prompt 即生效。不在线的会话不管（下次 spawn 全量拼，§2.3.1）。
    pub async fn refresh_live_sessions_skills(&self) {
        let sids: Vec<String> = {
            let entries = self.entries.lock().await;
            entries.keys().cloned().collect()
        };
        for sid in sids {
            let scope = self.bridge.session_policy(&sid).connector_scope();
            let project_workspace = self.project_workspace_for(&sid);
            let _ = tokio::task::spawn_blocking(move || {
                crate::features::assistant::skill_materialization::rewrite_session_skills(
                    &sid,
                    scope,
                    project_workspace.as_deref(),
                );
            })
            .await;
        }
    }

    /// 同步版在线会话组合目录重写（给保持同步签名的 tauri 命令用，如
    /// `uninstall_marketplace_tool`）。组合目录体量小、diff 重写极快，阻塞可接受。
    pub fn refresh_live_sessions_skills_blocking(&self) {
        let sids: Vec<String> = {
            let entries = self.entries.blocking_lock();
            entries.keys().cloned().collect()
        };
        for sid in sids {
            let scope = self.bridge.session_policy(&sid).connector_scope();
            let project_workspace = self.project_workspace_for(&sid);
            crate::features::assistant::skill_materialization::rewrite_session_skills(
                &sid,
                scope,
                project_workspace.as_deref(),
            );
        }
    }

    /// 为独立调用构造该 session 的 bridge。与 EnginePool lazy spawn 共用同一套
    /// runtime provider，保证检阅等旁路入口也不会绕过运行时凭据准备。
    pub(crate) async fn fresh_bridge_for(&self, session_id: &str) -> Result<Pinvou3Bridge> {
        self.fresh_bridge_for_policy(session_id, false).await
    }

    async fn prepare_runtime_model(
        &self,
        session_id: &str,
        scheduled_unattended: bool,
    ) -> Result<(Pinvou3Bridge, PreparedRuntimeModel, bool)> {
        let mut bridge = self.bridge.clone();
        bridge.prefs = UserPrefs::load();
        let scheduled_profile = self.store.scheduled_profile(session_id);
        let interactive_model_override = self.store.session_model_override(session_id);
        let pins_scheduled_model = scheduled_profile.is_some()
            && (scheduled_unattended || interactive_model_override.is_none());
        bridge.session_model = resolve_spawn_model(
            &bridge.prefs.advanced.saved_models,
            scheduled_profile.as_ref(),
            interactive_model_override.as_deref(),
            scheduled_unattended,
        )?;
        let selected = bridge
            .effective_model_owned()
            .context("No effective model is available for runtime preparation")?;
        let selected_id = selected.id.clone();
        let prepared = self
            .runtime_model_provider
            .prepare(RuntimeModelRequest {
                session_id: session_id.to_string(),
                model: selected,
                scheduled_unattended,
            })
            .await?;
        if prepared.model.id != selected_id {
            bail!(
                "Runtime model provider changed model identity from '{}' to '{}'",
                selected_id,
                prepared.model.id
            );
        }
        Ok((bridge, prepared, pins_scheduled_model))
    }

    async fn finalize_runtime_bridge(
        &self,
        mut bridge: Pinvou3Bridge,
        prepared: &PreparedRuntimeModel,
        pins_scheduled_model: bool,
    ) -> Pinvou3Bridge {
        bridge.session_model = Some(prepared.model.clone());
        bridge.runtime_model_credential = prepared.credential.clone();
        // 本地 vLLM:发请求的 model 名以 vLLM 实际 served name 为准(探测 /v1/models),
        // 免去写死 qwen36_35b_256k 与 --served-model-name 不一致的 model_not_found。
        // 探测失败(vLLM 没起)保持配置值;云端 provider 不探测。
        if bridge.provider() == "vllm" {
            let (served, max_len) =
                crate::features::monitor::probe_vllm_model_info(&bridge.base_url()).await;
            if let Some(served) = served.filter(|_| !pins_scheduled_model) {
                if let Some(mut model) = bridge.effective_model_owned() {
                    if model.model != served {
                        model.model = served;
                        bridge.session_model = Some(model);
                    }
                }
            }
            bridge.probed_context_tokens = max_len;
        }
        bridge
    }

    async fn fresh_bridge_for_policy(
        &self,
        session_id: &str,
        scheduled_unattended: bool,
    ) -> Result<Pinvou3Bridge> {
        let (bridge, prepared, pins_scheduled_model) = self
            .prepare_runtime_model(session_id, scheduled_unattended)
            .await?;
        Ok(self
            .finalize_runtime_bridge(bridge, &prepared, pins_scheduled_model)
            .await)
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
        let runtime_lock = self.runtime_model_locks.for_session(session_id).await;
        let _runtime = runtime_lock.lock().await;
        let (bridge, prepared, pins_scheduled_model) = self
            .prepare_runtime_model(session_id, scheduled_unattended)
            .await?;
        let model_update_revision = self.model_update_revisions.current(&prepared.model.id);
        let prepared = PreparedRuntimeState::new(prepared, model_update_revision);

        let stale = {
            let mut entries = self.entries.lock().await;
            if let Some(entry) = entries.get(session_id) {
                if !prepared.requires_rebuild_from(&entry.runtime_model) {
                    return Ok(entry.engine.clone());
                }
            }
            entries.remove(session_id)
        };
        if let Some(entry) = stale {
            self.reclaim_engine_entry(session_id, entry).await;
        }

        let is_scheduled = self.store.scheduled_profile(session_id).is_some();
        let bridge = self
            .finalize_runtime_bridge(bridge, &prepared.prepared, pins_scheduled_model)
            .await;
        // shell 执行目录与 engine cwd 同源：统一走 SessionStore::session_roots
        // （scheduled = automation workspace，原生代码绑项目会话 = 项目目录）。
        // 解析失败（如 scheduled 会话缺 profile）时维持原回退：bridge 侧解析。
        let shell_workspace = self
            .store
            .session_roots(session_id)
            .map(|roots| roots.execution)
            .unwrap_or_else(|_| bridge.session_workspace(session_id));
        let shell_manager = self.shell_managers.for_session(session_id, shell_workspace);
        let turn_shell_tasks = self
            .turn_shell_tasks
            .for_session(session_id, shell_manager.clone());
        let mut extra_tools = (self.tool_factory)(&self.app, session_id);
        extra_tools.push(Arc::new(
            crate::features::connectors::ima::ImaOpenApiTool::new(),
        ));
        // skill 双 scope 治理：spawn 全量拼组合目录（物化时机一，V-7）。组合目录
        // 是 EngineConfig.skills_dir 的发现根（build_engine_config_for_session_at
        // 注入路径），必须先于 spawn 存在，否则首轮 prompt 无 `## Skills` 块。
        {
            let sid = session_id.to_string();
            let scope = self.bridge.session_policy(&sid).connector_scope();
            let project_workspace = self.project_workspace_for(&sid);
            tokio::task::spawn_blocking(move || {
                crate::features::assistant::skill_materialization::materialize_session_skills(
                    &sid,
                    scope,
                    project_workspace.as_deref(),
                )
            })
            .await
            .map_err(|e| anyhow::anyhow!("materialize session skills join: {e}"))?
            .map_err(|e| anyhow::anyhow!("materialize session skills: {e}"))?;
        }
        let (engine, forwarder) = AppEngine::spawn_for_session(
            self.app.clone(),
            self.store.clone(),
            bridge,
            session_id,
            extra_tools,
            self.bridge
                .shape_disallowed_tools(session_id, self.compute_disallowed_tools()),
            self.turn_lifecycles.for_session(session_id),
            shell_manager,
            turn_shell_tasks,
        )
        .await?;

        // 即使 messages 为空也必须同步：SyncSession 不只注入历史，还把底层 Engine
        // 的内部 session id 对齐到预创建的持久化会话。跳过会让首轮 SessionUpdated
        // 因 id mismatch 被拒绝，最终只落盘 user 而丢失 assistant。
        match self.store.load(session_id) {
            Ok(saved) if should_sync_session(is_scheduled, !saved.messages.is_empty()) => {
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
                }
            }
            Ok(_) => {}
            Err(error) => {
                let _ = engine.handle.send(Op::Shutdown).await;
                forwarder.abort();
                return Err(error).with_context(|| {
                    format!("load session {session_id} before spawning its engine")
                });
            }
        }

        self.entries.lock().await.insert(
            session_id.to_string(),
            EngineEntry {
                engine: engine.clone(),
                forwarder,
                runtime_model: prepared,
                spawned_at_ms: Self::now_epoch_ms(),
            },
        );
        Ok(engine)
    }

    /// 该 session 引擎的纪元时间戳（UNIX ms）。None = 引擎没起。
    /// transcripts 投影用它甄别 worker ledger 里上一进程遗留的"running"。
    pub async fn engine_epoch_ms(&self, session_id: &str) -> Option<u64> {
        self.entries
            .lock()
            .await
            .get(session_id)
            .map(|e| e.spawned_at_ms)
    }

    /// 取已存在的 engine(不 spawn)。cancel / submit_user_input 等用:engine 没起
    /// 说明该 session 没在跑,这些操作天然是 no-op。
    pub async fn handle_for(&self, session_id: &str) -> Option<AppEngine> {
        self.entries
            .lock()
            .await
            .get(session_id)
            .map(|e| e.engine.clone())
    }

    /// 回收某 session 的 engine:cancel 在跑的 turn → Shutdown engine → abort forwarder。
    /// 删除 session 时调。
    pub async fn evict(&self, session_id: &str) {
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        self.evict_locked(session_id).await;
    }

    /// Delete an ordinary chat under the exact turn gate used by lazy spawn
    /// and send. No queued sender can slip between engine reclaim, disk delete,
    /// and lifecycle cleanup to resurrect the session.
    pub(crate) async fn delete_chat_session(&self, session_id: &str) -> Result<()> {
        delete_chat_session_with_gate(
            &self.turn_locks,
            &self.store,
            session_id,
            || self.evict_locked(session_id),
            || self.forget_session(session_id),
        )
        .await?;
        // 裸 `agent` 对**所有**会话可用（不只多智能体开关开启的），
        // 底座取消子智能体后的后台 ledger 写
        // （write_json_atomic 重建父目录）可能复活刚删的 sessions/<id>/。
        // 目录不存在是常态零成本，Shutdown 处理完后不再有新写入，必然收敛。
        Self::schedule_late_sweep(
            crate::platform::paths::sessions_root().join(session_id),
            "late sweep of deleted chat",
        );
        Ok(())
    }

    /// 删除后的延迟清扫：底座取消子智能体后在后台线程异步写 worker ledger
    /// （write_json_atomic 会重建父目录），刚删的目录可能被复活成孤儿。
    /// 两次延迟重删兜底；目标不存在视为已收敛。
    fn schedule_late_sweep(dir: std::path::PathBuf, label: &'static str) {
        tauri::async_runtime::spawn(async move {
            for delay_ms in [2000u64, 6000] {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        eprintln!("[engine_pool] {label} {} failed: {error}", dir.display())
                    }
                }
            }
        });
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

    /// 引擎回收时的收尾 op 序列：**先**级联取消全部子智能体，**后**关闭引擎。
    /// 顺序有语义——两个 op 走同一条 mpsc 通道，FIFO 保证引擎在处理 Shutdown
    /// 前先处理完取消；颠倒顺序等于没取消（Shutdown 直接 break 出事件循环）。
    fn shutdown_cancel_cascade_ops() -> [Op; 2] {
        [Op::CancelSubAgents, Op::Shutdown]
    }

    async fn reclaim_engine_entry(&self, session_id: &str, entry: EngineEntry) {
        // Stop the producer before admission fallback awaits disk I/O.
        // An in-flight SessionUpdated save is serialized with the fallback
        // by SessionStore's mutation gate; no later delta/tool event can
        // overtake the authoritative transcript_committed + done pair.
        let EngineEntry {
            engine, forwarder, ..
        } = entry;
        let shell_reclaim = self.turn_shell_tasks.begin_reclaim(session_id);
        let shell_reclaim_for_drain = shell_reclaim.clone();
        let shell_reclaim_for_terminal = shell_reclaim.clone();
        let reclaimed = quiesce_engine_before_reclaim(
            || engine.cancel_current(),
            || async move {
                shell_reclaim_for_drain.finalize().await;
                forwarder.abort();
                let _ = forwarder.await;
            },
            || {
                engine.finish_reclaimed_turn(
                    &self.app,
                    &self.store,
                    session_id,
                    shell_reclaim_for_terminal.cleanup_failed(),
                )
            },
        )
        .await;
        if reclaimed {
            log::warn!(
                "[engine_pool] emitted interrupted terminal before reclaim sid={}",
                session_id
            );
        }
        // 先级联取消全部后台子智能体，再关闭引擎（ADR-0006）。两个 op 走同一条
        // 通道，FIFO 保证取消先于关闭被处理；否则删除/换模型回收后，会话派生的
        // 裸子智能体会以孤儿任务继续跑到自己的步数/时限上限。已知限制：取消是
        // abort 不 join，子智能体已启动的独立 shell 子进程仍可能残留。
        for op in Self::shutdown_cancel_cascade_ops() {
            if let Err(e) = engine.handle.send(op).await {
                eprintln!("[engine_pool] shutdown {session_id} failed: {e:?}");
                break;
            }
        }
    }

    async fn evict_locked(&self, session_id: &str) {
        let runtime_lock = self.runtime_model_locks.for_session(session_id).await;
        let _runtime = runtime_lock.lock().await;
        if let Some(entry) = self.entries.lock().await.remove(session_id) {
            self.reclaim_engine_entry(session_id, entry).await;
        } else if let Some(lifecycle) = self.turn_lifecycles.get(session_id) {
            // A caller can reserve before lazy spawn. Reclaim that Reserved
            // phase without fabricating chat:done; the guard's eventual send
            // will observe that its reservation was invalidated.
            lifecycle.invalidate_unsubmitted_reservation();
        }
    }

    pub(crate) fn forget_session(&self, session_id: &str) {
        self.turn_lifecycles.remove(session_id);
        self.turn_shell_tasks.remove(session_id);
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

    /// 原子切换多智能体资源策略：先占用与发送相同的 lifecycle 槽位，再在
    /// session turn gate 内持久化状态并回收旧引擎。这样发送与切换不可能交错成
    /// “新开关 + 旧引擎”或“旧开关 + 新引擎”。未提交 reservation 会在回收时
    /// 静默失效，不产生伪造的 chat:done。
    pub(crate) async fn reconfigure_multi_agent_mode(
        &self,
        session_id: &str,
        enabled: bool,
    ) -> Result<()> {
        if enabled && !self.multi_agent_mode_available(session_id) {
            anyhow::bail!("Pinvou 多智能体模式本期仅支持工作会话");
        }
        let _reservation = self.turn_lifecycles.for_session(session_id).reserve()?;
        if enabled {
            // 先占 lifecycle 再写名册：慢磁盘期间若允许发送抢先 reserve，首轮会
            // 落进旧引擎，随后切换失败并回滚，形成界面已开但该轮未受限的竞态。
            let workspace = self.bridge.session_workspace(session_id);
            let sid = session_id.to_string();
            tokio::task::spawn_blocking(move || -> Result<()> {
                std::fs::create_dir_all(&workspace)
                    .with_context(|| format!("set_multi_agent_mode({sid}): 建工作区失败"))?;
                crate::features::multiagent::roster::enroll_expert_roles(&workspace)
                    .map_err(anyhow::Error::msg)
            })
            .await
            .with_context(|| format!("set_multi_agent_mode({session_id}): 后台任务失败"))??;
        }
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        self.store.set_multi_agent(session_id, enabled)?;
        self.evict_locked(session_id).await;
        Ok(())
    }

    /// Atomically reserve the single turn slot for a session before callers
    /// consume one-shot state, stage attachments, or perform other side effects.
    /// Dropping the returned guard before submission restores the slot.
    pub(crate) fn reserve_turn(&self, session_id: &str) -> Result<TurnReservation> {
        let mut reservation = self.turn_lifecycles.for_session(session_id).reserve()?;
        let baseline = self.store.load(session_id)?;
        reservation.set_base_transcript_revision(transcript_revision(&baseline.messages)?);
        Ok(reservation)
    }

    /// 该 session 当前是否有进行中的 turn（供前端 remount 后恢复 busy 展示）。
    pub fn is_turn_active(&self, session_id: &str) -> bool {
        self.turn_lifecycles
            .get(session_id)
            .is_some_and(|lifecycle| lifecycle.is_active())
    }

    /// Whether this session uses Pinvou's native Code execution lane.
    pub(crate) fn is_code_session(&self, session_id: &str) -> bool {
        self.bridge.is_code_session(session_id)
    }

    /// Product capability gate for Pinvou's multi-agent mode. This is shared
    /// by command, prompt, engine and roster paths so a hidden control cannot
    /// be bypassed by stale state or a direct IPC call.
    pub(crate) fn multi_agent_mode_available(&self, session_id: &str) -> bool {
        self.bridge
            .session_policy(session_id)
            .multi_agent_mode_available()
    }

    /// Resolve the Engine workspace, including a project-bound native Code
    /// session. CodeWhale keeps its project-scoped `.codewhale` state here.
    pub(crate) fn session_workspace(&self, session_id: &str) -> std::path::PathBuf {
        self.bridge.session_workspace(session_id)
    }

    /// 发用户消息给指定 session 的 engine(没起则 lazy spawn)。
    #[allow(dead_code)]
    pub async fn send_user_message(
        &self,
        session_id: &str,
        content: String,
        mode: AppMode,
        restrict_tools_for_turn: bool,
    ) -> Result<()> {
        let reservation = self.reserve_turn(session_id)?;
        let display_message = user_display_message(content.clone());
        self.send_reserved_user_message(
            session_id,
            content,
            display_message,
            mode,
            restrict_tools_for_turn,
            reservation,
        )
        .await
    }

    /// Submit a previously admitted append operation. This is the entry point
    /// used by chat commands that must reserve before resolving attachments.
    pub(crate) async fn send_reserved_user_message(
        &self,
        session_id: &str,
        content: String,
        display_message: Message,
        mode: AppMode,
        restrict_tools_for_turn: bool,
        mut reservation: TurnReservation,
    ) -> Result<()> {
        let baseline_revision = reservation
            .base_transcript_revision()
            .context("turn reservation has no base transcript revision")?
            .to_string();
        reservation.set_transcript_with_baseline(
            TranscriptOperation::Append,
            display_message,
            baseline_revision,
        )?;
        let scheduled_profile = self.store.scheduled_profile(session_id);
        if scheduled_profile.is_none() && session_id.starts_with("sched-") {
            bail!("Scheduled session '{session_id}' no longer exists");
        }
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        if let Some(profile) = scheduled_profile {
            scheduled_profile_after_turn_gate(&self.store, session_id, &profile.task_id)?;
        } else {
            self.store.load(session_id).with_context(|| {
                format!("Session '{session_id}' was deleted before the turn could start")
            })?;
        }
        reservation.ensure_active()?;
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
        let persona_reminder = active_card
            .as_ref()
            .map(crate::features::personas::equip_anchor);
        let restrict_tools = active_card.as_ref().is_some_and(|c| c.conversational_only);
        let restrict_tools = restrict_tools || restrict_tools_for_turn;
        self.get_or_spawn(session_id)
            .await?
            .send_reserved_user_message(
                content,
                mode,
                persona_reminder,
                restrict_tools,
                reservation,
            )
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

    /// 取消指定 session 正在生成的回复，并级联取消它派生的全部后台子智能体。
    /// engine 没起则 no-op。
    ///
    /// 分两阶段执行，避免与 `send_reserved_user_message` 争抢 `turn_lock` 导致
    /// 「停止按钮无响应」：cancel_token 是独立原子，置位不需要 turn_lock 保护，
    /// 因此第一步无锁先触发，turn_loop 的 biased select 会立即跳出并正常发
    /// `TurnComplete`(→ chat:done)；第二步再持锁清理 shell/lifecycle 状态。
    ///
    /// 第二步以 lifecycle 的提交状态（而非 Engine 是否存在）为权威依据：reservation
    /// 处于「reserved 未 submitted」阶段（消息尚未入队 engine）时，无论该会话是否
    /// 保留着上一轮的空闲 Engine，都立即认领未提交 Interrupted 终态并补发
    /// `chat:done`，使 reservation 失效（后续 `ensure_active` 失败、消息不再提交），
    /// 保证前端 busy 一定能复位。
    ///
    /// 「停止」按钮是子智能体唯一的确定性停止入口（卡片上没有取消按钮，
    /// 自然语言指令只是建议）；只取消宿主轮会留下继续烧钱的后台子智能体。
    pub async fn cancel(&self, session_id: &str) {
        // 阶段一：无锁先触发 cancel_token。
        // handle_for 只瞬时取 entries 锁(与 send 内部瞬时取 entries 锁不冲突)，
        // 不等 turn_lock——即使 send 正持锁，cancel 也能立刻让 turn_loop 跳出。
        if let Some(engine) = self.handle_for(session_id).await {
            engine.cancel_current();
        }
        // 阶段二：持锁清理 turn 状态与 shell 任务。
        let shell_cancellation = self.turn_shell_tasks.request_cancel(session_id);
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        // 持锁后以 lifecycle 的提交状态（而非 Engine 是否存在）为权威依据。
        // reserve_turn 不取 turn_lock，chat 在 reserve 与 send 之间有
        // set_disallowed_all 等异步让出点（chat.rs）；若该会话保留着上一轮的空闲
        // Engine，旧实现按「Engine 是否存在」分流会错误走 pending 分支：只 arm
        // pending、不认领终态、不发 chat:done，reservation 仍有效，原 chat future
        // 后续照常提交消息，前端 busy 在 cancel 后到 TurnStarted 之间无法复位。
        //
        // emit_unsubmitted_interrupted_terminal 内部 claim 检查「active && !submitted」：
        // 命中（含 engine 未 spawn 的 probe 窗口与空闲 engine 存在的未提交窗口）=
        // 认领 Interrupted 终态 + 发 chat:done + 重置闸门，reservation 随之失效
        // （后续 send 的 ensure_active 失败，消息不再提交）；未命中（已 submitted
        // 或无活动 turn）返回 false 且无副作用，fallthrough 到 engine 分支。
        // 必须用 claim（而非裸 invalidate）：reserve_turn 不取 turn_lock，若仅在
        // invalidate 后、chat:done 发出前释放闸门，新一轮 reserve 可抢先成功，迟到
        // 的 chat:done 会清掉新一轮 busy——claim 在发终态期间置 terminal_closing
        // 关闸，与权威终态路径一致的防重入语义。
        let lifecycle = self.turn_lifecycles.get(session_id);
        let claimed_unsubmitted = lifecycle
            .as_ref()
            .is_some_and(|lc| lc.emit_unsubmitted_interrupted_terminal(&self.app, session_id));
        let engine = self.handle_for(session_id).await;
        if !claimed_unsubmitted {
            // 持锁后复查 engine：阶段一可能因 send 正在 spawn 而拿不到。
            // 若阶段一已 cancel_current，这里再 cancel 是幂等 no-op；若阶段一没拿到，
            // 这里拿到刚注册的 engine 补 cancel。
            if let Some(engine) = engine.as_ref() {
                // 已提交（op 已入队或 TurnStarted 已抵达）：
                //  - TurnStarted 未抵达：arm pending，转发器收到 TurnStarted 后
                //    （reset_cancel_token 已完成）补 cancel 命中活跃 token；
                //  - TurnStarted 已抵达：cancel_current 直接命中当前活跃 token。
                // 必须在 cancel_current() 之前 arm——见 arm_pending_cancel 文档。
                if let Some(lifecycle) = lifecycle {
                    lifecycle.arm_pending_cancel();
                }
                engine.cancel_current();
            }
        }
        // 在 turn_lock 内对当前权威 engine 发级联取消，避免锁等待期间发生的
        // engine 替换让后台子智能体漏取消，也避免取消误伤随后开始的新一轮。
        if let Some(engine) = engine {
            if let Err(e) = engine.handle.send(Op::CancelSubAgents).await {
                eprintln!("[engine_pool] cancel subagents {session_id} failed: {e:?}");
            }
        }
        if let Some(cancellation) = shell_cancellation {
            cancellation.cleanup().await;
        }
    }

    /// pinvou3 工具开关(全局持久):把"被禁用的工具全名"(模型可见全名,小写)广播给
    /// **所有在跑的 session engine** → 写入各自 config.disallowed_tools,下一轮即隐藏。
    /// 没起的会话下次 spawn 时从持久列表读初值(build_engine_config),所以新窗口/新对话
    /// 都继承同一份禁用状态。
    pub async fn set_disallowed_all(&self, tools: Vec<String>) {
        let targets = self
            .entries
            .lock()
            .await
            .iter()
            .map(|(sid, entry)| (sid.clone(), entry.engine.clone()))
            .collect::<Vec<_>>();
        for (sid, engine) in targets {
            // 全局热刷同样按会话整形（代码会话保留 present_artifact 隐藏），
            // 且发送前释放 entries 锁，避免跨 await 持有全局引擎表锁。
            if let Err(e) = engine
                .handle
                .send(Op::SetDisallowedTools {
                    tools: self.bridge.shape_disallowed_tools(&sid, tools.clone()),
                })
                .await
            {
                eprintln!("[engine_pool] set_disallowed_all {sid} failed: {e:?}");
            }
        }
    }

    /// 专家池增删改后的名册联动（ADR-0006）：刷新所有开着多智能体开关的
    /// 会话，以及仍在运行且留有专家投影的已关闭会话。后者不能跳过：开关
    /// 关闭不会重建引擎，若此时删卡，旧 roster 会让已删除专家继续可用。
    /// 返回聚合错误：调用方（专家池命令）要如实告知用户哪些会话没刷新成功，
    /// 而不是界面显示成功、子智能体却用旧名单。
    pub async fn refresh_multi_agent_rosters_after_expert_change(&self) -> Result<(), String> {
        let mut failures: Vec<String> = Vec::new();
        let mut targets = self
            .store
            .multi_agent_session_ids()
            .into_iter()
            .filter(|session_id| self.multi_agent_mode_available(session_id))
            .collect::<std::collections::BTreeSet<_>>();
        let live_session_ids = self
            .entries
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for session_id in live_session_ids {
            if !self.multi_agent_mode_available(&session_id) {
                continue;
            }
            let workspace = self.bridge.session_workspace(&session_id);
            if crate::features::multiagent::roster::has_expert_role_projection(&workspace) {
                targets.insert(session_id);
            }
        }
        for session_id in targets {
            let workspace = self.bridge.session_workspace(&session_id);
            if let Err(err) = crate::features::multiagent::roster::enroll_expert_roles(&workspace) {
                failures.push(format!("{session_id}: {err}"));
                continue;
            }
            if let Err(err) = self.refresh_multi_agent_roster(&session_id).await {
                failures.push(format!("{session_id}: {err}"));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("；"))
        }
    }

    /// 多智能体开关开启后的即时名册刷新（ADR-0006）：把刚装配进会话工作区的
    /// 专家名册整册推给在跑引擎（底座 `Op::SetFleetRoster`，下一轮生效），
    /// 否则模型能在提醒里看到专家名字、启动时却报"不存在该 profile"。
    /// 工具面不随开关变化（与主线持平，不按会话改写禁用列表），引擎没起则 no-op——
    /// 下次 spawn 由 `build_engine_config_for_multi_agent` 从工作区装配。
    pub async fn refresh_multi_agent_roster(&self, session_id: &str) -> Result<(), String> {
        if !self.multi_agent_mode_available(session_id) {
            return Ok(());
        }
        let Some(engine) = self.handle_for(session_id).await else {
            // 引擎没起不是错误：下次 spawn 从工作区装配。
            return Ok(());
        };
        let workspace = self.bridge.session_workspace(session_id);
        let roster = std::sync::Arc::new(deepseek_tui::FleetRoster::load(
            &codewhale_config::FleetConfigToml::default(),
            &workspace,
        ));
        engine
            .handle
            .send(Op::SetFleetRoster { roster })
            .await
            .map_err(|e| format!("向在跑引擎推送名册失败: {e:?}"))
    }

    /// 当前 UNIX 时刻（毫秒）。引擎纪元与 worker ledger 的
    /// created_at_ms/updated_at_ms 同源（都是 SystemTime），可直接比较。
    fn now_epoch_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// 编辑/重发指定 session 最后一轮 user 消息。调用方在预留 turn 后分别传入
    /// 模型内容与干净展示消息，避免运行时提醒进入可见历史。
    pub(crate) async fn edit_last_turn_reserved(
        &self,
        session_id: &str,
        new_message: String,
        display_message: Message,
        mut reservation: TurnReservation,
    ) -> Result<()> {
        let baseline_revision = reservation
            .base_transcript_revision()
            .context("turn reservation has no base transcript revision")?
            .to_string();
        reservation.set_transcript_with_baseline(
            TranscriptOperation::EditLast,
            display_message,
            baseline_revision,
        )?;
        let scheduled_profile = self.store.scheduled_profile(session_id);
        if scheduled_profile.is_none() && session_id.starts_with("sched-") {
            bail!("Scheduled session '{session_id}' no longer exists");
        }
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        if let Some(profile) = scheduled_profile {
            scheduled_profile_after_turn_gate(&self.store, session_id, &profile.task_id)?;
        } else {
            self.store.load(session_id).with_context(|| {
                format!("Session '{session_id}' was deleted before the edit could start")
            })?;
        }
        reservation.ensure_active()?;
        self.get_or_spawn(session_id)
            .await?
            .edit_last_turn_reserved(new_message, reservation)
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

pub(crate) fn user_display_message(text: impl Into<String>) -> Message {
    Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: text.into(),
            cache_control: None,
        }],
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
// 测试借 platform::paths::tests::ENV_LOCK(std Mutex)串行化全局 env;单线程测试内跨 await 持有无竞争者,不会死锁。
#[allow(clippy::await_holding_lock)]
mod scheduled_model_tests {
    use super::{
        delete_chat_session_with_gate, delete_scheduled_run_with_gate,
        quiesce_engine_before_reclaim, resolve_scheduled_model, resolve_spawn_model,
        scheduled_profile_after_turn_gate, should_sync_session, ModelUpdateRevisions,
        PreparedRuntimeState, ScheduledUnattendedGuard, SessionShellManagers,
        SessionTurnLifecycles, SessionTurnLocks,
    };
    use crate::features::assistant::runtime_model::PreparedRuntimeModel;
    use crate::features::sessions::{ScheduledRunMode, ScheduledRunProfile, SessionStore};
    use crate::platform::credential_store::{CredentialEditAction, CredentialState};
    use crate::platform::prefs::{ImageCapabilityOverride, ModelPreset, SavedModel};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    /// ADR-0006：引擎回收必须**先**取消全部子智能体、**后**发 Shutdown。
    /// 两个 op 同通道 FIFO；颠倒顺序等于没取消（Shutdown 直接跳出事件循环，
    /// 会话派生的裸子智能体会以孤儿任务继续跑）。
    #[test]
    fn reclaim_cascade_cancels_subagents_before_shutdown() {
        let ops = super::EnginePool::shutdown_cancel_cascade_ops();
        assert!(
            matches!(ops[0], deepseek_tui::core::ops::Op::CancelSubAgents),
            "级联取消必须先行"
        );
        assert!(
            matches!(ops[1], deepseek_tui::core::ops::Op::Shutdown),
            "Shutdown 必须殿后"
        );
    }

    fn model(id: &str, wire_name: &str) -> SavedModel {
        SavedModel {
            id: id.to_string(),
            name: id.to_string(),
            preset: ModelPreset::OpenaiCompatible,
            context_window_tokens: None,
            max_output_tokens: None,
            model: wire_name.to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            image_capability_override: ImageCapabilityOverride::default(),
            vision_model_id: None,
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
    fn every_empty_session_is_synchronized_before_its_first_turn() {
        assert!(should_sync_session(true, false));
        assert!(should_sync_session(true, true));
        assert!(should_sync_session(false, true));
        assert!(should_sync_session(false, false));
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
    fn saved_model_update_revision_forces_next_turn_rebuild() {
        let revisions = ModelUpdateRevisions::default();
        let prepared = PreparedRuntimeModel::unchanged(model("model-1", "wire-model"));
        let previous = PreparedRuntimeState::new(prepared.clone(), revisions.current("model-1"));

        revisions.bump("model-1");
        let after_save = PreparedRuntimeState::new(prepared, revisions.current("model-1"));

        assert!(after_save.requires_rebuild_from(&previous));
    }

    #[tokio::test]
    async fn runtime_preparation_locks_are_isolated_between_sessions() {
        let locks = SessionTurnLocks::default();
        let first = locks.for_session("session-1").await;
        let second = locks.for_session("session-2").await;
        assert!(!Arc::ptr_eq(&first, &second));

        let _first_guard = first.lock().await;
        let _second_guard = tokio::time::timeout(std::time::Duration::from_secs(1), second.lock())
            .await
            .expect("a slow provider for one session must not block another session");
    }

    #[test]
    fn turn_lifecycle_survives_engine_entry_removal_without_faking_idle_cancel() {
        let lifecycles = SessionTurnLifecycles::default();
        let engine_lifecycle = lifecycles.for_session("session-1");
        engine_lifecycle.on_submitted();
        drop(engine_lifecycle);

        let pool_lifecycle = lifecycles.get("session-1").expect("session lifecycle");
        assert!(pool_lifecycle.finish_once(|| {}).is_some());
        assert_eq!(pool_lifecycle.finish_once(|| panic!("duplicate")), None);
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
    async fn engine_reclaim_quiesces_event_producer_before_persistence() {
        let order = Arc::new(StdMutex::new(Vec::new()));
        let cancel_order = order.clone();
        let abort_order = order.clone();
        let persist_order = order.clone();

        let result = quiesce_engine_before_reclaim(
            move || cancel_order.lock().unwrap().push("cancel"),
            move || async move {
                abort_order.lock().unwrap().push("abort");
                tokio::task::yield_now().await;
                abort_order.lock().unwrap().push("joined");
            },
            move || async move {
                persist_order.lock().unwrap().push("persist");
                42
            },
        )
        .await;

        assert_eq!(result, 42);
        assert_eq!(
            *order.lock().unwrap(),
            vec!["cancel", "abort", "joined", "persist"]
        );
    }

    #[tokio::test]
    async fn chat_delete_keeps_evict_delete_and_forget_ahead_of_waiting_send() {
        let _env_guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = std::env::temp_dir().join(format!(
            "pinvou3-engine-pool-chat-delete-race-{}",
            std::process::id()
        ));
        let previous_home = std::env::var("PINVOU3_HOME").ok();
        let _ = std::fs::remove_dir_all(&home);
        std::env::set_var("PINVOU3_HOME", &home);

        let store = SessionStore::boot().expect("session store");
        let session_id = store
            .create_new("wire-model".to_string(), None, home.join("workspace"))
            .expect("chat session")
            .metadata
            .id;
        let locks = SessionTurnLocks::default();
        let gate = locks.for_session(&session_id).await;
        let blocker = gate.lock().await;
        let fake_engine_present = Arc::new(AtomicBool::new(true));
        let forgotten = Arc::new(AtomicBool::new(false));

        let delete_locks = locks.clone();
        let delete_store = store.clone();
        let delete_id = session_id.clone();
        let delete_engine = fake_engine_present.clone();
        let delete_forgotten = forgotten.clone();
        let delete = tokio::spawn(async move {
            delete_chat_session_with_gate(
                &delete_locks,
                &delete_store,
                &delete_id,
                || async move {
                    delete_engine.store(false, Ordering::Release);
                },
                || delete_forgotten.store(true, Ordering::Release),
            )
            .await
        });
        tokio::task::yield_now().await;

        let send_locks = locks.clone();
        let send_store = store.clone();
        let send_id = session_id.clone();
        let waiting_send = tokio::spawn(async move {
            let gate = send_locks.for_session(&send_id).await;
            let _turn = gate.lock().await;
            send_store.load(&send_id)
        });
        tokio::task::yield_now().await;
        drop(blocker);
        drop(gate);

        delete
            .await
            .expect("delete task joins")
            .expect("delete chat");
        assert!(
            waiting_send.await.expect("send task joins").is_err(),
            "a sender queued on the gate must observe the completed delete"
        );
        assert!(!fake_engine_present.load(Ordering::Acquire));
        assert!(forgotten.load(Ordering::Acquire));
        assert!(store.load(&session_id).is_err());

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
