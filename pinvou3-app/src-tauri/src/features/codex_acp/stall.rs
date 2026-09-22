//! ACP 回合静默看门狗的可单测部分。
//!
//! 宿主只用 `session/prompt` 的响应判定回合结束；一旦 Agent 不再回答，
//! 回合会永久停在 `running`（界面「处理中」），只有重启应用时的孤儿回合
//! 收口能救回来。上游 `claude-agent-acp` 在「后台任务通知自启回合、宿主
//! prompt 被 `absorbed_mid_turn` 吸收」的情况下确实会漏掉这条响应
//! （agentclientprotocol/claude-agent-acp#896/#1027/#1039/#1114）。
//!
//! 本模块只提供判定：活动时钟、静默分级、回合认领、stderr 提示闸门与
//! 「反复卡死才重启」的计数器；实际动作（提示、请求取消、本地收口、重启
//! 运行时）由调用方执行。
//!
//! 作用域：这套看门狗挂在共享的 ACP 会话运行时上，因此对 Codex、Claude Code
//! 与 Kimi 三个适配器一视同仁；只是**阈值**参照 Claude 家族的实测（cancel floor
//! 约 30 秒、事件间隔秒级到分钟级）取值，换适配器时按同一口径重新标定即可。

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};

/// 看门狗巡检间隔。
pub(super) const STALL_TICK: Duration = Duration::from_secs(15);
/// 静默多久先提示用户。Agent 长时间思考不产生事件，只有超过这个量级才值得打扰。
pub(super) const STALL_NOTICE_AFTER: Duration = Duration::from_secs(180);
/// 静默多久开始兜底收口：先请求取消，宽限期后仍无响应则本地收口。
///
/// 取值刻意宽松：活动时钟只认 **Agent 侧入站事件**，而「一次长时间模型调用」
/// 与「Agent 在等 `run_in_background` 任务」这两种**仍然活着**的情形都不产生
/// 任何事件（后者只在完成时通知）。这一步会真的取消在跑的 query，所以阈值取
/// 30 分钟——远超本地观测到的正常静默（现场正常回合的事件间隔是秒级到 1-2 分
/// 钟，唯一一次 2 分 21 秒的空窗就是本次事故）。用户手动停止始终是更快的路径，
/// 且现在同样有界（`CANCEL_SETTLE_GRACE`）。
pub(super) const STALL_CANCEL_AFTER: Duration = Duration::from_secs(1800);
/// 请求取消后等待 Agent 自行收口的宽限（上游 cancel floor 实测约 30 秒）。
pub(super) const CANCEL_SETTLE_GRACE: Duration = Duration::from_secs(60);
/// 适配器 stderr 提示的最小间隔：同一阵发的多行只提示一次。
pub(super) const STDERR_NOTICE_MIN_INTERVAL: Duration = Duration::from_secs(30);
/// 提示给用户的 stderr 行长度上限（原文截断，不改写）。
pub(super) const STDERR_NOTICE_MAX_CHARS: usize = 400;
/// 同一会话反复卡死的判定窗口：窗口内第 `STALL_RESTART_AFTER` 次本地收口
/// 才升级为重启 Agent 会话。
///
/// 必须**大于一次 stall 收口的最短周期**（`STALL_CANCEL_AFTER` +
/// `CANCEL_SETTLE_GRACE` ≈ 31 分钟），否则两次真实的反复卡死永远落不进同一个
/// 窗口，升级路径就成了死代码。取 1 小时：既能覆盖「同一个坏会话连续卡两次」，
/// 又能在过了一小时之后把上一次当作陈旧记录不再计数。
pub(super) const STALL_RESTART_WINDOW: Duration = Duration::from_secs(3600);
pub(super) const STALL_RESTART_AFTER: usize = 2;

/// Agent 侧活动时钟（单调时钟）。只记录 Agent 的活动：入站通知与权限/询问
/// 请求（prompt 响应会直接跳出等待循环，不需要推进它）。宿主自己发出的事件
/// 不计入，否则看门狗会被自己的提示刷新。
///
/// 用 `Instant` 而非墙上时钟：后者被 NTP/手工调整往回拨时，「距上次活动的
/// 时长」会一直算成 0，看门狗静默失效——正是它要防的那种永久「处理中」。
pub(super) type ActivityClock = Arc<Mutex<Instant>>;

pub(super) fn new_activity_clock() -> ActivityClock {
    Arc::new(Mutex::new(Instant::now()))
}

pub(super) fn mark_activity(clock: &ActivityClock) {
    *clock.lock() = Instant::now();
}

/// 距最近一次 Agent 侧活动的时长。
pub(super) fn quiet_for(clock: &ActivityClock) -> Duration {
    clock.lock().elapsed()
}

/// Returns true when the monotonic quiet duration moved backwards, which can
/// only happen after the shared activity clock was refreshed.
pub(super) fn activity_resumed(previous: Option<Duration>, current: Duration) -> bool {
    previous.is_some_and(|previous| current < previous)
}

/// 一轮巡检应执行的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StallStep {
    /// 什么都不做（尚未到阈值，或在等 Agent 回应取消）。
    Idle,
    /// 提示用户 Agent 长时间没有动静（每个回合只提示一次）。
    Notice,
    /// 请求取消，给 Agent 最后一次回答的机会。
    Cancel,
    /// 本地收口：Agent 不再回答这个 prompt，宿主不能永远转圈。
    Settle,
}

/// 静默分级判定。`noticed` 是本回合是否已提示过，`cancel_elapsed` 是发起
/// 取消后经过的时间（尚未发起时为 `None`）。
pub(super) fn stall_step(
    quiet: Duration,
    noticed: bool,
    cancel_elapsed: Option<Duration>,
) -> StallStep {
    if quiet >= STALL_CANCEL_AFTER {
        return match cancel_elapsed {
            None => StallStep::Cancel,
            Some(elapsed) if elapsed >= CANCEL_SETTLE_GRACE => StallStep::Settle,
            Some(_) => StallStep::Idle,
        };
    }
    if !noticed && quiet >= STALL_NOTICE_AFTER {
        StallStep::Notice
    } else {
        StallStep::Idle
    }
}

/// 认领当前回合：第一个认领者清空它，后到的调用方不再补发 `turn_completed`。
/// prompt 响应与看门狗本地收口共享这条路径，晚到的一方必须让位。
pub(super) fn claim_current_turn(current: &RwLock<Option<String>>, turn_id: &str) -> bool {
    let mut guard = current.write();
    if guard.as_deref() == Some(turn_id) {
        *guard = None;
        true
    } else {
        false
    }
}

/// 适配器 stderr 里值得提示用户的片段。上游把真正的异常写成自由文本，这里只
/// 认跨版本稳定的几种措辞，避免把正常会话 banner 刷进界面。
pub(super) fn stderr_notice_worthy(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    ["cancel floor", "wedged", "never received"]
        .iter()
        .any(|needle| lower.contains(needle))
        || lower.starts_with("panic:")
        || lower.contains(" panicked at ")
        || lower.contains("unhandled exception")
        || lower.contains("uncaught exception")
}

/// 是否应当为这一行 stderr 发出一条提示事件：内容值得提示、与上一条不同、
/// 且过了限频窗口。抽成纯函数是为了让 spawn 侧那条 stderr 泵的逻辑可单测。
pub(super) fn stderr_notice_due(
    now: Instant,
    last_notice_at: Option<Instant>,
    last_notice_detail: Option<&str>,
    line: &str,
    detail: &str,
) -> bool {
    if !stderr_notice_worthy(line) {
        return false;
    }
    if last_notice_detail == Some(detail) {
        return false;
    }
    !last_notice_at.is_some_and(|at| now.saturating_duration_since(at) < STDERR_NOTICE_MIN_INTERVAL)
}

/// 记录本会话的 stall 收口次数，判断是否该升级为重启。
///
/// 首次卡死只摘掉回合：宿主从外部无法区分「Agent 的 query 真卡死」与
/// 「Agent 正在等一个长时间后台任务」（`run_in_background` 只在完成时通知），
/// 而杀进程树会连带杀掉用户自己的长时任务。只有同一会话在窗口内**反复**
/// 卡死时，才采纳上游「可能需要新会话」的结论去重启运行时。
#[derive(Debug, Default)]
pub(super) struct StallSettleTracker {
    recent: VecDeque<Instant>,
}

impl StallSettleTracker {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// 记录一次本地 stall 收口；返回是否应当重启该会话的 ACP 运行时。
    pub(super) fn record(&mut self, now: Instant) -> bool {
        while self
            .recent
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) > STALL_RESTART_WINDOW)
        {
            self.recent.pop_front();
        }
        self.recent.push_back(now);
        self.recent.len() >= STALL_RESTART_AFTER
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock_aged(age: Duration) -> ActivityClock {
        Arc::new(Mutex::new(Instant::now() - age))
    }

    #[test]
    fn forkguard_quiet_for_tracks_only_agent_activity() {
        let clock = clock_aged(Duration::from_secs(5));
        assert!(quiet_for(&clock) >= Duration::from_secs(5));
        mark_activity(&clock);
        assert!(quiet_for(&clock) < Duration::from_secs(1));
    }

    #[test]
    fn forkguard_stall_step_escalates_notice_cancel_then_settle() {
        let under = STALL_NOTICE_AFTER - Duration::from_secs(1);
        assert_eq!(
            stall_step(under, false, None),
            StallStep::Idle,
            "尚未到达提示阈值前保持安静"
        );
        assert_eq!(
            stall_step(STALL_NOTICE_AFTER, false, None),
            StallStep::Notice,
            "到达提示阈值先提示用户"
        );
        assert_eq!(
            stall_step(STALL_NOTICE_AFTER + Duration::from_secs(30), true, None),
            StallStep::Idle,
            "同一回合只提示一次"
        );
        assert_eq!(
            stall_step(STALL_CANCEL_AFTER, true, None),
            StallStep::Cancel,
            "继续静默则请求取消"
        );
        assert_eq!(
            stall_step(
                STALL_CANCEL_AFTER + Duration::from_secs(1),
                true,
                Some(CANCEL_SETTLE_GRACE - Duration::from_secs(1))
            ),
            StallStep::Idle,
            "取消宽限期内继续等 Agent"
        );
        assert_eq!(
            stall_step(
                STALL_CANCEL_AFTER + Duration::from_secs(1),
                true,
                Some(CANCEL_SETTLE_GRACE)
            ),
            StallStep::Settle,
            "宽限期用尽后本地收口"
        );
    }

    #[test]
    fn forkguard_stall_step_does_not_settle_without_requesting_cancel_first() {
        assert_eq!(
            stall_step(STALL_CANCEL_AFTER * 2, false, None),
            StallStep::Cancel,
            "即使远超阈值，也必须先发取消再收口"
        );
    }

    #[test]
    fn forkguard_new_activity_restarts_the_cancel_ladder() {
        assert!(!activity_resumed(None, Duration::from_secs(1)));
        assert!(!activity_resumed(
            Some(Duration::from_secs(1)),
            Duration::from_secs(2)
        ));
        assert!(activity_resumed(
            Some(STALL_CANCEL_AFTER),
            Duration::from_millis(10)
        ));
        assert_eq!(
            stall_step(STALL_CANCEL_AFTER, true, None),
            StallStep::Cancel,
            "a fresh silence period must request cancellation before settlement"
        );
    }

    #[test]
    fn forkguard_turn_claim_is_exclusive_and_id_scoped() {
        let current = RwLock::new(Some("turn-1".to_string()));
        assert!(claim_current_turn(&current, "turn-1"));
        assert!(
            !claim_current_turn(&current, "turn-1"),
            "晚到的收口方不得重复发 turn_completed"
        );
        let other = RwLock::new(Some("turn-2".to_string()));
        assert!(!claim_current_turn(&other, "turn-1"));
        assert_eq!(other.read().as_deref(), Some("turn-2"));
    }

    #[test]
    fn forkguard_stderr_notice_filters_to_real_agent_trouble() {
        // 现场真实捕获：本次事故里适配器唯一的自述。
        assert!(stderr_notice_worthy(
            "Session e6772626-52a8-45c4-8c2e-e89ddfefea99: cancel floor elapsed without the SDK yielding; forcing \"cancelled\". The underlying query may still be wedged — a new session may be required."
        ));
        assert!(stderr_notice_worthy(
            "Internal error: \"response to `session/prompt` never received: oneshot canceled\""
        ));
        assert!(stderr_notice_worthy("PANIC: unwrap failed"));
        assert!(stderr_notice_worthy(
            "worker panicked at adapter/src/main.rs:12"
        ));
        // 正常启动 banner 与普通日志不提示用户。
        assert!(!stderr_notice_worthy(
            "[session/query] sessionId=e6772626-52a8-45c4-8c2e-e89ddfefea99 resume=none apiType=native baseUrl=native"
        ));
        assert!(!stderr_notice_worthy(
            "warning: unhandled promise rejection was observed and recovered"
        ));
        assert!(!stderr_notice_worthy(""));
    }

    #[test]
    fn forkguard_stderr_notice_gate_throttles_and_dedupes() {
        let base = Instant::now();
        let long_line =
            "Session x: cancel floor elapsed without the SDK yielding; forcing \"cancelled\"";
        let other_line = "PANIC: unwrap failed";
        assert!(
            stderr_notice_due(base, None, None, long_line, long_line),
            "首条命中措辞的 stderr 应当提示"
        );
        assert!(
            !stderr_notice_due(
                base + Duration::from_secs(5),
                Some(base),
                Some(long_line),
                long_line,
                long_line,
            ),
            "同一条重复打印不再产生事件（长时间 wedged 不刷 timeline）"
        );
        assert!(
            !stderr_notice_due(
                base + Duration::from_secs(5),
                Some(base),
                Some(long_line),
                other_line,
                other_line,
            ),
            "限频窗口内即使内容不同也只留一条"
        );
        assert!(
            stderr_notice_due(
                base + STDERR_NOTICE_MIN_INTERVAL + Duration::from_secs(1),
                Some(base),
                Some(long_line),
                other_line,
                other_line,
            ),
            "过了限频窗口、内容不同才再提示"
        );
        assert!(
            !stderr_notice_due(
                base,
                None,
                None,
                "[session/query] sessionId=x resume=none",
                "[session/query] sessionId=x resume=none"
            ),
            "正常 banner 不提示"
        );

        let shared_detail = "PANIC: ".to_string() + &"x".repeat(STDERR_NOTICE_MAX_CHARS - 7);
        let first = format!("{shared_detail} first suffix");
        let second = format!("{shared_detail} second suffix");
        assert!(stderr_notice_due(base, None, None, &first, &shared_detail));
        assert!(
            !stderr_notice_due(
                base + STDERR_NOTICE_MIN_INTERVAL + Duration::from_secs(1),
                Some(base),
                Some(&shared_detail),
                &second,
                &shared_detail,
            ),
            "两条展示文本相同的长行必须按实际卡片内容去重"
        );
    }

    #[test]
    fn forkguard_stall_tracker_restarts_only_on_a_repeat_settle() {
        let mut tracker = StallSettleTracker::new();
        let base = Instant::now();
        assert!(
            !tracker.record(base),
            "首次卡死只摘回合：保留会话与它可能正在跑的后台任务"
        );
        assert!(
            tracker.record(base + Duration::from_secs(60)),
            "同一窗口内第二次卡死才升级为重启"
        );
    }

    #[test]
    fn forkguard_stall_tracker_forgets_settles_outside_the_window() {
        let mut tracker = StallSettleTracker::new();
        let base = Instant::now();
        assert!(!tracker.record(base));
        assert!(
            !tracker.record(base + STALL_RESTART_WINDOW + Duration::from_secs(1)),
            "窗口外的旧收口不再计数"
        );
    }

    /// 升级窗口必须容得下一次真实的反复卡死，否则重启路径永远不会触发。
    #[test]
    fn forkguard_stall_restart_window_covers_a_full_stall_cycle() {
        let mut tracker = StallSettleTracker::new();
        let first = Instant::now();
        assert!(!tracker.record(first));
        // 第二次卡死最早出现在「第一次收口 + 取消宽限」之后。
        let second = first + STALL_CANCEL_AFTER + CANCEL_SETTLE_GRACE;
        assert!(
            tracker.record(second),
            "两次连续 stall 收口必须落在同一个窗口内，否则升级路径不可达"
        );
    }
}
