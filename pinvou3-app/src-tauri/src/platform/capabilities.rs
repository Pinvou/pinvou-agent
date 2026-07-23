#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DesktopCapabilities {
    pub(crate) os: &'static str,
    pub(crate) show_megacube_site: bool,
    pub(crate) show_super_permission_settings: bool,
    pub(crate) uses_bundled_dependency_installer: bool,
    pub(crate) task_completion_notifications_default: bool,
    pub(crate) local_vllm_supported: bool,
}

pub(crate) const fn current() -> DesktopCapabilities {
    DesktopCapabilities {
        os: std::env::consts::OS,
        show_megacube_site: cfg!(target_os = "linux"),
        show_super_permission_settings: cfg!(target_os = "linux"),
        uses_bundled_dependency_installer: cfg!(target_os = "windows"),
        task_completion_notifications_default: !cfg!(target_os = "linux"),
        local_vllm_supported: cfg!(target_os = "linux"),
    }
}

pub(crate) const fn is_windows() -> bool {
    cfg!(target_os = "windows")
}

pub(crate) const fn is_linux() -> bool {
    cfg!(target_os = "linux")
}

pub(crate) const fn is_unix() -> bool {
    cfg!(unix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_capabilities_match_platform_selectors() {
        let capabilities = current();
        assert_eq!(capabilities.os, std::env::consts::OS);
        assert_eq!(capabilities.uses_bundled_dependency_installer, is_windows());
        assert_eq!(capabilities.show_super_permission_settings, is_linux());
        assert_eq!(
            capabilities.task_completion_notifications_default,
            !is_linux()
        );
    }
}
