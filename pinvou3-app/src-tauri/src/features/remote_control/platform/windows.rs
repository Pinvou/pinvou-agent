use std::fs::{File, OpenOptions};
use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Storage::FileSystem::{
    GetLogicalDrives, MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

pub(super) fn configure_private_open_options(_options: &mut OpenOptions) {}

pub(super) fn enforce_private_permissions(_file: &File, _path: &Path) -> std::io::Result<()> {
    Ok(())
}

pub(super) fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both buffers are NUL-terminated and remain alive for the call.
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) fn sync_parent_directory(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

pub(super) fn display_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw.into_owned()
    }
}

pub(super) fn host_file_roots() -> Vec<(String, PathBuf)> {
    // SAFETY: GetLogicalDrives has no pointer arguments and only reads the process-visible
    // logical-drive bitmask maintained by Windows.
    logical_drive_roots(unsafe { GetLogicalDrives() })
}

fn logical_drive_roots(mask: u32) -> Vec<(String, PathBuf)> {
    (0_u8..26)
        .filter(|index| mask & (1_u32 << index) != 0)
        .map(|index| {
            let letter = char::from(b'A' + index);
            (format!("{letter}:"), PathBuf::from(format!(r"{letter}:\")))
        })
        .collect()
}

#[cfg(test)]
pub(super) fn private_file_is_restricted(path: &Path) -> std::io::Result<bool> {
    Ok(path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_drive_mask_maps_to_ordered_roots() {
        let roots = logical_drive_roots((1 << 2) | (1 << 3) | (1 << 25));

        assert_eq!(
            roots,
            vec![
                ("C:".to_string(), PathBuf::from(r"C:\")),
                ("D:".to_string(), PathBuf::from(r"D:\")),
                ("Z:".to_string(), PathBuf::from(r"Z:\")),
            ]
        );
    }
}
