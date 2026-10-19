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
fn relative(parent: &File, component: &OsStr, mode: OpenMode) -> Result<File, Error> {
    let mut wide = name(component)?;
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
        SecurityDescriptor: std::ptr::null_mut(),
        SecurityQualityOfService: std::ptr::null_mut(),
    };
    let mut handle: HANDLE = std::ptr::null_mut();
    let mut io = IO_STATUS_BLOCK::default();
    // Same read/write access as ordinary file IO, plus SYNCHRONIZE for the
    // synchronous native handle. Link publication needs no DELETE permission.
    // Callers select read/write IO explicitly; an existing file is never truncated.
    let access = GENERIC_READ | SYNCHRONIZE | if mode.write { GENERIC_WRITE } else { 0 };
    let options = FILE_OPEN_REPARSE_POINT
        | FILE_SYNCHRONOUS_IO_NONALERT
        | if mode.directory {
            FILE_DIRECTORY_FILE
        } else {
            FILE_NON_DIRECTORY_FILE
        };
    // SAFETY: exact SDK structures, valid live parent, and a single checked
    // non-NUL component. All referenced buffers outlive this synchronous call.
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
