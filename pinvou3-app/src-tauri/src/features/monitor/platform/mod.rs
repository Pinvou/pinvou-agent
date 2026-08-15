//! 平台采样器适配层。
//!
//! **CPU/GPU 非对称设计**（Wave 3 源码回查记录）：
//!
//! | 平台 | CPU | GPU | 内存 |
//! |------|-----|-----|------|
//! | Windows | ✅ PDH 性能计数器 | ✅ 性能计数器 | ✅ |
//! | macOS | ❌ 返回 None | ✅ `ioreg` IOAccelerator | ✅ |
//! | Linux | ❌ 返回 None | ❌ 仅 `nvidia-smi` 回退 | ✅ |
//!
//! macOS 和 Linux 的 CPU 采样、Linux 的平台级 GPU 采样当前**有意返回 None**——这
//! 不是 bug，而是尚未实现平台专属采集器。任何采样失败均 graceful degrade（返回
//! None / OFFLINE），不影响应用功能。新增平台实现属于 feature 开发，非重构范畴。
//!
//! macOS GPU 通过 `ioreg -r -c IOAccelerator` 解析 Metal 设备信息；Linux GPU 仅依赖
//! 跨平台的 `nvidia-smi` 探针（见 `super::nvidia_gpu_snapshot`），无 Linux 专属实现。

#[cfg(target_os = "linux")]
mod linux_cpu;
#[cfg(target_os = "linux")]
mod linux_memory;
#[cfg(target_os = "macos")]
mod macos_cpu;
#[cfg(target_os = "macos")]
mod macos_gpu;
#[cfg(target_os = "macos")]
mod macos_memory;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows_cpu;
#[cfg(target_os = "windows")]
mod windows_gpu;
#[cfg(target_os = "windows")]
mod windows_memory;

#[cfg(target_os = "linux")]
pub use linux_cpu::cpu_snapshot;
#[cfg(target_os = "linux")]
pub use linux_memory::ram_snapshot;
#[cfg(target_os = "macos")]
pub use macos_cpu::cpu_snapshot;
#[cfg(target_os = "macos")]
pub use macos_memory::ram_snapshot;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub use unsupported::{cpu_snapshot, ram_snapshot};
#[cfg(target_os = "windows")]
pub use windows_cpu::cpu_snapshot;
#[cfg(target_os = "windows")]
pub use windows_gpu::gpu_snapshot;
#[cfg(target_os = "windows")]
pub use windows_memory::ram_snapshot;

#[cfg(target_os = "macos")]
pub use macos_gpu::gpu_snapshot;
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn gpu_snapshot() -> Option<super::GpuSnapshot> {
    None
}
