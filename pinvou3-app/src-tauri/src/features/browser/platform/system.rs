//! Thin adapters for macOS WKWebView and Linux WebKitGTK.
//!
//! Linux provides display and Agent automation through BrowserCore and WebKitWebDriver.
//! macOS uses the same BrowserCore page runtime with native WKWebView/AppKit adaptation.

use std::path::{Path, PathBuf};

use tauri::WebviewBuilder;

use super::NativeSurfaceCapabilities;
use super::host::PlatformWebviewConfig;

/// WKWebView's factory user agent omits the `Version/… Safari/…` tokens that
/// identify a Safari-class browser, so UA-sniffing sites downgrade the embedded
/// browser (bilibili.com redirects it to a "browser version too low" notice).
/// Safari's reduced-User-Agent policy freezes every other token in this string;
/// bump `Version/` only when a Safari major release moves the recognized floor.
#[cfg(target_os = "macos")]
const WKWEBVIEW_BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";

#[derive(Default)]
pub(crate) struct SystemWebviewConfig {
    initialized: bool,
    data_directory: Option<PathBuf>,
}

impl PlatformWebviewConfig for SystemWebviewConfig {
    const ACTIVATION_READY: bool = false;

    fn capabilities(&self) -> NativeSurfaceCapabilities {
        let enabled = crate::platform::capabilities::browser_product_enabled();
        NativeSurfaceCapabilities::new(enabled, enabled, false)
    }

    fn requires_reset(&self, _automation_port: Option<u16>, data_directory: &Path) -> bool {
        self.data_directory
            .as_deref()
            .is_some_and(|current| current != data_directory)
    }

    fn prepare(
        &mut self,
        _automation_port: Option<u16>,
        data_directory: &Path,
    ) -> Result<(), String> {
        self.initialized = true;
        self.data_directory = Some(data_directory.to_path_buf());
        Ok(())
    }

    fn configure_builder(
        &self,
        builder: WebviewBuilder<tauri::Wry>,
        data_directory: &Path,
    ) -> Result<WebviewBuilder<tauri::Wry>, String> {
        // WebKitGTK stores data, cache, and cookies in this directory. WKWebView does not
        // support a custom path, but Tauri still uses it as the key for an isolated
        // WebContext. The implementation keeps the system-default persistent
        // WKWebsiteDataStore for macOS 11 compatibility; custom identifiers require 14+.
        let builder = builder.data_directory(data_directory.to_path_buf());
        // The "obsolete browser" downgrades are specific to WKWebView, whose
        // factory user agent drops the Safari/Version tokens; the other ports
        // keep their system defaults.
        #[cfg(target_os = "macos")]
        let builder = builder.user_agent(WKWEBVIEW_BROWSER_USER_AGENT);
        Ok(builder)
    }

    fn reset(&mut self) {
        self.initialized = false;
        self.data_directory = None;
    }

    fn owns_port(&self, _port: u16) -> bool {
        false
    }

    fn is_initialized(&self) -> bool {
        self.initialized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webkit_capabilities_match_the_compiled_product_backend() {
        let mut config = SystemWebviewConfig::default();
        config.prepare(None, Path::new("profile")).unwrap();
        let capabilities = config.capabilities();
        assert_eq!(
            capabilities.native_display,
            crate::platform::capabilities::browser_product_enabled()
        );
        assert_eq!(
            capabilities.agent_automation,
            crate::platform::capabilities::browser_product_enabled()
        );
        assert!(!capabilities.chrome_devtools_protocol);
        const { assert!(!SystemWebviewConfig::ACTIVATION_READY) };
        assert!(config.is_initialized());
        assert!(!config.owns_port(9222));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_user_agent_keeps_the_frozen_safari_tokens() {
        // Sites that gate on user-agent sniffing downgrade a webview whose UA
        // lacks the Version/Safari tokens, so the frozen-token shape is the
        // behavioral contract this constant must preserve.
        assert!(
            WKWEBVIEW_BROWSER_USER_AGENT.starts_with(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) "
            )
        );
        assert!(WKWEBVIEW_BROWSER_USER_AGENT.contains(" Version/"));
        assert!(WKWEBVIEW_BROWSER_USER_AGENT.ends_with(" Safari/605.1.15"));
    }
}
