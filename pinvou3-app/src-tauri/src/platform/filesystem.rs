use std::fs::File;
use std::io;
use std::path::Path;

pub(crate) fn replace_file_atomically(tmp: &Path, target: &Path, backup: &Path) -> io::Result<()> {
    replace_file_atomically_impl(tmp, target, backup)
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

#[cfg(windows)]
fn replace_file_atomically_impl(tmp: &Path, target: &Path, backup: &Path) -> io::Result<()> {
    std::fs::rename(target, backup)?;
    if let Err(error) = std::fs::rename(tmp, target) {
        let _ = std::fs::rename(backup, target);
        return Err(error);
    }
    let _ = std::fs::remove_file(backup);
    Ok(())
}

#[cfg(not(windows))]
fn replace_file_atomically_impl(tmp: &Path, target: &Path, _backup: &Path) -> io::Result<()> {
    std::fs::rename(tmp, target)
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

    pub(crate) fn try_link_file(target: &Path, link: &Path) -> bool {
        try_link_file_impl(target, link)
    }

    pub(crate) fn try_link_dir(target: &Path, link: &Path) -> bool {
        try_link_dir_impl(target, link)
    }

    pub(crate) fn remove_dir_link(link: &Path) {
        remove_dir_link_impl(link)
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
