use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
extern crate libc as platform_libc;

// The anchored directory ABI below is intentionally limited to the Unix
// targets shipped by the desktop app. Other targets are rejected explicitly
// instead of guessing their libc `dirent` layout or using unsafe path fallback.
#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
mod libc {
    use std::os::raw::{c_char, c_int};

    #[repr(C)]
    pub(crate) struct DIR {
        _private: [u8; 0],
    }

    #[cfg(target_os = "macos")]
    #[repr(C)]
    pub(crate) struct Dirent {
        pub(crate) d_ino: u64,
        pub(crate) d_seekoff: u64,
        pub(crate) d_reclen: u16,
        pub(crate) d_namlen: u16,
        pub(crate) d_type: u8,
        pub(crate) d_name: [c_char; 1024],
    }

    #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
    #[repr(C)]
    pub(crate) struct Dirent {
        pub(crate) d_ino: u64,
        pub(crate) d_off: i64,
        pub(crate) d_reclen: u16,
        pub(crate) d_type: u8,
        pub(crate) d_name: [c_char; 256],
    }

    #[cfg(target_os = "macos")]
    const _: () = {
        assert!(std::mem::offset_of!(Dirent, d_name) == 21);
        assert!(std::mem::size_of::<Dirent>() == 1048);
    };

    #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
    const _: () = {
        assert!(std::mem::offset_of!(Dirent, d_name) == 19);
        assert!(std::mem::size_of::<Dirent>() == 280);
    };

    // Use the target libc's values rather than transcribing one architecture's
    // ABI. In particular, Linux/aarch64 assigns O_DIRECTORY and O_NOFOLLOW
    // different bits than Linux/x86_64.
    pub(crate) const O_RDONLY: c_int = super::platform_libc::O_RDONLY;
    pub(crate) const O_RDWR: c_int = super::platform_libc::O_RDWR;
    pub(crate) const O_CREAT: c_int = super::platform_libc::O_CREAT;
    pub(crate) const O_EXCL: c_int = super::platform_libc::O_EXCL;
    pub(crate) const O_CLOEXEC: c_int = super::platform_libc::O_CLOEXEC;
    pub(crate) const O_NOFOLLOW: c_int = super::platform_libc::O_NOFOLLOW;
    pub(crate) const O_DIRECTORY: c_int = super::platform_libc::O_DIRECTORY;
    pub(crate) const O_NONBLOCK: c_int = super::platform_libc::O_NONBLOCK;
    pub(crate) const AT_REMOVEDIR: c_int = super::platform_libc::AT_REMOVEDIR;
    #[cfg(target_os = "macos")]
    pub(crate) const RENAME_EXCL: u32 = 0x0000_0004;
    #[cfg(not(target_os = "macos"))]
    pub(crate) const RENAME_NOREPLACE: u32 = 0x0000_0001;
    #[cfg(target_os = "macos")]
    pub(crate) type ModeT = u16;
    #[cfg(not(target_os = "macos"))]
    pub(crate) type ModeT = u32;

    unsafe extern "C" {
        pub(crate) fn open(path: *const c_char, flags: c_int, ...) -> c_int;
        pub(crate) fn openat(directory: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
        pub(crate) fn mkdirat(directory: c_int, path: *const c_char, mode: ModeT) -> c_int;
        pub(crate) fn renameat(
            source_directory: c_int,
            source: *const c_char,
            destination_directory: c_int,
            destination: *const c_char,
        ) -> c_int;
        #[cfg(target_os = "linux")]
        pub(crate) fn renameat2(
            source_directory: c_int,
            source: *const c_char,
            destination_directory: c_int,
            destination: *const c_char,
            flags: u32,
        ) -> c_int;
        #[cfg(target_os = "macos")]
        pub(crate) fn renameatx_np(
            source_directory: c_int,
            source: *const c_char,
            destination_directory: c_int,
            destination: *const c_char,
            flags: u32,
        ) -> c_int;
        pub(crate) fn unlinkat(directory: c_int, path: *const c_char, flags: c_int) -> c_int;
        pub(crate) fn close(fd: c_int) -> c_int;
        #[cfg_attr(
            all(target_os = "macos", target_arch = "x86_64"),
            link_name = "fdopendir$INODE64"
        )]
        pub(crate) fn fdopendir(fd: c_int) -> *mut DIR;
        #[cfg_attr(
            all(target_os = "macos", target_arch = "x86_64"),
            link_name = "readdir$INODE64"
        )]
        pub(crate) fn readdir(directory: *mut DIR) -> *mut Dirent;
        pub(crate) fn closedir(directory: *mut DIR) -> c_int;

        #[cfg_attr(target_os = "linux", link_name = "__errno_location")]
        #[cfg_attr(target_os = "macos", link_name = "__error")]
        pub(crate) fn errno_location() -> *mut c_int;
    }
}

/// Return whether a path is a regular file the current platform can execute.
/// Unix discovery must not advertise a chmod 0644 placeholder as a usable
/// runtime; Windows execution permission is represented by ACLs/file type and
/// is ultimately enforced by process creation.
pub(crate) fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplaceState {
    Committed,
    RolledBack,
    RecoveryRequired,
}

#[derive(Debug)]
pub(crate) struct ReplaceError {
    state: ReplaceState,
    source: io::Error,
}

impl ReplaceError {
    pub(crate) fn new(state: ReplaceState, source: io::Error) -> Self {
        Self { state, source }
    }

    pub(crate) fn state(&self) -> ReplaceState {
        self.state
    }

    pub(crate) fn into_io_error(self) -> io::Error {
        io::Error::new(self.source.kind(), self.to_string())
    }
}

impl std::fmt::Display for ReplaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "atomic replace {:?}: {}",
            self.state, self.source
        )
    }
}

impl std::error::Error for ReplaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

pub(crate) type ReplaceResult = Result<ReplaceState, ReplaceError>;

pub(crate) fn replace_file_atomically(tmp: &Path, target: &Path, backup: &Path) -> ReplaceResult {
    replace_file_atomically_impl(tmp, target, backup)
}

/// Converge a layout left by an interrupted Windows replacement. Callers must
/// hold the same lifecycle lock used for writes so active temporary files are
/// never mistaken for recovery candidates.
pub(crate) fn recover_interrupted_replace(
    replacement: &Path,
    target: &Path,
    backup: &Path,
) -> ReplaceResult {
    if target.is_file() {
        return Ok(ReplaceState::Committed);
    }
    if backup.is_file() {
        return promote_replacement(backup, target).map_or_else(
            |error| {
                Err(ReplaceError::new(
                    ReplaceState::RecoveryRequired,
                    error.into_io_error(),
                ))
            },
            |_| {
                Err(ReplaceError::new(
                    ReplaceState::RolledBack,
                    io::Error::new(
                        io::ErrorKind::Other,
                        "interrupted replacement was rolled back",
                    ),
                ))
            },
        );
    }
    if replacement.is_file() {
        return promote_replacement(replacement, target);
    }
    Err(ReplaceError::new(
        ReplaceState::RecoveryRequired,
        io::Error::new(
            io::ErrorKind::NotFound,
            "interrupted replacement has no recoverable candidate",
        ),
    ))
}

/// 原子写文件：先写同目录临时文件并 sync，再原子替换目标。
///
/// 直接 `std::fs::write(target)` 在进程被中断（强杀/崩溃）时会留下截断或
/// 半写的目标文件；对持久化的会话状态（如 `_session_mode_states.json`、
/// settings.json）而言，一个损坏的目标会让启动恢复静默失败，表现为
/// "切换成功但重启后丢失"。tmp + rename 保证目标要么是旧完整内容、
/// 要么是新完整内容，永无中间态。失败清理按替换终态区分（对齐
/// artifacts 版）：不得把装着旧完整内容的恢复候选一并删掉。
pub(crate) fn atomic_write(path: &Path, content: &[u8]) -> io::Result<()> {
    atomic_write_impl(path, content, false)
}

/// Uses the same replacement semantics as `atomic_write`, but creates the temporary file
/// with private permissions immediately. Use it for configurations containing secrets or
/// unauthenticated local-port data to avoid a pre-rename default-umask window on Unix.
/// Windows continues to rely on the user-directory ACL.
pub(crate) fn atomic_write_private(path: &Path, content: &[u8]) -> io::Result<()> {
    atomic_write_impl(path, content, true)
}

fn atomic_write_impl(path: &Path, content: &[u8], private: bool) -> io::Result<()> {
    use std::io::Write as _;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("state.json");
    let token = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let tmp = parent.join(format!(".{file_name}.tmp-{token}"));
    let backup = parent.join(format!(".{file_name}.bak-{token}"));

    let stage_result = (|| -> io::Result<()> {
        let mut file = if private {
            create_secret_file(&tmp)?
        } else {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?
        };
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        Ok(())
    })();
    if let Err(error) = stage_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }

    // Windows 状态机内部自带「目标不存在 → promote（单文件移动）」分支，
    // POSIX 实现就是 rename，无需外层再区分首写/覆盖。
    match replace_file_atomically(&tmp, path, &backup) {
        Ok(ReplaceState::Committed) => {
            let _ = std::fs::remove_file(&backup);
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all();
            }
            Ok(())
        }
        Ok(state) => Err(io::Error::other(format!(
            "unexpected successful replacement state: {state:?}"
        ))),
        Err(error) => {
            // RolledBack：布局已收敛回旧内容，tmp/backup 是残留垃圾；
            // RecoveryRequired 且目标路径仍被占用（如目录占位）：恢复永无
            // 可能，同样是垃圾；目标真正丢失的 RecoveryRequired 保留候选，
            // 供恢复流程 promote——这正是不能无条件清理的原因。
            if error.state() == ReplaceState::RolledBack
                || (error.state() == ReplaceState::RecoveryRequired && path.exists())
            {
                let _ = std::fs::remove_file(&tmp);
                let _ = std::fs::remove_file(&backup);
            }
            Err(error.into_io_error())
        }
    }
}

/// 以 0600 权限创建（或截断）文件：写入含明文密钥的 CLI 配置时**直接**以
/// 0600 创建，避免「先按默认 umask 0644 写、再收紧」的暴露窗口（复审低危 4）；
/// Windows 无 POSIX 权限概念，忽略权限位。
pub(crate) fn create_secret_file(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

/// Open a private append-only data file without introducing a world-readable
/// creation window on Unix. Windows relies on the owning profile directory's
/// ACL, consistent with the rest of the application data tree.
pub(crate) fn open_private_append_file(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

#[derive(Clone)]
pub(crate) struct RealDirectoryIdentity {
    #[cfg(any(
        windows,
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    canonical_path: PathBuf,
    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    device: u64,
    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    inode: u64,
    #[cfg(windows)]
    volume_serial_number: u32,
    #[cfg(windows)]
    file_index: u64,
}

/// A private directory whose concrete filesystem object stays stable for
/// relative file operations. macOS and 64-bit Linux anchor operations to an
/// open directory fd; Windows holds non-delete-shared handles for every
/// canonical path component. Other targets reject this capability.
pub(crate) struct PrivateFileDirectory {
    #[cfg(windows)]
    canonical_path: PathBuf,
    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    directory_handle: File,
    #[cfg(windows)]
    _component_handles: Vec<File>,
}

#[cfg(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
pub(crate) fn open_private_file_directory(path: &Path) -> io::Result<PrivateFileDirectory> {
    let expected = ensure_private_real_directory(path)?;
    let directory = open_private_file_directory_impl(expected)?;
    directory.enforce_private_permissions()?;
    Ok(directory)
}

#[cfg(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
pub(crate) fn open_existing_private_file_directory(
    path: &Path,
) -> io::Result<Option<PrivateFileDirectory>> {
    let expected = match real_directory_identity(path) {
        Ok(expected) => expected,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let directory = open_private_file_directory_impl(expected)?;
    directory.enforce_private_permissions()?;
    Ok(Some(directory))
}

fn private_file_parent_and_name(path: &Path) -> io::Result<(&Path, &std::ffi::OsStr)> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "private file has no parent"))?;
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "private file has no filename")
    })?;
    Ok((parent, name))
}

/// Read a regular private file without following the final component or a
/// replaced parent directory. The directory handle remains pinned until the
/// complete file body has been read.
pub(crate) fn read_private_file_anchored(path: &Path) -> io::Result<Option<Vec<u8>>> {
    use std::io::Read as _;

    let (parent, name) = private_file_parent_and_name(path)?;
    let Some(directory) = open_existing_private_file_directory(parent)? else {
        return Ok(None);
    };
    let Some(mut file) = directory.open_plain_file(name)? else {
        return Ok(None);
    };
    let mut content = Vec::new();
    file.read_to_end(&mut content)?;
    Ok(Some(content))
}

/// Atomically replace a private file relative to a pinned directory object.
/// Creating or renaming the visible parent path cannot redirect this write.
pub(crate) fn atomic_write_private_anchored(path: &Path, content: &[u8]) -> io::Result<()> {
    let (parent, name) = private_file_parent_and_name(path)?;
    open_private_file_directory(parent)?.atomic_write_private_file(name, content)
}

/// Remove a regular private file relative to a pinned directory object. A
/// missing directory or file is idempotent success (`false`).
pub(crate) fn remove_private_file_anchored(path: &Path) -> io::Result<bool> {
    let (parent, name) = private_file_parent_and_name(path)?;
    let Some(directory) = open_existing_private_file_directory(parent)? else {
        return Ok(false);
    };
    directory.remove_plain_file(name)
}

// unsupported-private-file-directory:start
#[cfg(not(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
)))]
fn private_file_directory_unsupported<T>() -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private snapshot directory is unsupported on this platform",
    ))
}

#[cfg(not(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
)))]
pub(crate) fn open_private_file_directory(_path: &Path) -> io::Result<PrivateFileDirectory> {
    private_file_directory_unsupported()
}

#[cfg(not(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
)))]
pub(crate) fn open_existing_private_file_directory(
    _path: &Path,
) -> io::Result<Option<PrivateFileDirectory>> {
    private_file_directory_unsupported()
}

#[cfg(not(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
)))]
pub(crate) fn ensure_private_real_directory(_path: &Path) -> io::Result<RealDirectoryIdentity> {
    private_file_directory_unsupported()
}

#[cfg(not(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
)))]
impl PrivateFileDirectory {
    pub(crate) fn entry_names(&self) -> io::Result<Vec<std::ffi::OsString>> {
        private_file_directory_unsupported()
    }

    pub(crate) fn create_delete_on_close_file(&self, _name: &str) -> io::Result<File> {
        private_file_directory_unsupported()
    }

    pub(crate) fn remove_plain_file(&self, _name: &std::ffi::OsStr) -> io::Result<bool> {
        private_file_directory_unsupported()
    }

    pub(crate) fn metadata(&self) -> io::Result<std::fs::Metadata> {
        private_file_directory_unsupported()
    }

    pub(crate) fn open_child_directory(
        &self,
        _name: &std::ffi::OsStr,
    ) -> io::Result<PrivateFileDirectory> {
        private_file_directory_unsupported()
    }

    pub(crate) fn create_private_child_directory(
        &self,
        _name: &std::ffi::OsStr,
    ) -> io::Result<PrivateFileDirectory> {
        private_file_directory_unsupported()
    }

    pub(crate) fn open_plain_file(&self, _name: &std::ffi::OsStr) -> io::Result<Option<File>> {
        private_file_directory_unsupported()
    }

    pub(crate) fn atomic_write_private_file(
        &self,
        _name: &std::ffi::OsStr,
        _content: &[u8],
    ) -> io::Result<()> {
        private_file_directory_unsupported()
    }

    pub(crate) fn move_plain_file_to(
        &self,
        _source_name: &std::ffi::OsStr,
        _destination: &PrivateFileDirectory,
        _destination_name: &std::ffi::OsStr,
    ) -> io::Result<MovePlainFileOutcome> {
        private_file_directory_unsupported()
    }

    pub(crate) fn remove_empty_child_directory(&self, _name: &std::ffi::OsStr) -> io::Result<bool> {
        private_file_directory_unsupported()
    }
}
// unsupported-private-file-directory:end

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn open_private_file_directory_impl(
    expected: RealDirectoryIdentity,
) -> io::Result<PrivateFileDirectory> {
    use std::os::fd::{FromRawFd as _, RawFd};
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::MetadataExt as _;

    let path = std::ffi::CString::new(expected.canonical_path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "directory path contains NUL"))?;
    let fd: RawFd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let directory_handle = unsafe { File::from_raw_fd(fd) };
    let metadata = directory_handle.metadata()?;
    if !metadata.is_dir() || metadata.dev() != expected.device || metadata.ino() != expected.inode {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private directory changed while acquiring its stable handle",
        ));
    }
    Ok(PrivateFileDirectory { directory_handle })
}

#[cfg(windows)]
fn open_private_file_directory_impl(
    expected: RealDirectoryIdentity,
) -> io::Result<PrivateFileDirectory> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut current = PathBuf::new();
    let mut handles = Vec::new();
    for component in expected.canonical_path.components() {
        match component {
            std::path::Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            std::path::Component::RootDir => current.push(Path::new(r"\")),
            std::path::Component::Normal(name) => {
                current.push(name);
                let mut options = std::fs::OpenOptions::new();
                options
                    .read(true)
                    // Omitting FILE_SHARE_DELETE keeps the canonical chain
                    // from being renamed or replaced while path-based Win32
                    // leaf creation/enumeration is in progress.
                    .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                    .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
                let handle = options.open(&current)?;
                let metadata = handle.metadata()?;
                if !metadata_is_real_directory(&metadata) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "private directory chain contains a reparse point: {}",
                            current.display()
                        ),
                    ));
                }
                handles.push(handle);
            }
            std::path::Component::CurDir | std::path::Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "private directory path must be normalized",
                ));
            }
        }
    }
    let final_handle = handles.last().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "private directory cannot be a volume root",
        )
    })?;
    let (volume_serial_number, file_index) = windows_file_identity(final_handle)?;
    if volume_serial_number != expected.volume_serial_number || file_index != expected.file_index {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private directory changed while acquiring its stable handles",
        ));
    }
    Ok(PrivateFileDirectory {
        canonical_path: expected.canonical_path,
        _component_handles: handles,
    })
}

#[cfg(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
impl PrivateFileDirectory {
    fn enforce_private_permissions(&self) -> io::Result<()> {
        #[cfg(any(
            target_os = "macos",
            all(target_os = "linux", target_pointer_width = "64")
        ))]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let metadata = self.directory_handle.metadata()?;
            if metadata.permissions().mode() & 0o777 != 0o700 {
                self.directory_handle
                    .set_permissions(std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(())
    }

    pub(crate) fn entry_names(&self) -> io::Result<Vec<std::ffi::OsString>> {
        entry_names_impl(self)
    }

    /// Create a private read/write file and immediately remove its directory
    /// entry (Unix) or mark the exact file handle delete-pending (Windows).
    /// The returned handle is therefore the sole authority for its lifetime.
    pub(crate) fn create_delete_on_close_file(&self, name: &str) -> io::Result<File> {
        validate_private_file_name(std::ffi::OsStr::new(name))?;
        create_delete_on_close_file_impl(self, name)
    }

    pub(crate) fn remove_plain_file(&self, name: &std::ffi::OsStr) -> io::Result<bool> {
        validate_private_file_name(name)?;
        remove_plain_file_impl(self, name)
    }

    pub(crate) fn metadata(&self) -> io::Result<std::fs::Metadata> {
        directory_metadata_impl(self)
    }

    pub(crate) fn open_child_directory(
        &self,
        name: &std::ffi::OsStr,
    ) -> io::Result<PrivateFileDirectory> {
        validate_private_file_name(name)?;
        open_child_directory_impl(self, name)
    }

    pub(crate) fn create_private_child_directory(
        &self,
        name: &std::ffi::OsStr,
    ) -> io::Result<PrivateFileDirectory> {
        validate_private_file_name(name)?;
        create_private_child_directory_impl(self, name)
    }

    pub(crate) fn open_plain_file(&self, name: &std::ffi::OsStr) -> io::Result<Option<File>> {
        validate_private_file_name(name)?;
        open_plain_file_impl(self, name)
    }

    /// Atomically replace one private regular file relative to this pinned
    /// directory. The temporary file and final rename never re-resolve the
    /// directory path, so replacing the visible root cannot redirect the write.
    pub(crate) fn atomic_write_private_file(
        &self,
        name: &std::ffi::OsStr,
        content: &[u8],
    ) -> io::Result<()> {
        validate_private_file_name(name)?;
        atomic_write_private_file_impl(self, name, content)
    }

    pub(crate) fn move_plain_file_to(
        &self,
        source_name: &std::ffi::OsStr,
        destination: &PrivateFileDirectory,
        destination_name: &std::ffi::OsStr,
    ) -> io::Result<MovePlainFileOutcome> {
        validate_private_file_name(source_name)?;
        validate_private_file_name(destination_name)?;
        move_plain_file_to_impl(self, source_name, destination, destination_name)
    }

    pub(crate) fn remove_empty_child_directory(&self, name: &std::ffi::OsStr) -> io::Result<bool> {
        validate_private_file_name(name)?;
        remove_empty_child_directory_impl(self, name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MovePlainFileOutcome {
    Missing,
    AlreadyMoved,
    Moved,
}

#[cfg(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn validate_private_file_name(name: &std::ffi::OsStr) -> io::Result<()> {
    let path = Path::new(name);
    if path.components().count() != 1
        || !matches!(
            path.components().next(),
            Some(std::path::Component::Normal(_))
        )
        || path.file_name() != Some(name)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private file name must be one normal path component",
        ));
    }
    Ok(())
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn directory_metadata_impl(directory: &PrivateFileDirectory) -> io::Result<std::fs::Metadata> {
    directory.directory_handle.metadata()
}

#[cfg(windows)]
fn directory_metadata_impl(directory: &PrivateFileDirectory) -> io::Result<std::fs::Metadata> {
    directory
        ._component_handles
        .last()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "directory has no handle"))?
        .metadata()
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn open_child_directory_impl(
    directory: &PrivateFileDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<PrivateFileDirectory> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};

    let name = unix_name(name)?;
    let fd = unsafe {
        libc::openat(
            directory.directory_handle.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let child = PrivateFileDirectory {
        directory_handle: unsafe { File::from_raw_fd(fd) },
    };
    child.enforce_private_permissions()?;
    Ok(child)
}

#[cfg(windows)]
fn open_child_directory_impl(
    directory: &PrivateFileDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<PrivateFileDirectory> {
    let path = directory.canonical_path.join(name);
    open_existing_private_file_directory(&path)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("private child directory is missing: {}", path.display()),
        )
    })
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn create_private_child_directory_impl(
    directory: &PrivateFileDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<PrivateFileDirectory> {
    use std::os::fd::AsRawFd as _;

    let name_c = unix_name(name)?;
    if unsafe {
        libc::mkdirat(
            directory.directory_handle.as_raw_fd(),
            name_c.as_ptr(),
            0o700 as libc::ModeT,
        )
    } != 0
    {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(error);
        }
    }
    open_child_directory_impl(directory, name)
}

#[cfg(windows)]
fn create_private_child_directory_impl(
    directory: &PrivateFileDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<PrivateFileDirectory> {
    let path = directory.canonical_path.join(name);
    match std::fs::create_dir(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    open_child_directory_impl(directory, name)
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn open_plain_file_impl(
    directory: &PrivateFileDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<Option<File>> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};

    let name = unix_name(name)?;
    let fd = unsafe {
        libc::openat(
            directory.directory_handle.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(error)
        };
    }
    let file = unsafe { File::from_raw_fd(fd) };
    if !unix_named_file_matches(directory, &name, &file)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private entry is not one stable regular file",
        ));
    }
    Ok(Some(file))
}

#[cfg(windows)]
fn open_plain_file_impl(
    directory: &PrivateFileDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<Option<File>> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };

    let mut options = std::fs::OpenOptions::new();
    options
        .access_mode(FILE_GENERIC_READ)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = match options.open(directory.canonical_path.join(name)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata_is_plain_file(&file.metadata()?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private entry is a reparse point or non-file",
        ));
    }
    Ok(Some(file))
}

fn private_atomic_temp_name() -> std::ffi::OsString {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        ".pinvou-private-write-{}-{timestamp}-{sequence}.tmp",
        std::process::id()
    )
    .into()
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn atomic_write_private_file_impl(
    directory: &PrivateFileDirectory,
    name: &std::ffi::OsStr,
    content: &[u8],
) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::fd::{AsRawFd as _, FromRawFd as _};

    let destination = unix_name(name)?;
    let (temporary_name, mut temporary_file) = loop {
        let temporary_name = private_atomic_temp_name();
        let temporary_name_c = unix_name(&temporary_name)?;
        let fd = unsafe {
            libc::openat(
                directory.directory_handle.as_raw_fd(),
                temporary_name_c.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                // C variadic arguments apply integer promotion. On macOS,
                // mode_t is u16, so pass the promoted unsigned-int value.
                0o600 as std::os::raw::c_uint,
            )
        };
        if fd >= 0 {
            break (temporary_name, unsafe { File::from_raw_fd(fd) });
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(error);
        }
    };
    let temporary_name_c = unix_name(&temporary_name)?;
    let cleanup_temporary = || unsafe {
        libc::unlinkat(
            directory.directory_handle.as_raw_fd(),
            temporary_name_c.as_ptr(),
            0,
        )
    };
    if let Err(error) = temporary_file
        .write_all(content)
        .and_then(|()| temporary_file.sync_all())
    {
        drop(temporary_file);
        cleanup_temporary();
        return Err(error);
    }
    if unsafe {
        libc::renameat(
            directory.directory_handle.as_raw_fd(),
            temporary_name_c.as_ptr(),
            directory.directory_handle.as_raw_fd(),
            destination.as_ptr(),
        )
    } != 0
    {
        let error = io::Error::last_os_error();
        drop(temporary_file);
        cleanup_temporary();
        return Err(error);
    }
    directory.directory_handle.sync_all()
}

#[cfg(windows)]
fn atomic_write_private_file_impl(
    directory: &PrivateFileDirectory,
    name: &std::ffi::OsStr,
    content: &[u8],
) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FileRenameInfo, SetFileInformationByHandle, DELETE, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_RENAME_INFO, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let (temporary_name, mut temporary_file) = loop {
        let temporary_name = private_atomic_temp_name();
        let mut options = std::fs::OpenOptions::new();
        options
            .access_mode(FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE)
            .create_new(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        match options.open(directory.canonical_path.join(&temporary_name)) {
            Ok(file) => break (temporary_name, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    if !metadata_is_plain_file(&temporary_file.metadata()?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "created private temporary file is a reparse point or non-file",
        ));
    }
    if let Err(error) = temporary_file
        .write_all(content)
        .and_then(|()| temporary_file.sync_all())
    {
        let _ = mark_windows_file_handle_for_deletion(&temporary_file);
        return Err(error);
    }

    let expected = windows_file_identity(&temporary_file)?;
    let destination_handle = directory
        ._component_handles
        .last()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "directory has no handle"))?;
    let destination_name_wide = name.encode_wide().collect::<Vec<_>>();
    let header_size = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let buffer_size = header_size
        .checked_add(
            destination_name_wide
                .len()
                .saturating_mul(std::mem::size_of::<u16>()),
        )
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "filename is too long"))?;
    let mut rename_buffer = vec![0_usize; buffer_size.div_ceil(std::mem::size_of::<usize>())];
    let rename_info = rename_buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*rename_info).Anonymous.ReplaceIfExists = true;
        (*rename_info).RootDirectory = destination_handle.as_raw_handle();
        (*rename_info).FileNameLength =
            u32::try_from(destination_name_wide.len().saturating_mul(2))
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "filename is too long"))?;
        std::ptr::copy_nonoverlapping(
            destination_name_wide.as_ptr(),
            (*rename_info).FileName.as_mut_ptr(),
            destination_name_wide.len(),
        );
        if SetFileInformationByHandle(
            temporary_file.as_raw_handle(),
            FileRenameInfo,
            rename_info.cast(),
            u32::try_from(buffer_size).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "rename buffer is too large")
            })?,
        ) == 0
        {
            let error = io::Error::last_os_error();
            let _ = mark_windows_file_handle_for_deletion(&temporary_file);
            return Err(error);
        }
    }
    let moved = directory
        .open_plain_file(name)?
        .ok_or_else(|| io::Error::other("atomically written private file is not visible"))?;
    if expected != windows_file_identity(&moved)? {
        return Err(io::Error::other(
            "private file identity changed during anchored atomic write",
        ));
    }
    drop(temporary_name);
    Ok(())
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn move_plain_file_to_impl(
    source: &PrivateFileDirectory,
    source_name: &std::ffi::OsStr,
    destination: &PrivateFileDirectory,
    destination_name: &std::ffi::OsStr,
) -> io::Result<MovePlainFileOutcome> {
    use std::os::fd::AsRawFd as _;

    let source_file = source.open_plain_file(source_name)?;
    let destination_file = destination.open_plain_file(destination_name)?;
    match (source_file.as_ref(), destination_file.as_ref()) {
        (None, None) => return Ok(MovePlainFileOutcome::Missing),
        (None, Some(_)) => return Ok(MovePlainFileOutcome::AlreadyMoved),
        (Some(_), Some(_)) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "both source and destination private files exist",
            ))
        }
        (Some(_), None) => {}
    }
    let source_name_c = unix_name(source_name)?;
    let destination_name_c = unix_name(destination_name)?;
    #[cfg(target_os = "linux")]
    let rename_result = unsafe {
        libc::renameat2(
            source.directory_handle.as_raw_fd(),
            source_name_c.as_ptr(),
            destination.directory_handle.as_raw_fd(),
            destination_name_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let rename_result = unsafe {
        libc::renameatx_np(
            source.directory_handle.as_raw_fd(),
            source_name_c.as_ptr(),
            destination.directory_handle.as_raw_fd(),
            destination_name_c.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if rename_result != 0 {
        return Err(io::Error::last_os_error());
    }
    let moved = destination
        .open_plain_file(destination_name)?
        .ok_or_else(|| io::Error::other("moved private file is not visible"))?;
    let source_file = source_file.expect("source file checked above");
    use std::os::unix::fs::MetadataExt as _;
    let expected = source_file.metadata()?;
    let actual = moved.metadata()?;
    if expected.dev() != actual.dev() || expected.ino() != actual.ino() {
        return Err(io::Error::other(
            "private file identity changed during anchored rename",
        ));
    }
    Ok(MovePlainFileOutcome::Moved)
}

#[cfg(windows)]
fn move_plain_file_to_impl(
    source: &PrivateFileDirectory,
    source_name: &std::ffi::OsStr,
    destination: &PrivateFileDirectory,
    destination_name: &std::ffi::OsStr,
) -> io::Result<MovePlainFileOutcome> {
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FileRenameInfo, SetFileInformationByHandle, DELETE, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_GENERIC_READ, FILE_RENAME_INFO, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut source_options = std::fs::OpenOptions::new();
    source_options
        .access_mode(FILE_GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let source_file = match source_options.open(source.canonical_path.join(source_name)) {
        Ok(file) if metadata_is_plain_file(&file.metadata()?) => Some(file),
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "private source is a reparse point or non-file",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let destination_file = destination.open_plain_file(destination_name)?;
    match (source_file.as_ref(), destination_file.as_ref()) {
        (None, None) => return Ok(MovePlainFileOutcome::Missing),
        (None, Some(_)) => return Ok(MovePlainFileOutcome::AlreadyMoved),
        (Some(_), Some(_)) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "both source and destination private files exist",
            ))
        }
        (Some(_), None) => {}
    }
    let source_file = source_file.as_ref().expect("source checked above");
    let expected = windows_file_identity(source_file)?;
    let destination_handle = destination
        ._component_handles
        .last()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination has no handle"))?;
    let destination_name_wide = destination_name.encode_wide().collect::<Vec<_>>();
    let header_size = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let buffer_size = header_size
        .checked_add(
            destination_name_wide
                .len()
                .saturating_mul(std::mem::size_of::<u16>()),
        )
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "destination name is too long")
        })?;
    let mut rename_buffer = vec![0_usize; buffer_size.div_ceil(std::mem::size_of::<usize>())];
    let rename_info = rename_buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*rename_info).Anonymous.ReplaceIfExists = false;
        (*rename_info).RootDirectory = destination_handle.as_raw_handle();
        (*rename_info).FileNameLength =
            u32::try_from(destination_name_wide.len().saturating_mul(2)).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "destination name is too long")
            })?;
        std::ptr::copy_nonoverlapping(
            destination_name_wide.as_ptr(),
            (*rename_info).FileName.as_mut_ptr(),
            destination_name_wide.len(),
        );
        if SetFileInformationByHandle(
            source_file.as_raw_handle(),
            FileRenameInfo,
            rename_info.cast(),
            u32::try_from(buffer_size).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "rename buffer is too large")
            })?,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    let moved = destination
        .open_plain_file(destination_name)?
        .ok_or_else(|| io::Error::other("moved private file is not visible"))?;
    if expected != windows_file_identity(&moved)? {
        return Err(io::Error::other(
            "private file identity changed during anchored rename",
        ));
    }
    Ok(MovePlainFileOutcome::Moved)
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn remove_empty_child_directory_impl(
    directory: &PrivateFileDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<bool> {
    use std::os::fd::AsRawFd as _;

    let name = unix_name(name)?;
    if unsafe {
        libc::unlinkat(
            directory.directory_handle.as_raw_fd(),
            name.as_ptr(),
            libc::AT_REMOVEDIR,
        )
    } != 0
    {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::NotFound {
            Ok(false)
        } else {
            Err(error)
        };
    }
    Ok(true)
}

#[cfg(windows)]
fn remove_empty_child_directory_impl(
    directory: &PrivateFileDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<bool> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut options = std::fs::OpenOptions::new();
    options
        .access_mode(FILE_GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let file = match options.open(directory.canonical_path.join(name)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata_is_real_directory(&file.metadata()?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private child is a reparse point or non-directory",
        ));
    }
    mark_windows_file_handle_for_deletion(&file)?;
    drop(file);
    Ok(true)
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
struct UnixDirectoryStream(*mut libc::DIR);

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
impl Drop for UnixDirectoryStream {
    fn drop(&mut self) {
        unsafe { libc::closedir(self.0) };
    }
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn next_unix_directory_entry_with<ClearErrno, ReadEntry, ReadErrno>(
    clear_errno: ClearErrno,
    read_entry: ReadEntry,
    read_errno: ReadErrno,
) -> io::Result<Option<std::ffi::OsString>>
where
    ClearErrno: FnOnce(),
    ReadEntry: FnOnce() -> Option<std::ffi::OsString>,
    ReadErrno: FnOnce() -> i32,
{
    clear_errno();
    match read_entry() {
        Some(name) => Ok(Some(name)),
        None => match read_errno() {
            0 => Ok(None),
            errno => Err(io::Error::from_raw_os_error(errno)),
        },
    }
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn next_unix_directory_entry(
    stream: &UnixDirectoryStream,
) -> io::Result<Option<std::ffi::OsString>> {
    use std::os::unix::ffi::OsStringExt as _;

    next_unix_directory_entry_with(
        || unsafe { *libc::errno_location() = 0 },
        || {
            let entry = unsafe { libc::readdir(stream.0) };
            if entry.is_null() {
                None
            } else {
                let bytes =
                    unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
                Some(std::ffi::OsString::from_vec(bytes.to_vec()))
            }
        },
        || unsafe { *libc::errno_location() },
    )
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn entry_names_impl(directory: &PrivateFileDirectory) -> io::Result<Vec<std::ffi::OsString>> {
    use std::os::fd::AsRawFd as _;

    let current = std::ffi::CString::new(".").expect("literal contains no NUL");
    let duplicate = unsafe {
        libc::openat(
            directory.directory_handle.as_raw_fd(),
            current.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if duplicate < 0 {
        return Err(io::Error::last_os_error());
    }
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        let error = io::Error::last_os_error();
        unsafe { libc::close(duplicate) };
        return Err(error);
    }
    let stream = UnixDirectoryStream(stream);
    let mut names = Vec::new();
    while let Some(name) = next_unix_directory_entry(&stream)? {
        if name != "." && name != ".." {
            names.push(name);
        }
    }
    Ok(names)
}

#[cfg(windows)]
fn entry_names_impl(directory: &PrivateFileDirectory) -> io::Result<Vec<std::ffi::OsString>> {
    std::fs::read_dir(&directory.canonical_path)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect()
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn unix_name(name: &std::ffi::OsStr) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt as _;
    std::ffi::CString::new(name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "private file name contains NUL",
        )
    })
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn unix_named_file_matches(
    directory: &PrivateFileDirectory,
    name: &std::ffi::CString,
    file: &File,
) -> io::Result<bool> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::fs::MetadataExt as _;

    let named_fd = unsafe {
        libc::openat(
            directory.directory_handle.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if named_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let named = unsafe { File::from_raw_fd(named_fd) }.metadata()?;
    let opened = file.metadata()?;
    Ok(opened.file_type().is_file()
        && opened.nlink() == 1
        && opened.dev() == named.dev()
        && opened.ino() == named.ino())
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn create_delete_on_close_file_impl(
    directory: &PrivateFileDirectory,
    name: &str,
) -> io::Result<File> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};

    let name = unix_name(std::ffi::OsStr::new(name))?;
    let fd = unsafe {
        libc::openat(
            directory.directory_handle.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    if !unix_named_file_matches(directory, &name, &file)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "created private file changed before unlink",
        ));
    }
    if unsafe { libc::unlinkat(directory.directory_handle.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

#[cfg(windows)]
fn create_delete_on_close_file_impl(
    directory: &PrivateFileDirectory,
    name: &str,
) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let path = directory.canonical_path.join(name);
    let mut options = std::fs::OpenOptions::new();
    options
        .write(true)
        .access_mode(FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE)
        .create_new(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path)?;
    if !metadata_is_plain_file(&file.metadata()?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "created private file is a reparse point or non-file",
        ));
    }
    mark_windows_file_handle_for_deletion(&file)?;
    Ok(file)
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn remove_plain_file_impl(
    directory: &PrivateFileDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<bool> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};

    let name = unix_name(name)?;
    let fd = unsafe {
        libc::openat(
            directory.directory_handle.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::NotFound {
            Ok(false)
        } else {
            Err(error)
        };
    }
    let file = unsafe { File::from_raw_fd(fd) };
    if !unix_named_file_matches(directory, &name, &file)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private entry is non-regular, multiply linked, or changed during removal",
        ));
    }
    if unsafe { libc::unlinkat(directory.directory_handle.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::NotFound {
            Ok(false)
        } else {
            Err(error)
        };
    }
    Ok(true)
}

#[cfg(windows)]
fn remove_plain_file_impl(
    directory: &PrivateFileDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<bool> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut options = std::fs::OpenOptions::new();
    options
        .access_mode(FILE_GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = match options.open(directory.canonical_path.join(name)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata_is_plain_file(&file.metadata()?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private entry is a reparse point or non-file",
        ));
    }
    mark_windows_file_handle_for_deletion(&file)?;
    drop(file);
    Ok(true)
}

#[cfg(windows)]
fn mark_windows_file_handle_for_deletion(file: &File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };

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

/// Create an app-owned directory without a permissive Unix creation window,
/// then reject symlink, junction, or other Windows reparse-point roots.
#[cfg(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
pub(crate) fn ensure_private_real_directory(path: &Path) -> io::Result<RealDirectoryIdentity> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let builder = private_directory_builder();
            match builder.create(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    }

    let identity = real_directory_identity(path)?;
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        let expected = std::fs::canonicalize(parent)?.join(name);
        if identity.canonical_path != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "private directory resolves outside its app-owned parent: {}",
                    path.display()
                ),
            ));
        }
    }
    Ok(identity)
}

#[cfg(any(
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn private_directory_builder() -> std::fs::DirBuilder {
    use std::os::unix::fs::DirBuilderExt as _;

    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    builder
}

#[cfg(windows)]
fn private_directory_builder() -> std::fs::DirBuilder {
    std::fs::DirBuilder::new()
}

#[cfg(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn real_directory_identity(path: &Path) -> io::Result<RealDirectoryIdentity> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata_is_real_directory(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is not a real directory: {}", path.display()),
        ));
    }
    #[cfg(windows)]
    // Capture the identity from the original, non-following path before
    // canonicalization. If the root is exchanged for a junction between these
    // steps, the identity cannot accidentally describe the junction target and
    // the stable component-chain acquisition below rejects the mismatch.
    let (volume_serial_number, file_index) = windows_directory_identity(path)?;
    let canonical_path = std::fs::canonicalize(path)?;
    Ok(RealDirectoryIdentity {
        canonical_path,
        #[cfg(any(
            target_os = "macos",
            all(target_os = "linux", target_pointer_width = "64")
        ))]
        device: {
            use std::os::unix::fs::MetadataExt as _;
            metadata.dev()
        },
        #[cfg(any(
            target_os = "macos",
            all(target_os = "linux", target_pointer_width = "64")
        ))]
        inode: {
            use std::os::unix::fs::MetadataExt as _;
            metadata.ino()
        },
        #[cfg(windows)]
        volume_serial_number,
        #[cfg(windows)]
        file_index,
    })
}

#[cfg(windows)]
fn windows_directory_identity(path: &Path) -> io::Result<(u32, u64)> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut options = std::fs::OpenOptions::new();
    options
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let handle = options.open(path)?;
    windows_file_identity(&handle)
}

#[cfg(windows)]
fn windows_file_identity(file: &File) -> io::Result<(u32, u64)> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut information = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let information = unsafe { information.assume_init() };
    let file_index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok((information.dwVolumeSerialNumber, file_index))
}

#[cfg(windows)]
fn metadata_is_plain_file(metadata: &std::fs::Metadata) -> bool {
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    use std::os::windows::fs::MetadataExt as _;
    windows_attributes_are_not_reparse(metadata.file_attributes())
}

#[cfg(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", target_pointer_width = "64")
))]
fn metadata_is_real_directory(metadata: &std::fs::Metadata) -> bool {
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        return windows_attributes_are_not_reparse(metadata.file_attributes());
    }
    #[cfg(not(windows))]
    true
}

#[cfg(windows)]
fn windows_attributes_are_not_reparse(attributes: u32) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
}

#[cfg(windows)]
fn replace_file_atomically_impl(tmp: &Path, target: &Path, backup: &Path) -> ReplaceResult {
    replace_file_atomically_with(tmp, target, backup, system_replace_file)
}

#[cfg(windows)]
fn system_replace_file(target: &Path, replacement: &Path, backup: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};

    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>()
    };
    let target_wide = wide(target);
    let tmp_wide = wide(replacement);
    let backup_wide = wide(backup);
    let replaced = unsafe {
        ReplaceFileW(
            target_wide.as_ptr(),
            tmp_wide.as_ptr(),
            backup_wide.as_ptr(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
pub(crate) fn replace_file_atomically_with<F>(
    tmp: &Path,
    target: &Path,
    backup: &Path,
    replace: F,
) -> ReplaceResult
where
    F: FnOnce(&Path, &Path, &Path) -> io::Result<()>,
{
    if !target.exists() {
        return promote_replacement(tmp, target);
    }

    // The target is authoritative before ReplaceFileW starts. A stale backup
    // from an earlier completed operation is no longer a recovery candidate.
    let _ = std::fs::remove_file(backup);
    match replace(target, tmp, backup) {
        Ok(()) => {
            let _ = std::fs::remove_file(backup);
            Ok(ReplaceState::Committed)
        }
        Err(error) => converge_failed_windows_replace(tmp, target, backup, error),
    }
}

fn promote_replacement(replacement: &Path, target: &Path) -> ReplaceResult {
    std::fs::rename(replacement, target).map_or_else(
        |error| {
            let state = if target.is_file() {
                ReplaceState::RolledBack
            } else {
                ReplaceState::RecoveryRequired
            };
            Err(ReplaceError::new(state, error))
        },
        |_| Ok(ReplaceState::Committed),
    )
}

#[cfg(windows)]
fn converge_failed_windows_replace(
    replacement: &Path,
    target: &Path,
    backup: &Path,
    replace_error: io::Error,
) -> ReplaceResult {
    // ReplaceFileW documents partially-mutated layouts for errors 1175/1176/1177.
    // First restore an authoritative name. Until that is confirmed, neither rollback nor
    // replacement is deleted.
    if !target.is_file() {
        if backup.is_file() {
            if let Err(restore_error) = promote_replacement(backup, target) {
                return Err(ReplaceError::new(
                    ReplaceState::RecoveryRequired,
                    io::Error::new(
                        io::ErrorKind::Other,
                        format!(
                            "replace failed ({replace_error}); restoring backup failed: {restore_error}"
                        ),
                    ),
                ));
            }
        } else if replacement.is_file() {
            // No old authority survived, so the durable replacement is the only recoverable
            // candidate. Promoting it completes the write rather than returning an empty state.
            return promote_replacement(replacement, target);
        }
    }

    if !target.is_file() {
        return Err(ReplaceError::new(
            ReplaceState::RecoveryRequired,
            io::Error::new(
                replace_error.kind(),
                format!("atomic replace left no authoritative file: {replace_error}"),
            ),
        ));
    }

    // The old target either retained its name or was restored from backup.
    // Preserve every remaining candidate; callers may clean them only after
    // observing RolledBack and completing their own lifecycle transaction.
    Err(ReplaceError::new(ReplaceState::RolledBack, replace_error))
}

#[cfg(not(windows))]
fn replace_file_atomically_impl(tmp: &Path, target: &Path, _backup: &Path) -> ReplaceResult {
    promote_replacement(tmp, target)
}

pub(crate) fn reserved_target_is_unchanged(file: &File, path: &Path) -> bool {
    reserved_target_is_unchanged_impl(file, path)
}

#[cfg(unix)]
fn reserved_target_is_unchanged_impl(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    let (Ok(opened), Ok(named)) = (file.metadata(), std::fs::symlink_metadata(path)) else {
        return false;
    };
    named.file_type().is_file() && opened.dev() == named.dev() && opened.ino() == named.ino()
}

#[cfg(windows)]
fn reserved_target_is_unchanged_impl(_file: &File, path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt as _;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let Ok(named) = std::fs::symlink_metadata(path) else {
        return false;
    };
    named.file_type().is_file() && named.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
}

#[cfg(not(any(unix, windows)))]
fn reserved_target_is_unchanged_impl(_file: &File, path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use super::{atomic_write, atomic_write_private, is_executable_file};
    #[cfg(any(
        windows,
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    use super::{ensure_private_real_directory, open_private_file_directory};

    #[test]
    fn darwin_x86_64_directory_symbols_are_inode64_qualified() {
        let source = include_str!("filesystem.rs");
        let inode64 = ["$", "INODE64"].concat();
        for function in ["fdopendir", "readdir"] {
            let binding = format!("link_name = \"{function}{inode64}\"");
            assert_eq!(source.matches(&binding).count(), 1, "missing {binding}");
        }
        let x86_64_cfg = [
            "all(target_os = \"mac",
            "os\", target_arch = \"x86_",
            "64\")",
        ]
        .concat();
        assert_eq!(source.matches(&x86_64_cfg).count(), 2);
        let darwin_errno = ["__err", "or"].concat();
        let linux_errno = ["__errno_", "location"].concat();
        for errno_symbol in [darwin_errno, linux_errno] {
            let binding = format!("link_name = \"{errno_symbol}\"");
            assert_eq!(source.matches(&binding).count(), 1, "missing {binding}");
        }
    }

    #[test]
    fn unsupported_private_directory_contract_has_no_path_mutations() {
        let source = include_str!("filesystem.rs");
        let start = ["// unsupported-private-file-directory:", "start"].concat();
        let end = ["// unsupported-private-file-directory:", "end"].concat();
        let block = source
            .split_once(&start)
            .and_then(|(_, rest)| rest.split_once(&end).map(|(block, _)| block))
            .expect("unsupported private directory contract block");

        assert_eq!(block.matches("io::ErrorKind::Unsupported").count(), 1);
        assert_eq!(
            block
                .matches("private_file_directory_unsupported()")
                .count(),
            13
        );
        for forbidden in [
            "std::fs::",
            "OpenOptions",
            ".create_new(",
            "remove_file(",
            "read_dir(",
            ".join(",
        ] {
            assert!(
                !block.contains(forbidden),
                "forbidden fallback: {forbidden}"
            );
        }
    }

    #[cfg(any(
        windows,
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    #[test]
    fn windows_and_posix_private_directory_moves_and_removes_files_through_stable_handles() {
        use std::ffi::OsStr;
        use std::io::Read as _;

        let temp = tempfile::tempdir().unwrap();
        let root_path = temp.path().join("root");
        let root = open_private_file_directory(&root_path).unwrap();
        root.atomic_write_private_file(OsStr::new("state.json"), b"old")
            .unwrap();
        root.atomic_write_private_file(OsStr::new("state.json"), b"new")
            .unwrap();
        let mut state = root
            .open_plain_file(OsStr::new("state.json"))
            .unwrap()
            .unwrap();
        let mut state_raw = Vec::new();
        state.read_to_end(&mut state_raw).unwrap();
        assert_eq!(state_raw, b"new");
        drop(state);
        assert!(root.remove_plain_file(OsStr::new("state.json")).unwrap());
        std::fs::create_dir(root_path.join("non-file-entry")).unwrap();
        assert!(root
            .remove_plain_file(OsStr::new("non-file-entry"))
            .is_err());
        assert!(root_path.join("non-file-entry").is_dir());
        std::fs::remove_dir(root_path.join("non-file-entry")).unwrap();
        #[cfg(unix)]
        {
            std::fs::write(root_path.join("linked-source"), b"linked").unwrap();
            std::fs::hard_link(
                root_path.join("linked-source"),
                root_path.join("linked-entry"),
            )
            .unwrap();
            assert!(root.remove_plain_file(OsStr::new("linked-entry")).is_err());
            assert!(root_path.join("linked-source").is_file());
            assert!(root_path.join("linked-entry").is_file());
            std::fs::remove_file(root_path.join("linked-entry")).unwrap();
            std::fs::remove_file(root_path.join("linked-source")).unwrap();
        }
        std::fs::write(root_path.join("source"), b"anchored").unwrap();
        let child = root
            .create_private_child_directory(OsStr::new("child"))
            .unwrap();

        assert_eq!(
            root.move_plain_file_to(OsStr::new("source"), &child, OsStr::new("destination"))
                .unwrap(),
            super::MovePlainFileOutcome::Moved
        );
        assert!(root
            .open_plain_file(OsStr::new("source"))
            .unwrap()
            .is_none());
        let mut moved = child
            .open_plain_file(OsStr::new("destination"))
            .unwrap()
            .unwrap();
        let mut raw = Vec::new();
        moved.read_to_end(&mut raw).unwrap();
        assert_eq!(raw, b"anchored");
        drop(moved);
        assert!(child.remove_plain_file(OsStr::new("destination")).unwrap());
        drop(child);
        assert!(root
            .remove_empty_child_directory(OsStr::new("child"))
            .unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn anchored_directory_does_not_follow_replacement_root() {
        use std::ffi::OsStr;
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root_path = temp.path().join("root");
        let parked_path = temp.path().join("parked");
        let outside_path = temp.path().join("outside");
        std::fs::create_dir_all(&root_path).unwrap();
        std::fs::create_dir_all(&outside_path).unwrap();
        std::fs::write(root_path.join("victim"), b"inside").unwrap();
        std::fs::write(outside_path.join("victim"), b"outside").unwrap();
        let root = open_private_file_directory(&root_path).unwrap();

        std::fs::rename(&root_path, &parked_path).unwrap();
        symlink(&outside_path, &root_path).unwrap();
        assert!(root.remove_plain_file(OsStr::new("victim")).unwrap());
        root.atomic_write_private_file(OsStr::new("state.json"), b"anchored-write")
            .unwrap();

        assert!(!parked_path.join("victim").exists());
        assert_eq!(
            std::fs::read(parked_path.join("state.json")).unwrap(),
            b"anchored-write"
        );
        assert!(!outside_path.join("state.json").exists());
        assert_eq!(
            std::fs::read(outside_path.join("victim")).unwrap(),
            b"outside"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_private_directory_handle_blocks_root_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let root_path = temp.path().join("root");
        let parked_path = temp.path().join("parked");
        let root = open_private_file_directory(&root_path).unwrap();

        assert!(std::fs::rename(&root_path, &parked_path).is_err());
        drop(root);
        std::fs::rename(&root_path, &parked_path).unwrap();
    }

    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    #[test]
    fn readdir_null_propagates_errno_after_clearing_it() {
        let cleared = std::cell::Cell::new(false);
        let error = super::next_unix_directory_entry_with(
            || cleared.set(true),
            || {
                assert!(cleared.get(), "errno must be cleared before readdir");
                None
            },
            || 5,
        )
        .unwrap_err();

        assert_eq!(error.raw_os_error(), Some(5));
    }

    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    #[test]
    fn readdir_null_with_clear_errno_is_eof() {
        let result = super::next_unix_directory_entry_with(|| {}, || None, || 0).unwrap();
        assert!(result.is_none());
    }

    #[cfg(any(
        windows,
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    #[test]
    fn delete_on_close_file_round_trips_only_through_its_handle() {
        use std::io::{Read as _, Seek as _, Write as _};

        let parent = std::env::temp_dir().join(format!(
            "pinvou3-delete-on-close-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let directory_path = parent.join("owned");
        let directory = open_private_file_directory(&directory_path).unwrap();
        let mut file = directory
            .create_delete_on_close_file("download_round_trip.json")
            .unwrap();
        file.write_all(b"handle-only").unwrap();
        file.seek(std::io::SeekFrom::Start(0)).unwrap();
        let mut content = String::new();
        file.read_to_string(&mut content).unwrap();
        assert_eq!(content, "handle-only");

        drop(file);
        drop(directory);
        assert!(!directory_path.join("download_round_trip.json").exists());
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(any(
        windows,
        target_os = "macos",
        all(target_os = "linux", target_pointer_width = "64")
    ))]
    #[test]
    fn private_real_directory_is_created_without_a_link_root() {
        let parent = std::env::temp_dir().join(format!(
            "pinvou3-private-dir-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let directory = parent.join("owned");

        ensure_private_real_directory(&directory).unwrap();

        let metadata = std::fs::symlink_metadata(&directory).unwrap();
        assert!(metadata.is_dir());
        assert!(!metadata.file_type().is_symlink());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
        }
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_reparse_attribute_is_rejected() {
        assert!(!super::windows_attributes_are_not_reparse(0x400));
        assert!(super::windows_attributes_are_not_reparse(0));
    }

    #[test]
    fn executable_file_check_rejects_directories_and_unix_non_executable_files() {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-executable-file-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("runtime");
        std::fs::write(&file, b"runtime").unwrap();
        assert!(!is_executable_file(&dir));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(!is_executable_file(&file));
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(is_executable_file(&file));
        }
        #[cfg(not(unix))]
        assert!(is_executable_file(&file));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn atomic_write_round_trips_content() {
        let dir = std::env::temp_dir().join(format!("pinvou3-atomic-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");

        // 首次创建（目标不存在）：Windows 上必须走普通 rename，不能失败。
        let target = dir.join("_code_mode_states.json");
        atomic_write(&target, br#"{"session-1":"yolo"}"#).expect("first write");
        assert_eq!(
            std::fs::read_to_string(&target).expect("read back"),
            r#"{"session-1":"yolo"}"#
        );

        // 覆盖写入仍保持完整内容（原子替换语义）。
        atomic_write(&target, br#"{"session-1":"plan","session-2":"yolo"}"#).expect("rewrite");
        assert_eq!(
            std::fs::read_to_string(&target).expect("read rewritten"),
            r#"{"session-1":"plan","session-2":"yolo"}"#
        );

        // 成功后不残留临时/备份文件。
        let leftover: Vec<_> = std::fs::read_dir(&dir)
            .expect("list dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name != "_code_mode_states.json"
            })
            .collect();
        assert!(leftover.is_empty(), "leftover files: {leftover:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn private_atomic_write_replaces_existing_content() {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-private-atomic-write-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("secret.json");

        atomic_write_private(&path, b"old").expect("initial private write");
        atomic_write_private(&path, b"new").expect("replace private write");
        assert_eq!(std::fs::read(&path).expect("read target"), b"new");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&path)
                    .expect("target metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_failure_is_reported_without_residue() {
        let dir =
            std::env::temp_dir().join(format!("pinvou3-atomic-write-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");

        // 父级是普通文件 → tmp 无法创建，写入必须报错且不留任何残留。
        let file_as_parent = dir.join("not-a-dir");
        std::fs::write(&file_as_parent, "x").expect("seed parent file");
        let target = file_as_parent.join("state.json");
        assert!(atomic_write(&target, b"new").is_err());
        assert_eq!(
            std::fs::read_to_string(&file_as_parent).expect("parent untouched"),
            "x"
        );

        let leftover: Vec<_> = std::fs::read_dir(&dir)
            .expect("list dir")
            .filter_map(|entry| entry.ok())
            .collect();
        assert_eq!(leftover.len(), 1, "no tmp/backup residue: {leftover:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    pub(crate) fn try_link_file(target: &Path, link: &Path) -> bool {
        try_link_file_impl(target, link)
    }

    pub(crate) fn try_link_dir(target: &Path, link: &Path) -> bool {
        try_link_dir_impl(target, link)
    }

    pub(crate) fn remove_dir_link(link: &Path) {
        remove_dir_link_impl(link)
    }

    #[test]
    fn failed_atomic_replace_preserves_the_authoritative_target() {
        let root = std::env::temp_dir().join(format!(
            "pinvou-atomic-replace-failure-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("profile.json");
        let missing_tmp = root.join("missing.tmp");
        let backup = root.join("profile.bak");
        std::fs::write(&target, "authoritative").unwrap();

        assert!(super::replace_file_atomically(&missing_tmp, &target, &backup).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "authoritative");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    fn windows_replace_fixture(
        name: &str,
    ) -> (
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let root = std::env::temp_dir().join(format!(
            "pinvou-windows-replace-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("memory.json");
        let replacement = root.join("memory.tmp");
        let backup = root.join("memory.bak");
        (root, target, replacement, backup)
    }

    #[cfg(windows)]
    #[test]
    fn windows_atomic_replace_covers_first_write_and_success_cleanup() {
        let (root, target, replacement, backup) = windows_replace_fixture("success");
        std::fs::write(&replacement, "first").unwrap();
        super::replace_file_atomically(&replacement, &target, &backup).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "first");

        std::fs::write(&replacement, "second").unwrap();
        super::replace_file_atomically(&replacement, &target, &backup).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "second");
        assert!(!replacement.exists());
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_1175_retains_the_two_official_original_names() {
        let (root, target, replacement, backup) = windows_replace_fixture("1175");
        std::fs::write(&target, "old").unwrap();
        std::fs::write(&replacement, "new").unwrap();

        let error =
            super::replace_file_atomically_with(&replacement, &target, &backup, |_, _, _| {
                Err(std::io::Error::from_raw_os_error(1175))
            })
            .unwrap_err();

        assert_eq!(error.state(), super::ReplaceState::RolledBack);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(&replacement).unwrap(), "new");
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_1176_with_backup_name_retains_the_two_official_original_names() {
        let (root, target, replacement, backup) = windows_replace_fixture("1176");
        std::fs::write(&target, "old").unwrap();
        std::fs::write(&replacement, "new").unwrap();

        let error =
            super::replace_file_atomically_with(&replacement, &target, &backup, |_, _, _| {
                Err(std::io::Error::from_raw_os_error(1176))
            })
            .unwrap_err();

        assert_eq!(error.state(), super::ReplaceState::RolledBack);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(&replacement).unwrap(), "new");
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_1177_restores_the_official_backup_layout() {
        let (root, target, replacement, backup) = windows_replace_fixture("1177");
        std::fs::write(&target, "old").unwrap();
        std::fs::write(&replacement, "new").unwrap();

        let error = super::replace_file_atomically_with(
            &replacement,
            &target,
            &backup,
            |target, _, backup| {
                std::fs::rename(target, backup)?;
                Err(std::io::Error::from_raw_os_error(1177))
            },
        )
        .unwrap_err();

        assert_eq!(error.state(), super::ReplaceState::RolledBack);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(&replacement).unwrap(), "new");
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_partial_replace_promotes_only_surviving_replacement() {
        let (root, target, replacement, backup) = windows_replace_fixture("replacement-only");
        std::fs::write(&replacement, "new").unwrap();
        let state = super::converge_failed_windows_replace(
            &replacement,
            &target,
            &backup,
            std::io::Error::from_raw_os_error(1177),
        )
        .unwrap();
        assert_eq!(state, super::ReplaceState::Committed);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert!(!replacement.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_partial_replace_keeps_candidates_when_no_authority_can_be_restored() {
        let (root, target, replacement, backup) = windows_replace_fixture("no-candidate");
        let result = super::converge_failed_windows_replace(
            &replacement,
            &target,
            &backup,
            std::io::Error::from_raw_os_error(1176),
        );
        assert!(result.is_err());
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_partial_replace_preserves_occupied_recovery_candidates() {
        use std::cell::RefCell;
        use std::os::windows::fs::OpenOptionsExt as _;

        let (root, target, replacement, backup) = windows_replace_fixture("occupied-backup");
        std::fs::write(&target, "old").unwrap();
        std::fs::write(&replacement, "new").unwrap();
        let occupied = RefCell::new(None);
        let error = super::replace_file_atomically_with(
            &replacement,
            &target,
            &backup,
            |target, _, backup| {
                std::fs::rename(target, backup)?;
                *occupied.borrow_mut() = Some(
                    std::fs::OpenOptions::new()
                        .read(true)
                        .share_mode(0)
                        .open(backup)?,
                );
                Err(std::io::Error::from_raw_os_error(1177))
            },
        )
        .unwrap_err();
        assert_eq!(error.state(), super::ReplaceState::RecoveryRequired);
        assert!(!target.exists());
        assert!(replacement.exists());
        assert!(backup.exists());
        drop(occupied.borrow_mut().take());

        let recovered =
            super::recover_interrupted_replace(&replacement, &target, &backup).unwrap_err();
        assert_eq!(recovered.state(), super::ReplaceState::RolledBack);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(&replacement).unwrap(), "new");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_failed_first_promotion_preserves_the_complete_replacement() {
        let (root, target, replacement, backup) = windows_replace_fixture("promotion-blocked");
        std::fs::write(&replacement, "complete-new-value").unwrap();
        // 目标名被目录占用:target.exists() 为 true,走 ReplaceFileW(目标是目录,
        // 必然失败)→ 收敛分支 promote_replacement,rename(文件→目录名)同样必然
        // ERROR_ACCESS_DENIED,且 target.is_file() 为 false → RecoveryRequired。
        // 此前版本用 share_mode(0) 独占打开 replacement 期望首写提升的 rename
        // 失败,但 NTFS 的 rename 只改目录项、不受源文件独占句柄影响,Windows CI
        // 上 rename 直接成功,前提不成立(与 memory 侧
        // memory_write_cleans_tmp_backup_on_permanently_occupied_target 同构)。
        std::fs::create_dir(&target).unwrap();

        let error = super::replace_file_atomically(&replacement, &target, &backup).unwrap_err();

        assert_eq!(error.state(), super::ReplaceState::RecoveryRequired);
        assert!(target.is_dir());
        assert_eq!(
            std::fs::read_to_string(&replacement).unwrap(),
            "complete-new-value"
        );
        assert!(!backup.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_atomic_replace_never_exposes_a_partial_target_to_readers() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let (root, target, replacement, backup) = windows_replace_fixture("concurrent-reader");
        let old = "a".repeat(32 * 1024);
        let new = "b".repeat(32 * 1024);
        std::fs::write(&target, &old).unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let reader_running = Arc::clone(&running);
        let reader_target = target.clone();
        let old_for_reader = old.clone();
        let new_for_reader = new.clone();
        let reader = std::thread::spawn(move || {
            while reader_running.load(Ordering::Acquire) {
                if let Ok(value) = std::fs::read_to_string(&reader_target) {
                    assert!(value == old_for_reader || value == new_for_reader);
                }
            }
        });

        for index in 0..32 {
            let value = if index % 2 == 0 { &new } else { &old };
            std::fs::write(&replacement, value).unwrap();
            super::replace_file_atomically(&replacement, &target, &backup).unwrap();
        }
        running.store(false, Ordering::Release);
        reader.join().unwrap();
        let final_value = std::fs::read_to_string(&target).unwrap();
        assert!(final_value == old || final_value == new);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    fn try_link_file_impl(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn try_link_file_impl(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }

    #[cfg(not(any(unix, windows)))]
    fn try_link_file_impl(_target: &Path, _link: &Path) -> bool {
        false
    }

    #[cfg(unix)]
    fn try_link_dir_impl(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn try_link_dir_impl(target: &Path, link: &Path) -> bool {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return true;
        }
        std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                &link.to_string_lossy(),
                &target.to_string_lossy(),
            ])
            .status()
            .is_ok_and(|status| status.success())
    }

    #[cfg(not(any(unix, windows)))]
    fn try_link_dir_impl(_target: &Path, _link: &Path) -> bool {
        false
    }

    #[cfg(windows)]
    fn remove_dir_link_impl(link: &Path) {
        let _ = std::fs::remove_dir(link);
    }

    #[cfg(not(windows))]
    fn remove_dir_link_impl(link: &Path) {
        let _ = std::fs::remove_file(link);
    }
}
