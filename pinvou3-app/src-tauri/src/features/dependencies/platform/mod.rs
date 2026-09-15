#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "linux", test))]
mod linux_packages;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(any(target_os = "windows", test))]
mod windows_install_text;

#[cfg(target_os = "linux")]
pub use linux::install_dependencies;
#[cfg(target_os = "macos")]
pub use macos::install_dependencies;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub use unsupported::install_dependencies;
#[cfg(target_os = "windows")]
pub use windows::install_dependencies;
