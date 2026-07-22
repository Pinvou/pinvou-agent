pub fn install_dependencies(packages: Vec<String>) -> Result<(), String> {
    super::super::platform::install_dependencies(packages)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn linux_dependency_install_degrades_on_non_linux() {
        // Linux-only: verifies pkexec degradation when not running as root.
        // Mac uses `brew install` (no sudo needed) and Windows uses its own
        // installer; neither has the same "degrade to error" semantics this
        // test asserts, so the test is gated to Linux only.
        assert!(install_dependencies(vec!["pandoc".into()]).is_err());
    }
}
