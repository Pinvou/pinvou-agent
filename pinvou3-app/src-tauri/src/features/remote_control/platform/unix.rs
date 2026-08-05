use std::fs::{File, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

pub(super) fn configure_private_open_options(options: &mut OpenOptions) {
    options.mode(0o600);
}

pub(super) fn enforce_private_permissions(file: &File, _path: &Path) -> std::io::Result<()> {
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
}

pub(super) fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(source, target)
}

pub(super) fn sync_parent_directory(parent: &Path) -> std::io::Result<()> {
    File::open(parent)?.sync_all()
}

pub(super) fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(super) fn host_file_roots() -> Vec<(String, PathBuf)> {
    Vec::new()
}

#[cfg(test)]
pub(super) fn private_file_is_restricted(path: &Path) -> std::io::Result<bool> {
    Ok(std::fs::metadata(path)?.permissions().mode() & 0o777 == 0o600)
}
