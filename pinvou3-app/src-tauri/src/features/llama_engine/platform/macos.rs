//! macOS 适配：官方 release 按架构选包——arm64 用 macos-arm64 包（默认 Metal，
//! `-ngl 99` 全量卸载 GPU）；x86_64（Intel）用 macos-x64 包，是否 Metal 卸载
//! 由引擎自身决定，此处只保证下载的包能在该架构上 exec。

use std::path::Path;

pub fn engine_binary_name() -> &'static str {
    "llama-server"
}

#[cfg(target_arch = "aarch64")]
pub fn engine_asset_name(tag: &str) -> String {
    format!("llama-{tag}-bin-macos-arm64.tar.gz")
}

#[cfg(target_arch = "x86_64")]
pub fn engine_asset_name(tag: &str) -> String {
    format!("llama-{tag}-bin-macos-x64.tar.gz")
}

pub fn engine_url(tag: &str) -> String {
    format!(
        "https://github.com/ggml-org/llama.cpp/releases/download/{tag}/{}",
        engine_asset_name(tag)
    )
}

/// 钉版引擎资产（PINNED_ENGINE_TAG = b10299）的尺寸 + sha256
/// （来源：GitHub release asset digest）。非钉版 tag 由调用方按开发通道处理。
#[cfg(target_arch = "aarch64")]
pub fn pinned_engine_asset() -> Option<(u64, &'static str)> {
    Some((
        10_983_267,
        "5fb08d916e1f4056e9ca9bf82cdf7a8bdf8c410e6e83f14d054bf87828c6ce1e",
    ))
}

/// 钉版引擎资产（PINNED_ENGINE_TAG = b10299）的尺寸 + sha256
/// （来源：GitHub release asset digest）。非钉版 tag 由调用方按开发通道处理。
#[cfg(target_arch = "x86_64")]
pub fn pinned_engine_asset() -> Option<(u64, &'static str)> {
    Some((
        11_248_751,
        "8049f66b69bc89013cb79ac9768915a89ce2f1cabae92c02f931697d06af3a09",
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

/// GPU 失败提示按架构分流：arm64 包默认 Metal（`-ngl 99` 全量卸载），
/// 失败引导切 CPU；x86_64（Intel）包不保证 Metal（是否 GPU 卸载由引擎
/// 自身决定），只给通用提示、不暗示 Metal。
#[cfg(target_arch = "aarch64")]
pub fn gpu_error_hint() -> &'static str {
    "若 Metal 设备初始化失败，请在设置中切换到 CPU 设备"
}

#[cfg(target_arch = "x86_64")]
pub fn gpu_error_hint() -> &'static str {
    "若引擎初始化失败，请在设置中切换到 CPU 设备重试"
}
