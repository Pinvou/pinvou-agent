//! Feature-local cache custody while a process is alive. This is not an ACL,
//! administrator boundary, protected installation namespace or execution seal.
use crate::Error;
use serde::{Deserialize, Serialize};
use std::fs::File;
#[cfg(any(test, not(target_os = "windows")))]
use std::path::Path;

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Identity {
    volume: u32,
    index_high: u32,
    index_low: u32,
    created_high: u32,
    created_low: u32,
}
pub(crate) struct Directory {
    _handles: Vec<File>,
}

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub(crate) use windows::{directory, identity, read_identity};

#[cfg(not(target_os = "windows"))]
pub(crate) fn directory(_: &Path) -> Result<Directory, Error> {
    Err(Error::Unsupported)
}
#[cfg(not(target_os = "windows"))]
impl Directory {
    pub(crate) fn file(
        &self,
        _: impl AsRef<std::ffi::OsStr>,
        _: bool,
        _: bool,
        _: bool,
    ) -> Result<File, Error> {
        Err(Error::Unsupported)
    }
    pub(crate) fn create_dir(&self, _: &std::ffi::OsStr) -> Result<Directory, Error> {
        Err(Error::Unsupported)
    }
    pub(crate) fn publish(&self, _: &File, _: impl AsRef<std::ffi::OsStr>) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}
// Test convenience only. Production uses the already-retained actual parent,
// never a re-resolved path to open a job child.
#[cfg(test)]
pub(crate) fn file(path: &Path, write: bool, create: bool, exclusive: bool) -> Result<File, Error> {
    directory(path.parent().ok_or(Error::InvalidInput)?)?.file(
        path.file_name().ok_or(Error::InvalidInput)?,
        write,
        create,
        exclusive,
    )
}
#[cfg(not(target_os = "windows"))]
pub(crate) fn identity(_: &File) -> Result<Identity, Error> {
    Err(Error::Unsupported)
}
#[cfg(not(target_os = "windows"))]
fn read_identity(_: &File) -> Result<Identity, Error> {
    Err(Error::Unsupported)
}

pub(crate) fn read(file: &mut File, maximum: usize) -> Result<Vec<u8>, Error> {
    use std::io::{Read, Seek, SeekFrom};
    let before = read_identity(file)?;
    if file.metadata().map_err(|_| Error::IoUnavailable)?.len() > maximum as u64 {
        return Err(Error::InvalidInput);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| Error::IoUnavailable)?;
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::IoUnavailable)?;
    if bytes.len() > maximum || read_identity(file)? != before {
        return Err(Error::UnsafeObject);
    }
    Ok(bytes)
}

#[cfg(all(test, target_os = "windows"))]
mod windows_tests;

#[cfg(not(target_os = "windows"))]
#[test]
fn unsupported_adapter_does_not_touch_files() {
    assert!(matches!(
        directory(Path::new("unused")),
        Err(Error::Unsupported)
    ));
}
