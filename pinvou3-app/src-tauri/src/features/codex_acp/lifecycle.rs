//! Per-runtime lifecycle tracking and the spawn crash budget.
//!
//! The state belongs to one runtime generation (one spawned child), not to the
//! session id: a replacement runtime gets a fresh [`RuntimeLifecycle`], so a
//! delayed observation of an old generation can never mark the replacement
//! dead or consume its crash budget. `get_or_spawn` consults these types under
//! the pool sessions lock: live generations are reused, dead generations are
//! removed atomically in the same critical section ([`resolve_runtime`]), and
//! [`RuntimeCrashBudget`] backs off repeated respawns of a runtime that keeps
//! exiting so a broken install cannot turn every UI access into a tight
//! respawn loop.
//!
//! Ported from the official fork's `codex_acp/lifecycle.rs` (crash-budget
//! core). The controller-only custody helpers that share the upstream file
//! (turn-retirement gate, owned pool takes) stay unported: they serve the
//! remote-turn plumbing this fork does not carry.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const CRASH_WINDOW: Duration = Duration::from_secs(60);
const STABLE_RESET_AFTER: Duration = Duration::from_secs(2 * 60);
const MAX_CRASHES_IN_WINDOW: usize = 3;
const SECOND_RETRY_DELAY: Duration = Duration::from_millis(250);

/// Per-process lifetime state for one ACP runtime generation.
///
/// The state belongs to the runtime instance instead of the session id. A
/// delayed close from an old generation therefore cannot mark a replacement
/// generation dead.
pub(super) struct RuntimeLifecycle {
    generation: u64,
    started_at: Instant,
    running: AtomicBool,
    expected_shutdown: AtomicBool,
    unexpected_exit: AtomicBool,
    crash_counted: AtomicBool,
}

pub(super) enum RuntimeLookup<T> {
    Reusable(T),
    Dead(T),
    Missing,
}

/// Resolves one session entry under the pool lock. A dead entry is removed in
/// the same critical section, so no caller can reuse it after the decision.
pub(super) fn resolve_runtime<T: Clone>(
    sessions: &mut HashMap<String, T>,
    session_id: &str,
    is_running: impl FnOnce(&T) -> bool,
) -> RuntimeLookup<T> {
    let Some(runtime) = sessions.get(session_id).cloned() else {
        return RuntimeLookup::Missing;
    };
    if is_running(&runtime) {
        RuntimeLookup::Reusable(runtime)
    } else {
        sessions.remove(session_id);
        RuntimeLookup::Dead(runtime)
    }
}

impl RuntimeLifecycle {
    pub(super) fn new(generation: u64) -> Self {
        Self {
            generation,
            started_at: Instant::now(),
            running: AtomicBool::new(true),
            expected_shutdown: AtomicBool::new(false),
            unexpected_exit: AtomicBool::new(false),
            crash_counted: AtomicBool::new(false),
        }
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub(super) fn stable_for(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub(super) fn begin_expected_shutdown(&self) {
        self.expected_shutdown.store(true, Ordering::Release);
    }

    /// Marks transport completion and returns whether it was unexpected.
    pub(super) fn mark_connection_closed(&self) -> bool {
        let unexpected = !self.expected_shutdown.load(Ordering::Acquire);
        if unexpected {
            self.unexpected_exit.store(true, Ordering::Release);
        }
        self.running.store(false, Ordering::Release);
        unexpected
    }

    /// Returns true once for an unexpected exit, so a dead entry observed by
    /// concurrent callers consumes exactly one crash-budget slot.
    pub(super) fn take_uncounted_crash(&self) -> bool {
        self.unexpected_exit.load(Ordering::Acquire)
            && self
                .crash_counted
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SpawnAdmission {
    Allowed,
    RetryAfter {
        delay: Duration,
        budget_exhausted: bool,
    },
}

#[derive(Default)]
pub(super) struct RuntimeCrashBudget {
    crashes: VecDeque<Instant>,
}

impl RuntimeCrashBudget {
    pub(super) fn record_crash_at(&mut self, now: Instant) {
        self.prune(now);
        self.crashes.push_back(now);
    }

    pub(super) fn admission_at(&mut self, now: Instant) -> SpawnAdmission {
        self.prune(now);
        let Some(last) = self.crashes.back().copied() else {
            return SpawnAdmission::Allowed;
        };
        if self.crashes.len() >= MAX_CRASHES_IN_WINDOW {
            let oldest = self.crashes.front().copied().unwrap_or(last);
            return retry_until(now, oldest + CRASH_WINDOW, true);
        }
        if self.crashes.len() == 1 {
            return SpawnAdmission::Allowed;
        }
        let cooldown = SECOND_RETRY_DELAY;
        retry_until(now, last + cooldown, false)
    }

    fn prune(&mut self, now: Instant) {
        while self
            .crashes
            .front()
            .is_some_and(|crash| now.saturating_duration_since(*crash) >= CRASH_WINDOW)
        {
            self.crashes.pop_front();
        }
    }
}

pub(super) fn runtime_is_stable(lifecycle: &RuntimeLifecycle) -> bool {
    lifecycle.is_running() && lifecycle.stable_for() >= STABLE_RESET_AFTER
}

fn retry_until(now: Instant, deadline: Instant, budget_exhausted: bool) -> SpawnAdmission {
    match deadline.checked_duration_since(now) {
        Some(delay) if !delay.is_zero() => SpawnAdmission::RetryAfter {
            delay,
            budget_exhausted,
        },
        _ => SpawnAdmission::Allowed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unexpected_exit_is_dead_and_consumes_one_crash_slot() {
        let lifecycle = RuntimeLifecycle::new(7);
        assert!(lifecycle.is_running());
        assert!(lifecycle.mark_connection_closed());
        assert!(!lifecycle.is_running());
        assert!(lifecycle.take_uncounted_crash());
        assert!(!lifecycle.take_uncounted_crash());
        assert_eq!(lifecycle.generation(), 7);
    }

    #[test]
    fn resolution_reuses_live_generation_and_atomically_removes_dead_generation() {
        let mut sessions = HashMap::from([
            ("live".to_string(), (11_u64, true)),
            ("dead".to_string(), (10_u64, false)),
        ]);

        assert!(matches!(
            resolve_runtime(&mut sessions, "live", |entry| entry.1),
            RuntimeLookup::Reusable((11, true))
        ));
        assert!(sessions.contains_key("live"));

        assert!(matches!(
            resolve_runtime(&mut sessions, "dead", |entry| entry.1),
            RuntimeLookup::Dead((10, false))
        ));
        assert!(!sessions.contains_key("dead"));
        assert!(matches!(
            resolve_runtime(&mut sessions, "dead", |entry| entry.1),
            RuntimeLookup::Missing
        ));
    }

    #[test]
    fn old_generation_exit_cannot_change_replacement_generation() {
        let old = RuntimeLifecycle::new(21);
        let replacement = RuntimeLifecycle::new(22);

        old.mark_connection_closed();

        assert!(!old.is_running());
        assert!(replacement.is_running());
        assert_eq!(replacement.generation(), 22);
    }

    #[test]
    fn expected_shutdown_does_not_consume_crash_budget() {
        let lifecycle = RuntimeLifecycle::new(8);
        lifecycle.begin_expected_shutdown();
        assert!(!lifecycle.mark_connection_closed());
        assert!(!lifecycle.is_running());
        assert!(!lifecycle.take_uncounted_crash());
    }

    #[test]
    fn crash_budget_backs_off_then_stops_a_tight_respawn_loop() {
        let start = Instant::now();
        let mut budget = RuntimeCrashBudget::default();

        budget.record_crash_at(start);
        assert_eq!(budget.admission_at(start), SpawnAdmission::Allowed);

        budget.record_crash_at(start);
        assert_eq!(
            budget.admission_at(start),
            SpawnAdmission::RetryAfter {
                delay: SECOND_RETRY_DELAY,
                budget_exhausted: false,
            }
        );

        let third = start + SECOND_RETRY_DELAY;
        budget.record_crash_at(third);
        assert!(matches!(
            budget.admission_at(third),
            SpawnAdmission::RetryAfter {
                budget_exhausted: true,
                ..
            }
        ));
        assert_eq!(
            budget.admission_at(start + CRASH_WINDOW),
            SpawnAdmission::Allowed
        );
    }
}
