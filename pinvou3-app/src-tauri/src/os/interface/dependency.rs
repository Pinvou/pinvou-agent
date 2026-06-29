pub fn install_dependencies(packages: Vec<String>) -> Result<(), String> {
    super::super::platform::install_dependencies(packages)
}

#[cfg(all(test, not(target_os = "linux")))]
mod tests {
    use super::*;

    #[test]
    fn linux_dependency_install_degrades_on_non_linux() {
        assert!(install_dependencies(vec!["pandoc".into()]).is_err());
    }
}
