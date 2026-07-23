#[cfg(target_os = "linux")]
mod linux;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub use unsupported::*;
#[cfg(target_os = "windows")]
pub use windows::*;
