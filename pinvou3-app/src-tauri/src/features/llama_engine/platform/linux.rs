//! Linux 适配：Vulkan 包（ubuntu-vulkan-x64.tar.gz，解压后带顶层 `llama-*/bin/`），
//! GPU 走 Vulkan；无 Vulkan 驱动时回退 CPU。

use std::path::Path;

use super::super::EngineDevice;

pub fn engine_binary_name() -> &'static str {
    "llama-server"
}

pub fn engine_asset_name(tag: &str) -> String {
    format!("llama-{tag}-bin-ubuntu-vulkan-x64.tar.gz")
}

pub fn engine_url(tag: &str) -> String {
    format!(
        "https://github.com/ggml-org/llama.cpp/releases/download/{tag}/{}",
        engine_asset_name(tag)
    )
}

pub fn engine_archive_is_zip() -> bool {
    false
}

pub fn default_device() -> EngineDevice {
    EngineDevice::Gpu
}

pub fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("设置引擎可执行权限失败: {e}"))
}

pub fn gpu_error_hint() -> &'static str {
    "若显卡缺少 Vulkan 驱动，请在设置中切换到 CPU 设备"
}
