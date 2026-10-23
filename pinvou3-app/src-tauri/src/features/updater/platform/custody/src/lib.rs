//! Updater-local, process-lifetime filesystem custody. Not an ACL boundary,
//! anti-rollback store, trusted owner, execution seal or installation permission.
use serde::{Deserialize, Serialize};
use std::fs::File;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    InvalidInput,
    NotFound,
    Busy,
    AlreadyExists,
    IoUnavailable,
    UnsafeObject,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "CUSTODY_UNSUPPORTED",
            Self::InvalidInput => "CUSTODY_INPUT_INVALID",
            Self::NotFound => "CUSTODY_NOT_FOUND",
            Self::Busy => "CUSTODY_BUSY",
            Self::AlreadyExists => "CUSTODY_ALREADY_EXISTS",
            Self::IoUnavailable => "CUSTODY_IO_UNAVAILABLE",
            Self::UnsafeObject => "CUSTODY_OBJECT_REFUSED",
        })
    }
}
impl std::error::Error for Error {}

/// Same serialized identity profile used by the original download job. This is
/// an observed file identity, not an authenticated publisher or owner token.
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Identity {
    volume: u32,
    index_high: u32,
    index_low: u32,
    created_high: u32,
    created_low: u32,
}
/// One observed ordinary directory member. It attests no owner or host policy.
/// Reparse objects are refused by the native visitor rather than followed.
pub struct DirectoryEntry {
    pub name: String,
    pub directory: bool,
}

pub struct Directory {
    _handles: Vec<File>,
}
mod platform;
pub use platform::{CreationSecurity, directory, identity, read_identity};

impl Directory {
    /// Borrow the ACTUAL retained ancestry for caller-specific inspection. This
    /// ordinary handle view attests no owner/DACL policy or protected capability.
    pub fn retained_directories(&self) -> impl ExactSizeIterator<Item = &File> {
        self._handles.iter()
    }
}

pub fn read(file: &mut File, maximum: usize) -> Result<Vec<u8>, Error> {
    use std::io::{Read, Seek, SeekFrom};
    if maximum == 0 || maximum == usize::MAX {
        return Err(Error::InvalidInput);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_keeps_original_download_job_json_profile() {
        let original =
            r#"{"volume":7,"indexHigh":9,"indexLow":11,"createdHigh":13,"createdLow":15}"#;
        let identity: Identity = serde_json::from_str(original).unwrap();
        assert_eq!(serde_json::to_string(&identity).unwrap(), original);
        for bad in [
            r#"{"Volume":7,"indexHigh":9,"indexLow":11,"createdHigh":13,"createdLow":15}"#,
            r#"{"volume":7,"volume":8,"indexHigh":9,"indexLow":11,"createdHigh":13,"createdLow":15}"#,
            r#"{"volume":7,"indexHigh":9,"indexLow":11,"createdHigh":13,"createdLow":15,"path":"other"}"#,
        ] {
            assert!(serde_json::from_str::<Identity>(bad).is_err());
        }
    }
}
