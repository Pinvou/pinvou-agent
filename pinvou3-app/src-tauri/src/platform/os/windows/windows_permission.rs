pub fn super_permission_is_enabled() -> bool {
    false
}

pub fn enable_super_permission() -> Result<(), String> {
    Err("当前系统不支持 Linux sudo 超级权限开关".to_string())
}

pub fn disable_super_permission() -> Result<(), String> {
    Ok(())
}

pub fn super_permission_turn_reminder() -> &'static str {
    "当前系统不支持超级权限开关。**禁止用 sudo**(应用内会被 execpolicy 直接拒绝)。需要管理员权限的操作,请引导用户使用系统提供的管理员方式(如管理员终端)执行,不要尝试提权变通。"
}
