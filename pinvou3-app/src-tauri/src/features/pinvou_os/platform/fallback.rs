use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

pub(super) fn configure_private_ledger(_options: &mut OpenOptions) {}

pub(super) fn harden_private_runtime_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub(super) fn harden_private_ledger(_file: &File) -> io::Result<()> {
    Ok(())
}
