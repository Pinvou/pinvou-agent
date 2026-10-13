//! Windows share-mode opening and same-handle observations only. Parent directory
//! substitution, ACL and privileged callers remain external owner responsibilities.
use crate::CacheObjectError;
use std::{
    fs::{File, OpenOptions},
    os::windows::{
        ffi::OsStrExt,
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Component, Path, Prefix},
};
use windows_sys::Win32::Foundation::{DuplicateHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, GetFileInformationByHandle,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

pub(crate) fn create(path: &Path) -> Result<File, CacheObjectError> {
    validate_path(path)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                CacheObjectError::AlreadyExists
            } else {
                CacheObjectError::IoUnavailable
            }
        })?;
    validate(&file)?;
    Ok(file)
}
// create_new can create an ADS on an existing NTFS file. Reject stream syntax
// before any OS call; only a parsed disk/verbatim-disk prefix may contain ':'.
fn validate_path(path: &Path) -> Result<(), CacheObjectError> {
    for component in path.components() {
        if matches!(component, Component::Prefix(prefix)
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        {
            continue;
        }
        if component
            .as_os_str()
            .encode_wide()
            .any(|unit| unit == u16::from(b':'))
        {
            return Err(CacheObjectError::InvalidPath);
        }
    }
    Ok(())
}

// This single FFI observation borrows an existing valid File handle and writes
// only to a local initialized Windows information struct. No path is reopened.
#[allow(unsafe_code)]
fn observe(file: &File) -> Result<BY_HANDLE_FILE_INFORMATION, CacheObjectError> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: file owns a live handle for the whole call; information has the exact
    // API struct type and a writable lifetime. The API does not retain either.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(CacheObjectError::IoUnavailable);
    }
    Ok(information)
}
fn validate(file: &File) -> Result<(), CacheObjectError> {
    let information = observe(file)?;
    if information.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || information.nNumberOfLinks != 1
        || information.nFileSizeHigh != 0
        || information.nFileSizeLow != 0
    {
        return Err(CacheObjectError::InvalidObject);
    }
    // Link count/empty size are observed now, not perpetual namespace guarantees.
    Ok(())
}

// Only the new handle is owned by the returned File. No SAME_ACCESS or CLOSE_SOURCE
// option is used, and the pseudo process handle is not owned or closed here.
#[allow(unsafe_code)]
pub(crate) fn duplicate_read(source: &File) -> Result<File, CacheObjectError> {
    let mut duplicated = std::ptr::null_mut();
    // SAFETY: source is borrowed/live throughout. Both process arguments are the
    // current-process pseudo handle; the output is writable local storage. Access
    // is exactly FILE_GENERIC_READ, non-inheritable, with flags zero.
    let success = unsafe {
        let process = GetCurrentProcess();
        DuplicateHandle(
            process,
            source.as_raw_handle(),
            process,
            &mut duplicated,
            FILE_GENERIC_READ,
            0,
            0,
        )
    };
    if success == 0 || duplicated.is_null() || duplicated == INVALID_HANDLE_VALUE {
        return Err(CacheObjectError::IoUnavailable);
    }
    // SAFETY: successful DuplicateHandle yielded one new valid owned handle; only
    // this File takes ownership, exactly once. No existing handle is closed.
    Ok(unsafe { File::from_raw_handle(duplicated) })
}

#[cfg(test)]
mod tests;
