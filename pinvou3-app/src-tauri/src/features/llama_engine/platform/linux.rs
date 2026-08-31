//! Linux 适配：Vulkan 包（ubuntu-vulkan-x64.tar.gz，解压后带顶层 `llama-*/bin/`），
//! GPU 走 Vulkan；无 Vulkan 驱动时回退 CPU。

use std::path::Path;

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

/// 钉版引擎资产（PINNED_ENGINE_TAG = b10299）的尺寸 + sha256
/// （来源：GitHub release asset digest）。非钉版 tag 由调用方按开发通道处理。
pub fn pinned_engine_asset() -> Option<(u64, &'static str)> {
    Some((
        32_470_721,
        "57f555a6e2ff21f9b58fbb50e1bb83ec1706f1e6b7d576f486153bf4b957a791",
    ))
}

pub fn engine_archive_is_zip() -> bool {
    false
}

/// 目录所在卷的可用字节数（statvfs f_bavail * f_frsize）；查询失败返回 None。
pub fn available_disk_space(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

pub fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("设置引擎可执行权限失败: {e}"))
}
