#[cfg(not(windows))]
mod standard;
#[cfg(windows)]
mod windows;

#[cfg(not(windows))]
pub(super) use standard::*;
#[cfg(windows)]
pub(super) use windows::*;
