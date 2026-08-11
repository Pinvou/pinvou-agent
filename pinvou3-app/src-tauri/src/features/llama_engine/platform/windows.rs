//! Windows 适配：Vulkan 包（CPU + Vulkan 一体，官方 CI 打包 `build/bin/Release/*`
//! 全部内容，llama-server.exe 位于 zip 根目录），GPU 走 Vulkan（驱动自带）。

use std::path::Path;

use super::super::EngineDevice;

pub fn engine_binary_name() -> &'static str {
    "llama-server.exe"
}

pub fn engine_asset_name(tag: &str) -> String {
    format!("llama-{tag}-bin-win-vulkan-x64.zip")
}

pub fn engine_url(tag: &str) -> String {
    format!(
        "https://github.com/ggml-org/llama.cpp/releases/download/{tag}/{}",
        engine_asset_name(tag)
    )
}

pub fn engine_archive_is_zip() -> bool {
    true
}

pub fn default_device() -> EngineDevice {
    // Vulkan 包同时覆盖 CPU 与 GPU（-ngl 0 / 99 切换），默认 GPU 加速。
    EngineDevice::Gpu
}

pub fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

pub fn gpu_error_hint() -> &'static str {
    "若显卡缺少 Vulkan 驱动，请在设置中切换到 CPU 设备"
}
