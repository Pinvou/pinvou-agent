#[cfg(not(target_os = "windows"))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(not(target_os = "windows"))]
pub(super) use unsupported::resolve_current_identity;
#[cfg(target_os = "windows")]
pub(super) use windows::resolve_current_identity;
