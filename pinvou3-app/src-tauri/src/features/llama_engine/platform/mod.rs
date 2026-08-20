//! llama 引擎的平台适配（voice/platform 风格：cfg 分模块 + pub use 重导出）。
//!
//! 每平台提供同名函数集；`unsupported` 平台返回保守失败，不静默借用其他实现。

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod unsupported;

#[cfg(target_os = "windows")]
pub use windows::*;
#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub use unsupported::*;

/// 测试辅助：非 Windows 平台需要 `make_executable` 等 unix 行为。
#[cfg(test)]
pub(crate) fn unix_like() -> bool {
    !cfg!(target_os = "windows")
}
