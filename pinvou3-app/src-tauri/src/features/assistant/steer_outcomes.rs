//! Steer terminal fan-out: the in-process fan-out point for the engine
//! events (`SteerCommitted`/`SteerDropped`).
//!
//! The forwarder calls [`resolve_steer_outcome`] on the event; the consumer
//! (the in-flight terminal settlement of cross-session message delivery)
//! is injected at startup assembly via [`register_steer_resolution_hook`].
//! The dependency direction stays `consumer → assistant` — wiring is the
//! lib.rs fn-pointer injection, and assistant holds no code reference to
//! any consumer feature (the architecture guard's feature-cycle check
//! draws the line here). One in-process hook is enough: the only consumer
//! today is `features::messaging`; a second consumer would upgrade the fn
//! pointer to a Vec broadcast (the registration site already rejects
//! duplicates).

use std::sync::OnceLock;

type SteerResolutionHook = fn(&str, bool);

static STEER_RESOLUTION_HOOK: OnceLock<SteerResolutionHook> = OnceLock::new();

/// Register the steer terminal consumer. Only the first registration wins;
/// a duplicate is rejected instead of silently overwriting (the registry
/// refuses it and the registration site best-effort-writes the refusal to
/// stderr — startup assembly must call this exactly once).
pub fn register_steer_resolution_hook(hook: SteerResolutionHook) -> Result<(), ()> {
    STEER_RESOLUTION_HOOK.set(hook).map_err(|_| ())
}

/// Fan a steer terminal out: committed=true = the engine wrote it into the
/// transcript; false = the engine dropped it (a cancelled/interrupted
/// turn's settlement, a withdrawal, generation invalidation, or an engine
/// drop — a turn that ENDS normally does not settle; the steer stays
/// parked for the next turn's first drain). Silent when unregistered — safe
/// because the registry only arms entries inside the delivery watcher's
/// poll, and the watcher is spawned exactly once, later in the same GUI
/// setup closure as the hook registration this module exposes. The
/// windowless/headless host DOES build an engine pool and its forwarders
/// run, but its Builder registers neither this hook nor a delivery
/// watcher and hosts no steer initiator (no invoke handler, no scheduled
/// runtime), so no registry entry can exist there and the fan-out is an
/// empty no-op. Residual risk to keep in mind when extending hosts: a
/// future lane that spawns the delivery watcher WITHOUT registering this
/// hook would strand every steered file at the 24h `STEER_COMMIT_WAIT`
/// before a duplicate — the messaging suite's lib.rs source pin binds the
/// registration to the finalizer and asserts the registration precedes the
/// first `spawn_delivery_watcher` occurrence (order, not uniqueness: a
/// second spawn site in another host file would leave that pin green).
pub fn resolve_steer_outcome(steer_id: &str, committed: bool) {
    if let Some(hook) = STEER_RESOLUTION_HOOK.get() {
        hook(steer_id, committed);
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_steer_outcome;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Registration gate: the first registrant wins and duplicates are
    /// rejected (a silent overwrite would swap consumers — assembly errors
    /// must be visible). The consumer also records the arguments: an
    /// inversion inside [`resolve_steer_outcome`] (hook(.., !committed))
    /// previously shipped the entire suite green — every behavioral test
    /// drives the messaging finalizer directly and bypasses this fan-out,
    /// so the argument passthrough is only observable here (round-5 M2).
    #[test]
    fn hook_registration_accepts_first_and_rejects_rest() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        static FORWARDED: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());
        fn consumer(steer_id: &str, committed: bool) {
            CALLS.fetch_add(1, Ordering::SeqCst);
            FORWARDED
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push((steer_id.to_string(), committed));
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
        // The fan-out must forward (id, committed) verbatim — no inversion,
        // no id swap (round-5 M2).
        let forwarded = FORWARDED.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(
            forwarded.as_slice(),
            &[
                ("steer-1@0".to_string(), true),
                ("steer-2@0".to_string(), false)
            ][..],
            "resolve_steer_outcome must forward (steer_id, committed) to the hook verbatim"
        );
    }
}
