pub(super) mod detach;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod standard;

#[cfg(target_os = "linux")]
pub(super) use linux::effective_window_size;
#[cfg(target_os = "linux")]
pub(super) use linux::apply_pet_window_policy;
#[cfg(target_os = "linux")]
pub(super) use linux::finish_main_focus_raise;
#[cfg(target_os = "linux")]
pub(super) use linux::prepare_main_focus_raise;
#[cfg(target_os = "macos")]
pub(super) use macos::{apply_pet_window_policy, effective_window_size};
#[cfg(target_os = "macos")]
pub(super) use macos::finish_main_focus_raise;
#[cfg(target_os = "macos")]
pub(super) use macos::prepare_main_focus_raise;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) use standard::effective_window_size;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) use standard::apply_pet_window_policy;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) use standard::finish_main_focus_raise;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) use standard::prepare_main_focus_raise;
