use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as implementation;

#[cfg(not(unix))]
mod fallback;
#[cfg(not(unix))]
use fallback as implementation;

pub(super) fn configure_private_ledger(options: &mut OpenOptions) {
    implementation::configure_private_ledger(options);
}

pub(super) fn harden_private_runtime_dir(path: &Path) -> io::Result<()> {
    implementation::harden_private_runtime_dir(path)
}

pub(super) fn harden_private_ledger(file: &File) -> io::Result<()> {
    implementation::harden_private_ledger(file)
}
