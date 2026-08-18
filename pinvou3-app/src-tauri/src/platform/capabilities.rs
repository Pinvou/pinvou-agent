#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DesktopCapabilities {
    pub(crate) os: &'static str,
    pub(crate) show_megacube_site: bool,
    pub(crate) show_super_permission_settings: bool,
    pub(crate) uses_bundled_dependency_installer: bool,
    pub(crate) uses_homebrew_dependency_installer: bool,
    pub(crate) task_completion_notifications_default: bool,
    pub(crate) local_vllm_supported: bool,
    pub(crate) codex_acp_supported: bool,
}

pub(crate) fn current() -> DesktopCapabilities {
    DesktopCapabilities {
        os: std::env::consts::OS,
        show_megacube_site: cfg!(target_os = "linux"),
        show_super_permission_settings: cfg!(target_os = "linux"),
        uses_bundled_dependency_installer: cfg!(target_os = "windows"),
        // macOS 用 Homebrew 安装依赖(对称 Windows 的 uses_bundled_dependency_installer),
        // 让前端按语义能力选择 Homebrew 专属文案,而非裸判 os 字符串。
        uses_homebrew_dependency_installer: cfg!(target_os = "macos"),
        task_completion_notifications_default: !cfg!(target_os = "linux"),
        local_vllm_supported: cfg!(target_os = "linux"),
        codex_acp_supported: supports_codex_acp(std::env::consts::OS),
    }
}

pub(crate) fn supports_codex_acp(os: &str) -> bool {
    matches!(os, "linux" | "windows" | "macos")
}

pub(crate) const fn is_windows() -> bool {
    cfg!(target_os = "windows")
}

pub(crate) const fn is_musl() -> bool {
    cfg!(all(target_os = "linux", target_env = "musl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_capabilities_match_platform_selectors() {
        let capabilities = current();
        assert_eq!(capabilities.os, std::env::consts::OS);
        assert_eq!(capabilities.uses_bundled_dependency_installer, is_windows());
        assert_eq!(
            capabilities.uses_homebrew_dependency_installer,
            cfg!(target_os = "macos")
        );
        assert_eq!(
            capabilities.show_super_permission_settings,
            cfg!(target_os = "linux")
        );
        assert_eq!(
            capabilities.task_completion_notifications_default,
            !cfg!(target_os = "linux")
        );
        assert_eq!(
            capabilities.codex_acp_supported,
            supports_codex_acp(std::env::consts::OS)
        );
    }

    #[test]
    fn codex_acp_is_available_on_desktop_platforms() {
        assert!(supports_codex_acp("linux"));
        assert!(supports_codex_acp("windows"));
        assert!(supports_codex_acp("macos"));
        assert!(!supports_codex_acp("android"));
    }
}
