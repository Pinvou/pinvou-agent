mod windows_dependency;
mod windows_memory;
mod windows_path;
mod windows_permission;
mod windows_system;
mod windows_update;

pub use windows_dependency::install_dependencies;
pub use windows_memory::ram_snapshot;
pub use windows_path::{platform_compat_path, user_home_dir};
pub use windows_permission::{
    disable_super_permission, enable_super_permission, super_permission_is_enabled,
    super_permission_turn_reminder,
};
pub use windows_system::{command_exists, nvidia_smi_candidates, open_target};
pub use windows_update::{check_update_platform_support, install_update_package};
