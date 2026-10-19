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
