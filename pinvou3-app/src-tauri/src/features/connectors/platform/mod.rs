#[cfg(target_os = "linux")]
mod linux;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub(super) use linux::{eip_bin_path, zhidao_bin_path};
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(super) use unsupported::{eip_bin_path, zhidao_bin_path};
#[cfg(target_os = "windows")]
pub(super) use windows::{eip_bin_path, zhidao_bin_path};
