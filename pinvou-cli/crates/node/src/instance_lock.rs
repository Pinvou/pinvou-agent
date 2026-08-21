use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
};

use crate::NodeError;

#[derive(Debug)]
pub struct NodeInstanceLock {
    _file: File,
    diagnostic_pid: u32,
}

impl NodeInstanceLock {
    pub fn acquire(path: &Path) -> Result<Self, NodeError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = open_exclusive(path)?;
        file.set_len(0)?;
        write!(file, "{}", std::process::id())?;
        file.sync_data()?;
        Ok(Self {
            _file: file,
            diagnostic_pid: std::process::id(),
        })
    }
    pub const fn diagnostic_pid(&self) -> u32 {
        self.diagnostic_pid
    }
}

impl Drop for NodeInstanceLock {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        unsafe {
            use std::os::fd::AsRawFd;
            let mut lock: libc::flock = std::mem::zeroed();
            lock.l_type = libc::F_UNLCK as libc::c_short;
            lock.l_whence = libc::SEEK_SET as libc::c_short;
            libc::fcntl(self._file.as_raw_fd(), libc::F_OFD_SETLK, &lock);
        }
    }
}

#[cfg(windows)]
fn open_exclusive(path: &Path) -> Result<File, NodeError> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)
        .map_err(|error| match (error.kind(), error.raw_os_error()) {
            (std::io::ErrorKind::PermissionDenied, _) | (_, Some(32 | 33)) => {
                NodeError::AlreadyRunning
            }
            _ => NodeError::Io(error),
        })
}

#[cfg(target_os = "linux")]
fn open_exclusive(path: &Path) -> Result<File, NodeError> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let fd = file.as_raw_fd();
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return Err(NodeError::Io(std::io::Error::last_os_error()));
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG || stat.st_uid != unsafe { libc::geteuid() } {
        return Err(NodeError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "node lock must be a regular file owned by the current user",
        )));
    }
    if unsafe { libc::fchmod(fd, 0o600) } != 0 {
        return Err(NodeError::Io(std::io::Error::last_os_error()));
    }
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as libc::c_short;
    lock.l_whence = libc::SEEK_SET as libc::c_short;
    if unsafe { libc::fcntl(fd, libc::F_OFD_SETLK, &lock) } == 0 {
        return Ok(file);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(code) if code == libc::EACCES || code == libc::EAGAIN => {
            Err(NodeError::AlreadyRunning)
        }
        _ => Err(NodeError::Io(error)),
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn open_exclusive(_: &Path) -> Result<File, NodeError> {
    Err(NodeError::UnsupportedPlatform)
}
