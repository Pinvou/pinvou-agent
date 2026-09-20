//! 跨特性共用的空闲会话回收（idle-reaper）脚手架。
//!
//! `features::assistant::engine_pool`（进程内 engine 池）与
//! `features::codex_acp`（ACP 子进程池）各自维护着同构的空闲回收巡检：
//! 相同的阈值/巡检节奏常量、相同形状的纯函数回收判定、相同的
//! 「Drop 时 cancel + abort 双保险」巡检句柄，以及相同的「跳过首个立即
//! tick + 单轮 panic 隔离」巡检循环。本模块把这份脚手架收敛为单一实现；
//! 各特性只保留自己的会话身份语义（忙旗标的含义、回收动作、日志前缀），
//! 通过泛型参数接入，不改变任何时序与回收条件。

use std::collections::HashMap;
use std::time::Duration;

use tauri::async_runtime::JoinHandle;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// 空闲回收阈值：每个空闲条目都占一份活资源（engine 是进程内 task + 专属
/// 通道/工具集；ACP 会话是活的 node/codex/kimi 子进程），池无上限、内存随
/// 会话数线性涨。空闲超过该时长且无在途工作且非 active 会话时回收。回收
/// 只是回到 lazy spawn 语义（下次使用时重建），无损；30 分钟取偏保守值：
/// 宁可少回收也不误杀刚要被使用的会话。
pub(crate) const IDLE_EVICT_AFTER_SECS: u64 = 30 * 60;

/// 空闲回收巡检间隔：5 分钟一轮，及时性与巡检开销的折中。
pub(crate) const REAP_INTERVAL_SECS: u64 = 5 * 60;

/// 空闲回收判定（纯函数，便于单测）：两个忙旗标（assistant：turn 活跃 /
/// scheduled 轮进行中；codex_acp：prompt 在途（含等待权限/问询）/
/// 配置同步中）、当前 active 会话与未达空闲时长的会话一律不回收。
pub(crate) fn should_reap_idle(
    primary_busy: bool,
    secondary_busy: bool,
    is_active_session: bool,
    idle_for: Duration,
) -> bool {
    !primary_busy
        && !secondary_busy
        && !is_active_session
        && idle_for >= Duration::from_secs(IDLE_EVICT_AFTER_SECS)
}

/// `evict_if_idle` 的锁内复核 + 原子移除（泛型抽出以便用裸组件确定性测试）：
/// 在同一把 sessions 锁内按现值复核回收条件，满足才 remove 并返回；不满足
/// （快照后到达的 send_message 已置 busy / 刷新活动，或会话正被打开）返回
/// `None`，调用方不得回收。复核与移除原子完成，消除快照→回收的 TOCTOU。
pub(crate) async fn take_session_if_still_idle<T>(
    sessions: &Mutex<HashMap<String, T>>,
    session_id: &str,
    is_still_idle: impl Fn(&T) -> bool,
) -> Option<T> {
    let mut sessions = sessions.lock().await;
    let entry = sessions.get(session_id)?;
    if !is_still_idle(entry) {
        return None;
    }
    sessions.remove(session_id)
}

/// 后台巡检任务句柄：Drop 时先 cancel 再 abort 双保险停止巡检
/// （与 scheduled/tasks.rs ScheduledTaskState 的 Drop 清理同模式）。
pub(crate) struct IdleReaperGuard {
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

impl Drop for IdleReaperGuard {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.handle.abort();
    }
}

/// 启动空闲回收巡检（幂等）：每 [`REAP_INTERVAL_SECS`] 秒一轮。`slot` 是
/// 调用方池上的守卫槽（持锁传入；已有守卫则直接返回，保证幂等），巡检
/// 任务持有池 clone（内部全 Arc，廉价）并每轮调用 `reap_round(pool)` 执行
/// 单轮回收；`log_label` 仅用于单轮失败日志前缀。
///
/// 每轮独立 spawn 做 panic 隔离：回收逻辑 panic 会连坐整个巡检 async task
/// （静默停摆，池从此常驻）。每轮独立 spawn，panic 只终止当轮，外层循环
/// 下轮照常继续。
///
/// pool 是 Tauri managed state、进程级生命周期，巡检随进程退出自然终止；
/// guard 的 Drop 清理仅作防御（clone 间 Arc 循环意味着它平时不会触发，
/// 这不构成泄漏——常驻的只有一个每 5 分钟醒一次的轻任务）。
pub(crate) fn start_idle_reaper<P, F, Fut>(
    slot: &mut Option<IdleReaperGuard>,
    pool: P,
    reap_round: F,
    log_label: &'static str,
) where
    P: Clone + Send + 'static,
    F: Fn(P) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    if slot.is_some() {
        return;
    }
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    // `reap_round` 每轮都要调用（Fn），而 async move 需要持有它跨整轮循环；
    // 借 Arc 让每轮 spawn clone 一份，避免把 Fn 误当 FnOnce 移动进当轮块。
    let reap_round = std::sync::Arc::new(reap_round);
    let handle = tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(REAP_INTERVAL_SECS));
        // tokio::interval 首个 tick 立即到期：跳过，统一走周期节奏。
        interval.tick().await;
        loop {
            tokio::select! {
                _ = task_cancel.cancelled() => break,
                _ = interval.tick() => {}
            }
            // 单轮 panic 隔离（见模块/函数注释）。
            let round_pool = pool.clone();
            let round_reap_round = std::sync::Arc::clone(&reap_round);
            let round =
                tauri::async_runtime::spawn(async move { round_reap_round(round_pool).await });
            if let Err(error) = round.await {
                eprintln!("[{log_label}] 空闲巡检单轮失败（已隔离，下轮继续）: {error}");
            }
        }
    });
    *slot = Some(IdleReaperGuard { cancel, handle });
}
