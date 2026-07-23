#[cfg(target_os = "linux")]
mod linux_memory;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows_cpu;
#[cfg(target_os = "windows")]
mod windows_gpu;
#[cfg(target_os = "windows")]
mod windows_memory;

#[cfg(target_os = "linux")]
pub fn cpu_snapshot() -> Option<super::CpuSnapshot> {
    None
}

#[cfg(target_os = "linux")]
pub use linux_memory::ram_snapshot;
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub use unsupported::{cpu_snapshot, ram_snapshot};
#[cfg(target_os = "windows")]
pub use windows_cpu::cpu_snapshot;
#[cfg(target_os = "windows")]
pub use windows_gpu::gpu_snapshot;
#[cfg(target_os = "windows")]
pub use windows_memory::ram_snapshot;

#[cfg(not(target_os = "windows"))]
pub fn gpu_snapshot() -> Option<super::GpuSnapshot> {
    None
}
