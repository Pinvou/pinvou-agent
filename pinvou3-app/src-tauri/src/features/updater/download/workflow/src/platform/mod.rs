//! Compatibility adapter: the reusable namespace has no download semantics.
use crate::Error;
pub(crate) use pinvou_upgrade_custody::Identity;
use std::{ffi::OsStr, fs::File, path::Path};
pub(crate) struct Directory(pinvou_upgrade_custody::Directory);
impl From<pinvou_upgrade_custody::Error> for Error {
    fn from(error: pinvou_upgrade_custody::Error) -> Self {
        use pinvou_upgrade_custody::Error as E;
        match error {
            E::Unsupported => Self::Unsupported,
            E::InvalidInput => Self::InvalidInput,
            E::NotFound => Self::NotFound,
            E::Busy => Self::Busy,
            E::AlreadyExists => Self::AlreadyExists,
            E::IoUnavailable => Self::IoUnavailable,
            E::UnsafeObject => Self::UnsafeObject,
        }
    }
}
pub(crate) fn directory(path: &Path) -> Result<Directory, Error> {
    pinvou_upgrade_custody::directory(path)
        .map(Directory)
        .map_err(Into::into)
}
pub(crate) fn identity(file: &File) -> Result<Identity, Error> {
    pinvou_upgrade_custody::identity(file).map_err(Into::into)
}
pub(crate) fn read(file: &mut File, maximum: usize) -> Result<Vec<u8>, Error> {
    pinvou_upgrade_custody::read(file, maximum).map_err(Into::into)
}
impl Directory {
    pub(crate) fn file(
        &self,
        name: impl AsRef<OsStr>,
        write: bool,
        create: bool,
        exclusive: bool,
    ) -> Result<File, Error> {
        self.0
            .file(name, write, create, exclusive)
            .map_err(Into::into)
    }
    pub(crate) fn create_dir(&self, name: &OsStr) -> Result<Self, Error> {
        self.0.create_dir(name).map(Self).map_err(Into::into)
    }
    pub(crate) fn publish(&self, file: &File, name: impl AsRef<OsStr>) -> Result<(), Error> {
        self.0.publish(file, name).map_err(Into::into)
    }
}
#[cfg(test)]
pub(crate) fn file(path: &Path, write: bool, create: bool, exclusive: bool) -> Result<File, Error> {
    directory(path.parent().ok_or(Error::InvalidInput)?)?.file(
        path.file_name().ok_or(Error::InvalidInput)?,
        write,
        create,
        exclusive,
    )
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

#[cfg(all(test, target_os = "windows"))]
mod windows_control_tests;

#[cfg(all(test, target_os = "windows"))]
mod windows_resume_tests;
