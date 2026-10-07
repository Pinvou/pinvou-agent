//! Cross-platform file identity and link/reparse-point detection shared by the
//! GAIA dataset verifier ([`crate::dataset`]) and snapshot fetcher
//! ([`crate::fetch`]). Each check keeps its platform gating: the Windows
//! variants detect symlink and reparse-point (junction) attributes, and
//! [`path_identity`] refuses invalid file indexes instead of reusing another
//! platform's implementation.

#[cfg(unix)]
use std::fs;
use std::fs::Metadata;
use std::path::Path;

#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    pub(crate) volume: u32,
    pub(crate) index: u64,
}

#[cfg(not(any(unix, windows)))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity;

impl FileIdentity {
    #[cfg(unix)]
    pub(crate) fn is_valid(self) -> bool {
        self.inode != 0
    }

    #[cfg(windows)]
    pub(crate) fn is_valid(self) -> bool {
        self.index != 0
    }

    #[cfg(not(any(unix, windows)))]
    pub(crate) fn is_valid(self) -> bool {
        true
    }
}

#[cfg(unix)]
pub(crate) fn path_identity(path: &Path) -> Result<FileIdentity, ()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(windows)]
pub(crate) fn path_identity(path: &Path) -> Result<FileIdentity, ()> {
    use std::mem::MaybeUninit;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING,
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(());
    }
    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let succeeded = unsafe { GetFileInformationByHandle(handle, information.as_mut_ptr()) };
    unsafe { CloseHandle(handle) };
    if succeeded == 0 {
        return Err(());
    }
    let information = unsafe { information.assume_init() };
    let identity = FileIdentity {
        volume: information.dwVolumeSerialNumber,
        index: (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    };
    if identity.index == 0 {
        return Err(());
    }
    Ok(identity)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn path_identity(_path: &Path) -> Result<FileIdentity, ()> {
    Ok(FileIdentity)
}

#[cfg(unix)]
pub(crate) fn is_link_or_reparse(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
pub(crate) fn is_link_or_reparse(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.file_type().is_symlink()
        || windows_attributes_indicate_reparse(metadata.file_attributes())
}

#[cfg(windows)]
pub(crate) fn windows_attributes_indicate_reparse(attributes: u32) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn is_link_or_reparse(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}
