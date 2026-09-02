//! macOS 权限管理子模块。
//!
//! macOS 没有 pkexec / sudoers.d 等价机制，`super_permission` 全部不支持。
//! 需要 root 的操作应引导用户在终端手动执行。

pub fn super_permission_is_enabled() -> bool {
    false
}

pub fn enable_super_permission() -> Result<(), String> {
    Err("macOS 不支持超级权限开关（无 sudoers 等价机制）".to_string())
}

pub fn disable_super_permission() -> Result<(), String> {
    Ok(())
}

pub fn super_permission_turn_reminder() -> &'static str {
    "macOS 不支持超级权限开关。需要 root 的操作请引导用户在终端手动执行。"
}
