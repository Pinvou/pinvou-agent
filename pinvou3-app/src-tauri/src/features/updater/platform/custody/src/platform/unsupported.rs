use crate::{Directory, Error, Identity};
use std::{ffi::OsStr, fs::File, path::Path};
pub fn directory(_: &Path) -> Result<Directory, Error> {
    Err(Error::Unsupported)
}
pub fn identity(_: &File) -> Result<Identity, Error> {
    Err(Error::Unsupported)
}
pub fn read_identity(_: &File) -> Result<Identity, Error> {
    Err(Error::Unsupported)
}
impl Directory {
    pub fn file(&self, _: impl AsRef<OsStr>, _: bool, _: bool, _: bool) -> Result<File, Error> {
        Err(Error::Unsupported)
    }
    pub fn create_dir(&self, _: &OsStr) -> Result<Self, Error> {
        Err(Error::Unsupported)
    }
    pub fn publish(&self, _: &File, _: impl AsRef<OsStr>) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}
#[test]
fn unavailable_is_explicit() {
    assert!(matches!(
        directory(Path::new("unused")),
        Err(Error::Unsupported)
    ));
}

/// Ordinary creation descriptor. Unsupported adapters never touch the namespace.
pub struct CreationSecurity;
impl CreationSecurity {
    pub fn from_sddl(_: &str) -> Result<Self, Error> {
        Err(Error::Unsupported)
    }
}
impl Directory {
    pub fn create_dir_with_security(&self, _: &OsStr, _: &CreationSecurity) -> Result<Self, Error> {
        Err(Error::Unsupported)
    }
    pub fn create_file_with_security(
        &self,
        _: &OsStr,
        _: &CreationSecurity,
    ) -> Result<File, Error> {
        Err(Error::Unsupported)
    }
}

impl Directory {
    pub fn open_dir(&self, _: &OsStr) -> Result<Self, Error> {
        Err(Error::Unsupported)
    }
}

impl Directory {
    pub fn visit_entries(
        &self,
        _visitor: impl FnMut(crate::DirectoryEntry) -> Result<(), Error>,
    ) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}

impl Directory {
    pub fn remove_file_exact(&self, _component: &OsStr, _expected: &Identity) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}

impl Directory {
    pub fn directory_identity(&self) -> Result<Identity, Error> {
        Err(Error::Unsupported)
    }
    pub fn remove_empty_dir_exact(
        &self,
        _component: &OsStr,
        _expected: &Identity,
    ) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}
