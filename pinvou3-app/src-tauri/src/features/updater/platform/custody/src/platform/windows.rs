//! Actual directory-handle namespace: paths are used only for the drive root.
//! Every later component/create/open/link is relative to a retained real handle.
//! Sharing alone does NOT block FILE_WRITE_ATTRIBUTES reparse mutation. A changed
//! parent must fail closed, and a leaf reparse object is inspected, never followed.
use crate::Error;
use crate::{Directory, Identity};
use std::{
    ffi::OsStr,
    fs::{File, OpenOptions},
    mem::{offset_of, size_of},
    os::windows::{
        ffi::OsStrExt,
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FILE_CREATE, FILE_DIRECTORY_FILE, FILE_LINK_INFORMATION, FILE_NON_DIRECTORY_FILE,
            FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, FileLinkInformation,
            NtCreateFile, NtSetInformationFile,
        },
    },
    Win32::{
        Foundation::{
            GENERIC_READ, GENERIC_WRITE, HANDLE, OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE,
            RtlNtStatusToDosError, STATUS_FILE_IS_A_DIRECTORY, STATUS_NOT_A_DIRECTORY,
            UNICODE_STRING,
        },
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, SYNCHRONIZE,
        },
        System::IO::IO_STATUS_BLOCK,
    },
};

fn os_error(code: u32) -> Error {
    match code {
        2 | 3 => Error::NotFound,
        32 | 33 => Error::Busy,
        80 | 183 => Error::AlreadyExists,
        // Includes an opened parent changed into a junction (STATUS_REPARSE).
        1920 | 1921 | 4390..=4395 => Error::UnsafeObject,
        _ => Error::IoUnavailable,
    }
}
#[allow(unsafe_code)]
fn status_error(status: i32) -> Error {
    // Type mismatches are object refusal, not IO uncertainty. In particular a
    // directory junction opened with NON_DIRECTORY is never followed as a file.
    if matches!(status, STATUS_FILE_IS_A_DIRECTORY | STATUS_NOT_A_DIRECTORY) {
        return Error::UnsafeObject;
    }
    // SAFETY: pure Windows status translation; no borrowed memory is retained.
    os_error(unsafe { RtlNtStatusToDosError(status) })
}
#[allow(unsafe_code)]
fn information(file: &File) -> Result<BY_HANDLE_FILE_INFORMATION, Error> {
    let mut value = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: File owns the live handle; this initialized exact SDK struct is
    // writable for the synchronous, nonretaining kernel call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut value) } == 0 {
        return Err(Error::IoUnavailable);
    }
    Ok(value)
}
fn observe_identity(file: &File, maximum_links: u32) -> Result<Identity, Error> {
    let value = information(file)?;
    if value.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || value.nNumberOfLinks == 0
        || value.nNumberOfLinks > maximum_links
    {
        return Err(Error::UnsafeObject);
    }
    Ok(Identity {
        volume: value.dwVolumeSerialNumber,
        index_high: value.nFileIndexHigh,
        index_low: value.nFileIndexLow,
        created_high: value.ftCreationTime.dwHighDateTime,
        created_low: value.ftCreationTime.dwLowDateTime,
    })
}
pub fn identity(file: &File) -> Result<Identity, Error> {
    observe_identity(file, 1)
}
pub fn read_identity(file: &File) -> Result<Identity, Error> {
    observe_identity(file, 2)
}
fn check_directory(file: &File) -> Result<(), Error> {
    let value = information(file)?;
    if value.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        || value.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(Error::UnsafeObject);
    }
    Ok(())
}
fn name(value: &OsStr) -> Result<Vec<u16>, Error> {
    let text = value.to_str().ok_or(Error::InvalidInput)?;
    if text.is_empty()
        || text == "."
        || text == ".."
        || text.ends_with(['.', ' '])
        || text.contains(['/', '\\', ':', '\0'])
    {
        return Err(Error::InvalidInput);
    }
    let wide: Vec<u16> = value.encode_wide().collect();
    if wide.len() > u16::MAX as usize / 2 {
        return Err(Error::InvalidInput);
    }
    Ok(wide)
}

struct OpenMode {
    directory: bool,
    write: bool,
    create: bool,
    exclusive: bool,
}
#[allow(unsafe_code)]
fn relative_with_security(
    parent: &File,
    component: &OsStr,
    mode: OpenMode,
    security: Option<&CreationSecurity>,
) -> Result<File, Error> {
    let wide = name(component)?;
    native_relative(parent, wide, mode, security)
}

#[allow(unsafe_code)]
fn native_relative(
    parent: &File,
    wide: Vec<u16>,
    mode: OpenMode,
    security: Option<&CreationSecurity>,
) -> Result<File, Error> {
    native_relative_access(parent, wide, mode, security, false)
}
#[allow(unsafe_code)]
fn native_relative_access(
    parent: &File,
    mut wide: Vec<u16>,
    mode: OpenMode,
    security: Option<&CreationSecurity>,
    remove: bool,
) -> Result<File, Error> {
    let string = UNICODE_STRING {
        Length: (wide.len() * 2) as u16,
        MaximumLength: (wide.len() * 2) as u16,
        Buffer: wide.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &string,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: security
            .map_or(std::ptr::null(), |s| s.pointer.as_ptr().cast_const().cast()),
        SecurityQualityOfService: std::ptr::null_mut(),
    };
    let mut handle: HANDLE = std::ptr::null_mut();
    let mut io = IO_STATUS_BLOCK::default();
    // Same read/write access as ordinary file IO, plus SYNCHRONIZE for the
    // synchronous native handle. Link publication needs no DELETE permission.
    // Callers select read/write IO explicitly; an existing file is never truncated.
    let access = GENERIC_READ
        | SYNCHRONIZE
        | if mode.write { GENERIC_WRITE } else { 0 }
        | if remove { 0x00010000 } else { 0 };
    let options = FILE_OPEN_REPARSE_POINT
        | FILE_SYNCHRONOUS_IO_NONALERT
        | if mode.directory {
            FILE_DIRECTORY_FILE
        } else {
            FILE_NON_DIRECTORY_FILE
        };
    // SAFETY: exact SDK structures, valid live parent, and a single checked
    // non-NUL component, or the internal fixed empty self-directory name.
    // All referenced buffers outlive this synchronous call.
    // A successful handle is immediately adopted once by File; failure does
    // not leave a Rust-owned handle behind. No absolute name may escape root.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &attributes,
            &mut io,
            std::ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            if mode.exclusive {
                0
            } else if mode.directory {
                FILE_SHARE_READ | FILE_SHARE_WRITE
            } else {
                FILE_SHARE_READ
            },
            if mode.create { FILE_CREATE } else { FILE_OPEN },
            options,
            std::ptr::null(),
            0,
        )
    };
    if status < 0 {
        return Err(status_error(status));
    }
    if handle.is_null() {
        return Err(Error::IoUnavailable);
    }
    // SAFETY: successful NtCreateFile transferred this new owned kernel handle.
    let file = unsafe { File::from_raw_handle(handle) };
    if mode.directory {
        check_directory(&file)?;
    } else {
        observe_identity(&file, if mode.exclusive { 1 } else { 2 })?;
    }
    Ok(file)
}

fn relative(parent: &File, component: &OsStr, mode: OpenMode) -> Result<File, Error> {
    relative_with_security(parent, component, mode, None)
}

/// Native validated creation SD, with no claim about what policy it grants.
/// It may describe an entirely unprotected directory. The caller must inspect
/// the actual created object under its own policy before trusting that object.
pub struct CreationSecurity {
    pointer: std::ptr::NonNull<std::ffi::c_void>,
}
impl CreationSecurity {
    #[allow(unsafe_code)]
    pub fn from_sddl(sddl: &str) -> Result<Self, Error> {
        use windows_sys::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
        };
        if sddl.is_empty() || sddl.len() > 4096 || sddl.contains('\0') {
            return Err(Error::InvalidInput);
        }
        let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut pointer = std::ptr::null_mut();
        let mut size = 0u32;
        // SAFETY: SDK validates the bounded NUL-terminated SDDL and returns a
        // newly LocalAlloc-owned descriptor. No untrusted raw pointer enters.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut pointer,
                &mut size,
            )
        } == 0
        {
            return Err(Error::InvalidInput);
        }
        let value = Self {
            pointer: std::ptr::NonNull::new(pointer).ok_or(Error::IoUnavailable)?,
        };
        if size == 0 || size > 65536 {
            return Err(Error::InvalidInput);
        }
        Ok(value)
    }
}
impl Drop for CreationSecurity {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: the descriptor came from the SDK LocalAlloc factory and has
        // one owner; synchronous create calls borrow it only for their call.
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(self.pointer.as_ptr());
        }
    }
}

pub fn directory(path: &Path) -> Result<Directory, Error> {
    let absolute = std::path::absolute(path).map_err(|_| Error::InvalidInput)?;
    let mut components = absolute.components();
    let prefix = components.next().ok_or(Error::InvalidInput)?;
    if !matches!(prefix, Component::Prefix(p) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        || !matches!(components.next(), Some(Component::RootDir))
    {
        return Err(Error::UnsafeObject);
    }
    let mut drive = PathBuf::from(prefix.as_os_str());
    drive.push(std::path::MAIN_SEPARATOR_STR);
    // Only the drive root uses a Win32 path. Later names are all relative to
    // actual captured handles, so DOS path replacement cannot redirect them.
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(drive)
        .map_err(|_| Error::IoUnavailable)?;
    check_directory(&file)?;
    let mut handles = vec![file];
    for component in components {
        let Component::Normal(component) = component else {
            return Err(Error::UnsafeObject);
        };
        let file = relative(
            handles.last().ok_or(Error::UnsafeObject)?,
            component,
            OpenMode {
                directory: true,
                write: false,
                create: false,
                exclusive: false,
            },
        )?;
        handles.push(file);
    }
    Ok(Directory { _handles: handles })
}

impl Directory {
    fn root(&self) -> Result<&File, Error> {
        self._handles.last().ok_or(Error::UnsafeObject)
    }
    pub fn file(
        &self,
        component: impl AsRef<OsStr>,
        write: bool,
        create: bool,
        exclusive: bool,
    ) -> Result<File, Error> {
        relative(
            self.root()?,
            component.as_ref(),
            OpenMode {
                directory: false,
                write,
                create,
                exclusive,
            },
        )
    }
    pub fn create_dir(&self, component: &OsStr) -> Result<Directory, Error> {
        let child = relative(
            self.root()?,
            component,
            OpenMode {
                directory: true,
                write: false,
                create: true,
                exclusive: false,
            },
        )?;
        let mut handles: Vec<File> = self
            ._handles
            .iter()
            .map(File::try_clone)
            .collect::<Result<_, _>>()
            .map_err(|_| Error::IoUnavailable)?;
        handles.push(child);
        Ok(Directory { _handles: handles })
    }
    #[allow(unsafe_code)]
    pub fn publish(&self, source: &File, component: impl AsRef<OsStr>) -> Result<(), Error> {
        let wide = name(component.as_ref())?;
        let bytes = offset_of!(FILE_LINK_INFORMATION, FileName) + wide.len() * size_of::<u16>();
        // Word-backed storage supplies SDK structure alignment and initialized
        // padding, including the variable UTF-16 tail. No replacement flag.
        let mut storage = vec![
            0usize;
            bytes
                .max(size_of::<FILE_LINK_INFORMATION>())
                .div_ceil(size_of::<usize>())
        ];
        let info = storage.as_mut_ptr().cast::<FILE_LINK_INFORMATION>();
        // SAFETY: aligned initialized storage above covers both the exact SDK
        // header and checked tail. Source and destination parent remain live;
        // the kernel receives one fixed relative name and no process path.
        unsafe {
            (*info).RootDirectory = self.root()?.as_raw_handle();
            (*info).FileNameLength = (wide.len() * 2) as u32;
            std::ptr::copy_nonoverlapping(
                wide.as_ptr(),
                std::ptr::addr_of_mut!((*info).FileName).cast(),
                wide.len(),
            );
        }
        let mut io = IO_STATUS_BLOCK::default();
        // SAFETY: synchronous nonretaining call with the live handles and
        // initialized variable-length buffer described above.
        let status = unsafe {
            NtSetInformationFile(
                source.as_raw_handle(),
                &mut io,
                info.cast(),
                bytes as u32,
                FileLinkInformation,
            )
        };
        if status < 0 {
            return Err(status_error(status));
        }
        Ok(())
    }
}

impl Directory {
    /// Atomic creation SD, not a post-create ACL repair or a protected-host token.
    pub fn create_dir_with_security(
        &self,
        component: &OsStr,
        security: &CreationSecurity,
    ) -> Result<Self, Error> {
        let child = relative_with_security(
            self.root()?,
            component,
            OpenMode {
                directory: true,
                write: false,
                create: true,
                exclusive: false,
            },
            Some(security),
        )?;
        let mut handles = self
            ._handles
            .iter()
            .map(File::try_clone)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| Error::IoUnavailable)?;
        handles.push(child);
        Ok(Self { _handles: handles })
    }
    /// Fresh exclusive writer with the supplied creation SD. Existing objects
    /// are never opened/truncated/re-ACL'd through this method.
    pub fn create_file_with_security(
        &self,
        component: &OsStr,
        security: &CreationSecurity,
    ) -> Result<File, Error> {
        relative_with_security(
            self.root()?,
            component,
            OpenMode {
                directory: false,
                write: true,
                create: true,
                exclusive: true,
            },
            Some(security),
        )
    }
}

impl Directory {
    /// Open one ordinary existing child relative to this retained parent.
    pub fn open_dir(&self, component: &OsStr) -> Result<Self, Error> {
        let child = relative(
            self.root()?,
            component,
            OpenMode {
                directory: true,
                write: false,
                create: false,
                exclusive: false,
            },
        )?;
        let mut handles = self
            ._handles
            .iter()
            .map(File::try_clone)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| Error::IoUnavailable)?;
        handles.push(child);
        Ok(Self { _handles: handles })
    }
}

impl Directory {
    /// Independent actual directory cursor, fixed 64KiB native pages. No whole
    /// directory Vec and no pathname reopen. Duplicate callers do not share
    /// their enumeration position. Callback errors stop at that actual entry.
    #[allow(unsafe_code)]
    pub fn visit_entries(
        &self,
        mut visitor: impl FnMut(crate::DirectoryEntry) -> Result<(), Error>,
    ) -> Result<(), Error> {
        use windows_sys::{
            Wdk::Storage::FileSystem::{
                FILE_DIRECTORY_INFORMATION, FileDirectoryInformation, NtQueryDirectoryFile,
            },
            Win32::Foundation::STATUS_NO_MORE_FILES,
        };
        // An empty native relative name opens the retained directory itself as
        // a new file object. ReOpenFile does not support directory handles.
        // This fixed internal name never enters the public component API and
        // cannot select a parent, sibling, or DOS pathname.
        let original = information(self.root()?)?;
        let cursor = native_relative(
            self.root()?,
            Vec::new(),
            OpenMode {
                directory: true,
                write: false,
                create: false,
                exclusive: false,
            },
            None,
        )?;
        let opened = information(&cursor)?;
        if original.dwVolumeSerialNumber != opened.dwVolumeSerialNumber
            || original.nFileIndexHigh != opened.nFileIndexHigh
            || original.nFileIndexLow != opened.nFileIndexLow
            || original.ftCreationTime.dwHighDateTime != opened.ftCreationTime.dwHighDateTime
            || original.ftCreationTime.dwLowDateTime != opened.ftCreationTime.dwLowDateTime
        {
            return Err(Error::UnsafeObject);
        }
        let mut storage = vec![0usize; 65536 / size_of::<usize>()];
        let mut restart = true;
        loop {
            let mut io = IO_STATUS_BLOCK::default();
            let status = unsafe {
                NtQueryDirectoryFile(
                    cursor.as_raw_handle(),
                    std::ptr::null_mut(),
                    None,
                    std::ptr::null(),
                    &mut io,
                    storage.as_mut_ptr().cast(),
                    65536,
                    FileDirectoryInformation,
                    false,
                    std::ptr::null(),
                    restart,
                )
            };
            restart = false;
            if status == STATUS_NO_MORE_FILES {
                break;
            }
            if status < 0 {
                return Err(status_error(status));
            }
            let used = io.Information;
            if used == 0 || used > 65536 {
                return Err(Error::UnsafeObject);
            }
            let header = offset_of!(FILE_DIRECTORY_INFORMATION, FileName);
            let mut offset = 0usize;
            loop {
                if offset.checked_add(header).is_none_or(|end| end > used) {
                    return Err(Error::UnsafeObject);
                }
                let entry = unsafe {
                    storage
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset)
                        .cast::<FILE_DIRECTORY_INFORMATION>()
                };
                // Read only documented fixed fields. The SDK struct's trailing
                // FileName[1]/padding need not fit a final short native record.
                let next = unsafe {
                    std::ptr::read_unaligned(std::ptr::addr_of!((*entry).NextEntryOffset))
                } as usize;
                let attrs = unsafe {
                    std::ptr::read_unaligned(std::ptr::addr_of!((*entry).FileAttributes))
                };
                let bytes = unsafe {
                    std::ptr::read_unaligned(std::ptr::addr_of!((*entry).FileNameLength))
                } as usize;
                let end = offset
                    .checked_add(header)
                    .and_then(|p| p.checked_add(bytes))
                    .ok_or(Error::UnsafeObject)?;
                if bytes == 0 || bytes % 2 != 0 || bytes > 510 || end > used {
                    return Err(Error::UnsafeObject);
                }
                let pointer = unsafe {
                    storage
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset + header)
                        .cast::<u16>()
                };
                let name =
                    String::from_utf16(unsafe { std::slice::from_raw_parts(pointer, bytes / 2) })
                        .map_err(|_| Error::UnsafeObject)?;
                if name != "." && name != ".." {
                    if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0
                        || name.contains(['\\', '/', ':', '\0'])
                    {
                        return Err(Error::UnsafeObject);
                    }
                    visitor(crate::DirectoryEntry {
                        name,
                        directory: attrs & FILE_ATTRIBUTE_DIRECTORY != 0,
                    })?;
                }
                if next == 0 {
                    break;
                }
                if next % 8 != 0
                    || next < header + bytes
                    || offset.checked_add(next).is_none_or(|p| p >= used)
                {
                    return Err(Error::UnsafeObject);
                }
                offset += next;
            }
        }
        check_directory(&cursor)?;
        Ok(())
    }
}

impl Directory {
    /// Remove only the exact observed single-link ordinary file. A retained
    /// reader/image lease that denies DELETE makes this fail, never bypassed by
    /// POSIX disposition, path deletion, reparse following, or force flags.
    #[allow(unsafe_code)]
    pub fn remove_file_exact(&self, component: &OsStr, expected: &Identity) -> Result<(), Error> {
        use windows_sys::Wdk::Storage::FileSystem::{
            FILE_DISPOSITION_INFORMATION, FileDispositionInformation,
        };
        check_directory(self.root()?)?;
        let file = native_relative_access(
            self.root()?,
            name(component)?,
            OpenMode {
                directory: false,
                write: false,
                create: false,
                exclusive: false,
            },
            None,
            true,
        )?;
        if identity(&file)? != *expected {
            return Err(Error::UnsafeObject);
        }
        let info = FILE_DISPOSITION_INFORMATION { DeleteFile: true };
        let mut io = IO_STATUS_BLOCK::default();
        // Synchronous native file object and stack SDK struct stay live until
        // disposition completes; no delete-on-close handle escapes this method.
        let status = unsafe {
            NtSetInformationFile(
                file.as_raw_handle(),
                &mut io,
                (&info as *const FILE_DISPOSITION_INFORMATION).cast(),
                size_of::<FILE_DISPOSITION_INFORMATION>() as u32,
                FileDispositionInformation,
            )
        };
        if status < 0 {
            return Err(status_error(status));
        }
        drop(file);
        check_directory(self.root()?)?;
        Ok(())
    }
}

impl Directory {
    /// Identity of this actual retained non-reparse directory; does not change
    /// the ordinary Directory into a protected-host capability.
    pub fn directory_identity(&self) -> Result<Identity, Error> {
        let file = self.root()?;
        check_directory(file)?;
        let value = information(file)?;
        Ok(Identity {
            volume: value.dwVolumeSerialNumber,
            index_high: value.nFileIndexHigh,
            index_low: value.nFileIndexLow,
            created_high: value.ftCreationTime.dwHighDateTime,
            created_low: value.ftCreationTime.dwLowDateTime,
        })
    }
    /// Classic native disposition refuses non-empty directories atomically.
    /// No recursive traversal, force/POSIX flag, path fallback or delete lease
    /// escapes. All caller-owned handles to the child must already be closed.
    #[allow(unsafe_code)]
    pub fn remove_empty_dir_exact(
        &self,
        component: &OsStr,
        expected: &Identity,
    ) -> Result<(), Error> {
        use windows_sys::Wdk::Storage::FileSystem::{
            FILE_DISPOSITION_INFORMATION, FileDispositionInformation,
        };
        check_directory(self.root()?)?;
        let child = native_relative_access(
            self.root()?,
            name(component)?,
            OpenMode {
                directory: true,
                write: false,
                create: false,
                exclusive: false,
            },
            None,
            true,
        )?;
        let value = information(&child)?;
        let actual = Identity {
            volume: value.dwVolumeSerialNumber,
            index_high: value.nFileIndexHigh,
            index_low: value.nFileIndexLow,
            created_high: value.ftCreationTime.dwHighDateTime,
            created_low: value.ftCreationTime.dwLowDateTime,
        };
        if actual != *expected {
            return Err(Error::UnsafeObject);
        }
        let info = FILE_DISPOSITION_INFORMATION { DeleteFile: true };
        let mut io = IO_STATUS_BLOCK::default();
        let status = unsafe {
            NtSetInformationFile(
                child.as_raw_handle(),
                &mut io,
                (&info as *const FILE_DISPOSITION_INFORMATION).cast(),
                size_of::<FILE_DISPOSITION_INFORMATION>() as u32,
                FileDispositionInformation,
            )
        };
        if status < 0 {
            return Err(status_error(status));
        }
        drop(child);
        check_directory(self.root()?)?;
        Ok(())
    }
}

impl Directory {
    /// Read-only re-open of the original path and comparison of every retained
    /// ancestor's native identity. This grants no owner/ACL or image authority.
    pub fn recheck(&self, path: &Path) -> Result<(), Error> {
        let reopened = directory(path)?;
        if self._handles.len() != reopened._handles.len() {
            return Err(Error::UnsafeObject);
        }
        for (held, current) in self._handles.iter().zip(&reopened._handles) {
            check_directory(held)?;
            check_directory(current)?;
            let a = information(held)?;
            let b = information(current)?;
            if a.dwVolumeSerialNumber != b.dwVolumeSerialNumber
                || a.nFileIndexHigh != b.nFileIndexHigh
                || a.nFileIndexLow != b.nFileIndexLow
                || a.ftCreationTime.dwHighDateTime != b.ftCreationTime.dwHighDateTime
                || a.ftCreationTime.dwLowDateTime != b.ftCreationTime.dwLowDateTime
            {
                return Err(Error::UnsafeObject);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod recheck_tests {
    use super::*;
    #[test]
    fn current_path_must_reopen_the_actual_retained_directory_chain() {
        let unique = format!(
            "pinvou-custody-recheck-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        std::fs::create_dir(&root).unwrap();
        struct Temp(PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                for name in ["actual", "sibling", "moved"] {
                    let _ = std::fs::remove_dir(self.0.join(name));
                }
                let _ = std::fs::remove_dir(&self.0);
            }
        }
        let _temp = Temp(root.clone());
        std::fs::create_dir(root.join("actual")).unwrap();
        std::fs::create_dir(root.join("sibling")).unwrap();
        let held = directory(&root.join("actual")).unwrap();
        held.recheck(&root.join("actual")).unwrap();
        assert!(matches!(
            held.recheck(&root.join("sibling")),
            Err(Error::UnsafeObject)
        ));
        assert!(std::fs::rename(root.join("actual"), root.join("moved")).is_err());
        drop(held);
    }
}
