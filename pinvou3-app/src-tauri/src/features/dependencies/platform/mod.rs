#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "linux", test))]
mod linux_packages;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::install_dependencies;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub use unsupported::install_dependencies;
#[cfg(target_os = "windows")]
pub use windows::install_dependencies;
