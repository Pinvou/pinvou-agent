#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "macos")]
mod voice_asr_speech;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub use unsupported::*;
#[cfg(target_os = "windows")]
pub use windows::*;
