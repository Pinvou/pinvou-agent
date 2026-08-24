//! Secure startup cleanup for abandoned draft attachments.
//!
//! Upload directories are shallow (`<upload-id>/<one regular file>`), so the
//! cleanup deliberately avoids recursive deletion. Platform implementations
//! pin the root directory, reject links/reparse points, validate the expected
//! shape, remove the exact file without following links, then remove the now
//! empty upload directory.

use std::path::Path;
use std::time::{Duration, SystemTime};

type NameValidator = fn(&str) -> bool;

pub(in crate::features::files) fn sweep_stale_upload_root(
    root: &Path,
    now: SystemTime,
    stale_age: Duration,
    valid_upload_id: NameValidator,
    valid_filename: NameValidator,
) -> usize {
    platform::sweep(root, now, stale_age, valid_upload_id, valid_filename)
}

#[cfg(test)]
pub(in crate::features::files) fn create_test_directory_link(target: &Path, link: &Path) {
    platform::create_test_directory_link(target, link);
}

#[cfg(test)]
pub(in crate::features::files) fn remove_test_directory_link(link: &Path) {
    platform::remove_test_directory_link(link);
}

fn is_stale(metadata: &std::fs::Metadata, now: SystemTime, stale_age: Duration) -> bool {
    metadata
        .modified()
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age >= stale_age)
}

#[cfg(any(unix, test))]
fn device_numbers_match<T>(stat_device: T, metadata_device: u64) -> bool
where
    T: TryInto<u64>,
{
    // `dev_t` is unsigned on Linux but signed on macOS. Reject values that
    // cannot be represented losslessly instead of truncating or wrapping.
    stat_device
        .try_into()
        .is_ok_and(|stat_device| stat_device == metadata_device)
}

#[cfg(unix)]
mod platform {
    use super::{device_numbers_match, is_stale, NameValidator};
    use std::ffi::{CStr, CString};
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Component, Path};
    use std::time::{Duration, SystemTime};

    #[cfg(test)]
    pub(super) fn create_test_directory_link(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[cfg(test)]
    pub(super) fn remove_test_directory_link(link: &Path) {
        std::fs::remove_file(link).unwrap();
    }

    pub(super) fn sweep(
        root: &Path,
        now: SystemTime,
        stale_age: Duration,
        valid_upload_id: NameValidator,
        valid_filename: NameValidator,
    ) -> usize {
        let Ok(root_handle) = open_absolute_directory(root) else {
            return 0;
        };
        let Ok(upload_ids) = directory_names(&root_handle) else {
            return 0;
        };

        upload_ids
            .into_iter()
            .filter(|upload_id| valid_upload_id(upload_id))
            .filter(|upload_id| {
                remove_stale_upload(&root_handle, upload_id, now, stale_age, valid_filename)
                    .unwrap_or(false)
            })
            .count()
    }

    fn open_absolute_directory(path: &Path) -> io::Result<File> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cleanup root must be absolute",
            ));
        }
        // SAFETY: the literal root name is NUL terminated; successful fds are
        // immediately transferred to `File` ownership.
        let root_fd = unsafe {
            libc::open(
                c"/".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if root_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut current = unsafe { File::from_raw_fd(root_fd) };
        for component in path.components() {
            match component {
                Component::RootDir => continue,
                Component::Normal(name) => {
                    let name = CString::new(name.as_bytes()).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL")
                    })?;
                    let fd = unsafe {
                        libc::openat(
                            current.as_raw_fd(),
                            name.as_ptr(),
                            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                        )
                    };
                    if fd < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    current = unsafe { File::from_raw_fd(fd) };
                }
                Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "cleanup root is not normalized",
                    ));
                }
            }
        }
        Ok(current)
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    unsafe fn errno_location() -> *mut libc::c_int {
        #[cfg(target_os = "linux")]
        {
            unsafe { libc::__errno_location() }
        }
        #[cfg(target_os = "macos")]
        {
            unsafe { libc::__error() }
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn directory_names(directory: &File) -> io::Result<Vec<String>> {
        let duplicate = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error());
        }
        let stream = unsafe { libc::fdopendir(duplicate) };
        if stream.is_null() {
            let error = io::Error::last_os_error();
            unsafe { libc::close(duplicate) };
            return Err(error);
        }

        let mut names = Vec::new();
        let mut enumeration_error = None;
        loop {
            unsafe { *errno_location() = 0 };
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                let errno = unsafe { *errno_location() };
                if errno != 0 {
                    enumeration_error = Some(io::Error::from_raw_os_error(errno));
                }
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            let Ok(name) = name.to_str() else {
                enumeration_error = Some(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "cleanup directory contains a non-UTF-8 name",
                ));
                break;
            };
            if name != "." && name != ".." {
                names.push(name.to_string());
            }
        }
        if unsafe { libc::closedir(stream) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if let Some(error) = enumeration_error {
            return Err(error);
        }
        Ok(names)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn directory_names(_directory: &File) -> io::Result<Vec<String>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "secure attachment cleanup is unsupported on this Unix target",
        ))
    }

    fn open_directory_at(parent: &File, name: &CString) -> io::Result<File> {
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn open_regular_file_at(parent: &File, name: &CString) -> io::Result<File> {
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn remove_stale_upload(
        root: &File,
        upload_id: &str,
        now: SystemTime,
        stale_age: Duration,
        valid_filename: NameValidator,
    ) -> io::Result<bool> {
        let upload_name = CString::new(upload_id)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "upload id contains NUL"))?;
        let upload_dir = open_directory_at(root, &upload_name)?;
        let names = directory_names(&upload_dir)?;
        let [filename] = names.as_slice() else {
            return Ok(false);
        };
        if !valid_filename(filename) {
            return Ok(false);
        }

        let filename_c = CString::new(filename.as_str())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "filename contains NUL"))?;
        let file = open_regular_file_at(&upload_dir, &filename_c)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 || !is_stale(&metadata, now, stale_age) {
            return Ok(false);
        }

        // Revalidate the anchored name against the opened inode. `unlinkat`
        // never follows a replacement symlink and cannot escape `upload_dir`.
        let mut current = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                upload_dir.as_raw_fd(),
                filename_c.as_ptr(),
                current.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Ok(false);
        }
        let current = unsafe { current.assume_init() };
        if !device_numbers_match(current.st_dev, metadata.dev())
            || current.st_ino != metadata.ino()
            || current.st_nlink != 1
            || current.st_mode & libc::S_IFMT != libc::S_IFREG
        {
            return Ok(false);
        }
        if unsafe { libc::unlinkat(upload_dir.as_raw_fd(), filename_c.as_ptr(), 0) } != 0 {
            return Ok(false);
        }
        drop(file);
        if unsafe { libc::unlinkat(root.as_raw_fd(), upload_name.as_ptr(), libc::AT_REMOVEDIR) }
            != 0
        {
            return Ok(false);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::device_numbers_match;

    #[test]
    fn device_number_comparison_accepts_linux_and_macos_representations() {
        let linux_device: u64 = 42;
        let macos_device: i32 = 42;

        assert!(device_numbers_match(linux_device, 42));
        assert!(device_numbers_match(macos_device, 42));
        assert!(device_numbers_match(u64::MAX, u64::MAX));
        assert!(device_numbers_match(i32::MAX, i32::MAX as u64));
        assert!(!device_numbers_match(linux_device, 43));
        assert!(!device_numbers_match(macos_device, 43));
    }

    #[test]
    fn device_number_comparison_fails_closed_when_conversion_is_invalid() {
        let invalid_macos_device: i32 = -1;

        assert!(!device_numbers_match(invalid_macos_device, u64::MAX));
    }
}

#[cfg(windows)]
mod platform {
    use super::{is_stale, NameValidator};
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::{Component, Path, PathBuf};
    use std::time::{Duration, SystemTime};
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, GetFileInformationByHandle, SetFileInformationByHandle,
        BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    #[cfg(test)]
    pub(super) fn create_test_directory_link(target: &Path, link: &Path) {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return;
        }
        let status = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run mklink for junction regression test");
        assert!(status.success(), "failed to create directory reparse point");
    }

    #[cfg(test)]
    pub(super) fn remove_test_directory_link(link: &Path) {
        std::fs::remove_dir(link).unwrap();
    }

    struct PinnedRoot {
        path: PathBuf,
        _component_handles: Vec<File>,
    }

    pub(super) fn sweep(
        root: &Path,
        now: SystemTime,
        stale_age: Duration,
        valid_upload_id: NameValidator,
        valid_filename: NameValidator,
    ) -> usize {
        let Ok(root) = pin_root(root) else {
            return 0;
        };
        let Ok(entries) = std::fs::read_dir(&root.path) else {
            return 0;
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .filter(|upload_id| valid_upload_id(upload_id))
            .filter(|upload_id| {
                remove_stale_upload(&root.path, upload_id, now, stale_age, valid_filename)
                    .unwrap_or(false)
            })
            .count()
    }

    fn pin_root(path: &Path) -> io::Result<PinnedRoot> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cleanup root must be absolute",
            ));
        }
        let mut current = PathBuf::new();
        let mut handles = Vec::new();
        for component in path.components() {
            match component {
                Component::Prefix(prefix) => current.push(prefix.as_os_str()),
                Component::RootDir => current.push(Path::new(r"\")),
                Component::Normal(name) => {
                    current.push(name);
                    handles.push(open_directory(&current, false)?);
                }
                Component::CurDir | Component::ParentDir => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "cleanup root is not normalized",
                    ));
                }
            }
        }
        if handles.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cleanup root cannot be a volume root",
            ));
        }
        Ok(PinnedRoot {
            path: path.to_path_buf(),
            _component_handles: handles,
        })
    }

    fn open_directory(path: &Path, delete_access: bool) -> io::Result<File> {
        let access = FILE_GENERIC_READ | if delete_access { DELETE } else { 0 };
        let file = OpenOptions::new()
            .access_mode(access)
            // Excluding FILE_SHARE_DELETE pins this component's identity.
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "cleanup directory is a reparse point or has the wrong type",
            ));
        }
        Ok(file)
    }

    fn open_regular_file(path: &Path) -> io::Result<(File, std::fs::Metadata)> {
        let file = OpenOptions::new()
            .access_mode(FILE_GENERIC_READ | DELETE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "cleanup file is a reparse point or has the wrong type",
            ));
        }
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if information.nNumberOfLinks != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "cleanup file has multiple links",
            ));
        }
        Ok((file, metadata))
    }

    fn mark_for_deletion(file: &File) -> io::Result<()> {
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        if unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileDispositionInfo,
                (&raw const disposition).cast(),
                std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn remove_stale_upload(
        root: &Path,
        upload_id: &str,
        now: SystemTime,
        stale_age: Duration,
        valid_filename: NameValidator,
    ) -> io::Result<bool> {
        let upload_path = root.join(upload_id);
        let upload_dir = open_directory(&upload_path, true)?;
        let entries = std::fs::read_dir(&upload_path)?.collect::<Result<Vec<_>, _>>()?;
        let [entry] = entries.as_slice() else {
            return Ok(false);
        };
        let Some(filename) = entry.file_name().to_str().map(str::to_owned) else {
            return Ok(false);
        };
        if !valid_filename(&filename) {
            return Ok(false);
        }

        let (file, metadata) = open_regular_file(&upload_path.join(filename))?;
        if !is_stale(&metadata, now, stale_age) {
            return Ok(false);
        }
        mark_for_deletion(&file)?;
        drop(file);
        mark_for_deletion(&upload_dir)?;
        drop(upload_dir);
        Ok(true)
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use super::NameValidator;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    #[cfg(test)]
    pub(super) fn create_test_directory_link(_target: &Path, _link: &Path) {
        panic!("directory links are unsupported on this platform");
    }

    #[cfg(test)]
    pub(super) fn remove_test_directory_link(_link: &Path) {}

    pub(super) fn sweep(
        _root: &Path,
        _now: SystemTime,
        _stale_age: Duration,
        _valid_upload_id: NameValidator,
        _valid_filename: NameValidator,
    ) -> usize {
        0
    }
}
