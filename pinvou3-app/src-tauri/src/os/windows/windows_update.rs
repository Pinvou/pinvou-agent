use std::path::Path;

pub fn check_update_platform_support() -> Result<(), String> {
    Err("当前平台暂不支持应用内 .deb 更新；Windows MSI 更新需要独立设计".to_string())
}

pub fn install_update_package(_path: &Path) -> Result<(), String> {
    Err("当前平台暂不支持应用内 .deb 更新；Windows MSI 更新需要独立设计".to_string())
}
