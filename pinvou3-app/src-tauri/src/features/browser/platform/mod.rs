//! Platform boundary for native browser display.
//!
//! Windows, macOS, and Linux share workspace, tab, layout, navigation, and security policy
//! in `host`. Platform modules provide only system-WebView construction parameters and an
//! accurate declaration of Agent automation capability.

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
mod host;

pub(crate) mod state;

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod system;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
mod linux_automation;

#[cfg(target_os = "linux")]
mod linux_surface;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeSurfaceCapabilities {
    pub(crate) native_display: bool,
    pub(crate) agent_automation: bool,
    pub(crate) chrome_devtools_protocol: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeWorkspaceRestore {
    pub(crate) urls: Vec<String>,
    pub(crate) active_index: usize,
}

/// BrowserCore's platform-neutral input vocabulary. The browser feature maps
/// DOM uids to page-local coordinates; the selected platform adapter decides
/// whether it can dispatch a trusted event to that task-owned WebView.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NativeInput {
    MouseMove {
        x: f64,
        y: f64,
    },
    MouseClick {
        x: f64,
        y: f64,
        button: u32,
        click_count: u8,
    },
    Drag {
        from_x: f64,
        from_y: f64,
        to_x: f64,
        to_y: f64,
    },
    Key {
        key: String,
    },
    Text {
        text: String,
    },
    Scroll {
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
    },
}

/// BrowserCore evaluations are read-only unless the caller explicitly marks
/// them as capable of mutating page state. A mutating script that has already
/// crossed the system-WebView dispatch boundary must never surface as an
/// ordinary retryable transport timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserCoreEvaluationMode {
    ReadOnly,
    MayMutate,
}

/// Keep the mutating-evaluation authorization invariant at the common platform
/// boundary. Read-only BrowserCore observations deliberately carry no control
/// capability; page-mutating JavaScript must carry the exact begun tab lease
/// all the way to the native UI-thread callback.
fn evaluation_authorization<'a>(
    mode: BrowserCoreEvaluationMode,
    authorization: Option<&'a state::NativeTabLease>,
) -> Result<Option<&'a state::NativeTabLease>, String> {
    match mode {
        BrowserCoreEvaluationMode::ReadOnly => Ok(None),
        BrowserCoreEvaluationMode::MayMutate => authorization
            .map(Some)
            .ok_or_else(|| "browser/mutating-script-lease-required".to_string()),
    }
}

pub(crate) const ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION: &str =
    "browser/action-commit-unknown-after-script-interruption";

const ASYNC_DISPATCH_PENDING: u8 = 0;
const ASYNC_DISPATCH_RUNNING: u8 = 1;
const ASYNC_DISPATCH_CANCELLED: u8 = 2;
const ASYNC_DISPATCH_FINISHED: u8 = 3;

/// Shared commit boundary for callbacks queued onto GTK/AppKit. A timeout can
/// cancel work only while it is still pending. Once the native callback has
/// started, callers choose whether an interrupted result is ordinary
/// (read-only observation) or commit-unknown (page/native mutation).
#[derive(Clone)]
struct AsyncDispatchState {
    state: std::sync::Arc<std::sync::atomic::AtomicU8>,
}

impl AsyncDispatchState {
    fn new() -> Self {
        Self {
            state: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(ASYNC_DISPATCH_PENDING)),
        }
    }

    fn begin(&self) -> bool {
        self.state
            .compare_exchange(
                ASYNC_DISPATCH_PENDING,
                ASYNC_DISPATCH_RUNNING,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
    }

    fn finish(&self) {
        let _ = self.state.fetch_update(
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
            |state| match state {
                ASYNC_DISPATCH_PENDING | ASYNC_DISPATCH_RUNNING => Some(ASYNC_DISPATCH_FINISHED),
                ASYNC_DISPATCH_CANCELLED | ASYNC_DISPATCH_FINISHED => None,
                _ => None,
            },
        );
    }

    fn cancel_pending(&self) -> bool {
        self.state
            .compare_exchange(
                ASYNC_DISPATCH_PENDING,
                ASYNC_DISPATCH_CANCELLED,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
    }

    fn interruption_error(&self, cause: &str, commit_unknown_prefix: Option<&str>) -> String {
        if matches!(
            self.state.load(std::sync::atomic::Ordering::Acquire),
            ASYNC_DISPATCH_RUNNING | ASYNC_DISPATCH_FINISHED
        ) {
            if let Some(prefix) = commit_unknown_prefix {
                return format!("{prefix}: {cause}");
            }
        }
        cause.to_string()
    }

    async fn wait<T>(
        &self,
        mut rx: tokio::sync::oneshot::Receiver<Result<T, String>>,
        timeout: std::time::Duration,
        timeout_error: &str,
        callback_closed_error: &str,
        commit_unknown_prefix: Option<&str>,
    ) -> Result<T, String> {
        match tokio::time::timeout(timeout, &mut rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) if self.cancel_pending() => Err(callback_closed_error.to_string()),
            Ok(Err(_)) => {
                Err(self.interruption_error(callback_closed_error, commit_unknown_prefix))
            }
            Err(_) => {
                // Prefer a result that raced the timer before deciding whether
                // the queued callback is still cancelable.
                match rx.try_recv() {
                    Ok(result) => return result,
                    Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                        if self.cancel_pending() {
                            return Err(callback_closed_error.to_string());
                        }
                        return Err(
                            self.interruption_error(callback_closed_error, commit_unknown_prefix)
                        );
                    }
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                }

                if self.cancel_pending() {
                    return Err(timeout_error.to_string());
                }

                // The callback may have completed between the first receive
                // probe and the cancellation CAS. Preserve its exact result.
                match rx.try_recv() {
                    Ok(result) => result,
                    Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                        Err(self.interruption_error(callback_closed_error, commit_unknown_prefix))
                    }
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                        Err(self.interruption_error(timeout_error, commit_unknown_prefix))
                    }
                }
            }
        }
    }
}

pub(crate) fn browser_core_available() -> bool {
    if !crate::platform::capabilities::browser_product_enabled() {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        return linux_automation::backend_available();
    }
    #[cfg(target_os = "macos")]
    {
        return macos::backend_available();
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    false
}

pub(crate) fn browser_core_backend_name() -> &'static str {
    if !crate::platform::capabilities::browser_product_enabled() {
        "browser-core-unavailable"
    } else if cfg!(target_os = "macos") {
        "browser-core-wkwebview"
    } else if cfg!(target_os = "linux") {
        "browser-core-webkitgtk"
    } else {
        "browser-core-unavailable"
    }
}

/// Register a WebView label with the platform automation adapter. When Linux must rebuild the
/// private WebDriver-handle map, the host temporarily navigates that exact WebView to a fresh
/// internal marker and then reloads its prior URL. No binding identity enters a remote page.
fn register_browser_core_webview_binding(
    label: &str,
    tab_token: &str,
    control: &std::sync::Arc<state::WorkspaceControl>,
    navigation: &std::sync::Arc<parking_lot::Mutex<state::UserNavigationState>>,
    host_bootstrap_pending: bool,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        return linux_automation::register_webview_binding_with_navigation(
            label,
            tab_token,
            control,
            navigation,
            host_bootstrap_pending,
        );
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (navigation, host_bootstrap_pending);
        return macos::register_webview_binding(label, tab_token, control);
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (
            label,
            tab_token,
            control,
            navigation,
            host_bootstrap_pending,
        );
        Ok(())
    }
}

pub(crate) fn unregister_browser_core_webview_binding(label: &str) {
    #[cfg(target_os = "linux")]
    linux_automation::unregister_webview_binding(label);
    #[cfg(target_os = "macos")]
    macos::unregister_webview_binding(label);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = label;
}

/// Return true only for the exact host-generated navigation currently used to
/// bind a Linux WebKitWebDriver handle. Other platforms never expose an
/// intermediate binding URL to their page host.
fn classify_browser_core_binding_navigation(label: &str, url: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        return linux_automation::classify_binding_navigation(label, url);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (label, url);
        false
    }
}

/// Release Linux's first-bind barrier after the exact host bootstrap Finished
/// callback has committed and the navigation admission gate is idle.
fn settle_browser_core_host_bootstrap(label: &str, payload_url: &str, live_url: Option<&str>) {
    #[cfg(target_os = "linux")]
    {
        linux_automation::settle_host_bootstrap_page_load(label, payload_url, live_url);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (label, payload_url, live_url);
    }
}

/// Page-load callbacks are the authoritative main-document commit signal. The
/// Linux WebDriver adapter briefly commits a private marker while recovering a
/// handle; keep that exact implementation URL out of UI and persistence.
fn is_browser_core_binding_url(url: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        return linux_automation::is_binding_marker_url(url);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = url;
        false
    }
}

#[cfg(target_os = "linux")]
fn attach_native_surface(webview: &tauri::Webview) -> Result<(), String> {
    linux_surface::attach(webview)
}

#[cfg(not(target_os = "linux"))]
fn attach_native_surface(_webview: &tauri::Webview) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn show_native_surface(
    webview: &tauri::Webview,
    bounds: Option<super::NativeSurfaceBounds>,
) -> Result<(), String> {
    linux_surface::show(webview, bounds)
}

#[cfg(not(target_os = "linux"))]
fn show_native_surface(
    webview: &tauri::Webview,
    bounds: Option<super::NativeSurfaceBounds>,
) -> Result<(), String> {
    if let Some(bounds) = bounds {
        webview
            .set_bounds(tauri::Rect {
                position: tauri::PhysicalPosition::new(bounds.x, bounds.y).into(),
                size: tauri::PhysicalSize::new(bounds.width as u32, bounds.height as u32).into(),
            })
            .map_err(|error| format!("Failed to reposition browser tab: {error}"))?;
    }
    webview
        .show()
        .map_err(|error| format!("Failed to show browser tab: {error}"))
}

#[cfg(target_os = "linux")]
fn hide_native_surface(webview: &tauri::Webview) -> Result<(), String> {
    linux_surface::hide(webview)
}

#[cfg(not(target_os = "linux"))]
fn hide_native_surface(webview: &tauri::Webview) -> Result<(), String> {
    webview
        .hide()
        .map_err(|error| format!("Failed to hide browser tab: {error}"))
}

/// Prepare process-wide browser automation state before Tauri creates any
/// WebKit context. Other platforms intentionally have no process hook.
pub(crate) fn prepare_process_environment() {
    if !crate::platform::capabilities::browser_product_enabled() {
        return;
    }
    #[cfg(target_os = "linux")]
    if let Err(error) = linux_automation::prepare_process_environment() {
        eprintln!("[browser] Linux WebKit automation environment unavailable: {error}");
    }
}

/// Register the dedicated Linux browser WebKit context before the first task
/// browser child WebView is created. The main application WebView remains in
/// Tauri's default, non-automated context.
pub(crate) fn install_automation_context(_app: &mut tauri::App) {
    if !crate::platform::capabilities::browser_product_enabled() {
        return;
    }
    #[cfg(target_os = "linux")]
    if let Err(error) = linux_automation::install_automation_context(_app) {
        eprintln!("[browser] Linux WebKit automation context unavailable: {error}");
    }
    #[cfg(target_os = "linux")]
    {
        use tauri::Manager;
        if let Some(main_webview) = _app.get_webview("main") {
            if let Err(error) = linux_surface::prepare(&main_webview) {
                eprintln!(
                    "[browser] Failed to preinitialize Linux native browser overlay: {error}"
                );
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) async fn evaluate_browser_core_json(
    webview: &tauri::Webview,
    script: String,
    mode: BrowserCoreEvaluationMode,
    authorization: Option<&state::NativeTabLease>,
) -> Result<serde_json::Value, String> {
    linux::evaluate_json(webview, script, mode, authorization).await
}

#[cfg(target_os = "macos")]
pub(crate) async fn evaluate_browser_core_json(
    webview: &tauri::Webview,
    script: String,
    mode: BrowserCoreEvaluationMode,
    authorization: Option<&state::NativeTabLease>,
) -> Result<serde_json::Value, String> {
    macos::evaluate_json(webview, script, mode, authorization).await
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) async fn evaluate_browser_core_json(
    _webview: &tauri::Webview,
    _script: String,
    _mode: BrowserCoreEvaluationMode,
    _authorization: Option<&state::NativeTabLease>,
) -> Result<serde_json::Value, String> {
    Err("browser/core-backend-unavailable".to_string())
}

#[cfg(target_os = "linux")]
pub(crate) async fn dispatch_browser_core_input(
    webview: &tauri::Webview,
    authorization: &state::NativeTabLease,
    input: NativeInput,
) -> Result<(), String> {
    linux::dispatch_input(webview, authorization, input).await
}

#[cfg(target_os = "macos")]
pub(crate) async fn dispatch_browser_core_input(
    webview: &tauri::Webview,
    authorization: &state::NativeTabLease,
    input: NativeInput,
) -> Result<(), String> {
    macos::dispatch_input(webview, authorization, input).await
}

#[cfg(target_os = "linux")]
pub(crate) async fn wait_browser_core_ready() -> Result<(), String> {
    linux::wait_until_ready().await
}

#[cfg(target_os = "macos")]
pub(crate) async fn wait_browser_core_ready() -> Result<(), String> {
    macos::wait_until_ready().await
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) async fn wait_browser_core_ready() -> Result<(), String> {
    Err("browser/core-backend-unavailable".to_string())
}

#[cfg(target_os = "linux")]
pub(crate) async fn bind_browser_core_webview(webview: &tauri::Webview) -> Result<(), String> {
    linux::bind_webview(webview).await
}

#[cfg(target_os = "macos")]
pub(crate) async fn bind_browser_core_webview(webview: &tauri::Webview) -> Result<(), String> {
    macos::bind_webview(webview).await
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) async fn bind_browser_core_webview(_webview: &tauri::Webview) -> Result<(), String> {
    Err("browser/core-backend-unavailable".to_string())
}

#[cfg(target_os = "linux")]
pub(crate) async fn click_browser_core_element(
    webview: &tauri::Webview,
    authorization: &state::NativeTabLease,
    uid: &str,
    click_count: u8,
) -> Result<(), String> {
    linux::click_element(webview, authorization, uid, click_count).await
}

#[cfg(target_os = "macos")]
pub(crate) async fn click_browser_core_element(
    webview: &tauri::Webview,
    authorization: &state::NativeTabLease,
    uid: &str,
    click_count: u8,
) -> Result<(), String> {
    macos::click_element(webview, authorization, uid, click_count).await
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) async fn click_browser_core_element(
    _webview: &tauri::Webview,
    _authorization: &state::NativeTabLease,
    _uid: &str,
    _click_count: u8,
) -> Result<(), String> {
    Err("browser/trusted-input-backend-unavailable".to_string())
}

#[cfg(target_os = "linux")]
pub(crate) async fn fill_browser_core_element(
    webview: &tauri::Webview,
    authorization: &state::NativeTabLease,
    uid: &str,
    value: &str,
) -> Result<(), String> {
    linux::fill_element(webview, authorization, uid, value).await
}

#[cfg(target_os = "macos")]
pub(crate) async fn fill_browser_core_element(
    webview: &tauri::Webview,
    authorization: &state::NativeTabLease,
    uid: &str,
    value: &str,
) -> Result<(), String> {
    macos::fill_element(webview, authorization, uid, value).await
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) async fn fill_browser_core_element(
    _webview: &tauri::Webview,
    _authorization: &state::NativeTabLease,
    _uid: &str,
    _value: &str,
) -> Result<(), String> {
    Err("browser/trusted-input-backend-unavailable".to_string())
}

#[cfg(target_os = "linux")]
pub(crate) async fn type_browser_core_text(
    webview: &tauri::Webview,
    authorization: &state::NativeTabLease,
    text: &str,
    submit_key: Option<&str>,
) -> Result<(), String> {
    linux::type_text(webview, authorization, text, submit_key).await
}

#[cfg(target_os = "macos")]
pub(crate) async fn type_browser_core_text(
    webview: &tauri::Webview,
    authorization: &state::NativeTabLease,
    text: &str,
    submit_key: Option<&str>,
) -> Result<(), String> {
    macos::type_text(webview, authorization, text, submit_key).await
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) async fn type_browser_core_text(
    _webview: &tauri::Webview,
    _authorization: &state::NativeTabLease,
    _text: &str,
    _submit_key: Option<&str>,
) -> Result<(), String> {
    Err("browser/trusted-input-backend-unavailable".to_string())
}

#[cfg(target_os = "linux")]
pub(crate) async fn press_browser_core_key(
    webview: &tauri::Webview,
    authorization: &state::NativeTabLease,
    key: &str,
) -> Result<(), String> {
    linux::press_key(webview, authorization, key).await
}

#[cfg(target_os = "macos")]
pub(crate) async fn press_browser_core_key(
    webview: &tauri::Webview,
    authorization: &state::NativeTabLease,
    key: &str,
) -> Result<(), String> {
    macos::press_key(webview, authorization, key).await
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) async fn press_browser_core_key(
    _webview: &tauri::Webview,
    _authorization: &state::NativeTabLease,
    _key: &str,
) -> Result<(), String> {
    Err("browser/trusted-input-backend-unavailable".to_string())
}

#[cfg(target_os = "linux")]
pub(crate) async fn handle_browser_core_dialog(
    webview: &tauri::Webview,
    authorization: &state::NativeTabLease,
    action: &str,
    prompt_text: Option<&str>,
) -> Result<String, String> {
    linux::handle_dialog(webview, authorization, action, prompt_text).await
}

#[cfg(target_os = "macos")]
pub(crate) async fn handle_browser_core_dialog(
    webview: &tauri::Webview,
    authorization: &state::NativeTabLease,
    action: &str,
    prompt_text: Option<&str>,
) -> Result<String, String> {
    macos::handle_dialog(webview, authorization, action, prompt_text).await
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) async fn handle_browser_core_dialog(
    _webview: &tauri::Webview,
    _authorization: &state::NativeTabLease,
    _action: &str,
    _prompt_text: Option<&str>,
) -> Result<String, String> {
    Err("browser/dialog-backend-unavailable".to_string())
}

/// Stop the shared BrowserCore automation runtime at an explicit lifecycle
/// boundary. Linux serializes this reset with every WebDriver operation, so a
/// select/action already in flight cannot restart the driver after stop has
/// committed.
pub(crate) async fn shutdown_browser_core_for_stop() {
    #[cfg(target_os = "linux")]
    linux_automation::shutdown_for_stop().await;
}

/// Permanently close process-level BrowserCore admission before an application
/// restart. Linux checks this latch only after acquiring its operation gate, so
/// even work queued before restart cannot spawn a driver after the final reset.
pub(crate) fn begin_browser_core_process_shutdown() {
    #[cfg(target_os = "linux")]
    linux_automation::begin_process_shutdown();
}

/// Synchronous process-exit collector. Exit cannot await the WebDriver
/// operation gate, so Linux first closes process admission and then takes the
/// same child slot lock used by spawn publication. That transaction guarantees
/// no WebKitWebDriver child can be published after the collector has run.
pub(crate) fn shutdown_browser_core_for_exit() {
    #[cfg(target_os = "linux")]
    linux_automation::shutdown_for_exit();
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) async fn dispatch_browser_core_input(
    _webview: &tauri::Webview,
    _authorization: &state::NativeTabLease,
    _input: NativeInput,
) -> Result<(), String> {
    Err("browser/trusted-input-backend-unavailable".to_string())
}

impl NativeSurfaceCapabilities {
    pub(crate) const fn new(
        native_display: bool,
        agent_automation: bool,
        chrome_devtools_protocol: bool,
    ) -> Self {
        Self {
            native_display,
            agent_automation,
            chrome_devtools_protocol,
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) type NativeBrowserSurface = host::DesktopBrowserSurface<windows::WindowsWebviewConfig>;

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) type NativeBrowserSurface = host::DesktopBrowserSurface<system::SystemWebviewConfig>;

// Tauri mobile targets are outside this desktop-browser scope. Keep them explicitly
// unsupported so desktop APIs cannot be used accidentally.
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
#[derive(Default)]
pub(crate) struct NativeBrowserSurface;

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
impl NativeBrowserSurface {
    pub fn capabilities(&self) -> NativeSurfaceCapabilities {
        NativeSurfaceCapabilities::new(false, false, false)
    }

    pub fn prepare(
        &mut self,
        _app: &tauri::AppHandle,
        _session_id: &str,
        _session_token: &str,
        _port: u16,
        _data_directory: &std::path::Path,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn read_restore_workspace(
        _session_id: &str,
    ) -> Result<Option<NativeWorkspaceRestore>, String> {
        Ok(None)
    }

    pub fn write_restore_workspace(
        _session_id: &str,
        _restore: &NativeWorkspaceRestore,
    ) -> Result<(), String> {
        Err("Native browser pages are not supported on this platform".to_string())
    }

    pub fn prepare_restored_surface(
        &mut self,
        _app: &tauri::AppHandle,
        _session_id: &str,
        _session_token: &str,
        _automation_port: Option<u16>,
        _data_directory: &std::path::Path,
        _restore: &NativeWorkspaceRestore,
    ) -> Result<Vec<String>, String> {
        Err("Native browser pages are not supported on this platform".to_string())
    }

    pub fn show(
        &mut self,
        _window: &tauri::Window,
        _session_id: &str,
        _bounds: super::NativeSurfaceBounds,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn hide(
        &mut self,
        _app: Option<&tauri::AppHandle>,
        _session_id: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }

    pub fn close(&mut self, _app: Option<&tauri::AppHandle>) -> Result<(), String> {
        Ok(())
    }

    pub fn close_preserving_restore(
        &mut self,
        _app: Option<&tauri::AppHandle>,
    ) -> Result<(), String> {
        Ok(())
    }

    pub fn persist_restore_workspace(
        &self,
        _app: &tauri::AppHandle,
        _session_id: &str,
    ) -> Result<(), String> {
        Err("Native browser pages are not supported on this platform".to_string())
    }

    pub fn persist_navigation_state(
        &self,
        _app: &tauri::AppHandle,
        _session_id: &str,
    ) -> Result<(), String> {
        Err("Native browser pages are not supported on this platform".to_string())
    }

    pub fn persist_all_restore(&self, _app: &tauri::AppHandle) -> Result<(), String> {
        Ok(())
    }

    pub fn close_session(
        &mut self,
        _app: Option<&tauri::AppHandle>,
        _session_id: &str,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn close_session_preserving_restore(
        &mut self,
        _app: Option<&tauri::AppHandle>,
        _session_id: &str,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn session_state(
        &self,
        _app: Option<&tauri::AppHandle>,
        _session_id: &str,
    ) -> Option<(String, String)> {
        None
    }

    pub fn list_tabs(
        &self,
        _app: Option<&tauri::AppHandle>,
        _session_id: &str,
    ) -> Option<Vec<super::TabInfo>> {
        None
    }

    pub fn navigate_tab_for_agent(
        &self,
        _app: &tauri::AppHandle,
        _session_id: &str,
        _tab_token: &str,
        _url: &str,
        _authorization: &state::NativeTabLease,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn history_step_tab_for_agent(
        &self,
        _app: &tauri::AppHandle,
        _session_id: &str,
        _tab_token: &str,
        _delta: i8,
        _authorization: &state::NativeTabLease,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn reload_tab_for_agent(
        &self,
        _app: &tauri::AppHandle,
        _session_id: &str,
        _tab_token: &str,
        _authorization: &state::NativeTabLease,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn create_tab(
        &mut self,
        _app: &tauri::AppHandle,
        _session_id: &str,
        _tab_token: &str,
        _url: &str,
        _background: bool,
    ) -> Result<Option<String>, String> {
        Ok(None)
    }

    pub fn create_tab_for_agent(
        &mut self,
        _app: &tauri::AppHandle,
        _session_id: &str,
        _tab_token: &str,
        _url: &str,
        _background: bool,
        _authorization: &state::NativeTabLease,
        _creation_id: &str,
    ) -> Result<Option<String>, String> {
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_created_tab_for_agent<F>(
        &mut self,
        _app: &tauri::AppHandle,
        _session_id: &str,
        _tab_token: &str,
        _target_id: &str,
        _requested_url: &str,
        _background: bool,
        _authorization: &state::NativeTabLease,
        _creation_id: &str,
        _retained_popup: Option<&state::RetainedAgentOperation>,
        _caller_guard: F,
    ) -> Result<bool, String>
    where
        F: FnMut() -> Result<(), String>,
    {
        Ok(false)
    }

    pub fn bind_target(
        &mut self,
        _session_id: &str,
        _tab_token: &str,
        _target_id: &str,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn target_for_tab(&self, _session_id: &str, _tab_token: &str) -> Option<String> {
        None
    }

    pub fn unbound_tabs(&self, _session_id: &str) -> Vec<String> {
        Vec::new()
    }

    pub fn has_tab(&self, _session_id: &str, _tab_token: &str) -> bool {
        false
    }

    pub fn generate_tab_token(&self) -> String {
        format!("{:016x}", rand::random::<u64>())
    }

    pub fn control_state(&self, _session_id: &str) -> Option<state::ControlSnapshot> {
        None
    }

    pub fn hand_back_to_agent(
        &mut self,
        _app: Option<&tauri::AppHandle>,
        _session_id: &str,
    ) -> Result<Option<state::NativeTabLease>, String> {
        Ok(None)
    }

    pub fn activate_tab_with_lease(
        &mut self,
        _app: Option<&tauri::AppHandle>,
        _session_id: &str,
        _tab_token: &str,
    ) -> Result<Option<state::NativeTabLease>, String> {
        Ok(None)
    }

    pub fn assert_lease(&self, _lease: &state::NativeTabLease) -> Result<bool, String> {
        Ok(false)
    }

    pub fn begin_agent_operation(
        &self,
        _lease: &state::NativeTabLease,
        _emits_trusted_input: bool,
        _observational_only: bool,
        _caller_pid: u32,
        _wrapper_instance_nonce: &str,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn refresh_agent_input(&self, _lease: &state::NativeTabLease) -> Result<bool, String> {
        Ok(false)
    }

    pub fn refresh_agent_operation(&self, _lease: &state::NativeTabLease) -> Result<bool, String> {
        Ok(false)
    }

    pub fn end_agent_operation(&self, _lease: &state::NativeTabLease) {}

    pub fn release_popup_agent_operation(&self, _retained: &state::RetainedAgentOperation) {}

    pub fn authorize_popup_agent_operation(
        &self,
        _retained: &state::RetainedAgentOperation,
    ) -> bool {
        false
    }

    pub fn close_tab_for_agent(
        &mut self,
        _app: Option<&tauri::AppHandle>,
        _session_id: &str,
        _tab_token: &str,
        _authorization: &state::NativeTabLease,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn rollback_created_tab(
        &mut self,
        _app: Option<&tauri::AppHandle>,
        _session_id: &str,
        _tab_token: &str,
        _creation_id: &str,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn rollback_user_created_tab(
        &mut self,
        _app: Option<&tauri::AppHandle>,
        _session_id: &str,
        _tab_token: &str,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn claim_request(
        &mut self,
        _session_id: &str,
        _request_id: &str,
    ) -> Result<state::NativeRequestClaim, String> {
        Err("Agent browser automation is not supported on this platform".to_string())
    }

    pub fn complete_request(
        &mut self,
        _session_id: &str,
        _request_id: &str,
        _result: serde_json::Value,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn cancel_request(
        &mut self,
        _session_id: &str,
        _request_id: &str,
    ) -> Result<state::NativeRequestCancel, String> {
        Ok(state::NativeRequestCancel::Tombstoned)
    }

    pub fn acknowledge_request_cancellation(
        &mut self,
        _session_id: &str,
        _request_id: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    pub fn purge_session_requests(&mut self, _session_id: &str) -> Result<usize, String> {
        Ok(0)
    }

    pub fn activate_tab(
        &mut self,
        _app: Option<&tauri::AppHandle>,
        _session_id: &str,
        _tab_token: &str,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn close_tab(
        &mut self,
        _app: Option<&tauri::AppHandle>,
        _session_id: &str,
        _tab_token: &str,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn has_session(&self, _session_id: &str) -> bool {
        false
    }

    pub fn has_sessions(&self) -> bool {
        false
    }

    pub fn session_ids(&self) -> Vec<String> {
        Vec::new()
    }

    pub fn delete_restore_workspace(_session_id: &str) -> Result<(), String> {
        Ok(())
    }

    pub fn is_initialized(&self) -> bool {
        false
    }

    pub fn navigate(
        &self,
        _app: Option<&tauri::AppHandle>,
        _session_id: &str,
        _url: &str,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn navigate_tab_after_bind(
        &self,
        _app: Option<&tauri::AppHandle>,
        _session_id: &str,
        _tab_token: &str,
        _url: &str,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn history_step(
        &self,
        _app: Option<&tauri::AppHandle>,
        _session_id: &str,
        _delta: i8,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn reload(
        &self,
        _app: Option<&tauri::AppHandle>,
        _session_id: &str,
    ) -> Result<bool, String> {
        Ok(false)
    }

    pub fn owns_port(&self, _port: u16) -> bool {
        false
    }
}

#[cfg(test)]
mod async_dispatch_tests {
    use super::*;
    use std::time::Duration;

    const TIMEOUT: &str = "browser/test-dispatch-timeout";
    const CLOSED: &str = "browser/test-dispatch-callback-closed";

    #[test]
    fn mutating_evaluation_requires_an_explicit_native_tab_lease() {
        assert_eq!(
            evaluation_authorization(BrowserCoreEvaluationMode::ReadOnly, None),
            Ok(None)
        );
        assert_eq!(
            evaluation_authorization(BrowserCoreEvaluationMode::MayMutate, None),
            Err("browser/mutating-script-lease-required".to_string())
        );

        let lease = state::NativeTabLease::from_assertion(
            "session-a",
            "0123456789abcdef",
            "target-a",
            7,
            "0123456789abcdeffedcba9876543210",
        )
        .unwrap();
        assert_eq!(
            evaluation_authorization(BrowserCoreEvaluationMode::MayMutate, Some(&lease)),
            Ok(Some(&lease))
        );
        assert_eq!(
            evaluation_authorization(BrowserCoreEvaluationMode::ReadOnly, Some(&lease)),
            Ok(None),
            "read-only observations must not retain a page-mutation capability"
        );
    }

    #[tokio::test]
    async fn queued_mutation_timeout_cancels_before_native_dispatch() {
        let (_tx, rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
        let state = AsyncDispatchState::new();

        assert_eq!(
            state
                .wait(
                    rx,
                    Duration::ZERO,
                    TIMEOUT,
                    CLOSED,
                    Some(ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION),
                )
                .await,
            Err(TIMEOUT.to_string())
        );
        assert!(!state.begin(), "a cancelled native callback must be inert");
    }

    #[tokio::test]
    async fn dispatched_mutation_timeout_is_commit_unknown_but_read_only_is_ordinary() {
        let (_mutating_tx, mutating_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
        let mutating = AsyncDispatchState::new();
        assert!(mutating.begin());
        assert_eq!(
            mutating
                .wait(
                    mutating_rx,
                    Duration::ZERO,
                    TIMEOUT,
                    CLOSED,
                    Some(ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION),
                )
                .await,
            Err(format!(
                "{ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION}: {TIMEOUT}"
            ))
        );

        let (_read_tx, read_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
        let read_only = AsyncDispatchState::new();
        assert!(read_only.begin());
        assert_eq!(
            read_only
                .wait(read_rx, Duration::ZERO, TIMEOUT, CLOSED, None)
                .await,
            Err(TIMEOUT.to_string())
        );
    }

    #[tokio::test]
    async fn callback_close_obeys_the_same_dispatch_boundary() {
        let (pending_tx, pending_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
        let pending = AsyncDispatchState::new();
        drop(pending_tx);
        assert_eq!(
            pending
                .wait(
                    pending_rx,
                    Duration::from_secs(1),
                    TIMEOUT,
                    CLOSED,
                    Some(ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION),
                )
                .await,
            Err(CLOSED.to_string())
        );
        assert!(
            !pending.begin(),
            "a closed pending callback must stay cancelled"
        );

        let (running_tx, running_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
        let running = AsyncDispatchState::new();
        assert!(running.begin());
        drop(running_tx);
        assert_eq!(
            running
                .wait(
                    running_rx,
                    Duration::from_secs(1),
                    TIMEOUT,
                    CLOSED,
                    Some(ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION),
                )
                .await,
            Err(format!(
                "{ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION}: {CLOSED}"
            ))
        );
    }

    #[tokio::test]
    async fn callback_result_wins_a_timer_race_exactly() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let state = AsyncDispatchState::new();
        assert!(state.begin());
        tx.send(Err::<(), _>("exact-platform-error".to_string()))
            .unwrap();
        state.finish();

        assert_eq!(
            state
                .wait(
                    rx,
                    Duration::ZERO,
                    TIMEOUT,
                    CLOSED,
                    Some(ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION),
                )
                .await,
            Err("exact-platform-error".to_string())
        );
    }
}
