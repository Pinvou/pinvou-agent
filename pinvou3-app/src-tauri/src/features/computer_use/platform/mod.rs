//! Computer Use platform backend selection. Each of the three platforms implements
//! [`ComputerUseBackend`]; `cfg(target_os)` is used only inside the `platform/` adapter layer.

/// Pure helpers shared by every per-OS backend (no OS handles, unit-tested on
/// all targets).
mod helpers;
pub(crate) use helpers::normalize_typed_newlines;
// `sanitize_name` is also the tool layer's display bound for consent target
// lines: the backends carry raw role/name strings so screening keeps seeing
// them untruncated, and the bound is applied where the label is rendered.
pub(crate) use helpers::sanitize_name;
// The denylist verdict itself is taken inside platform/; this re-export exists
// for the tool-layer regression test that pins screening against the raw,
// untruncated accessible name.
#[cfg(test)]
pub(crate) use helpers::screening_hit;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
/// macOS-only: ScreenCaptureKit static screenshot adapter (macOS 15.2+, preferred by
/// `macos`; older systems fall back to xcap's CGWindowList path).
#[cfg(target_os = "macos")]
mod screen_capture_kit;
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

#[cfg(target_os = "linux")]
use self::linux as imp;
#[cfg(target_os = "macos")]
use self::macos as imp;
#[cfg(target_os = "windows")]
use self::windows as imp;

use super::backend::ComputerUseBackend;
use super::types::ComputerUseError;

/// Create the backend for the current OS. **Must be called on computer_use's dedicated
/// worker thread** (guaranteed by BackendHandle): xcap/enigo/a11y objects have thread affinity.
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
pub(crate) fn create_backend() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> {
    imp::create_backend()
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub(crate) fn create_backend() -> Result<Box<dyn ComputerUseBackend>, ComputerUseError> {
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
