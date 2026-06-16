#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "windows")]
pub(crate) mod windows;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) mod unsupported;

mod interface;

#[cfg(target_os = "linux")]
pub(crate) use linux as platform;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) use unsupported as platform;
#[cfg(target_os = "windows")]
pub(crate) use windows as platform;

pub use interface::{
    check_update_platform_support, command_exists, disable_super_permission,
    enable_super_permission, install_dependencies, install_update_package, nvidia_smi_candidates,
    open_target, platform_compat_path, ram_snapshot, super_permission_is_enabled,
    super_permission_turn_reminder, user_home_dir,
};
