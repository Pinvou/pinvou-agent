use std::fs::{File, OpenOptions};
use std::path::Path;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

#[cfg(not(any(unix, windows)))]
mod fallback;
#[cfg(not(any(unix, windows)))]
use fallback as imp;

pub(super) fn configure_private_open_options(options: &mut OpenOptions) {
    imp::configure_private_open_options(options);
}

pub(super) fn enforce_private_permissions(file: &File, path: &Path) -> std::io::Result<()> {
    imp::enforce_private_permissions(file, path)
}

pub(super) fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    imp::atomic_replace(source, target)
}

pub(super) fn sync_parent_directory(parent: &Path) -> std::io::Result<()> {
    imp::sync_parent_directory(parent)
}

pub(super) fn display_path(path: &Path) -> String {
    imp::display_path(path)
}

#[cfg(test)]
pub(super) fn private_file_is_restricted(path: &Path) -> std::io::Result<bool> {
    imp::private_file_is_restricted(path)
}
