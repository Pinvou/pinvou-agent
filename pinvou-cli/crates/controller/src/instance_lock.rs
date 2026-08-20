use std::{
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::Path,
};

use crate::ControllerError;

#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
    diagnostic_pid: u32,
}

impl InstanceLock {
    pub fn acquire(path: &Path) -> Result<Self, ControllerError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = open_exclusive(path)?;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        let diagnostic_pid = std::process::id();
        write!(file, "{diagnostic_pid}")?;
        file.sync_data()?;
        Ok(Self {
            _file: file,
            diagnostic_pid,
        })
    }

    pub const fn diagnostic_pid(&self) -> u32 {
        self.diagnostic_pid
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        unsafe {
            use std::os::fd::AsRawFd;
            libc::flock(self._file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[cfg(target_os = "linux")]
fn open_exclusive(path: &Path) -> Result<File, ControllerError> {
    use std::os::fd::AsRawFd;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        Ok(file)
    } else {
        Err(ControllerError::AlreadyRunning)
    }
}

#[cfg(windows)]
fn open_exclusive(path: &Path) -> Result<File, ControllerError> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)
        .map_err(|error| match (error.kind(), error.raw_os_error()) {
            (std::io::ErrorKind::PermissionDenied, _) | (_, Some(32 | 33)) => {
                ControllerError::AlreadyRunning
            }
            _ => ControllerError::Io(error),
        })
}

#[cfg(not(any(windows, target_os = "linux")))]
fn open_exclusive(_path: &Path) -> Result<File, ControllerError> {
    Err(ControllerError::UnsupportedPlatform)
}
