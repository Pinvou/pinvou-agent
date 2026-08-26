//! macOS WKWebView 与 Linux WebKitGTK 的薄适配。
//!
//! Linux 通过 BrowserCore + WebKitWebDriver 提供显示和 Agent 自动化；macOS
//! 通过同一 BrowserCore 页面运行时和 WKWebView/AppKit 原生适配提供核心能力。

use std::path::{Path, PathBuf};

use tauri::WebviewBuilder;

use super::host::PlatformWebviewConfig;
use super::NativeSurfaceCapabilities;

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
        // WebKitGTK 使用该目录保存 data/cache/cookie。WKWebView 不支持自定义路径，
        // 但 Tauri 仍以此作为独立 WebContext 的键；底层继续使用系统默认的持久
        // WKWebsiteDataStore，从而兼容项目支持的 macOS 11（自定义 identifier 要求 14+）。
        Ok(builder.data_directory(data_directory.to_path_buf()))
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
        assert!(!SystemWebviewConfig::ACTIVATION_READY);
        assert!(config.is_initialized());
        assert!(!config.owns_port(9222));
    }
}
