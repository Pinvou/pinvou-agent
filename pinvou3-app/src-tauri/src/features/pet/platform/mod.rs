#[cfg(target_os = "linux")]
mod linux;
#[cfg(not(target_os = "linux"))]
mod standard;

#[cfg(target_os = "linux")]
pub(super) use linux::effective_window_size;
#[cfg(not(target_os = "linux"))]
pub(super) use standard::effective_window_size;
