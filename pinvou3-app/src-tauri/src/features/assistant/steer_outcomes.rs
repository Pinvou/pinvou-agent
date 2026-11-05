//! Steer terminal fan-out: the in-process fan-out point for the engine
//! events (`SteerCommitted`/`SteerDropped`).
//!
//! The forwarder calls [`resolve_steer_outcome`] on the event; the consumer
//! (the in-flight terminal settlement of cross-session message delivery)
//! is injected at startup assembly via [`register_steer_resolution_hook`].
//! The dependency direction stays `consumer → assistant` (the consumer
//! depends on this module); assistant never depends back on a consumer
//! feature — the architecture guard's feature-cycle check draws the line
//! here. One in-process hook is enough: the only consumer today is
//! `features::messaging`; a second consumer would upgrade the fn pointer
//! to a Vec broadcast (the registration site already rejects duplicates).

use std::sync::OnceLock;

type SteerResolutionHook = fn(&str, bool);

static STEER_RESOLUTION_HOOK: OnceLock<SteerResolutionHook> = OnceLock::new();

/// Register the steer terminal consumer. Only the first registration wins;
/// a duplicate is an assembly error (fail-loud instead of a silent
/// overwrite) — startup assembly must call this exactly once.
pub fn register_steer_resolution_hook(hook: SteerResolutionHook) -> Result<(), ()> {
    STEER_RESOLUTION_HOOK.set(hook).map_err(|_| ())
}

/// Fan a steer terminal out: committed=true = the engine wrote it into the
/// transcript; false = the engine dropped it (turn termination /
/// withdrawal / generation invalidation). Silent when unregistered — a
/// session's steer events have no cross-session consumer today.
pub fn resolve_steer_outcome(steer_id: &str, committed: bool) {
    if let Some(hook) = STEER_RESOLUTION_HOOK.get() {
        hook(steer_id, committed);
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_steer_outcome;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Registration gate: the first registrant wins and duplicates are
    /// rejected (a silent overwrite would swap consumers — assembly errors
    /// must be visible).
    #[test]
    fn hook_registration_accepts_first_and_rejects_rest() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn consumer(_steer_id: &str, _committed: bool) {
            CALLS.fetch_add(1, Ordering::SeqCst);
        }
        fn second(_steer_id: &str, _committed: bool) {
            unreachable!("the second hook must never be installed");
        }
        assert!(
            super::register_steer_resolution_hook(consumer).is_ok(),
            "first registration wins"
        );
        assert!(
            super::register_steer_resolution_hook(second).is_err(),
            "duplicate registration must be rejected"
        );
        resolve_steer_outcome("steer-1@0", true);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        // After registration the "silent without hook" branch cannot be
        // unit-tested in the same process (the OnceLock is global and the
        // first registrant stays installed) — the branch is guaranteed by
        // the get()==None short-circuit.
        resolve_steer_outcome("steer-2@0", false);
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    }
}
