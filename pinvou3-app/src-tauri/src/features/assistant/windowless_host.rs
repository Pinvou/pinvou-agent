//! Windowless host marker.
//!
//! The headless host lanes (`product_runtime::headless_bridge`'s
//! `run_windowless_host` / `run_bare_host`, benchmark-hooks) boot a product
//! host with no window and no interactive surface: nothing in the process
//! can answer an approval card. The forwarder consults this marker so an
//! interactive ask raised on such a host resolves deterministically
//! (decline) instead of parking the turn until its deadline. The module
//! lives outside the benchmark-hooks-gated `product_runtime` tree because
//! the forwarder (core chat code) must consult it in every build, while
//! the marker can only ever be set by the gated boot lanes — a build
//! without them can never report a windowless host.

use std::sync::atomic::{AtomicBool, Ordering};

static WINDOWLESS_HOST: AtomicBool = AtomicBool::new(false);

/// Mark this process as a windowless product host. Called once by the
/// headless boot lanes before any engine runs; there is no unmark — the
/// property is a process-level boot fact, not session state.
#[cfg(feature = "benchmark-hooks")]
pub(crate) fn mark_windowless_host() {
    WINDOWLESS_HOST.store(true, Ordering::Release);
}

/// True when this process booted a windowless product host: an interactive
/// approval raised here has no decision consumer.
pub(crate) fn is_windowless_host() -> bool {
    WINDOWLESS_HOST.load(Ordering::Acquire)
}

#[cfg(all(test, feature = "benchmark-hooks"))]
mod tests {
    use super::*;

    /// The marker is a monotonic process-level boot fact: the headless boot
    /// lanes only ever set it (no unmark), so the pin is one-sided. Builds
    /// without benchmark-hooks can never mark the host; the read side stays
    /// callable there (the forwarder consults it unconditionally).
    #[test]
    fn mark_makes_the_host_windowless() {
        mark_windowless_host();
        assert!(is_windowless_host());
    }
}
