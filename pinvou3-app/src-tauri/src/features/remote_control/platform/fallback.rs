use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

pub(super) fn configure_private_open_options(_options: &mut OpenOptions) {}

pub(super) fn enforce_private_permissions(_file: &File, _path: &Path) -> std::io::Result<()> {
    Ok(())
}

pub(super) fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(source, target)
}

pub(super) fn sync_parent_directory(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

pub(super) fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(super) fn host_file_roots() -> Vec<(String, PathBuf)> {
    Vec::new()
}

#[cfg(test)]
pub(super) fn private_file_is_restricted(path: &Path) -> std::io::Result<bool> {
    Ok(path.is_file())
}
