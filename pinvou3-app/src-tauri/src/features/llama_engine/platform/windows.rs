//! Windows 适配：Vulkan 包（CPU + Vulkan 一体，官方 CI 打包 `build/bin/Release/*`
//! 全部内容，llama-server.exe 位于 zip 根目录），GPU 走 Vulkan（驱动自带）。

use std::path::Path;

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

/// 钉版引擎资产（PINNED_ENGINE_TAG = b10299）的尺寸 + sha256
/// （来源：GitHub release asset digest）。非钉版 tag 由调用方按开发通道处理。
pub fn pinned_engine_asset() -> Option<(u64, &'static str)> {
    Some((
        34_108_404,
        "c5cdc4f4394f8cc828c6dae2dbc602b84a7c81674ca97d032518cff74cf36e1c",
    ))
}

pub fn engine_archive_is_zip() -> bool {
    true
}

/// 目录所在卷的可用字节数（GetDiskFreeSpaceExW）；查询失败返回 None。
pub fn available_disk_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free_bytes: u64 = 0;
    if unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_bytes,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return None;
    }
    Some(free_bytes)
}

pub fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

pub fn gpu_error_hint() -> &'static str {
    "若显卡缺少 Vulkan 驱动，请在设置中切换到 CPU 设备"
}
