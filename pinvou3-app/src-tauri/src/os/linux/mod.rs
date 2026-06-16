mod linux_dependency;
mod linux_path;
mod linux_permission;
mod linux_system;
mod linux_update;

pub use linux_dependency::install_dependencies;
pub use linux_path::{platform_compat_path, user_home_dir};
pub use linux_permission::{
    disable_super_permission, enable_super_permission, super_permission_is_enabled,
    super_permission_turn_reminder,
};
pub use linux_system::{command_exists, nvidia_smi_candidates, open_target};
pub use linux_update::{check_update_platform_support, install_update_package};
