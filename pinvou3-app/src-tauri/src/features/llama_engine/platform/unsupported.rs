//! 不支持平台：显式返回失败，不静默借用其他平台实现。

use std::path::Path;

pub fn engine_binary_name() -> &'static str {
    "llama-server"
}

pub fn engine_asset_name(_tag: &str) -> String {
    String::new()
}

pub fn engine_url(_tag: &str) -> String {
    String::new()
}

/// 不支持平台无钉版资产（engine_asset_name 为空会先于校验触发明确错误）。
pub fn pinned_engine_asset() -> Option<(u64, &'static str)> {
    None
}

pub fn engine_archive_is_zip() -> bool {
    true
}

/// 不支持平台无法查询可用空间（调用方按跳过检查处理）。
pub fn available_disk_space(_path: &Path) -> Option<u64> {
    None
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
