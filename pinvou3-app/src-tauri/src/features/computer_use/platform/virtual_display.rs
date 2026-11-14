//! Virtual display shared glue: availability probe, the one-time virtual-vs-
//! physical target decision, and the per-OS create/sweep dispatch.
//!
//! This module deliberately contains NO business logic: the per-OS stacks live
//! in [`linux_virtual`] / [`macos_virtual`] / [`windows_virtual`], selected by
//! the same `cfg(target_os)` alias pattern as the physical `create_backend`
//! (platform/mod.rs). The dispatch here only forwards to the per-OS module
//! behind the `virtual_imp` alias.
//!
//! Fallback contract (the one-time creation decision): the virtual-vs-
//! physical decision happens exactly ONCE, at backend creation, through
//! [`resolve_target`]; a failed virtual spawn mid-session must degrade to an
//! explicit error, never to a silent physical flip. The decision is recorded
//! in the created backend's `Capabilities.target` and never re-evaluated.

/// Per-OS virtual display stacks. Each module exposes the same three
/// functions: `probe`, `create_backend`, and (linux only) `sweep_orphans`.
#[cfg(target_os = "linux")]
mod linux_virtual;
#[cfg(target_os = "macos")]
mod macos_virtual;
#[cfg(target_os = "windows")]
mod windows_virtual;

#[cfg(target_os = "linux")]
use self::linux_virtual as virtual_imp;
#[cfg(target_os = "macos")]
use self::macos_virtual as virtual_imp;
#[cfg(target_os = "windows")]
use self::windows_virtual as virtual_imp;

use super::super::backend::ComputerUseBackend;
use super::super::types::ComputerUseError;

/// Cheap availability probe for the current platform's virtual display stack:
/// binary presence on PATH (linux) / private-API class checks (macOS) /
/// shell-binary presence (Windows — `explorer.exe`). NO side effects, NO
/// server spawn — it runs at the refresh points (startup replay, preference
/// flips, creation reporting); the status command reads the CACHED snapshot
/// between them (guard.rs `refresh_virtual_availability` documents the
/// exact points).
pub(crate) fn probe() -> VirtualAvailability {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    return virtual_imp::probe();
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        VirtualAvailability::Unavailable {
            detail: "no virtual display backend exists for this operating system".to_string(),
        }
    }
}

/// Create the virtual display backend for the current OS. Called only after
/// [`resolve_target`] selected the virtual arm. **Must be called on
/// computer_use's dedicated worker thread** (same thread-affinity rule as the
/// physical `create_backend`).
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub(crate) fn create_virtual_backend() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> {
    virtual_imp::create_backend()
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub(crate) fn create_virtual_backend() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> {
    Err(ComputerUseError::unsupported(
        "computer_use",
        "no virtual display backend exists for this operating system",
    ))
}

/// Crash-orphan sweep for the current OS's virtual display stack (linux:
/// kill pidfile-recorded Xvfb/WM trees from crashed sessions; elsewhere a
/// no-op — process death reclaims the macOS/Windows targets). Called at app
/// setup AND in the exit/restart harvest (idempotent — the identity gate
/// makes a second pass over the same pidfiles harmless). Kept unconditional
/// at the call sites so lib.rs needs no
/// `cfg(target_os)` (architecture-guard rule: target cfg stays in the
/// platform adapter layer).
pub(crate) fn sweep_orphans() {
    #[cfg(target_os = "linux")]
    virtual_imp::sweep_orphans();
    #[cfg(not(target_os = "linux"))]
    {
        // Nothing to sweep: the macOS/Windows virtual targets are reclaimed
        // by process death (no pidfile-recorded process trees on those
        // platforms).
    }
}

/// The cheap probe verdict. `Unavailable` carries the model/user-facing
/// diagnostic (e.g. the missing-binary install hint on linux).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VirtualAvailability {
    /// The platform's virtual display stack is ready to create.
    Available,
    /// Not usable now; `detail` says why and (where applicable) how to fix.
    Unavailable { detail: String },
}

impl VirtualAvailability {
    pub(crate) fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }
}

/// The resolved global snapshot stored in `ComputerUseShared`: the probe
/// verdict combined with the user preference. `state` is the frozen string
/// vocabulary of the design ("available" | "unavailable" | "disabled"); the
/// status command maps it plus the per-session target registry onto the
/// user-visible "active" | "ready" | "unavailable" | "disabled".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VirtualAvailabilitySnapshot {
    /// One of `"available" | "unavailable" | "disabled"`.
    pub state: &'static str,
    /// Diagnostic detail ("" when nothing is wrong).
    pub detail: String,
}

/// Pure resolution of the global snapshot: the preference wins over the probe
/// (pref off ⇒ "disabled" regardless of probe), then the probe verdict.
pub(crate) fn resolve_snapshot(
    prefer_virtual: bool,
    availability: &VirtualAvailability,
) -> VirtualAvailabilitySnapshot {
    if !prefer_virtual {
        return VirtualAvailabilitySnapshot {
            state: "disabled",
            detail: String::new(),
        };
    }
    match availability {
        VirtualAvailability::Available => VirtualAvailabilitySnapshot {
            state: "available",
            detail: String::new(),
        },
        VirtualAvailability::Unavailable { detail } => VirtualAvailabilitySnapshot {
            state: "unavailable",
            detail: detail.clone(),
        },
    }
}

/// The one-time virtual-vs-physical decision taken at backend creation —
/// never re-evaluated, never silently flipped mid-session. Pure function so
/// the fallback rule is exhaustively unit-testable without any OS state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetDecision {
    /// Prefer the virtual target and the probe says it is available: build
    /// the virtual backend (its `Capabilities.target` records Virtual).
    Virtual,
    /// Stay physical: either the user prefers the real desktop or the virtual
    /// stack is unavailable (the created backend's `Capabilities.target`
    /// records Physical and its note fields stay empty — physical backends
    /// keep the pre-virtual shape; the WHY arrives through the status
    /// snapshot's `virtualDisplayDetail`, which the availability refresh
    /// fills from the probe).
    Physical,
}

/// The ONE-TIME fallback decision. Exhaustive rule set:
/// - preference off ⇒ physical, whatever the probe says (the user opted out);
/// - preference on + probe available ⇒ virtual;
/// - preference on + probe unavailable ⇒ physical (explicit fallback, the
///   probe detail explains why in the status surface).
pub(crate) fn resolve_target(
    prefer_virtual: bool,
    availability: &VirtualAvailability,
) -> TargetDecision {
    if prefer_virtual && availability.is_available() {
        TargetDecision::Virtual
    } else {
        TargetDecision::Physical
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unavailable(detail: &str) -> VirtualAvailability {
        VirtualAvailability::Unavailable {
            detail: detail.to_string(),
        }
    }

    /// The exhaustive fallback rule: the preference gates first, then the
    /// probe verdict. The fallback decision happens at backend
    /// creation only — the decision itself is a pure function of exactly
    /// these two inputs, so a mid-session flip has no input to come from.
    #[test]
    fn resolve_target_covers_every_preference_and_probe_combination() {
        assert_eq!(
            resolve_target(true, &VirtualAvailability::Available),
            TargetDecision::Virtual,
            "pref on + available must select the virtual target"
        );
        assert_eq!(
            resolve_target(true, &unavailable("Xvfb not found")),
            TargetDecision::Physical,
            "pref on + unavailable must fall back to physical"
        );
        assert_eq!(
            resolve_target(false, &VirtualAvailability::Available),
            TargetDecision::Physical,
            "pref off must stay physical even when the probe is green"
        );
        assert_eq!(
            resolve_target(false, &unavailable("anything")),
            TargetDecision::Physical,
            "pref off + unavailable must stay physical"
        );
    }

    /// Purity pin: the same inputs always produce the same decision (no
    /// hidden state that could let a later creation silently differ from an
    /// earlier one — the recorded `Capabilities.target` is the contract).
    #[test]
    fn resolve_target_is_a_pure_function_of_its_inputs() {
        for prefer in [true, false] {
            for availability in [
                VirtualAvailability::Available,
                unavailable("d1"),
                unavailable(""),
            ] {
                let first = resolve_target(prefer, &availability);
                for _ in 0..3 {
                    assert_eq!(
                        resolve_target(prefer, &availability),
                        first,
                        "decision must not depend on call history"
                    );
                }
            }
        }
    }

    /// The global snapshot resolution: preference off collapses to "disabled"
    /// with an empty detail even on a green probe; pref on maps the probe
    /// verdict onto "available"/"unavailable" and carries the install hint.
    #[test]
    fn resolve_snapshot_maps_preference_then_probe() {
        let disabled = resolve_snapshot(false, &VirtualAvailability::Available);
        assert_eq!(disabled.state, "disabled");
        assert_eq!(disabled.detail, "");

        let ready = resolve_snapshot(true, &VirtualAvailability::Available);
        assert_eq!(ready.state, "available");
        assert_eq!(ready.detail, "");

        let unavailable = resolve_snapshot(true, &unavailable("install xvfb"));
        assert_eq!(unavailable.state, "unavailable");
        assert_eq!(unavailable.detail, "install xvfb");
    }
}
