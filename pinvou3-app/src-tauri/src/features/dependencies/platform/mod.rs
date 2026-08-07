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

#[cfg(target_os = "linux")]
pub use linux::install_dependencies;
#[cfg(target_os = "macos")]
pub use macos::install_dependencies;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub use unsupported::install_dependencies;
#[cfg(target_os = "windows")]
pub use windows::install_dependencies;

pub(super) fn dependency_check_policy() -> super::DependencyCheckPolicy {
    #[cfg(target_os = "linux")]
    return linux::dependency_check_policy();
    #[cfg(target_os = "macos")]
    return macos::dependency_check_policy();
    #[cfg(target_os = "windows")]
    return windows::dependency_check_policy();
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    return unsupported::dependency_check_policy();
}
