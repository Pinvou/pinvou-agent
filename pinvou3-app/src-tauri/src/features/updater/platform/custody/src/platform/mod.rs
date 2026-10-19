//! Native adapters remain updater-local; unsupported hosts touch no files.
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::{directory, identity, read_identity};
#[cfg(not(target_os = "windows"))]
mod unsupported;
#[cfg(not(target_os = "windows"))]
pub use unsupported::{directory, identity, read_identity};
