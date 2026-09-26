//! Testable components of the ACP turn-silence watchdog.
//!
//! The host considers a turn finished only after the `session/prompt` response.
//! If the agent stops answering, the turn stays `running` forever and only the
//! orphan cleanup on application restart can recover it. Upstream
//! `claude-agent-acp` can omit this response when a background-task notification
//! starts a turn and absorbs the host prompt as `absorbed_mid_turn`
//! （agentclientprotocol/claude-agent-acp#896/#1027/#1039/#1114）。
//!
//! This module contains decisions only: the activity clock, silence escalation,
//! turn claiming, the stderr-notice gate, and repeated-stall tracking. Callers
//! perform the actual notice, cancellation, local settlement, and restart.
//!
//! Scope: the watchdog is attached to the shared ACP session runtime and applies
//! equally to Codex, Claude Code, and Kimi. Only the thresholds are calibrated
//! from Claude-family observations (about a 30-second cancellation floor and
//! event intervals ranging from seconds to minutes); recalibrate them by the
//! same method when adapters change.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};

/// Watchdog polling interval.
pub(super) const STALL_TICK: Duration = Duration::from_secs(15);
/// Silence duration before notifying the user. Long agent reasoning may emit no events.
pub(super) const STALL_NOTICE_AFTER: Duration = Duration::from_secs(180);
/// Silence duration before requesting cancellation and starting the settlement grace period.
///
/// This is deliberately generous because the activity clock sees only inbound
/// agent events. A long model call and an agent waiting for a `run_in_background`
/// task can both remain healthy without emitting any event (the latter reports
/// only on completion). This step cancels a live query, so the threshold is 30
/// minutes, far beyond locally observed normal silence: seconds to 1-2 minutes,
/// with the incident's 2m21s gap as the only outlier. Manual Stop remains the
/// faster path and is now bounded by `CANCEL_SETTLE_GRACE` as well.
pub(super) const STALL_CANCEL_AFTER: Duration = Duration::from_secs(1800);
/// Grace period for the agent to finish after cancellation (upstream floor is about 30 seconds).
pub(super) const CANCEL_SETTLE_GRACE: Duration = Duration::from_secs(60);
/// Minimum adapter-stderr notice interval; a burst of lines produces one notice.
pub(super) const STDERR_NOTICE_MIN_INTERVAL: Duration = Duration::from_secs(30);
/// Maximum displayed stderr length; the original is truncated without rewriting it.
pub(super) const STDERR_NOTICE_MAX_CHARS: usize = 400;
/// Window for repeated stalls in one session. Restart only after the
/// `STALL_RESTART_AFTER`th local settlement within this window.
///
/// It must exceed the shortest full stall cycle (`STALL_CANCEL_AFTER` plus
/// `CANCEL_SETTLE_GRACE`, about 31 minutes), or two genuine stalls can never
/// fall in the same window and restart becomes dead code. One hour covers two
/// consecutive stalls in a bad session while expiring older history.
pub(super) const STALL_RESTART_WINDOW: Duration = Duration::from_secs(3600);
pub(super) const STALL_RESTART_AFTER: usize = 2;

/// Monotonic agent-side activity clock. It records inbound notifications and
/// permission/elicitation requests only. A prompt response exits the wait loop
/// directly, and host-emitted events must not refresh the watchdog.
///
/// `Instant` avoids wall-clock rollback from NTP or manual adjustment, which
/// could otherwise freeze elapsed time at zero and disable the watchdog.
pub(super) type ActivityClock = Arc<Mutex<Instant>>;

pub(super) fn new_activity_clock() -> ActivityClock {
    Arc::new(Mutex::new(Instant::now()))
}

pub(super) fn mark_activity(clock: &ActivityClock) {
    *clock.lock() = Instant::now();
}

/// Elapsed time since the most recent agent-side activity.
pub(super) fn quiet_for(clock: &ActivityClock) -> Duration {
    clock.lock().elapsed()
}

/// Returns true when the monotonic quiet duration moved backwards, which can
/// only happen after the shared activity clock was refreshed.
pub(super) fn activity_resumed(previous: Option<Duration>, current: Duration) -> bool {
    previous.is_some_and(|previous| current < previous)
}

/// Action to take on one watchdog tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StallStep {
    /// Do nothing: no threshold was reached, or cancellation is still in grace.
    Idle,
    /// Notify the user that the agent has been silent; at most once per turn.
    Notice,
    /// Request cancellation and give the agent one final chance to answer.
    Cancel,
    /// Settle locally because the agent no longer answers this prompt.
    Settle,
}

/// Classify a silence interval. `noticed` tracks the per-turn notice and
/// `cancel_elapsed` is the time since cancellation, or `None` before it starts.
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

/// Claim the current turn. The first claimant clears it, so later callers do
/// not emit another `turn_completed`. Prompt responses and local watchdog
/// settlement share this path.
pub(super) fn claim_current_turn(current: &RwLock<Option<String>>, turn_id: &str) -> bool {
    let mut guard = current.write();
    if guard.as_deref() == Some(turn_id) {
        *guard = None;
        true
    } else {
        false
    }
}

/// Whether an adapter stderr line contains a stable, user-actionable failure
/// phrase. Ordinary session banners must not reach the UI.
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

/// Whether to emit a notice for this stderr line: it must be actionable,
/// different from the previous displayed detail, and outside the throttle
/// window. This pure helper keeps the spawned stderr pump testable.
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

/// Track local stall settlements in a session and decide when to restart.
///
/// The first stall settles only the turn. The host cannot distinguish a wedged
/// agent query from an agent waiting for a long background task, and killing
/// the process tree would also kill user work. Restart only after repeated
/// stalls in the same session and window.
#[derive(Debug, Default)]
pub(super) struct StallSettleTracker {
    recent: VecDeque<Instant>,
}

impl StallSettleTracker {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Record a local stall settlement and return whether its ACP runtime should restart.
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
            "remain idle before the notice threshold"
        );
        assert_eq!(
            stall_step(STALL_NOTICE_AFTER, false, None),
            StallStep::Notice,
            "notify the user at the notice threshold"
        );
        assert_eq!(
            stall_step(STALL_NOTICE_AFTER + Duration::from_secs(30), true, None),
            StallStep::Idle,
            "notify at most once per turn"
        );
        assert_eq!(
            stall_step(STALL_CANCEL_AFTER, true, None),
            StallStep::Cancel,
            "continued silence requests cancellation"
        );
        assert_eq!(
            stall_step(
                STALL_CANCEL_AFTER + Duration::from_secs(1),
                true,
                Some(CANCEL_SETTLE_GRACE - Duration::from_secs(1))
            ),
            StallStep::Idle,
            "keep waiting during the cancellation grace period"
        );
        assert_eq!(
            stall_step(
                STALL_CANCEL_AFTER + Duration::from_secs(1),
                true,
                Some(CANCEL_SETTLE_GRACE)
            ),
            StallStep::Settle,
            "settle locally after the grace period"
        );
    }

    #[test]
    fn forkguard_stall_step_does_not_settle_without_requesting_cancel_first() {
        assert_eq!(
            stall_step(STALL_CANCEL_AFTER * 2, false, None),
            StallStep::Cancel,
            "even extreme silence must request cancellation before settlement"
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
            "a late closer must not emit a duplicate turn_completed"
        );
        let other = RwLock::new(Some("turn-2".to_string()));
        assert!(!claim_current_turn(&other, "turn-1"));
        assert_eq!(other.read().as_deref(), Some("turn-2"));
    }

    #[test]
    fn forkguard_stderr_notice_filters_to_real_agent_trouble() {
        // Field evidence: the adapter's only diagnostic during the incident.
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
        // Ordinary startup banners and logs do not notify the user.
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
            "the first actionable stderr line should notify"
        );
        assert!(
            !stderr_notice_due(
                base + Duration::from_secs(5),
                Some(base),
                Some(long_line),
                long_line,
                long_line,
            ),
            "a repeated line must not keep appending timeline events"
        );
        assert!(
            !stderr_notice_due(
                base + Duration::from_secs(5),
                Some(base),
                Some(long_line),
                other_line,
                other_line,
            ),
            "different content remains throttled inside the interval"
        );
        assert!(
            stderr_notice_due(
                base + STDERR_NOTICE_MIN_INTERVAL + Duration::from_secs(1),
                Some(base),
                Some(long_line),
                other_line,
                other_line,
            ),
            "different content notifies after the throttle interval"
        );
        assert!(
            !stderr_notice_due(
                base,
                None,
                None,
                "[session/query] sessionId=x resume=none",
                "[session/query] sessionId=x resume=none"
            ),
            "an ordinary banner must not notify"
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
            "long lines with the same displayed text must deduplicate"
        );
    }

    #[test]
    fn forkguard_stall_tracker_restarts_only_on_a_repeat_settle() {
        let mut tracker = StallSettleTracker::new();
        let base = Instant::now();
        assert!(
            !tracker.record(base),
            "the first stall preserves the session and possible background work"
        );
        assert!(
            tracker.record(base + Duration::from_secs(60)),
            "the second stall in the window escalates to restart"
        );
    }

    #[test]
    fn forkguard_stall_tracker_forgets_settles_outside_the_window() {
        let mut tracker = StallSettleTracker::new();
        let base = Instant::now();
        assert!(!tracker.record(base));
        assert!(
            !tracker.record(base + STALL_RESTART_WINDOW + Duration::from_secs(1)),
            "settlements outside the window no longer count"
        );
    }

    /// The escalation window must hold one complete repeated-stall cycle.
    #[test]
    fn forkguard_stall_restart_window_covers_a_full_stall_cycle() {
        let mut tracker = StallSettleTracker::new();
        let first = Instant::now();
        assert!(!tracker.record(first));
        // The second stall cannot occur before one cancellation grace after the first settlement.
        let second = first + STALL_CANCEL_AFTER + CANCEL_SETTLE_GRACE;
        assert!(
            tracker.record(second),
            "two consecutive stall settlements must fit in one escalation window"
        );
    }
}
