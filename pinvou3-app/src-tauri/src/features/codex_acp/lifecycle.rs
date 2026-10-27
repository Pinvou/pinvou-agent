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
//! respawn loop. The budget is per session: one broken install with N
//! resident sessions admits up to N×3 attempts per window (no global cap),
//! so one session's throttle never blocks another session's first spawn.
//!
//! Budget schedule, per session, inside a 60 s window: the first crash
//! respawns immediately; from the second crash a respawn waits 250 ms after
//! the latest crash; from the third crash automatic rebuilds pause until the
//! oldest crash leaves the window. A reuse of a generation that has been
//! alive for two minutes — measured from spawn start, initialization
//! included — clears the budget, so a runtime that only fails every few
//! minutes is never throttled; the budget targets tight loops.
//!
//! Detection boundary: a dead generation is observed through the transport
//! (`incoming_closed`), which resolves only once every writer of the runtime
//! child's stdout pipe is gone. A child that exits while a forked descendant
//! keeps the pipe open stays `running` here; the stall watchdog (with a turn
//! in flight) or the idle reaper still reclaims it later. Such a blind exit
//! is classified as a stall when reclaimed (`was_running` is still true), so
//! it clears rather than feeds the crash budget: a blind crash-loop cycles at
//! the stall-watchdog cadence without the respawn backoff, until the
//! descendant lets the pipe close.
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
const RETRY_BACKOFF: Duration = Duration::from_millis(250);

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

/// Resolves one session entry under the pool lock. An entry that is not
/// reusable — its process exited, or a replacement was requested for a
/// still-running process — is removed in the same critical section, so no
/// caller can reuse it after the decision. The caller tells the two apart
/// from the lifecycle itself.
pub(super) fn resolve_runtime<T: Clone>(
    sessions: &mut HashMap<String, T>,
    session_id: &str,
    is_reusable: impl FnOnce(&T) -> bool,
) -> RuntimeLookup<T> {
    let Some(runtime) = sessions.get(session_id).cloned() else {
        return RuntimeLookup::Missing;
    };
    if is_reusable(&runtime) {
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
        retry_until(now, last + RETRY_BACKOFF, false)
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
    fn expected_shutdown_does_not_consume_crash_budget() {
        let lifecycle = RuntimeLifecycle::new(8);
        lifecycle.begin_expected_shutdown();
        // Announcing the shutdown must not flip the generation to dead on
        // its own: only the transport ending does.
        assert!(lifecycle.is_running());
        assert!(!lifecycle.mark_connection_closed());
        assert!(!lifecycle.is_running());
        assert!(!lifecycle.take_uncounted_crash());
    }

    #[test]
    fn concurrent_observers_consume_exactly_one_crash_slot() {
        let lifecycle = std::sync::Arc::new(RuntimeLifecycle::new(3));
        assert!(lifecycle.mark_connection_closed());
        let observers = 8;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(observers));
        let handles = (0..observers)
            .map(|_| {
                let lifecycle = lifecycle.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    lifecycle.take_uncounted_crash()
                })
            })
            .collect::<Vec<_>>();
        let taken = handles
            .into_iter()
            .map(|handle| handle.join().expect("observer thread"))
            .filter(|took| *took)
            .count();
        assert_eq!(taken, 1);
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
                delay: RETRY_BACKOFF,
                budget_exhausted: false,
            }
        );

        let third = start + RETRY_BACKOFF;
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

    /// Anchors the short backoff to the LATEST crash, as the module doc
    /// promises. Anchoring to the older of the two surviving crashes would
    /// make this admission Allowed (its deadline expired long before),
    /// silently dropping the second-crash gate whenever the crashes are far
    /// apart.
    #[test]
    fn backoff_is_measured_from_the_latest_crash() {
        let start = Instant::now();
        let mut budget = RuntimeCrashBudget::default();
        budget.record_crash_at(start);
        let latest = start + Duration::from_secs(40);
        budget.record_crash_at(latest);
        assert_eq!(
            budget.admission_at(latest + Duration::from_millis(100)),
            SpawnAdmission::RetryAfter {
                delay: Duration::from_millis(150),
                budget_exhausted: false,
            }
        );
    }

    #[test]
    fn admission_prunes_expired_crashes_before_judging_the_window() {
        let start = Instant::now();
        let mut budget = RuntimeCrashBudget::default();
        budget.record_crash_at(start);
        let late = start + CRASH_WINDOW - Duration::from_millis(100);
        budget.record_crash_at(late);
        budget.record_crash_at(late);
        assert!(matches!(
            budget.admission_at(late),
            SpawnAdmission::RetryAfter {
                budget_exhausted: true,
                ..
            }
        ));

        // The instant the first crash leaves the window only two remain: the
        // pause must lift into the short backoff (measured from the latest
        // crash), neither staying exhausted nor falling through to Allowed.
        // Only an admission-time prune produces this state — the records
        // above happened before anything expired.
        let at_window = start + CRASH_WINDOW;
        assert_eq!(
            budget.admission_at(at_window),
            SpawnAdmission::RetryAfter {
                delay: Duration::from_millis(150),
                budget_exhausted: false,
            }
        );
    }
}
