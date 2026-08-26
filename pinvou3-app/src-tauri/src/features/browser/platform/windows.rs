//! Thin Windows WebView2 adapter.
//!
//! `host` owns the shared page, tab, and layout lifecycle. This module configures only the
//! WebView2 user-data directory, local CDP port, and Windows-specific browser arguments.

use std::path::Path;

use tauri::WebviewBuilder;

use super::host::PlatformWebviewConfig;
use super::NativeSurfaceCapabilities;

#[derive(Default)]
pub(crate) struct WindowsWebviewConfig {
    initialized: bool,
    port: Option<u16>,
}

impl PlatformWebviewConfig for WindowsWebviewConfig {
    const ACTIVATION_READY: bool = true;

    fn capabilities(&self) -> NativeSurfaceCapabilities {
        NativeSurfaceCapabilities::new(true, true, true)
    }

    fn requires_reset(&self, automation_port: Option<u16>, _data_directory: &Path) -> bool {
        self.initialized && self.port != automation_port
    }

    fn prepare(
        &mut self,
        automation_port: Option<u16>,
        _data_directory: &Path,
    ) -> Result<(), String> {
        self.initialized = true;
        self.port = automation_port;
        Ok(())
    }

    fn configure_builder(
        &self,
        builder: WebviewBuilder<tauri::Wry>,
        data_directory: &Path,
    ) -> Result<WebviewBuilder<tauri::Wry>, String> {
        let builder = builder.data_directory(data_directory.to_path_buf());
        Ok(match self.port {
            Some(port) => builder.additional_browser_args(&webview2_browser_args(port)),
            None => builder,
        })
    }

    fn reset(&mut self) {
        self.initialized = false;
        self.port = None;
    }

    fn owns_port(&self, port: u16) -> bool {
        self.port == Some(port)
    }

    fn is_initialized(&self) -> bool {
        self.initialized
    }
}

fn webview2_browser_args(port: u16) -> String {
    format!(
        "--remote-debugging-port={port} --remote-debugging-address=127.0.0.1 \
         --disable-features=msWebOOUI,msPdfOOUI,Translate,MediaRouter \
         --no-first-run --no-default-browser-check --disable-sync"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webview2_args_keep_security_protection_enabled() {
        let args = webview2_browser_args(9222);
        assert!(args.contains("--remote-debugging-address=127.0.0.1"));
        assert!(args.contains("--remote-debugging-port=9222"));
        assert!(!args.contains("msSmartScreenProtection"));
    }

    #[test]
    fn windows_reports_native_cdp_automation() {
        let mut config = WindowsWebviewConfig::default();
        config.prepare(Some(9222), Path::new("profile")).unwrap();
        let capabilities = config.capabilities();
        assert!(capabilities.native_display);
        assert!(capabilities.agent_automation);
        assert!(capabilities.chrome_devtools_protocol);
        assert!(config.is_initialized());
        assert!(config.owns_port(9222));
        assert!(config.requires_reset(Some(9333), Path::new("profile")));
    }
}
