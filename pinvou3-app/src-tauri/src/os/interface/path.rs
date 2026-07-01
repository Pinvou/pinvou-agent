use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub fn user_home_dir() -> PathBuf {
    super::super::platform::user_home_dir()
}

pub fn platform_compat_path(value: &str) -> PathBuf {
    super::super::platform::platform_compat_path(value)
}

pub fn validate_upload_location(canon: &Path) -> Result<(), String> {
    super::super::platform::validate_upload_location(canon)
}

pub fn path_component_eq(component: &OsStr, expected: &str) -> bool {
    super::super::platform::path_component_eq(component, expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_path_api_returns_pathbuf() {
        let p = platform_compat_path("/tmp/pinvou3-os-test");
        assert!(!p.as_os_str().is_empty());
    }

    #[test]
    fn user_home_dir_returns_some_path() {
        assert!(!user_home_dir().as_os_str().is_empty());
    }
}
