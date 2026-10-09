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
    "macOS 不支持超级权限开关。**禁止用 sudo**(应用内会被 execpolicy 直接拒绝)。需要 root 的操作请引导用户在系统终端手动执行。"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reminder states the real contract (sudo is execpolicy-denied on
    /// macOS — safety_deny_rules always emits the sudo deny rules here) and
    /// must not claim a Linux-only toggle exists. Pin the anchors.
    #[test]
    fn reminder_pins_sudo_denied_contract() {
        let reminder = super_permission_turn_reminder();
        for anchor in ["禁止用 sudo", "execpolicy 直接拒绝", "系统终端"] {
            assert!(
                reminder.contains(anchor),
                "reminder lost `{anchor}`: {reminder}"
            );
        }
        assert!(
            !reminder.contains("设置"),
            "macOS reminder must not point at the in-app toggle"
        );
    }
}
