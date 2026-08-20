//! 不支持平台：显式返回失败，不静默借用其他平台实现。

use std::path::Path;

use super::super::EngineDevice;

pub fn engine_binary_name() -> &'static str {
    "llama-server"
}

pub fn engine_asset_name(_tag: &str) -> String {
    String::new()
}

pub fn engine_url(_tag: &str) -> String {
    String::new()
}

pub fn engine_archive_is_zip() -> bool {
    true
}

pub fn default_device() -> EngineDevice {
    EngineDevice::Cpu
}

pub fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

pub fn gpu_error_hint() -> &'static str {
    ""
}

/// 不支持平台不可下载/不可运行（engine_asset_name 为空会触发明确错误）。
pub fn unsupported_hint() -> &'static str {
    "当前平台暂不支持本地多模态引擎"
}
