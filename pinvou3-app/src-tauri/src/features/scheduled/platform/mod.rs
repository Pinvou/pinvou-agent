//! Native filesystem adaptation for scheduled-request evidence.

use std::{io, path::Path};

/// Atomically move a file without replacing an existing destination. Native
/// no-replace rename operations keep this usable on filesystems without hard
/// links. Other platforms explicitly return Unsupported; callers retain the
/// source and can retry or park it without losing either evidence file.
pub(super) fn move_file_noclobber(source: &Path, target: &Path) -> io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let source = std::ffi::CString::new(source.as_os_str().as_bytes())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let target = std::ffi::CString::new(target.as_os_str().as_bytes())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        #[cfg(target_os = "linux")]
        // SAFETY: both C strings are NUL-terminated and live for this call;
        // AT_FDCWD makes the supplied paths relative to the current directory.
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        // SAFETY: both C strings are NUL-terminated and live for this call.
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt as _;
        let wide = |path: &Path| -> io::Result<Vec<u16>> {
            let mut encoded: Vec<u16> = path.as_os_str().encode_wide().collect();
            if encoded.contains(&0) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "NUL in file path",
                ));
            }
            encoded.push(0);
            Ok(encoded)
        };
        let source = wide(source)?;
        let target = wide(target)?;
        // SAFETY: both UTF-16 strings are NUL-terminated and live for this
        // call. Zero flags deliberately omit MOVEFILE_REPLACE_EXISTING.
        let result = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                source.as_ptr(),
                target.as_ptr(),
                0,
            )
        };
        if result != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (source, target);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no-replace rename is unsupported on this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn noclobber_move_preserves_an_existing_destination() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        std::fs::write(&source, b"new").unwrap();
        std::fs::write(&target, b"old").unwrap();
        assert_eq!(
            super::move_file_noclobber(&source, &target)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"new");
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        std::fs::remove_file(&target).unwrap();
        super::move_file_noclobber(&source, &target).unwrap();
        assert!(!source.exists());
        assert_eq!(std::fs::read(target).unwrap(), b"new");
    }
}
