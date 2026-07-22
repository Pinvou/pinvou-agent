pub fn super_permission_is_enabled() -> bool {
    super::super::platform::super_permission_is_enabled()
}

pub fn enable_super_permission() -> Result<(), String> {
    super::super::platform::enable_super_permission()
}

pub fn disable_super_permission() -> Result<(), String> {
    super::super::platform::disable_super_permission()
}

pub fn super_permission_turn_reminder() -> &'static str {
    super::super::platform::super_permission_turn_reminder()
}

#[cfg(all(test, not(target_os = "linux")))]
mod tests {
    use super::*;

    #[test]
    fn linux_super_permission_degrades_on_non_linux() {
        assert!(enable_super_permission().is_err());
    }
}
