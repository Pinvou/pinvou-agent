mod dependency;
mod memory;
mod path;
mod permission;
mod system;
mod update;

pub use dependency::install_dependencies;
pub use memory::ram_snapshot;
pub use path::{platform_compat_path, user_home_dir};
pub use permission::{
    disable_super_permission, enable_super_permission, super_permission_is_enabled,
    super_permission_turn_reminder,
};
pub use system::{command_exists, nvidia_smi_candidates, open_target};
pub use update::{check_update_platform_support, install_update_package};
