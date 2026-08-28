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
    pub(crate) browser_native_display: bool,
    pub(crate) browser_agent_automation: bool,
    pub(crate) browser_cdp: bool,
}

/// Sole production-release switch for macOS BrowserCore. Keep it `false` until physical
/// device E2E is complete. Acceptance builds use the non-default `browser-macos-preview`
/// Cargo feature without changing production defaults.
const MACOS_BROWSER_RELEASED: bool = false;

fn browser_product_enabled_for(os: &str, macos_preview: bool) -> bool {
    match os {
        "windows" | "linux" => true,
        "macos" => MACOS_BROWSER_RELEASED || macos_preview,
        _ => false,
    }
}

/// Single semantic product gate for the embedded browser. Runtime MCP, the native workspace,
/// and public capabilities must consume this function together, preventing a half-enabled
/// state where the Agent has tools but the user has no visible surface.
pub(crate) fn browser_product_enabled() -> bool {
    browser_product_enabled_for(
        std::env::consts::OS,
        cfg!(feature = "browser-macos-preview"),
    )
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
        // Enable display and Agent automation atomically. Separate platform cfg declarations
        // could give the model tools while the user cannot see the same page. macOS
        // acceptance builds use this same semantic helper.
        browser_native_display: browser_product_enabled(),
        browser_agent_automation: browser_product_enabled(),
        browser_cdp: cfg!(target_os = "windows"),
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
        assert_eq!(
            capabilities.browser_native_display,
            browser_product_enabled()
        );
        assert_eq!(
            capabilities.browser_agent_automation,
            browser_product_enabled()
        );
        assert_eq!(capabilities.browser_cdp, is_windows());
    }

    #[test]
    fn codex_acp_is_available_on_desktop_platforms() {
        assert!(supports_codex_acp("linux"));
        assert!(supports_codex_acp("windows"));
        assert!(supports_codex_acp("macos"));
        assert!(!supports_codex_acp("android"));
    }

    #[test]
    fn browser_product_gate_matches_release_and_preview_semantics() {
        assert!(browser_product_enabled_for("windows", false));
        assert!(browser_product_enabled_for("linux", false));
        assert_eq!(
            browser_product_enabled_for("macos", false),
            MACOS_BROWSER_RELEASED
        );
        assert!(browser_product_enabled_for("macos", true));
        assert!(!browser_product_enabled_for("android", true));
    }
}
