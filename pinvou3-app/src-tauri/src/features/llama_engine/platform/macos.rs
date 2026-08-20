//! macOS 适配：官方 release 的 macos-arm64 包（默认 Metal，`-ngl 99` 全量卸载 GPU）。

use std::path::Path;

use super::super::EngineDevice;

pub fn engine_binary_name() -> &'static str {
    "llama-server"
}

pub fn engine_asset_name(tag: &str) -> String {
    format!("llama-{tag}-bin-macos-arm64.tar.gz")
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
    "若 Metal 设备初始化失败，请在设置中切换到 CPU 设备"
}
