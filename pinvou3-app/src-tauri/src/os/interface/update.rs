use std::path::Path;

pub fn check_update_platform_support() -> Result<(), String> {
    super::super::platform::check_update_platform_support()
}

pub fn install_update_package(path: &Path) -> Result<(), String> {
    super::super::platform::install_update_package(path)
}

#[cfg(all(test, not(target_os = "linux")))]
mod tests {
    use super::*;

    #[test]
    fn deb_update_degrades_on_non_linux() {
        assert!(check_update_platform_support().is_err());
    }
}
