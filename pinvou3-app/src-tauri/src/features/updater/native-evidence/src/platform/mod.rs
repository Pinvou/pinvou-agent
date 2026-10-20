#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::ConsoleCancellation;
#[cfg(target_os = "windows")]
pub(crate) use windows::{inspect, lock};
#[cfg(not(target_os = "windows"))]
mod unsupported;
#[cfg(not(target_os = "windows"))]
pub use unsupported::ConsoleCancellation;
#[cfg(not(target_os = "windows"))]
pub(crate) use unsupported::{inspect, lock};
