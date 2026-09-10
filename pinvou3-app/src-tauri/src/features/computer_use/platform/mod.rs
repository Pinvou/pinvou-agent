//! Computer Use 平台后端选择。三平台各自实现
//! [`ComputerUseBackend`]；选择只在 `platform/` 适配层用 `cfg(target_os)`。

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
/// macOS 专属:ScreenCaptureKit 静态截图适配(macOS 14+,被 `macos` 优先
/// 使用;老系统回退 xcap 的 CGWindowList 路径)。
#[cfg(target_os = "macos")]
mod screen_capture_kit;
/// Linux 专属:Wayland 同会话截屏(portal ScreenCast 流的 PipeWire 接收端,
/// 被 `wayland_portal` 在会话启动后使用)。
#[cfg(target_os = "linux")]
mod wayland_capture;
/// Linux 专属:Wayland 输入注入的 portal RemoteDesktop 适配
/// (被 `linux` 在 Wayland 会话下使用)。
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

/// 创建当前操作系统的后端。**必须在 computer_use 的专用 worker 线程上调用**
/// （BackendHandle 保证）：xcap/enigo/a11y 对象线程亲和。
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

/// 当前操作系统是否有 computer_use 后端实现（Tauri 状态命令的平台能力位）。
pub(crate) fn backend_supported() -> bool {
    cfg!(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux"
    ))
}

/// 触发平台授权引导：macOS 弹 Screen Recording + Accessibility 两条 TCC 系统
/// 窗；Windows/Linux 无需系统授权，显式返回 unsupported（不静默 no-op）。
#[cfg(target_os = "macos")]
pub(crate) fn request_permissions() -> Result<(), ComputerUseError> {
    imp::request_permissions();
    Ok(())
}

/// 触发平台授权引导：本平台无系统授权弹窗，显式返回 unsupported（不静默 no-op）。
#[cfg(not(target_os = "macos"))]
pub(crate) fn request_permissions() -> Result<(), ComputerUseError> {
    Err(ComputerUseError::unsupported(
        "request_permissions",
        "this operating system has no computer use permission prompt",
    ))
}
