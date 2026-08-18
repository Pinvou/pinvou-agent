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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
use crate::features::assistant::eval::{EvalModelSelection, EvalSuiteModelSnapshot, ModelIdentity};
use crate::features::assistant::expert_roster::ExpertRosterSnapshot;
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
struct EvalModelSnapshots {
    next_token: Arc<AtomicU64>,
    saved_models: Arc<SyncMutex<HashMap<String, SavedModel>>>,
    suite_models: Arc<SyncMutex<HashMap<String, SavedModel>>>,
    session_models: Arc<SyncMutex<HashMap<String, SavedModel>>>,
}

impl EvalModelSnapshots {
    fn pin(&self, saved_model: SavedModel, identity: ModelIdentity) -> EvalModelSelection {
        let sequence = self.next_token.fetch_add(1, Ordering::Relaxed);
        let token = format!("eval-model-{}-{sequence}", std::process::id());
        let model_id = Some(saved_model.id.clone());
        self.saved_models.lock().insert(token.clone(), saved_model);
        EvalModelSelection::new(token, model_id, identity)
    }

    fn pin_suite(
        &self,
        saved_model: SavedModel,
        identity: ModelIdentity,
    ) -> EvalSuiteModelSnapshot {
        let sequence = self.next_token.fetch_add(1, Ordering::Relaxed);
        let token = format!("eval-suite-model-{}-{sequence}", std::process::id());
        self.suite_models.lock().insert(token.clone(), saved_model);
        EvalSuiteModelSnapshot::new(token, identity)
    }

    fn derive_case_selection(&self, suite: &EvalSuiteModelSnapshot) -> Result<EvalModelSelection> {
        let saved_model = self
            .suite_models
            .lock()
            .get(suite.token())
            .cloned()
            .context("evaluation suite model snapshot is missing")?;
        Ok(self.pin(saved_model, suite.identity().clone()))
    }

    fn discard_suite(&self, suite: &EvalSuiteModelSnapshot) {
        self.suite_models.lock().remove(suite.token());
    }

    fn bind_to_session(&self, session_id: &str, selection: &EvalModelSelection) -> Result<()> {
        let saved_model = self
            .saved_models
            .lock()
            .remove(selection.token())
            .with_context(|| "evaluation model selection is missing or already consumed")?;
        self.session_models
            .lock()
            .insert(session_id.to_string(), saved_model);
        Ok(())
    }

    fn for_session(&self, session_id: &str) -> Option<SavedModel> {
        self.session_models.lock().get(session_id).cloned()
    }

    fn forget_session(&self, session_id: &str) {
        self.session_models.lock().remove(session_id);
    }

    fn discard(&self, selection: &EvalModelSelection) {
        self.saved_models.lock().remove(selection.token());
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
    delete_then_forget(|| store.delete(session_id), forget)
}

fn delete_then_forget<D, G>(delete: D, forget: G) -> Result<()>
where
    D: FnOnce() -> Result<()>,
    G: FnOnce(),
{
    let delete_result = delete();
    forget();
    delete_result
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

/// 两个 epoch 快照是否仍指向同一轮次，供 cancel 跨 turn_lock 边界守护使用。
///
/// `(None, None)`（空闲→空闲）视为匹配：空闲会话的 cancel 本就是 no-op，
/// 走原逻辑无副作用；`(Some, Some)` 且相等才匹配；跨 `Some`/`None` 或不相等
/// 都表示目标轮已结束、新轮已 reserve，cancel 必须整体 no-op。
fn generation_matches(target: Option<u64>, current: Option<u64>) -> bool {
    match (target, current) {
        (Some(a), Some(b)) => a == b,
        (None, None) => true,
        _ => false,
    }
}

/// 级联取消补发的守卫：mismatch 后是否仍可安全补发 `CancelSubAgents`。
///
/// 新轮尚未提交（`SendMessage` 需等同一把 `turn_lock`，cancel 持锁期间 engine
/// 里仍是旧轮遗留子代理）或当前空闲时补发不会命中新轮刚启动的子代理；新轮已
/// 提交（`submitted=true`）则 engine 可能已启动新轮子代理，补发会误杀，必须
/// 跳过。所有 mismatch 发现点（入口复查 / `get_engine` await 后复查 / arm 被拒）
/// 统一用本谓词，保证补发路径在 G1 漏发点（`get_engine` await 后切轮）同样生效。
fn should_retry_cascade(lifecycle: Option<&TurnLifecycle>) -> bool {
    !lifecycle.is_some_and(|lc| lc.is_current_turn_submitted())
}

/// 取消逻辑的可测主体，从 [`EnginePool::cancel`] 抽出以便用裸 Default 组件
/// （`SessionTurnLocks` / `SessionTurnLifecycles` / `SessionTurnShellTasks`）+
/// 闭包注入确定性测试，绕开 `Pinvou3Bridge::boot` / `AppHandle` / 真实
/// `EngineHandle`（跨 crate 私有不可构造）。
///
/// **两阶段 generation 守护**：cancel 不绑定发起时刻的轮次身份时，并发取消
/// 请求（C1/C2）中排队较晚的 C2 在 `turn_lock` 释放后会读到「当前 lifecycle」
/// （可能已是新轮），无差别取消新轮。这里在两阶段各比对一次 epoch：
///
/// - 阶段一（无锁 `get_engine`）前快照 `target`，与即时 `current` 比对；
/// - 阶段二（持 `turn_lock`）后再次比对，不匹配则在 `request_cancel` /
///   `claim_unsubmitted` / `arm_pending_cancel` / `cancel_engine` 全部之前
///   early-return，避免误取消新轮的 engine 与 shell scope。
///
/// **阶段一的 TOCTOU 防护**：`get_engine` 闭包内部有 await（`handle_for` 取
/// entries 锁），await 期间旧轮可能结束、新轮可能 reserve 并
/// `reset_cancel_token()`。因此 epoch 校验必须放在 `get_engine().await`
/// **之后**、`cancel_current` **之前**（发起时的快照 `target` 与 await 后重读
/// 的 `current` 比对，不匹配则整体 no-op）——不能先校验后 await 再取消，
/// 否则 `cancel_current` 会命中新轮的活跃 token，阶段二发现不匹配也撤不回。
///
/// **arm 顺序约束**：每次 `cancel_current` 之前必须先 `arm_pending_cancel`
/// （见 [`TurnLifecycle::arm_pending_cancel`] 文档）。若先 cancel 后 arm：
/// cancel 命中旧 token → engine `reset_cancel_token()` 并发 TurnStarted →
/// forwarder 因尚未 arm 不补 cancel → 此处再 arm 时 `turn_id` 已存在被拒，
/// 停止请求丢失。先 arm 则 TurnStarted 在两步之间抵达时，forwarder 能
/// `take_pending_cancel` 并重放 cancel（随后的 cancel 只是幂等 no-op）。
///
/// `get_engine` 返回在场 engine 句柄（`None` 表示无 engine，走未提交认领
/// 终态）。`claim_unsubmitted` 接收目标 epoch，在「未提交 reservation」时于
/// lifecycle state 锁内与 `turn_epoch == target` 原子校验后认领 Interrupted
/// 终态并补发 `chat:done`，返回是否认领；epoch 不匹配（轮次已切换）必须
/// no-op，不得认领新轮（reviewer 点 7）。
///
/// `arm_pending_cancel_and_cancel` 把「epoch 校验 + arm + 同步取消」合并为
/// 同一 lifecycle state 锁临界区内的原子操作：`cancel_current` 持锁执行，
/// `reserve_turn` 需要同一把 state 锁，无法在「校验/arm」与「取消」之间
/// 插入轮次切换（reviewer 点 8——单独 arm 时锁已释放，到 cancel 之间的
/// 同步调用段仍可被多线程 runtime 抢占）。返回 `false` 表示 generation 复查
/// 通过后、arm 前轮次已切换（新轮已 reserve），此时取消闭包不执行，必须
/// 跳过 `cancel_current` / `cascade_cancel`，否则会命中新轮已
/// `reset_cancel_token` 的活跃 token（reviewer 点 6）。
///
/// `cancel_current` 同步取消 engine 的当前 token（幂等，阶段一、二都执行；
/// 实现方可顺带 try_send 级联取消，尽力而为）。`cascade_cancel` 异步发送
/// 级联取消（CancelSubAgents），**只在阶段二持 `turn_lock` 时 await 调用**：
/// 保证级联取消在释放 turn gate 前完成入队——下一轮 `SendMessage` 必须等
/// 同一把 `turn_lock`（`send_reserved_user_message`），因此级联取消必先于
/// 新轮消息入队，FIFO 保证 engine 先取消旧轮子智能体、后启动新轮，迟到的
/// 级联取消不会误杀新轮刚启动的子智能体（reviewer 点 4：spawn 异步发送
/// 失去相对下一轮 SendMessage 的入队顺序保证）。
///
/// **级联取消送达守护**（reviewer 点 9 + G1 补发收敛）：phase 1 的 best-effort
/// `try_send` 在 ops 通道满（容量 32）时可能失败且被静默忽略，`CancelSubAgents`
/// 从未入队。若随后旧轮结束、新轮在 phase 2 取得 turn gate 前完成 `reserve`，
/// 阶段二会因 generation mismatch 直接 return，`cascade_cancel` 永不执行——
/// 旧轮派生的 detached 子代理继续运行。因此**所有** mismatch 发现点（入口
/// 复查 L330、`get_engine` await 后复查、`arm_pending_cancel_and_cancel` 返回
/// false）统一按 [`should_retry_cascade`] 判定：新轮尚未提交（仅 reserve 未
/// send，`SendMessage` 需等同一把 `turn_lock`，engine 里仍是旧轮遗留子代理）
/// 或当前空闲时持锁补发一次 `cascade_cancel`，不会命中新轮子代理。不再需要
/// `cascade_queued` 标志：`CancelSubAgents` 幂等，phase 1 已入队时再补发一次
/// 是 no-op（简化③）。
///
/// [`should_retry_cascade`]: fn@should_retry_cascade
async fn cancel_turn_with_gates<G, E, EFut, X, F, FFut, C>(
    turn_locks: &SessionTurnLocks,
    turn_lifecycles: &SessionTurnLifecycles,
    turn_shell_tasks: &SessionTurnShellTasks,
    session_id: &str,
    mut get_engine: E,
    mut cancel_current: X,
    mut cascade_cancel: F,
    claim_unsubmitted: C,
) where
    E: FnMut() -> EFut,
    EFut: Future<Output = Option<G>>,
    X: FnMut(&G),
    F: FnMut(&G) -> FFut,
    FFut: Future<Output = ()>,
    C: Fn(&Arc<TurnLifecycle>, u64) -> bool,
{
    // 阶段一：无锁先触发 cancel_token——仅在仍是发起时刻那一轮时才取消，
    // 否则会误命中随后已 reset_cancel_token 的新轮活跃 token。
    let target = turn_lifecycles
        .get(session_id)
        .and_then(|lc| lc.current_turn_generation());
    // 精简①：前置校验可省——get_engine 不产生副作用，await 后的复查 +
    // arm_pending_cancel_and_cancel 的锁内原子校验已闭合同一 TOCTOU 窗口，
    // 前置仅多一次 entries 锁读。
    if let Some(engine) = get_engine().await {
        // get_engine 内部有 await（handle_for 取 entries 锁），await 期间轮次
        // 可能切换：必须在 await 之后、cancel 之前重新校验 epoch（TOCTOU）。
        if generation_matches(
            target,
            turn_lifecycles
                .get(session_id)
                .and_then(|lc| lc.current_turn_generation()),
        ) {
            // 先 arm 再 cancel，且二者在同一 state 锁临界区内原子完成：
            // arm_pending_cancel_and_cancel 持锁校验 turn_epoch == target、
            // 设置 pending（条件满足时）、并执行 cancel_current——reserve_turn
            // 需要同一把 state 锁，无法在「校验/arm」与「取消」之间插入轮次
            // 切换，旧 cancel 不会命中新轮已 reset_cancel_token 的活跃 token
            // （reviewer 点 8）。epoch 不匹配时返回 false 且不执行取消闭包，
            // 阶段二持锁复查 generation 会整体 no-op（reviewer 点 6）。
            if let Some(lifecycle) = turn_lifecycles.get(session_id) {
                lifecycle
                    .arm_pending_cancel_and_cancel(target.unwrap_or(0), || cancel_current(&engine));
            } else {
                cancel_current(&engine);
            }
        }
    }

    // 阶段二：持锁清理 turn 状态与 shell 任务。
    let turn_lock = turn_locks.for_session(session_id).await;
    let _turn = turn_lock.lock().await;

    let lifecycle = turn_lifecycles.get(session_id);
    let current = lifecycle
        .as_ref()
        .and_then(|lc| lc.current_turn_generation());
    if !generation_matches(target, current) {
        // 目标轮已结束（新轮已 reserve）：整体 no-op。此 early-return 必须在
        // request_cancel / claim_unsubmitted / cancel_engine 之前——request_cancel
        // 会取消当前 active shell scope，若已是新轮会误清理新轮的 shell 任务。
        //
        // 例外（reviewer 点 9 + G1）：phase 1 的 best-effort try_send 若因 ops
        // 通道满而未送达，旧轮派生的 detached 子代理未被取消。此时若新轮尚未
        // 提交（仅 reserve 未 send：SendMessage 需等同一把 turn_lock、被本函数
        // 持有，engine 里仍是旧轮遗留子代理）或当前空闲，持锁补发一次级联取消
        // 是安全的——不会命中新轮刚启动的子代理。幂等：phase 1 已入队时再补发
        // 一次是 no-op（简化③，无需 cascade_queued 标志）。
        if should_retry_cascade(lifecycle.as_deref()) {
            if let Some(engine) = get_engine().await {
                cascade_cancel(&engine).await;
            }
        }
        return;
    }

    // generation 匹配，目标轮仍是发起时刻那一轮：清理它的 shell 任务。
    let shell_cancellation = turn_shell_tasks.request_cancel(session_id);
    // claim_unsubmitted 在 lifecycle state 锁内与 target epoch 原子校验并认领：
    // 本行与上方 generation 复查之间另一 worker 可能已结束旧轮并 reserve 新轮
    // （reserve_turn 不取 turn_lock），epoch 不匹配时认领必须 no-op，不得把
    // 新轮 reservation 误认领为 Interrupted（reviewer 点 7）。
    let claimed_unsubmitted = lifecycle
        .as_ref()
        .is_some_and(|lc| claim_unsubmitted(lc, target.unwrap_or(0)));
    if !claimed_unsubmitted {
        // 持锁后复查 engine：阶段一可能因 send 正在 spawn 而拿不到。
        // 幂等：阶段一已取消则再 cancel 是 no-op；阶段一未取消则这里补 cancel。
        // 与阶段一同理：get_engine 的 await 之后重新校验 epoch，再先 arm 后 cancel。
        if let Some(engine) = get_engine().await {
            if generation_matches(
                target,
                turn_lifecycles
                    .get(session_id)
                    .and_then(|lc| lc.current_turn_generation()),
            ) {
                if let Some(lifecycle) = lifecycle.as_ref() {
                    // arm 用发起时快照 target（已校验仍是 target 轮），转发器在
                    // TurnStarted 后 take_pending_cancel 时再次校验 epoch，
                    // 跨轮 stale pending 被丢弃。arm + cancel_current 在 state
                    // 锁内原子完成（与阶段一同理，reviewer 点 8）：reserve_turn
                    // 需要同一把 state 锁，无法在「校验/arm」与「取消」之间插入
                    // 轮次切换。返回 false = 复查通过后轮次已切换：跳过 cancel
                    // 及级联副作用，避免命中新轮活跃 token（reviewer 点 6）。
                    let armed = lifecycle
                        .arm_pending_cancel_and_cancel(target.unwrap_or(0), || {
                            cancel_current(&engine)
                        });
                    if armed {
                        // 级联取消必须在释放 turn gate 前完成入队（reviewer 点 4）：
                        // 下一轮 SendMessage 需等同一把 turn_lock，级联取消必先入队，
                        // FIFO 保证 engine 先取消旧轮子智能体、后启动新轮。
                        cascade_cancel(&engine).await;
                    } else if should_retry_cascade(Some(lifecycle.as_ref())) {
                        // G1 漏发点：arm 被拒 = 复查通过后轮次已切换。phase-1 的
                        // best-effort try_send 若未送达（通道满）且新轮尚未提交
                        // （engine 里仍是旧轮遗留子代理），补发级联取消，避免
                        // 旧轮 detached 子代理继续运行（与入口 mismatch 分支同一
                        // 谓词，见 should_retry_cascade）。
                        cascade_cancel(&engine).await;
                    }
                    // 被拒：轮次已切换，不得取消新轮 engine / 子智能体。
                } else {
                    cancel_current(&engine);
                    // lifecycle 缺失（无活跃轮）时 cascade 无意义：级联取消
                    // CancelSubAgents 针对的是 engine 当前子智能体，空闲 engine
                    // 上没有活跃子智能体，不发也无损（保持原行为）。
                }
            } else if should_retry_cascade(lifecycle.as_deref()) {
                // G1 漏发点：get_engine await 之后复查 mismatch（T2 在 await 期间
                // reserve）。phase-1 的 try_send 若未送达且新轮尚未提交，补发级联
                // 取消——入口 mismatch 分支的补发在此发现点不会被评估，必须单独
                // 补上（reviewer 点 9 的同类窗口）。
                cascade_cancel(&engine).await;
            }
        }
    }
    if let Some(cancellation) = shell_cancellation {
        cancellation.cleanup().await;
    }
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
    eval_model_snapshots: EvalModelSnapshots,
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
            eval_model_snapshots: EvalModelSnapshots::default(),
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
            let scope = self.bridge.session_policy(&sid).mode();
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
            let scope = self.bridge.session_policy(&sid).mode();
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
        explicit_model_override: Option<SavedModel>,
    ) -> Result<(Pinvou3Bridge, PreparedRuntimeModel, bool)> {
        let mut bridge = self.bridge.clone();
        bridge.prefs = UserPrefs::load();
        let scheduled_profile = self.store.scheduled_profile(session_id);
        // 与命令层 chat.rs 的 is_scheduled 同口径(scheduled_profile 存在即算):
        // scheduled 会话图片固定走 image_analyze 硬规则,即使带 interactive
        // 模型覆盖也不例外,故 always 标记不得用更窄的 pins_scheduled_model。
        bridge.image_analyze_always = scheduled_profile.is_some();
        let interactive_model_override = self.store.session_model_override(session_id);
        let pins_scheduled_model = scheduled_profile.is_some()
            && (scheduled_unattended || interactive_model_override.is_none());
        bridge.session_model = resolve_runtime_model_override(explicit_model_override, || {
            resolve_spawn_model(
                &bridge.prefs.advanced.saved_models,
                scheduled_profile.as_ref(),
                interactive_model_override.as_deref(),
                scheduled_unattended,
            )
        })?;
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
        let eval_model = self.eval_model_snapshots.for_session(session_id);
        let (bridge, prepared, pins_scheduled_model) = self
            .prepare_runtime_model(session_id, scheduled_unattended, eval_model)
            .await?;
        Ok(self
            .finalize_runtime_bridge(bridge, &prepared, pins_scheduled_model)
            .await)
    }

    /// 取该 session 的 engine,没有就 spawn 一个。spawn 后若该 session 有磁盘历史
    /// 则一次性 `SyncSession` 把历史 messages 注水进新 engine(冷启动 / app 重启后
    /// 打开旧会话再发消息的场景)。
    pub async fn get_or_spawn(&self, session_id: &str) -> Result<AppEngine> {
        let eval_model = self.eval_model_snapshots.for_session(session_id);
        self.get_or_spawn_with_policy(session_id, false, eval_model)
            .await
    }

    /// Spawn policy for an unattended automation turn is deliberately distinct
    /// from an interactive continuation: the task profile remains authoritative
    /// even if the user temporarily selected another model while viewing it.
    async fn get_or_spawn_with_policy(
        &self,
        session_id: &str,
        scheduled_unattended: bool,
        explicit_model_override: Option<SavedModel>,
    ) -> Result<AppEngine> {
        let runtime_lock = self.runtime_model_locks.for_session(session_id).await;
        let _runtime = runtime_lock.lock().await;
        let (bridge, prepared, pins_scheduled_model) = self
            .prepare_runtime_model(session_id, scheduled_unattended, explicit_model_override)
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
        // 是 EngineConfig.skills_dir 的发现根（build_engine_config_for_session_roots
        // 注入路径），必须先于 spawn 存在，否则首轮 prompt 无 `## Skills` 块。
        {
            let sid = session_id.to_string();
            let scope = self.bridge.session_policy(&sid).mode();
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

    /// Eval-only deletion keeps ordinary delete semantics, but also schedules the existing
    /// late sweep when the immediate disk deletion fails. The error remains observable to the
    /// Judge adapter, while the sweep prevents a transient filesystem failure from silently
    /// retaining the temporary transcript forever.
    pub(crate) async fn delete_eval_session(&self, session_id: &str) -> Result<()> {
        let result = self.delete_chat_session(session_id).await;
        if result.is_err() {
            Self::schedule_late_sweep(
                crate::platform::paths::sessions_root().join(session_id),
                "late sweep of failed eval cleanup",
            );
        }
        result
    }

    pub(crate) fn schedule_eval_cleanup(&self, session_id: &str) {
        Self::schedule_late_sweep(
            crate::platform::paths::sessions_root().join(session_id),
            "background takeover of eval cleanup",
        );
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
                eprintln!("[engine_pool] shutdown {session_id} failed: {e:#}");
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
        self.eval_model_snapshots.forget_session(session_id);
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
        default_model_for_new_session_from(&prefs, &self.bridge)
    }

    pub(crate) fn tested_eval_identity(&self) -> ModelIdentity {
        let prefs = UserPrefs::load();
        identity_for_active_model(&self.bridge, &prefs)
    }

    pub(crate) fn pin_active_eval_suite_model(&self) -> Result<EvalSuiteModelSnapshot> {
        let prefs = UserPrefs::load();
        let saved_model = prefs
            .active_model()
            .cloned()
            .context("active evaluation model is not configured")?;
        let identity = identity_for_saved_model(&self.bridge, &saved_model);
        Ok(self.eval_model_snapshots.pin_suite(saved_model, identity))
    }

    pub(crate) fn derive_eval_suite_case_selection(
        &self,
        suite: &EvalSuiteModelSnapshot,
    ) -> Result<EvalModelSelection> {
        self.eval_model_snapshots.derive_case_selection(suite)
    }

    pub(crate) fn discard_eval_suite_model(&self, suite: &EvalSuiteModelSnapshot) {
        self.eval_model_snapshots.discard_suite(suite);
    }

    /// Resolve and privately pin the complete SavedModel while returning only a
    /// non-sensitive opaque selection to the evaluation layer. Callers that do
    /// not pass the selection to `prepare_eval_session` must explicitly discard it.
    pub(crate) fn pin_eval_model_selection(&self, model_id: &str) -> Result<EvalModelSelection> {
        let prefs = UserPrefs::load();
        let (saved, identity) = resolve_eval_model_selection_from(
            &self.bridge,
            &prefs.advanced.saved_models,
            model_id,
        )?;
        Ok(self.eval_model_snapshots.pin(saved, identity))
    }

    pub(crate) fn discard_eval_model_selection(&self, selection: &EvalModelSelection) {
        self.eval_model_snapshots.discard(selection);
    }

    /// 创建并加载一次性评测会话。评测 runner 预先决定 session ID，以便报告和
    /// 清理精确关联；普通 GUI 会话继续使用 SessionStore 自动生成的 ID。
    pub(crate) async fn prepare_eval_session(
        &self,
        session_id: &str,
        model_selection: Option<&EvalModelSelection>,
    ) -> Result<()> {
        match model_selection {
            None => {
                let (model, model_id) = self.default_model_for_new_session();
                self.store.create_empty_with_id(
                    session_id.to_string(),
                    model,
                    model_id,
                    self.bridge.workspace.clone(),
                )?;
                self.get_or_spawn(session_id).await?;
            }
            Some(selection) => {
                self.eval_model_snapshots
                    .bind_to_session(session_id, selection)?;
                let prepare_result = self.store.create_empty_with_id(
                    session_id.to_string(),
                    selection.wire_model().to_string(),
                    selection.model_id().map(str::to_string),
                    self.bridge.workspace.clone(),
                );
                if let Err(error) = prepare_result {
                    self.eval_model_snapshots.forget_session(session_id);
                    return Err(error);
                }
                if let Err(error) = self.get_or_spawn(session_id).await {
                    self.eval_model_snapshots.forget_session(session_id);
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn eval_session_execution_root(
        &self,
        session_id: &str,
    ) -> Result<std::path::PathBuf> {
        Ok(self.store.session_roots(session_id)?.execution)
    }

    /// 读取评测临时会话的 transcript 快照，不暴露可变存储句柄。
    pub(crate) fn load_eval_transcript(&self, session_id: &str) -> Result<Vec<Message>> {
        Ok(self.store.load(session_id)?.messages)
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
            anyhow::bail!("当前会话不支持 Pinvou 多智能体模式");
        }
        let _reservation = self.turn_lifecycles.for_session(session_id).reserve()?;
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
        self.bridge.multi_agent_mode_available(session_id)
    }

    /// Resolve the session-owned delegated-agent runtime-state root.
    /// For project-bound Code sessions this is distinct from the execution root.
    pub(crate) fn session_state_root(
        &self,
        session_id: &str,
    ) -> std::result::Result<std::path::PathBuf, String> {
        self.store
            .session_roots(session_id)
            .map(|roots| roots.ledger)
            .map_err(|error| format!("解析会话状态根失败: {error:#}"))
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
        let expert_snapshot = (self.store.mode_state(session_id).multi_agent
            && self.multi_agent_mode_available(session_id))
        .then(ExpertRosterSnapshot::capture);
        self.send_reserved_user_message(
            session_id,
            content,
            display_message,
            mode,
            restrict_tools_for_turn,
            expert_snapshot,
            reservation,
        )
        .await
    }

    pub(crate) async fn send_eval_user_message(
        &self,
        session_id: &str,
        content: String,
        policy: &crate::features::assistant::product_runtime::eval_tool_policy::EvalTurnPolicy,
    ) -> Result<()> {
        let reservation = self.reserve_turn(session_id)?;
        let display_message = user_display_message(content.clone());
        self.send_reserved_eval_user_message(
            session_id,
            content,
            display_message,
            policy,
            reservation,
        )
        .await
    }

    async fn send_reserved_eval_user_message(
        &self,
        session_id: &str,
        content: String,
        display_message: Message,
        policy: &crate::features::assistant::product_runtime::eval_tool_policy::EvalTurnPolicy,
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
        let turn_lock = self.turn_locks.for_session(session_id).await;
        let _turn = turn_lock.lock().await;
        self.store.load(session_id).with_context(|| {
            format!("Session '{session_id}' was deleted before the eval turn could start")
        })?;
        reservation.ensure_active()?;
        self.get_or_spawn(session_id)
            .await?
            .send_reserved_eval_message(content, policy, reservation)
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
        expert_snapshot: Option<std::sync::Arc<ExpertRosterSnapshot>>,
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
        // 完整 body 已在加持首条消息一次性注入(commands::chat take_pending_turn_injections)。
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
                expert_snapshot,
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
            let engine = match self
                .get_or_spawn_with_policy(session_id, true, None)
                .await
            {
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
        // 两阶段 generation 守护见 cancel_turn_with_gates：cancel 请求绑定发起
        // 时刻的轮次 epoch，并发请求中排队较晚的 C2 在 turn_lock 释放后若发现
        // 目标轮已结束（新轮已 reserve），整体 no-op，不误取消新轮。
        let app = &self.app;
        cancel_turn_with_gates(
            &self.turn_locks,
            &self.turn_lifecycles,
            &self.turn_shell_tasks,
            session_id,
            // get_engine：取在场 engine 句柄（不取消，epoch 校验由
            // cancel_turn_with_gates 在 await 之后、cancel 之前执行，消除
            // handle_for 取 entries 锁期间的 TOCTOU 窗口）。
            // handle_for 只瞬时取 entries 锁（与 send 内部瞬时取 entries 锁不冲突），
            // 不等 turn_lock——阶段一无锁先触发，turn_loop 的 biased select 立即跳出。
            || async move { self.handle_for(session_id).await },
            // cancel_current：对在场 engine 触发同步取消（幂等），并尽力
            // try_send 级联取消该 engine 的后台子智能体（multiagent，
            // ADR-0006）：「停止」按钮是子智能体唯一的确定性停止入口（卡片上
            // 没有取消按钮，自然语言指令只是建议），只取消宿主轮会留下继续
            // 烧钱的后台子智能体。try_send 不阻塞、通道有空位时立即入队
            // （早于下一轮 SendMessage）；通道满（容量 32）时放弃，由阶段二
            // 及 mismatch 补发路径持锁 await 保证送达（reviewer 点 9 + G1）。
            |engine| {
                engine.cancel_current();
                let _ = engine.handle.try_send(Op::CancelSubAgents);
            },
            // cascade_cancel：阶段二持 turn_lock 时 await 发送级联取消，保证在
            // 释放 turn gate 前完成入队——下一轮 SendMessage 必须等同一把
            // turn_lock（send_reserved_user_message），因此级联取消必先于新轮
            // 消息入队，engine 先取消旧轮子智能体、后启动新轮，不会误杀新轮
            // 刚启动的子智能体（reviewer 点 4）。双发无害：与阶段一 try_send
            // 及回收路径的 CancelSubAgents 都是幂等 no-op。
            |engine| {
                let handle = engine.handle.clone();
                let sid = session_id.to_string();
                async move {
                    if let Err(e) = handle.send(Op::CancelSubAgents).await {
                        eprintln!("[engine_pool] cancel subagents {sid} failed: {e:#}");
                    }
                }
            },
            // claim_unsubmitted：未提交 reservation 的认领终态路径（同步认领+发终态）。
            // 携带 target epoch：认领与 turn_epoch 校验在 state 锁内原子完成，
            // 复查后已切轮（新轮已 reserve）时认领 no-op，不误杀新轮。
            |lifecycle, target| {
                lifecycle.emit_unsubmitted_interrupted_terminal_for_epoch(app, session_id, target)
            },
        )
        .await
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
                eprintln!("[engine_pool] set_disallowed_all {sid} failed: {e:#}");
            }
        }
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

fn identity_for_saved_model(bridge: &Pinvou3Bridge, saved: &SavedModel) -> ModelIdentity {
    let effective = bridge.with_session_model(Some(saved.clone()));
    ModelIdentity::new(effective.provider(), effective.model())
}

fn identity_for_active_model(bridge: &Pinvou3Bridge, prefs: &UserPrefs) -> ModelIdentity {
    match prefs.active_model() {
        Some(saved) => identity_for_saved_model(bridge, saved),
        None => ModelIdentity::new(bridge.provider(), bridge.model()),
    }
}

fn default_model_for_new_session_from(
    prefs: &UserPrefs,
    bridge: &Pinvou3Bridge,
) -> (String, Option<String>) {
    match prefs.active_model() {
        Some(model) => (model.model.clone(), Some(model.id.clone())),
        None => (bridge.model(), None),
    }
}

fn resolve_eval_model_selection_from(
    bridge: &Pinvou3Bridge,
    models: &[SavedModel],
    model_id: &str,
) -> Result<(SavedModel, ModelIdentity)> {
    let saved = models
        .iter()
        .find(|model| model.id == model_id)
        .with_context(|| format!("evaluation model ID '{model_id}' was not found"))?;
    let identity = identity_for_saved_model(bridge, saved);
    Ok((saved.clone(), identity))
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

fn resolve_runtime_model_override<F>(
    explicit_model_override: Option<SavedModel>,
    resolve_normal: F,
) -> Result<Option<SavedModel>>
where
    F: FnOnce() -> Result<Option<SavedModel>>,
{
    match explicit_model_override {
        Some(model) => Ok(Some(model)),
        None => resolve_normal(),
    }
}

#[cfg(test)]
// 测试借 platform::paths::tests::ENV_LOCK(std Mutex)串行化全局 env;单线程测试内跨 await 持有无竞争者,不会死锁。
#[allow(clippy::await_holding_lock)]
mod scheduled_model_tests {
    use super::{
        cancel_turn_with_gates, default_model_for_new_session_from, delete_chat_session_with_gate,
        delete_scheduled_run_with_gate, delete_then_forget, generation_matches,
        identity_for_active_model, identity_for_saved_model, quiesce_engine_before_reclaim,
        resolve_eval_model_selection_from, resolve_runtime_model_override, resolve_scheduled_model,
        resolve_spawn_model, scheduled_profile_after_turn_gate, should_sync_session,
        EvalModelSnapshots, ModelIdentity, ModelUpdateRevisions, Pinvou3Bridge,
        PreparedRuntimeState, ScheduledUnattendedGuard, SessionShellManagers,
        SessionTurnLifecycles, SessionTurnLocks, SessionTurnShellTasks,
    };
    use crate::features::assistant::runtime_model::PreparedRuntimeModel;
    use crate::features::sessions::{ScheduledRunMode, ScheduledRunProfile, SessionStore};
    use crate::platform::credential_store::{CredentialEditAction, CredentialState};
    use crate::platform::prefs::{ImageCapabilityOverride, ModelPreset, SavedModel};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    struct EnvRestore(Vec<(&'static str, Option<std::ffi::OsString>)>);

    impl EnvRestore {
        fn capture(names: &[&'static str]) -> Self {
            Self(
                names
                    .iter()
                    .map(|name| (*name, std::env::var_os(name)))
                    .collect(),
            )
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (name, value) in self.0.drain(..) {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    fn isolated_eval_bridge() -> (Pinvou3Bridge, std::path::PathBuf, EnvRestore) {
        let restore = EnvRestore::capture(&[
            "PINVOU3_HOME",
            "DEEPSEEK_MODEL",
            "DEEPSEEK_PROVIDER",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_MAX_OUTPUT_TOKENS",
            "PINVOU3_SESSION_ARTIFACTS",
        ]);
        let home = std::env::temp_dir().join(format!(
            "pinvou-eval-selection-{}-{}",
            std::process::id(),
            crate::platform::paths::tests::unique_suffix()
        ));
        std::env::set_var("PINVOU3_HOME", &home);
        std::env::remove_var("DEEPSEEK_MODEL");
        std::env::remove_var("DEEPSEEK_PROVIDER");
        std::env::remove_var("DEEPSEEK_BASE_URL");
        let bridge = Pinvou3Bridge::boot().expect("boot isolated test bridge");
        (bridge, home, restore)
    }

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
            reasoning_effort: None,
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

    #[test]
    fn missing_eval_model_error_names_only_the_requested_id() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (bridge, home, _env) = isolated_eval_bridge();
        let configured = model("configured", "wire-model");
        let error = resolve_eval_model_selection_from(&bridge, &[configured], "missing-judge")
            .expect_err("unknown model ID must fail");
        let message = format!("{error:#}");

        assert!(message.contains("missing-judge"));
        assert!(!message.to_ascii_lowercase().contains("api_key"));
        assert!(!message.to_ascii_lowercase().contains("secret"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn identity_uses_actual_bridge_provider_and_model_overrides() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (bridge, home, _env) = isolated_eval_bridge();
        std::env::set_var("DEEPSEEK_MODEL", "actual-model");
        std::env::set_var("DEEPSEEK_PROVIDER", "actual-provider");

        let first = identity_for_saved_model(&bridge, &model("first", "raw-one"));
        let second = identity_for_saved_model(&bridge, &model("second", "raw-two"));

        assert_eq!(first, second);
        assert!(
            crate::features::assistant::eval::validate_judge_identity(&first, &second).is_err()
        );

        std::env::remove_var("DEEPSEEK_MODEL");
        let mut models = vec![model("judge", "snapshot-model")];
        let (saved, identity) = resolve_eval_model_selection_from(&bridge, &models, "judge")
            .expect("resolve saved model");
        let snapshots = EvalModelSnapshots::default();
        let selection = snapshots.pin(saved, identity);
        models[0].model = "changed-after-resolution".to_string();
        assert_eq!(selection.wire_model(), "snapshot-model");
        assert_eq!(selection.model_id(), Some("judge"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn pinned_eval_snapshot_binds_once_and_survives_later_config_changes() {
        let snapshots = EvalModelSnapshots::default();
        let saved = model("judge", "judge-wire");
        let selection = snapshots.pin(
            saved.clone(),
            super::ModelIdentity::new("openai", "judge-wire"),
        );

        snapshots
            .bind_to_session("judge-session", &selection)
            .expect("first bind");
        assert!(snapshots
            .bind_to_session("other-session", &selection)
            .is_err());
        assert!(snapshots.saved_models.lock().is_empty());
        let mut latest_models = vec![model("judge", "changed-wire")];
        latest_models.clear();
        assert!(latest_models.is_empty());
        let resolved =
            resolve_runtime_model_override(snapshots.for_session("judge-session"), || {
                panic!("deleted prefs model must not be consulted for a pinned eval session");
                #[allow(unreachable_code)]
                Ok(None)
            })
            .expect("resolve lifecycle pin");
        assert_eq!(resolved, Some(saved));
        let debug = format!("{selection:?}");
        assert!(!debug.contains("example.invalid"));
        assert!(!debug.contains("api_key"));
    }

    #[test]
    fn discarded_eval_snapshot_releases_private_saved_model() {
        let snapshots = EvalModelSnapshots::default();
        let selection = snapshots.pin(
            model("judge", "judge-wire"),
            super::ModelIdentity::new("openai", "judge-wire"),
        );
        assert_eq!(snapshots.saved_models.lock().len(), 1);

        snapshots.discard(&selection);

        assert!(snapshots.saved_models.lock().is_empty());
    }

    #[test]
    fn suite_snapshot_derives_unique_case_selections_from_one_model() {
        let snapshots = EvalModelSnapshots::default();
        let suite = snapshots.pin_suite(
            model("tested-a", "wire-a"),
            super::ModelIdentity::new("provider-a", "wire-a"),
        );
        // Represents preferences switching to B after suite startup. Derivation must not use it.
        let _latest_active = model("tested-b", "wire-b");

        let first = snapshots
            .derive_case_selection(&suite)
            .expect("derive first case");
        let second = snapshots
            .derive_case_selection(&suite)
            .expect("derive second case");

        assert_ne!(first.token(), second.token());
        assert_eq!(first.model_id(), Some("tested-a"));
        assert_eq!(second.model_id(), Some("tested-a"));
        assert_eq!(first.identity(), suite.identity());
        assert_eq!(second.identity(), suite.identity());
    }

    #[test]
    fn discarded_suite_snapshot_cannot_derive_more_case_models() {
        let snapshots = EvalModelSnapshots::default();
        let suite = snapshots.pin_suite(
            model("tested-a", "wire-a"),
            super::ModelIdentity::new("provider-a", "wire-a"),
        );

        snapshots.discard_suite(&suite);

        assert!(snapshots.derive_case_selection(&suite).is_err());
        assert!(snapshots.suite_models.lock().is_empty());
    }

    #[test]
    fn forgetting_eval_session_releases_lifecycle_model_pin() {
        let snapshots = EvalModelSnapshots::default();
        let saved = model("judge", "judge-wire");
        let selection = snapshots.pin(
            saved.clone(),
            super::ModelIdentity::new("openai", "judge-wire"),
        );
        snapshots
            .bind_to_session("judge-session", &selection)
            .expect("bind session");

        snapshots.forget_session("judge-session");

        assert_eq!(snapshots.for_session("judge-session"), None);
        assert!(snapshots.session_models.lock().is_empty());
    }

    #[test]
    fn explicit_eval_override_wins_without_consulting_normal_resolution() {
        let explicit = model("pinned", "pinned-wire");
        let selected = resolve_runtime_model_override(Some(explicit.clone()), || {
            panic!("normal model resolution must not run for an explicit eval snapshot");
            #[allow(unreachable_code)]
            Ok(None)
        })
        .expect("resolve explicit model");

        assert_eq!(selected, Some(explicit));
    }

    #[test]
    fn tested_identity_uses_latest_active_model_instead_of_boot_bridge_model() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (bridge, home, _env) = isolated_eval_bridge();
        let boot_identity = ModelIdentity::new(bridge.provider(), bridge.model());
        let mut prefs = crate::platform::prefs::UserPrefs::default();
        let mut active = model("active-b", "active-model-b");
        active.vendor = Some("kimi".to_string());
        prefs.advanced.saved_models = vec![active];
        prefs.advanced.active_model_id = Some("active-b".to_string());

        let identity = identity_for_active_model(&bridge, &prefs);

        assert_eq!(identity.provider, "moonshot");
        assert_eq!(identity.model, "active-model-b");
        assert_ne!(identity, boot_identity);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn default_eval_metadata_keeps_raw_saved_model_despite_env_override() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (bridge, home, _env) = isolated_eval_bridge();
        std::env::set_var("DEEPSEEK_MODEL", "env-wire-override");
        let mut prefs = crate::platform::prefs::UserPrefs::default();
        prefs.advanced.saved_models = vec![model("active", "raw-saved-wire")];
        prefs.advanced.active_model_id = Some("active".to_string());

        assert_eq!(
            default_model_for_new_session_from(&prefs, &bridge),
            ("raw-saved-wire".to_string(), Some("active".to_string()))
        );
        let _ = std::fs::remove_dir_all(home);
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

    #[test]
    fn chat_delete_failure_still_forgets_runtime_once_and_preserves_error() {
        let forget_count = Arc::new(AtomicUsize::new(0));
        let observed_count = forget_count.clone();

        let error = delete_then_forget(
            || Err(anyhow::anyhow!("sentinel delete failure")),
            move || {
                forget_count.fetch_add(1, Ordering::SeqCst);
            },
        )
        .expect_err("injected deletion must fail");

        assert_eq!(observed_count.load(Ordering::SeqCst), 1);
        assert_eq!(error.to_string(), "sentinel delete failure");
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

    // ── cancel turn generation 守护(review #4872749559 跨轮误取消回归)──────
    // cancel_turn_with_gates 把 cancel 主体从 EnginePool::cancel 抽出，使这三组
    // 测试能用裸 Default 组件 + 探针闭包确定性编排 C1/C2 时序，绕开 bridge /
    // AppHandle / 真实 EngineHandle（跨 crate 私有）。

    #[test]
    fn generation_matches_treats_idle_to_idle_as_match() {
        // (None, None)：空闲会话的 cancel 本就是 no-op，视为匹配走原逻辑。
        assert!(generation_matches(None, None));
        // 同一轮 epoch：匹配。
        assert!(generation_matches(Some(1), Some(1)));
        // 不同 epoch（目标轮已结束、新轮已 reserve）：不匹配 → no-op。
        assert!(!generation_matches(Some(1), Some(2)));
        // 跨 Some/None（目标轮活动、当前空闲，或反之）：不匹配 → no-op。
        assert!(!generation_matches(Some(1), None));
        assert!(!generation_matches(None, Some(1)));
    }

    #[tokio::test]
    async fn stale_cancel_after_turn_change_leaves_new_turn_intact() {
        // reviewer 时序的正向验证（review #4872749559）：
        //   C1/C2 并发取消 turn1。C1 先完整跑完（取消 turn1 + 发终态 + 清 shell），
        //   turn2 在 C1 释放锁前 reserve_turn（不取 turn_lock）抢先 reserve。
        //   C2 的**阶段一**（无锁 cancel_current）此时仍属 turn1（正确，会取消 turn1）；
        //   真正的缺陷窗口在**阶段二**：C2 拿锁后已变 turn2，原实现会无差别
        //   cancel/arm pending 到 turn2。generation 守护让阶段二 early-return。
        //
        // 这里直接编排「C2 快照 turn1 → turn1 终态 → turn2 reserve → C2 阶段二」：
        // 先用 blocker 占住 turn_lock，让 C2 的阶段一快照 turn1 后在阶段二挂起，
        // 主线程推进到 turn2，再释放锁让 C2 阶段二恢复。
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-stale-cancel";

        let lifecycle = lifecycles.for_session(sid);
        // turn1：on_submitted 激活（active+submitted+epoch 自增），使阶段一 cancel_engine
        // 能匹配 generation，且 finish_once 可 claim（需 submitted）。
        assert!(lifecycle.on_submitted());

        let gate = locks.for_session(sid).await;
        let blocker = gate.lock().await;

        // 阶段二侧探针：阶段二若执行 cancel_engine 复查会置位。
        let phase_two_cancel_called = Arc::new(AtomicBool::new(false));
        let phase_one_count = Arc::new(AtomicU64::new(0));
        let probe2 = phase_two_cancel_called.clone();
        let probe1 = phase_one_count.clone();
        // cascade 探针：阶段二 early-return（新轮已 reserve）时级联取消
        // 不得执行——此时 CancelSubAgents 若发出会误杀新轮刚启动的子智能体
        // （reviewer 点 4）。
        let cascade_called = Arc::new(AtomicBool::new(false));
        let probe_cascade = cascade_called.clone();
        // phase 1 级联取消视为已成功入队（本测试不覆盖 reviewer 点 9 的
        // try_send 失败场景；保持 mismatch early-return 的既有断言语义）。
        // C2：阶段一无锁快照 target=Some(1) → 匹配 → get_engine 返回在场 engine
        //     → 校验 epoch 仍匹配 → arm + cancel_current（probe1++）；
        //     阶段二持锁（被 blocker 阻塞），恢复后比对 current ≠ target → early return。
        let cancel_task = tokio::spawn(async move {
            cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                // get_engine：engine 在场（阶段一与阶段二复查都返回 Some）。
                || async { Some(()) },
                // cancel_current：阶段一与阶段二复查都走这里：用计数区分。
                // 阶段一（turn1）probe1 0→1；阶段二若误执行 probe2 置位。
                move |_engine: &()| {
                    let prev = probe1.fetch_add(1, Ordering::AcqRel);
                    if prev >= 1 {
                        probe2.store(true, Ordering::Release);
                    }
                },
                // cascade_cancel：阶段二 early-return，不应被调用。
                move |_engine: &()| {
                    probe_cascade.store(true, Ordering::Release);
                    async {}
                },
                // claim_unsubmitted 不应被调用（turn1 已 submitted）。
                |_lc, _target| false,
            )
            .await
        });
        // 让 C2 进展到阶段一完成、阶段二 gate.lock().await 挂起。
        tokio::task::yield_now().await;

        // turn1 终态（submitted，走 claim 路径）→ turn2 reserve（epoch=2）。
        assert!(lifecycle.finish_once(|| {}).is_some());
        let reservation2 = lifecycle.reserve().expect("turn2 reserve");

        // 释放 turn_lock，C2 阶段二恢复：current=Some(2) ≠ target=Some(1) → early return。
        drop(blocker);
        drop(gate);
        cancel_task.await.expect("cancel task joins");

        // 阶段一执行了一次（取消 turn1，正确）。
        assert_eq!(
            phase_one_count.load(Ordering::Acquire),
            1,
            "phase one cancel on the originating turn must run exactly once"
        );
        // 阶段二未误执行 cancel_engine（守护生效），turn2 不被误取消。
        assert!(
            !phase_two_cancel_called.load(Ordering::Acquire),
            "phase two must not cancel the engine of a turn that started after the cancel was issued"
        );
        // 简化③后（删 cascade_queued）：新轮仅 reserve 未提交时，mismatch 分支
        // 按 should_retry_cascade 补发级联——engine 里仍是旧轮遗留子代理
        // （SendMessage 需等同一把 turn_lock、被本函数持有），补发只取消旧轮
        // 子代理、不命中新轮（submitted=false）。CancelSubAgents 幂等，安全。
        assert!(
            cascade_called.load(Ordering::Acquire),
            "stale cancel must still cascade old subagents when the new turn is only reserved (not submitted)"
        );
        assert!(
            reservation2.ensure_active().is_ok(),
            "new turn reservation must remain valid after a stale cancel's phase two recovered"
        );
    }

    #[tokio::test]
    async fn fresh_cancel_after_turn_change_still_cancels_new_turn() {
        // 对照测试，防止 generation 守护过度：用户在新轮启动**后**才点停止，
        // 快照到新轮 epoch，匹配 → 合法取消新轮（cancel_engine 触发）。
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-fresh-cancel";

        let lifecycle = lifecycles.for_session(sid);
        // turn1 reserve → 终态（未提交认领路径）→ turn2 reserve（epoch=2）。
        let _reservation1 = lifecycle.reserve().expect("turn1 reserve");
        assert!(lifecycle.finish_unsubmitted_once());
        let _reservation2 = lifecycle.reserve().expect("turn2 reserve");

        let cancel_called = Arc::new(AtomicBool::new(false));
        let probe = cancel_called.clone();
        // cascade 探针：正常取消路径（fresh cancel）必须执行级联取消。
        let cascade_called = Arc::new(AtomicBool::new(false));
        let probe_cascade = cascade_called.clone();
        // phase 1 级联取消视为已成功入队（本测试不覆盖 reviewer 点 9 场景）。
        cancel_turn_with_gates(
            &locks,
            &lifecycles,
            &shell_tasks,
            sid,
            // get_engine：engine 在场。
            || async { Some(()) },
            // cancel_current：记录触发。
            move |_engine: &()| {
                probe.store(true, Ordering::Release);
            },
            // cascade_cancel：fresh cancel 在阶段二 generation 匹配后必须被
            // 调用（在 turn gate 内 await 完成入队，reviewer 点 4）。
            move |_engine: &()| {
                probe_cascade.store(true, Ordering::Release);
                async {}
            },
            |_lc, _target| false,
        )
        .await;

        // target=Some(2)=current → 匹配 → get_engine 返回 Some → cancel_current 触发。
        assert!(
            cancel_called.load(Ordering::Acquire),
            "a fresh cancel on the current turn must still cancel its engine"
        );
        assert!(
            cascade_called.load(Ordering::Acquire),
            "a fresh cancel on the current turn must also cascade-cancel its subagents inside the turn gate"
        );
    }

    #[tokio::test]
    async fn stale_cancel_retries_cascade_when_phase_one_try_send_failed() {
        // reviewer 点 9 的确定性回归：phase 1 的 best-effort try_send 因 ops
        // 通道满（容量 32）而失败时，CancelSubAgents 从未入队；若随后旧轮结束、
        // 新轮在 phase 2 取得 turn gate 前完成 reserve，阶段二 generation
        // mismatch 直接 return 会让级联取消永久丢失，旧轮派生的 detached 子代理
        // 继续运行。修复后 mismatch 分支必须在新轮尚未提交（仅 reserve 未
        // send——SendMessage 需等同一把 turn_lock、被本函数持有，engine 里仍是
        // 旧轮遗留子代理）时持锁补发一次 cascade（简化③后不再区分 try_send
        // 是否失败，按 should_retry_cascade 统一补发，幂等无害）。
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-try-send-failure";

        let lifecycle = lifecycles.for_session(sid);
        // turn1：on_submitted 激活（active+submitted+epoch=1）。
        assert!(lifecycle.on_submitted());

        let gate = locks.for_session(sid).await;
        let blocker = gate.lock().await;

        let cascade_called = Arc::new(AtomicBool::new(false));
        let probe_cascade = cascade_called.clone();
        let cancel_called = Arc::new(AtomicBool::new(false));
        let probe_cancel = cancel_called.clone();
        // phase 1 的 try_send 失败（ops 通道满）：补发路径不依赖失败记录，
        // 由 should_retry_cascade（新轮未提交）统一判定。

        let cancel_task = tokio::spawn(async move {
            cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                // get_engine：engine 在场（阶段一与补发复查都返回 Some）。
                || async { Some(()) },
                // cancel_current：阶段一执行一次（取消旧轮）。
                move |_engine: &()| {
                    probe_cancel.store(true, Ordering::Release);
                },
                // cascade_cancel：新轮未提交 → mismatch 分支必须补发，不能因
                // generation mismatch 直接丢弃级联取消。
                move |_engine: &()| {
                    probe_cascade.store(true, Ordering::Release);
                    async {}
                },
                // claim_unsubmitted 不应被调用（turn1 已 submitted）。
                |_lc, _target| false,
            )
            .await
        });
        // 让 cancel 进展到阶段一完成、阶段二 gate.lock().await 挂起。
        tokio::task::yield_now().await;

        // turn1 终态（submitted，走 claim 路径）→ turn2 reserve（epoch=2，
        // 未提交——SendMessage 被阶段二持有的 turn_lock 阻塞）。
        assert!(lifecycle.finish_once(|| {}).is_some());
        let reservation2 = lifecycle.reserve().expect("turn2 reserve");

        // 释放 turn_lock：阶段二恢复，current=Some(2) ≠ target=Some(1) → mismatch
        // → should_retry_cascade（新轮未提交）→ 补发级联取消。
        drop(blocker);
        drop(gate);
        cancel_task.await.expect("cancel task joins");

        assert!(
            cascade_called.load(Ordering::Acquire),
            "a stale cancel must retry the cascade when phase one try_send failed and the new turn has not started"
        );
        assert!(
            cancel_called.load(Ordering::Acquire),
            "phase one must still cancel the originating turn's engine"
        );
        assert!(
            reservation2.ensure_active().is_ok(),
            "new turn reservation must remain valid after the cascade retry"
        );
    }

    #[tokio::test]
    async fn stale_cancel_skips_cascade_retry_when_new_turn_already_submitted() {
        // reviewer 点 9 的补发边界：phase 1 try_send 失败 + 新轮**已提交**
        // （SendMessage 已入 engine，新轮可能已启动子代理）时，mismatch 分支
        // 不得补发级联取消——否则 CancelSubAgents 会误杀新轮刚启动的子代理。
        // 补发仅在「新轮尚未提交」的窗口内安全（engine 里仍是旧轮遗留子代理）。
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-try-send-failure-submitted";

        let lifecycle = lifecycles.for_session(sid);
        // turn1：on_submitted 激活（epoch=1）。
        assert!(lifecycle.on_submitted());

        let gate = locks.for_session(sid).await;
        let blocker = gate.lock().await;

        let cascade_called = Arc::new(AtomicBool::new(false));
        let probe_cascade = cascade_called.clone();
        let cancel_called = Arc::new(AtomicBool::new(false));
        let probe_cancel = cancel_called.clone();

        let cancel_task = tokio::spawn(async move {
            cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                || async { Some(()) },
                move |_engine: &()| {
                    probe_cancel.store(true, Ordering::Release);
                },
                move |_engine: &()| {
                    probe_cascade.store(true, Ordering::Release);
                    async {}
                },
                |_lc, _target| false,
            )
            .await
        });
        // 让 cancel 进展到阶段一完成、阶段二 gate.lock().await 挂起。
        tokio::task::yield_now().await;

        // turn1 终态 → turn2 on_submitted 激活（epoch=2，submitted=true——
        // SendMessage 已入 engine，可能已启动新轮子代理）。
        assert!(lifecycle.finish_once(|| {}).is_some());
        assert!(lifecycle.on_submitted());

        // 释放 turn_lock：阶段二 current=Some(2) ≠ target=Some(1) → mismatch；
        // 新轮已提交 → 不得补发级联取消。
        drop(blocker);
        drop(gate);
        cancel_task.await.expect("cancel task joins");

        assert!(
            !cascade_called.load(Ordering::Acquire),
            "cascade retry must be skipped when the new turn has already been submitted"
        );
        assert!(
            cancel_called.load(Ordering::Acquire),
            "phase one must still cancel the originating turn's engine"
        );
    }

    #[tokio::test]
    async fn stale_cancel_retries_cascade_when_mismatch_surfaces_after_phase_two_engine_lookup() {
        // G1 的确定性回归：mismatch 首次在**阶段二 get_engine await 之后**的
        // 复查（而非入口复查）被发现时，级联补发必须仍然执行。
        //
        // 旧实现只在阶段二入口 mismatch 分支补发；若入口复查仍匹配（T1 仍
        // active，current_turn_generation 报 Some(T1)）、claim no-op（T1 已
        // submitted）、随后 get_engine().await 期间 T2 reserve，后置复查
        // mismatch 会直接跳过——phase-1 的 best-effort try_send 失败时旧轮
        // detached 子代理被静默丢弃。
        //
        // 修复：get_engine await 后复查 mismatch 处，与入口 mismatch 分支共用
        // should_retry_cascade 谓词补发级联。
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-g1-mismatch-after-engine-lookup";

        let lifecycle = lifecycles.for_session(sid);
        // turn1：on_submitted 激活（active+submitted+epoch=1）。
        assert!(lifecycle.on_submitted());
        let target = lifecycle.current_turn_generation().expect("turn1 epoch");
        assert_eq!(target, 1_u64);

        // 阶段二 get_engine await 的挂起点：阶段一第一次调用直接返回；阶段二
        // 第二次调用通知 entered 后挂起，主线程在 await 窗口内切轮，再放行。
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let entered_main = entered.clone();
        let release_main = release.clone();
        let get_engine_calls = Arc::new(AtomicU64::new(0));
        let probe_engine = get_engine_calls.clone();

        let cascade_called = Arc::new(AtomicBool::new(false));
        let probe_cascade = cascade_called.clone();
        let cancel_calls = Arc::new(AtomicU64::new(0));
        let probe_cancel = cancel_calls.clone();

        let cancel_task = tokio::spawn(async move {
            cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    let probe = probe_engine.clone();
                    async move {
                        if probe.fetch_add(1, Ordering::SeqCst) == 1 {
                            // 阶段二复查前的 get_engine：模拟 handle_for 取
                            // entries 锁的 await，挂起等待主线程切轮。
                            entered.notify_one();
                            release.notified().await;
                        }
                        Some(())
                    }
                },
                // cancel_current：阶段一触发一次（取消 T1），阶段二应因后置
                // mismatch 跳过。
                move |_engine: &()| {
                    probe_cancel.fetch_add(1, Ordering::SeqCst);
                },
                // cascade_cancel：后置复查 mismatch + 新轮未提交 → 必须补发。
                move |_engine: &()| {
                    probe_cascade.store(true, Ordering::Release);
                    async {}
                },
                // claim_unsubmitted：T1 已 submitted → no-op（不认领）。
                |_lc, _target| false,
            )
            .await
        });
        // 让 cancel 完成阶段一（第一次 get_engine 直接返回 + arm+cancel），
        // 阶段二取得 turn_lock 并完成入口复查（current=Some(1)==target，匹配），
        // 再进入第二次 get_engine 挂起。
        entered_main.notified().await;

        // 此刻阶段二入口复查已匹配通过、正挂起在 get_engine await 内：切轮。
        // T1 终态 → T2 reserve（epoch=2，未提交——SendMessage 被 turn_lock 阻塞）。
        assert!(lifecycle.finish_once(|| {}).is_some());
        let reservation2 = lifecycle.reserve().expect("turn2 reserve");

        // 放行 get_engine：后置复查 current=Some(2)≠Some(1) → mismatch →
        // should_retry_cascade（新轮未提交）→ 补发级联。
        release_main.notify_one();
        cancel_task.await.expect("cancel task joins");

        assert!(
            get_engine_calls.load(Ordering::SeqCst) >= 2,
            "get_engine must be consulted in phase one and phase two"
        );
        assert!(
            cascade_called.load(Ordering::Acquire),
            "G1: cascade retry must fire when the mismatch is first observed after the phase-two engine lookup"
        );
        assert!(
            cancel_calls.load(Ordering::SeqCst) == 1,
            "phase one cancels turn1; phase two must skip cancel_current on mismatch"
        );
        assert!(
            reservation2.ensure_active().is_ok(),
            "new turn reservation must remain valid after the G1 cascade retry"
        );
    }

    #[tokio::test]
    async fn stale_cancel_skips_unsubmitted_claim_and_leaves_new_turn_intact() {
        // invalidate 路径：cancel_engine 返回 false（模拟 engine 不在场）时，
        // generation 不匹配必须阻止 claim_unsubmitted（认领未提交终态使 reservation
        // 失效），否则新轮未提交 reservation 会被误清。
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-stale-invalidate";

        let lifecycle = lifecycles.for_session(sid);
        let _reservation1 = lifecycle.reserve().expect("turn1 reserve");
        let gate = locks.for_session(sid).await;
        let blocker = gate.lock().await;

        let claimed = Arc::new(AtomicBool::new(false));
        let probe = claimed.clone();
        // phase 1 无 engine，级联取消未送达；但新轮已 reserve 未 send 时
        // mismatch 分支会补发（reviewer 点 9）——engine 不在场则补发 no-op。
        let cancel_task = tokio::spawn(async move {
            cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                // engine 不在场 → get_engine 返回 None → 不取消，走 claim_unsubmitted 分支。
                || async { None::<()> },
                // cancel_current：engine 不在场时不应被调用。
                |_engine: &()| {},
                // cascade_cancel：engine 不在场，阶段二不调用。
                |_engine: &()| async {},
                move |lc, _target| {
                    probe.store(true, Ordering::Release);
                    // 复用真实的未提交认领路径以观察副作用。
                    lc.claim_unsubmitted_terminal()
                },
            )
            .await
        });
        tokio::task::yield_now().await;

        // turn1 终态（未提交认领路径）→ turn2 未提交 reservation（epoch=2）。
        assert!(lifecycle.finish_unsubmitted_once());
        let reservation2 = lifecycle.reserve().expect("turn2 reserve");

        drop(blocker);
        drop(gate);
        cancel_task.await.expect("cancel task joins");

        // 断言：claim_unsubmitted 闭包未被调用，turn2 reservation 仍有效。
        assert!(
            !claimed.load(Ordering::Acquire),
            "stale cancel must not claim an unsubmitted terminal on a new turn"
        );
        assert!(
            reservation2.ensure_active().is_ok(),
            "new turn unsubmitted reservation must survive a stale cancel"
        );
    }

    #[tokio::test]
    async fn stale_cancel_claim_with_epoch_guard_rejects_new_turn() {
        // reviewer 点 7 的集成验证：阶段二 generation 检查通过后、认领前轮次
        // 切换时（reserve_turn 不取 turn_lock），claim_unsubmitted 必须与发起时
        // 快照 target 在 state 锁内原子校验——epoch 不匹配则 no-op，不得把新轮
        // reservation 认领为 Interrupted。这里在 claim 闭包内模拟「检查后切轮」
        // （真实场景由另一 worker 完成），验证 for_epoch 拒绝 stale target 且
        // 新轮 reservation 保持有效。
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-claim-epoch-guard";

        let lifecycle = lifecycles.for_session(sid);
        // turn1：reserve 未提交（epoch=1），engine 不在场 → cancel 走 claim 分支。
        let _reservation1 = lifecycle.reserve().expect("turn1 reserve");

        let new_turn_intact = Arc::new(AtomicBool::new(false));
        let probe = new_turn_intact.clone();
        // phase 1 无 engine，级联取消未送达；engine 不在场时 mismatch 分支
        // 的补发是 no-op，不影响本测试的 claim 语义。
        cancel_turn_with_gates(
            &locks,
            &lifecycles,
            &shell_tasks,
            sid,
            // engine 不在场 → get_engine 返回 None → 不 cancel，走 claim_unsubmitted。
            || async { None::<()> },
            // cancel_current：engine 不在场，不应被调用。
            |_engine: &()| {},
            // cascade_cancel：engine 不在场，阶段二不调用。
            |_engine: &()| async {},
            // claim 闭包：模拟「generation 检查通过后、认领前」切轮，再用发起
            // 时快照 target 认领——必须被拒（epoch 不匹配），新轮完好。
            move |lc, target| {
                assert!(lc.finish_unsubmitted_once(), "turn1 terminal");
                let new_reservation = lc.reserve().expect("turn2 reserve");
                let claimed = lc.claim_unsubmitted_terminal_for_epoch(target);
                probe.store(
                    !claimed && new_reservation.ensure_active().is_ok(),
                    Ordering::Release,
                );
                // 闭包结束时 new_reservation drop：未 submitted → on_reservation_failed
                // 回滚 turn2 的 active 状态。此时 claim 已被拒、阶段二后续不再
                // 触碰 lifecycle（engine 不在场），回滚幂等无害。
                claimed
            },
        )
        .await;

        assert!(
            new_turn_intact.load(Ordering::Acquire),
            "stale claim must reject the new turn and leave its reservation intact"
        );
    }

    #[tokio::test]
    async fn stale_cancel_during_phase_one_engine_lookup_leaves_new_turn_intact() {
        // reviewer 点 1（阶段一 TOCTOU）的确定性回归：generation 校验不能只
        // 发生在 `get_engine().await` **之前**——`get_engine` 内部有 await
        // （`handle_for` 取 entries 锁），await 期间旧轮可能结束、新轮可能
        // reserve 并 `reset_cancel_token()`，随后 `cancel_current()` 会命中
        // 新轮的活跃 token，阶段二发现 epoch 不匹配也撤不回已发生的取消。
        //
        // 这里把轮次切换安排在阶段一的 `get_engine` await **期间**（原测试
        // `stale_cancel_after_turn_change_leaves_new_turn_intact` 只覆盖了
        // 阶段一完成之后的切换，漏掉此窗口）：get_engine 探针挂起在一个
        // oneshot 上模拟取 entries 锁的等待，主线程在此期间推进 turn1 终态 +
        // turn2 reserve，再放行 get_engine → 阶段一 await 后重新校验 epoch
        // 发现不匹配 → 不 cancel；阶段二同样 early-return，turn2 完好。
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-phase1-toctou";

        let lifecycle = lifecycles.for_session(sid);
        // turn1：on_submitted 激活（active+submitted+epoch=1）。
        assert!(lifecycle.on_submitted());

        // 用 Notify 协调：探针先通知「已进入 get_engine 的 await」，挂起等待
        // release；主线程收到 entered 后推进轮次，再放行探针。
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        // 主线程侧副本（spawn 的 async move 会把原件移入任务）。
        let entered_main = entered.clone();
        let release_main = release.clone();
        let cancel_called = Arc::new(AtomicBool::new(false));
        let probe = cancel_called.clone();
        let get_engine_calls = Arc::new(AtomicU64::new(0));
        let probe_calls = get_engine_calls.clone();
        // phase 1 级联取消视为已成功入队（本测试聚焦阶段一 TOCTOU 守护，
        // 不覆盖 reviewer 点 9 的 try_send 失败补发路径；保持 mismatch
        // early-return 不级联的既有断言）。

        let cancel_task = tokio::spawn(async move {
            cancel_turn_with_gates(
                &locks,
                &lifecycles,
                &shell_tasks,
                sid,
                // get_engine：第一次调用（阶段一）通知已进入后挂起（模拟
                // handle_for 取 entries 锁的 await），放行后返回「engine 在场」。
                // 若实现退化（阶段二缺 generation 守护，即 #205 原始 bug），
                // 阶段二会再次调用 get_engine——此时直接返回「engine 在场」让
                // cancel_current 探针触发、测试红；若这里仍 park 于 Notify，
                // 会二次挂起成死锁（CI 表现为超时而非断言失败）。
                move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    let calls = probe_calls.clone();
                    async move {
                        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                            // notify_one 会为未来的 waiter 保留 permit：即使
                            // spawned task 先执行到这里而主线程尚未注册
                            // notified().await，信号也不丢失（reviewer 点 5：
                            // notify_waiters 不为后注册的 waiter 保留通知，
                            // 会永久等待）。
                            entered.notify_one();
                            release.notified().await;
                        }
                        Some(())
                    }
                },
                // cancel_current：不应被调用（阶段一 await 后 epoch 不匹配）。
                move |_engine: &()| {
                    probe.store(true, Ordering::Release);
                },
                // cascade_cancel：generation 不匹配，不应被调用。
                |_engine: &()| async {},
                |_lc, _target| false,
            )
            .await
        });
        // 等 C2 阶段一进入 get_engine 的 await（即拿到 entries 锁前的挂起点）。
        entered_main.notified().await;

        // 在 await 窗口内切换轮次：turn1 终态（submitted → claim 路径）→ turn2 reserve。
        assert!(lifecycle.finish_once(|| {}).is_some());
        let reservation2 = lifecycle.reserve().expect("turn2 reserve");

        // 放行 get_engine：阶段一 await 后重新校验 epoch → target=Some(1) ≠ current=Some(2)
        // → 不 cancel；阶段二同样 early-return。
        // notify_one 与 entered 侧同理：为已注册/未来 waiter 都保留 permit。
        release_main.notify_one();
        cancel_task.await.expect("cancel task joins");

        assert!(
            !cancel_called.load(Ordering::Acquire),
            "phase one must not cancel the new turn when the turn switched during the engine lookup await"
        );
        assert!(
            reservation2.ensure_active().is_ok(),
            "new turn reservation must remain valid after a phase-one TOCTOU"
        );
    }

    #[tokio::test]
    async fn pending_cancel_is_armed_before_cancel_current_in_phase_two() {
        // reviewer 点 2 的确定性回归：阶段二必须先 `arm_pending_cancel` 再
        // `cancel_current`。若顺序颠倒（先 cancel 后 arm）：
        //   cancel 命中旧 token → engine reset_cancel_token + TurnStarted →
        //   forwarder 因尚未 arm 不补 cancel → 此处再 arm 时 `turn_id` 已存在
        //   被拒 → 停止请求丢失。
        //
        // 原子化后（reviewer 点 8）arm 与 cancel_current 在同一个 state 锁
        // 临界区内完成，顺序由 `arm_pending_cancel_and_cancel` 内部保证，
        // TurnStarted 无法插入两步之间（forwarder 消费 pending 也需同一把锁）。
        // 这里验证送达性不变量：cancel 执行后 pending 仍可被 forwarder 消费
        // （take 得到 Some）——即「先 arm 后 cancel」的效果保持。
        let locks = SessionTurnLocks::default();
        let lifecycles = SessionTurnLifecycles::default();
        let shell_tasks = SessionTurnShellTasks::default();
        let sid = "session-arm-order";

        let lifecycle = lifecycles.for_session(sid);
        // turn：on_submitted 激活（submitted 未 started，turn_id 仍为 None，
        // epoch=1）——arm_pending_cancel 的前置条件满足。
        assert!(lifecycle.on_submitted());

        let cancel_calls = Arc::new(AtomicU64::new(0));
        let probe_calls = cancel_calls.clone();
        // phase 1 级联取消视为已成功入队（本测试聚焦 arm 顺序不变量，
        // 不覆盖 reviewer 点 9 的 try_send 失败补发路径）。
        cancel_turn_with_gates(
            &locks,
            &lifecycles,
            &shell_tasks,
            sid,
            // get_engine：engine 在场。
            || async { Some(()) },
            // cancel_current 探针：仅计数。cancel 在 state 锁内执行，探针不能
            // 再取 lifecycle 锁（std Mutex 非重入，会死锁），改为在调用结束后
            // 验证 pending 仍可被 forwarder 消费。
            move |_engine: &()| {
                probe_calls.fetch_add(1, Ordering::SeqCst);
            },
            // cascade_cancel：正常取消路径，阶段二会调用；此处 no-op 探针。
            |_engine: &()| async {},
            |_lc, _target| false,
        )
        .await;

        // 阶段一、阶段二各 cancel 一次（engine 在场、epoch 匹配）。
        assert!(
            cancel_calls.load(Ordering::Acquire) >= 1,
            "cancel_current must run on the originating turn"
        );
        // arm 先于 cancel：cancel 执行后 pending 仍可被 forwarder 消费
        // （模拟 TurnStarted 到达时 take 并重放）。
        let epoch = lifecycle.current_turn_generation().unwrap_or(0);
        assert!(
            lifecycle.take_pending_cancel(epoch).is_some(),
            "pending_cancel must be armed before cancel_current so a TurnStarted can be replayed"
        );
    }
}
