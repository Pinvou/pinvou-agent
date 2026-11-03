//! Computer Use platform backend selection. Each of the three platforms implements
//! [`ComputerUseBackend`]; `cfg(target_os)` is used only inside the `platform/` adapter layer.

/// Pure helpers shared by every per-OS backend (no OS handles, unit-tested on
/// all targets).
mod helpers;
pub(crate) use helpers::normalize_typed_newlines;
// `sanitize_name` is also the tool layer's display bound for consent target
// lines: the backends carry raw role/name strings so screening keeps seeing
// them untruncated, and the bound is applied where the label is rendered.
pub(crate) use helpers::MAX_NAME_CHARS;
pub(crate) use helpers::sanitize_name;
// `screening_hit` is also consumed by the tool layer for the role: macOS
// carries a free-form, app-supplied AXRole, and the streaming matcher keeps
// a hostile multi-megabyte role from costing a proportional fold allocation.
pub(crate) use helpers::screening_hit;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
/// macOS-only: ScreenCaptureKit static screenshot adapter (macOS 15.2+, preferred by
/// `macos`; older systems fall back to xcap's CGWindowList path).
#[cfg(target_os = "macos")]
mod screen_capture_kit;
/// Virtual display shared glue (probe / one-time target decision / per-OS
/// dispatch). Compiles on every target; the per-OS stacks are cfg'd inside.
mod virtual_display;
/// Linux-only: Wayland same-session screen capture (the PipeWire receiver of the portal
/// ScreenCast stream, used by `wayland_portal` after the session starts).
#[cfg(target_os = "linux")]
mod wayland_capture;
/// Linux-only: the portal RemoteDesktop adapter for Wayland input injection
/// (used by `linux` under Wayland sessions).
#[cfg(target_os = "linux")]
mod wayland_portal;
#[cfg(target_os = "windows")]
mod windows;

pub(crate) use self::virtual_display::{
    TargetDecision, VirtualAvailabilitySnapshot, probe as probe_virtual_display, resolve_snapshot,
    resolve_target, sweep_orphans,
};

#[cfg(target_os = "linux")]
use self::linux as imp;
#[cfg(target_os = "macos")]
use self::macos as imp;
#[cfg(target_os = "windows")]
use self::windows as imp;

use std::sync::OnceLock;

use tauri::AppHandle;

use super::backend::ComputerUseBackend;
use super::types::ComputerUseError;

/// The app handle for [`dispatch_to_main_thread`], stored once at setup.
/// macOS virtual-display creation (and any future main-run-loop-bound work)
/// needs to hop onto the main thread; the platform layer has no other handle
/// path (the tool's `TauriEventSink` gets one per session, but backend
/// creation runs later on the worker thread).
static MAIN_THREAD_APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// Store the app handle for [`dispatch_to_main_thread`]. Called once from
/// lib.rs setup; a second call is a no-op (the first handle wins — there is
/// exactly one main thread per process).
pub(crate) fn set_main_thread_app_handle(app: AppHandle) {
    let _ = MAIN_THREAD_APP_HANDLE.set(app);
}

/// Hand a task to the application's main thread. Returns `false` when the
/// app handle is not (yet) stored or the enqueue failed — callers treat that
/// as "main-thread dispatch unavailable" and run their explicit fallback
/// (e.g. the macOS virtual display answers Unavailable so the one-time
/// dispatch picks the physical backend). Linux/Windows call sites ignore it.
/// The task runs asynchronously: this returns after the ENQUEUE, not after
/// the task ran; wait-for-completion needs the caller's own channel.
pub(crate) fn dispatch_to_main_thread(task: Box<dyn FnOnce() + Send>) -> bool {
    let Some(app) = MAIN_THREAD_APP_HANDLE.get() else {
        return false;
    };
    app.run_on_main_thread(move || task()).is_ok()
}

/// Create the backend for the current OS, applying the ONE-TIME virtual-vs-
/// physical fallback decision (§9: taken here, at creation, recorded in the
/// returned backend's `Capabilities.target`, and never re-evaluated
/// mid-session): a virtual preference with a green probe builds the virtual
/// backend, everything else stays physical. **Must be called on computer_use's
/// dedicated worker thread** (guaranteed by BackendHandle): xcap/enigo/a11y
/// objects have thread affinity.
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub(crate) fn create_backend(
    prefer_virtual: bool,
) -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> {
    let availability = virtual_display::probe();
    match virtual_display::resolve_target(prefer_virtual, &availability) {
        TargetDecision::Virtual => virtual_display::create_virtual_backend(),
        TargetDecision::Physical => imp::create_backend(),
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub(crate) fn create_backend(
    prefer_virtual: bool,
) -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> {
    // No physical backend exists for this OS; the preference never matters
    // here, but consume the argument so both signatures stay aligned.
    let _ = prefer_virtual;
    Err(ComputerUseError::unsupported(
        "computer_use",
        "no computer use backend exists for this operating system",
    ))
}

/// Whether the current OS has a computer_use backend implementation (the platform
/// capability bit for the Tauri state commands).
pub(crate) fn backend_supported() -> bool {
    cfg!(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No-handle fallback pin: without a stored app handle (test binaries
    /// never run lib.rs setup, and nothing else stores one), the main-thread
    /// dispatch answers `false` — callers must take their explicit fallback
    /// path (the macOS virtual display answers Unavailable → physical)
    /// instead of silently dropping a task on the floor.
    #[test]
    fn dispatch_without_an_app_handle_returns_false() {
        assert!(
            !dispatch_to_main_thread(Box::new(|| {
                panic!("the task must not run without a stored app handle");
            })),
            "dispatch must report the missing handle instead of dropping the task"
        );
    }
}

/// Trigger platform permission onboarding: on macOS pop both the Screen Recording and
/// Accessibility TCC system dialogs; Windows/Linux need no system authorization, so return
/// unsupported explicitly (never a silent no-op).
#[cfg(target_os = "macos")]
pub(crate) fn request_permissions() -> Result<(), ComputerUseError> {
    imp::request_permissions();
    Ok(())
}

/// Trigger platform permission onboarding: this platform has no system permission dialog,
/// so return unsupported explicitly (never a silent no-op).
#[cfg(not(target_os = "macos"))]
pub(crate) fn request_permissions() -> Result<(), ComputerUseError> {
    Err(ComputerUseError::unsupported(
        "request_permissions",
        "this operating system has no computer use permission prompt",
    ))
}

/// Snapshot of the OS permission grants computer use depends on:
/// `(screen_recording, accessibility)`. `None` where the platform has no
/// system permission flow (Windows/Linux) — the settings UI hides the
/// onboarding row instead of showing permanently-false toggles. A `false`
/// here is advisory, not authoritative for the action: the tool path
/// re-checks per action, and macOS applies a fresh TCC grant only to
/// processes launched after it (the settings UI must pair a miss with the
/// restart guidance).
#[cfg(target_os = "macos")]
pub(crate) fn permission_status() -> Option<(bool, bool)> {
    imp::permission_status()
}

/// This platform has no system permission flow, so there is no status to report.
#[cfg(not(target_os = "macos"))]
pub(crate) fn permission_status() -> Option<(bool, bool)> {
    None
}
