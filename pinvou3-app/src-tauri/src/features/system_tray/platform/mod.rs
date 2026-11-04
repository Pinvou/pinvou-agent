#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub(crate) use windows::{before_restart, install, on_exit, on_window_event, refresh_language};

// These are lifecycle hooks, not a cross-platform tray capability. Other
// desktops retain their existing close and exit behavior.
#[cfg(not(target_os = "windows"))]
pub(crate) fn install(_: &tauri::AppHandle, _: crate::platform::prefs::Language) {}

#[cfg(not(target_os = "windows"))]
pub(crate) fn refresh_language(_: &tauri::AppHandle, _: crate::platform::prefs::Language) {}

#[cfg(not(target_os = "windows"))]
pub(crate) fn on_window_event(_: &tauri::Window, _: &tauri::WindowEvent) {}

#[cfg(not(target_os = "windows"))]
pub(crate) fn on_exit() {}

#[cfg(not(target_os = "windows"))]
pub(crate) async fn before_restart(_: &tauri::AppHandle) {}

#[cfg(any(target_os = "windows", test))]
mod close_policy;
