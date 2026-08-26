//! Linux WebKitGTK automation bootstrap and WebDriver transport.
//!
//! Tauri normally enables automation on its first WebContext, which belongs
//! to the application shell. Pinvou pre-registers a dedicated browser profile
//! instead, then connects the open-source `WebKitWebDriver` to WebKitGTK's
//! loopback inspector. W3C Actions produce page-local trusted input without
//! moving the user's desktop-wide pointer or injecting JavaScript events.

use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use parking_lot::Mutex;
use reqwest::Method;
use serde_json::{json, Value};
use tauri::Webview;
use tauri_runtime_wry::tao::event::Event;
use tauri_runtime_wry::tao::event_loop::{ControlFlow, EventLoopProxy, EventLoopWindowTarget};
use tauri_runtime_wry::{
    Context, EventLoopIterationContext, Message, Plugin, PluginBuilder, WebContext, WebContextStore,
};

use super::state::{NativeTabLease, WorkspaceControl};
use super::NativeInput;

const DRIVER_BIN_ENV: &str = "PINVOU3_WEBKIT_WEBDRIVER_BIN";
const SESSION_READY_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const DRIVER_START_RETRY_INTERVAL: Duration = Duration::from_millis(50);
const DRIVER_SESSION_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);
const BINDING_MARKER_PREFIX: &str = "about:blank#pinvou-webdriver-bind-";
const ACTION_COMMIT_UNKNOWN_WEBDRIVER: &str = "browser/action-commit-unknown-webdriver";

static INSPECTOR_PORT: OnceLock<u16> = OnceLock::new();
static AUTOMATION_CONTEXT_READY: AtomicBool = AtomicBool::new(false);
static AUTOMATION_CONTEXT_ERROR: OnceLock<String> = OnceLock::new();
static DRIVER_RUNTIME: OnceLock<Arc<WebDriverRuntime>> = OnceLock::new();
static WEBVIEW_BINDINGS: OnceLock<Mutex<HashMap<String, WebviewBinding>>> = OnceLock::new();

#[derive(Clone)]
struct WebviewBinding {
    /// Rotating, host-only challenge used solely to recover the WebDriver
    /// window handle after driver/session loss.
    nonce: String,
    /// Exact challenge currently used by a host-initiated binding navigation.
    /// It remains active until the WebView returns to its real URL, so only
    /// that process-local transition can be hidden from UI and persistence.
    active_binding_nonce: Option<String>,
    binding_marker_seen: bool,
    /// Exact host tab identity. This never enters the page or WebDriver.
    tab_token: String,
    /// Do not keep a closed workspace alive merely because the process-wide
    /// WebDriver runtime once observed one of its WebViews.
    control: Weak<WorkspaceControl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DriverSessionState {
    Idle,
    Starting,
    Ready {
        endpoint: String,
        session_id: String,
    },
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DriverSession {
    endpoint: String,
    session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WebDriverRequestFailure {
    /// `reqwest` could not produce a response. Once an authorized POST has been
    /// handed to the HTTP stack, this does not prove that the remote end did not
    /// receive and execute it.
    Transport,
    /// The remote end returned HTTP bytes, but the response body was not a
    /// decodable W3C envelope. A successful mutation may therefore already have
    /// happened even when its acknowledgement is unusable.
    ResponseDecode,
    /// A complete W3C error envelope. Only a small set of codes describes a
    /// pre-action element/dialog check; all other codes remain conservative.
    Protocol { code: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebDriverMutationCommitState {
    NotCommitted,
    Unknown,
}

#[derive(Debug)]
struct WebDriverRequestError {
    message: String,
    restart_session: bool,
    failure: WebDriverRequestFailure,
}

#[derive(Debug)]
struct DriverSessionStartError {
    message: String,
    retryable: bool,
}

#[derive(Default)]
struct WebDriverOperationGate {
    lock: tokio::sync::Mutex<()>,
}

impl WebDriverOperationGate {
    async fn run<T>(&self, operation: impl Future<Output = T>) -> T {
        let _guard = self.lock.lock().await;
        operation.await
    }
}

struct WebDriverRuntime {
    inspector_port: u16,
    session: Mutex<DriverSessionState>,
    child: Mutex<Option<Child>>,
    handles: Mutex<HashMap<String, String>>,
    operations: WebDriverOperationGate,
    client: reqwest::Client,
}

pub(super) fn prepare_process_environment() -> Result<(), String> {
    if INSPECTOR_PORT.get().is_some() {
        return Ok(());
    }

    let port = reserve_loopback_port()?;
    std::env::set_var("WEBKIT_INSPECTOR_SERVER", format!("127.0.0.1:{port}"));
    INSPECTOR_PORT
        .set(port)
        .map_err(|_| "inspector endpoint initialized concurrently".to_string())?;
    eprintln!("[browser] Linux WebKit inspector reserved at 127.0.0.1:{port}");
    Ok(())
}

pub(super) fn install_automation_context(app: &mut tauri::App) -> Result<(), String> {
    let inspector_port = INSPECTOR_PORT
        .get()
        .copied()
        .ok_or_else(|| "inspector endpoint was not prepared".to_string())?;
    let profile = crate::platform::paths::browser_webview_profile_dir();
    std::fs::create_dir_all(&profile)
        .map_err(|error| format!("create browser profile {}: {error}", profile.display()))?;
    crate::platform::os::make_private_dir(&profile);

    app.wry_plugin(BrowserAutomationContextPlugin { profile });
    if !AUTOMATION_CONTEXT_READY.load(Ordering::SeqCst) {
        return Err(AUTOMATION_CONTEXT_ERROR
            .get()
            .cloned()
            .unwrap_or_else(|| "Wry plugin did not run on the main thread".to_string()));
    }

    let driver = WebDriverRuntime::new(inspector_port)?;
    if DRIVER_RUNTIME.set(driver).is_err() {
        return Err("WebKitWebDriver runtime initialized concurrently".to_string());
    }
    eprintln!(
        "[browser] Linux WebKit automation context ready on 127.0.0.1:{inspector_port}; driver starts on first browser prepare"
    );
    Ok(())
}

pub(super) fn backend_available() -> bool {
    AUTOMATION_CONTEXT_READY.load(Ordering::SeqCst)
        && DRIVER_RUNTIME.get().is_some()
        && find_driver_binary().is_some()
}

/// Register a host-only WebView label for later WebDriver-handle binding. The
/// registry value is process-local and is never persisted or injected into a
/// document. It binds the opaque WebView label to the exact host tab/control
/// so every native mutation can revalidate its original operation lease. When
/// a WebDriver session must (re)bind, the host
/// temporarily navigates this exact WebView to a fresh internal marker. No
/// identity or challenge is ever injected into an untrusted remote document.
pub(super) fn register_webview_binding(
    label: &str,
    tab_token: &str,
    control: &Arc<WorkspaceControl>,
) -> Result<(), String> {
    if label.is_empty() || label.len() > 256 {
        return Err("browser/webkit-binding-label-invalid".to_string());
    }
    if tab_token.len() != 16 || !tab_token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("browser/webkit-binding-tab-token-invalid".to_string());
    }
    let registry = WEBVIEW_BINDINGS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = registry.lock();
    let incoming_control = Arc::downgrade(control);
    let registration_is_identical = registry.get(label).is_some_and(|binding| {
        binding.tab_token == tab_token && Weak::ptr_eq(&binding.control, &incoming_control)
    });
    if !registration_is_identical {
        let nonce = fresh_binding_nonce(&registry);
        registry.insert(
            label.to_string(),
            WebviewBinding {
                nonce,
                active_binding_nonce: None,
                binding_marker_seen: false,
                tab_token: tab_token.to_string(),
                control: incoming_control,
            },
        );
    }
    if let Some(runtime) = DRIVER_RUNTIME.get() {
        runtime.handles.lock().remove(label);
    }
    Ok(())
}

pub(super) fn unregister_webview_binding(label: &str) {
    if let Some(registry) = WEBVIEW_BINDINGS.get() {
        registry.lock().remove(label);
    }
    if let Some(runtime) = DRIVER_RUNTIME.get() {
        runtime.handles.lock().remove(label);
    }
}

fn fresh_binding_nonce(registry: &HashMap<String, WebviewBinding>) -> String {
    loop {
        let candidate = format!(
            "{:032x}{:032x}",
            rand::random::<u128>(),
            rand::random::<u128>()
        );
        if !registry
            .values()
            .any(|existing| existing.nonce == candidate)
        {
            return candidate;
        }
    }
}

fn rotate_binding_nonce(label: &str) -> Result<String, String> {
    let registry = WEBVIEW_BINDINGS
        .get()
        .ok_or_else(|| "browser/webkit-binding-not-registered".to_string())?;
    let mut registry = registry.lock();
    if !registry.contains_key(label) {
        return Err("browser/webkit-binding-not-registered".to_string());
    }
    let nonce = fresh_binding_nonce(&registry);
    let binding = registry
        .get_mut(label)
        .expect("binding presence was checked under the same lock");
    binding.nonce = nonce.clone();
    binding.active_binding_nonce = Some(nonce.clone());
    binding.binding_marker_seen = false;
    Ok(nonce)
}

/// Classify a navigation against the exact, process-local binding challenge.
/// The first different URL is the restored real page and closes the window.
pub(super) fn classify_binding_navigation(label: &str, url: &str) -> bool {
    let Some(registry) = WEBVIEW_BINDINGS.get() else {
        return false;
    };
    let mut registry = registry.lock();
    let Some(binding) = registry.get_mut(label) else {
        return false;
    };
    let Some(nonce) = binding.active_binding_nonce.as_deref() else {
        return false;
    };
    let marker_matches = url
        .strip_prefix(BINDING_MARKER_PREFIX)
        .is_some_and(|candidate| candidate == nonce);
    if marker_matches {
        binding.binding_marker_seen = true;
        return true;
    }
    // A callback already queued before the host initiated its marker
    // navigation must not cancel the classification window. Only the real URL
    // following an observed marker closes it.
    if binding.binding_marker_seen {
        binding.active_binding_nonce = None;
        binding.binding_marker_seen = false;
    }
    false
}

fn cancel_binding_navigation(label: &str, nonce: &str) {
    if let Some(registry) = WEBVIEW_BINDINGS.get() {
        let mut registry = registry.lock();
        if let Some(binding) = registry.get_mut(label) {
            if binding.active_binding_nonce.as_deref() == Some(nonce) {
                binding.active_binding_nonce = None;
                binding.binding_marker_seen = false;
            }
        }
    }
}

/// Revalidate the exact original host operation immediately before a native
/// WebDriver mutation. Selection and DOM resolution may take arbitrarily long;
/// neither a stale operation from the same tab nor a current operation from a
/// different tab may borrow this WebView binding.
fn authorize_registered_mutation(
    label: &str,
    authorization: &NativeTabLease,
    emits_takeover_signal: bool,
) -> Result<(), String> {
    let control = {
        let bindings = WEBVIEW_BINDINGS
            .get()
            .ok_or_else(|| "browser/webkit-binding-stale".to_string())?
            .lock();
        let binding = bindings
            .get(label)
            .ok_or_else(|| "browser/webkit-binding-stale".to_string())?;
        if binding.tab_token != authorization.tab_token {
            return Err("browser/webkit-tab-binding-mismatch".to_string());
        }
        Weak::upgrade(&binding.control)
    }
    .ok_or_else(|| "browser/webkit-binding-stale".to_string())?;
    let authorized = if emits_takeover_signal {
        control.refresh_agent_input_window(authorization)
    } else {
        control.authorize_agent_dispatch(authorization)
    };
    if !authorized {
        return Err("browser/webkit-control-lease-lost".to_string());
    }
    Ok(())
}

fn partially_committed_error(action: &str, completed_steps: usize, error: String) -> String {
    format!(
        "browser/action-partially-committed: {action} completed {completed_steps} native step(s) before failure: {error}"
    )
}

fn binding_marker_url(nonce: &str) -> Result<tauri::Url, String> {
    format!("{BINDING_MARKER_PREFIX}{nonce}")
        .parse::<tauri::Url>()
        .map_err(|error| format!("browser/webkit-binding-marker-invalid: {error}"))
}

pub(super) async fn wait_until_ready() -> Result<(), String> {
    let runtime = runtime()?;
    runtime
        .operations
        .run(runtime.ensure_session_locked(true))
        .await
        .map(|_| ())
}

pub(super) async fn bind_webview(webview: &Webview) -> Result<(), String> {
    let runtime = runtime()?;
    runtime
        .operations
        .run(runtime.select_webview_locked(webview))
        .await
        .map(|_| ())
}

pub(super) async fn dispatch_input(
    webview: &Webview,
    authorization: &NativeTabLease,
    input: NativeInput,
) -> Result<(), String> {
    let runtime = runtime()?;
    let emits_takeover_signal = !matches!(&input, NativeInput::MouseMove { .. });
    let actions = actions_for_input(input)?;
    let authorization = authorization.clone();
    runtime
        .operations
        .run(async {
            runtime.select_webview_locked(webview).await?;
            let session = runtime.current_session_locked()?;
            runtime
                .request_authorized_locked(
                    &session,
                    webview.label(),
                    &authorization,
                    emits_takeover_signal,
                    Method::POST,
                    "actions",
                    Some(json!({ "actions": actions })),
                )
                .await
                .map(|_| ())
        })
        .await
}

pub(super) async fn click_element(
    webview: &Webview,
    authorization: &NativeTabLease,
    uid: &str,
    click_count: u8,
) -> Result<(), String> {
    if !(1..=2).contains(&click_count) {
        return Err("browser/unsupported-click-count".to_string());
    }
    let runtime = runtime()?;
    let authorization = authorization.clone();
    runtime
        .operations
        .run(async {
            runtime.select_webview_locked(webview).await?;
            let session = runtime.current_session_locked()?;
            let element = runtime.element_for_uid_locked(&session, uid).await?;
            for completed_clicks in 0..click_count {
                if let Err(error) = runtime
                    .request_authorized_locked(
                        &session,
                        webview.label(),
                        &authorization,
                        true,
                        Method::POST,
                        &format!("element/{element}/click"),
                        Some(json!({})),
                    )
                    .await
                {
                    return Err(if completed_clicks > 0 {
                        partially_committed_error("click", completed_clicks as usize, error)
                    } else {
                        error
                    });
                }
            }
            Ok(())
        })
        .await
}

pub(super) async fn fill_element(
    webview: &Webview,
    authorization: &NativeTabLease,
    uid: &str,
    value: &str,
) -> Result<(), String> {
    let runtime = runtime()?;
    let authorization = authorization.clone();
    runtime
        .operations
        .run(async {
            runtime.select_webview_locked(webview).await?;
            let session = runtime.current_session_locked()?;
            let element = runtime.element_for_uid_locked(&session, uid).await?;
            runtime
                .request_authorized_locked(
                    &session,
                    webview.label(),
                    &authorization,
                    false,
                    Method::POST,
                    &format!("element/{element}/clear"),
                    Some(json!({})),
                )
                .await?;
            if value.is_empty() {
                return Ok(());
            }
            runtime
                .send_keys_to_element_locked(
                    &session,
                    webview.label(),
                    &authorization,
                    &element,
                    value,
                )
                .await
                .map_err(|error| partially_committed_error("fill", 1, error))
        })
        .await
}

pub(super) async fn type_text(
    webview: &Webview,
    authorization: &NativeTabLease,
    text: &str,
    submit_key: Option<&str>,
) -> Result<(), String> {
    let runtime = runtime()?;
    let submit = submit_key.map(webdriver_key_sequence).transpose()?;
    let authorization = authorization.clone();
    runtime
        .operations
        .run(async {
            runtime.select_webview_locked(webview).await?;
            let session = runtime.current_session_locked()?;
            let element = runtime.active_element_locked(&session).await?;
            let mut text_committed = false;
            if !text.is_empty() {
                runtime
                    .send_keys_to_element_locked(
                        &session,
                        webview.label(),
                        &authorization,
                        &element,
                        text,
                    )
                    .await?;
                text_committed = true;
            }
            if let Some(key) = submit.as_deref() {
                if let Err(error) = runtime
                    .send_keys_to_element_locked(
                        &session,
                        webview.label(),
                        &authorization,
                        &element,
                        key,
                    )
                    .await
                {
                    return Err(if text_committed {
                        partially_committed_error("type_text", 1, error)
                    } else {
                        error
                    });
                }
            }
            Ok(())
        })
        .await
}

pub(super) async fn press_key(
    webview: &Webview,
    authorization: &NativeTabLease,
    key: &str,
) -> Result<(), String> {
    let runtime = runtime()?;
    let sequence = webdriver_key_sequence(key)?;
    let authorization = authorization.clone();
    runtime
        .operations
        .run(async {
            runtime.select_webview_locked(webview).await?;
            let session = runtime.current_session_locked()?;
            let element = runtime.active_element_locked(&session).await?;
            runtime
                .send_keys_to_element_locked(
                    &session,
                    webview.label(),
                    &authorization,
                    &element,
                    &sequence,
                )
                .await
        })
        .await
}

/// Handle the modal JavaScript dialog for this exact task-owned WebView. The
/// whole select/read/respond sequence shares the same operation gate as every
/// other WebDriver operation, so another task cannot change the current
/// browsing context between observing and resolving the dialog.
pub(super) async fn handle_dialog(
    webview: &Webview,
    authorization: &NativeTabLease,
    action: &str,
    prompt_text: Option<&str>,
) -> Result<String, String> {
    let endpoint = dialog_action_endpoint(action)?;
    let runtime = runtime()?;
    let authorization = authorization.clone();
    runtime
        .operations
        .run(async {
            runtime.select_webview_locked(webview).await?;
            let session = runtime.current_session_locked()?;
            let dialog_text = runtime
                .request_in_session_locked(&session, Method::GET, "alert/text", None)
                .await?
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| "WebKitWebDriver returned invalid dialog text".to_string())?;
            let mut prompt_text_committed = false;
            if action == "accept" {
                if let Some(prompt_text) = prompt_text {
                    runtime
                        .request_authorized_locked(
                            &session,
                            webview.label(),
                            &authorization,
                            false,
                            Method::POST,
                            "alert/text",
                            Some(json!({ "text": prompt_text })),
                        )
                        .await?;
                    prompt_text_committed = true;
                }
            }
            if let Err(error) = runtime
                .request_authorized_locked(
                    &session,
                    webview.label(),
                    &authorization,
                    false,
                    Method::POST,
                    endpoint,
                    Some(json!({})),
                )
                .await
            {
                return Err(if prompt_text_committed {
                    partially_committed_error("handle_dialog", 1, error)
                } else {
                    error
                });
            }
            Ok(dialog_text)
        })
        .await
}

fn dialog_action_endpoint(action: &str) -> Result<&'static str, String> {
    match action {
        "accept" => Ok("alert/accept"),
        "dismiss" => Ok("alert/dismiss"),
        _ => Err("browser/invalid-argument: action".to_string()),
    }
}

/// Stop requested by the browser lifecycle. It shares the operation gate with
/// selection and input so an in-flight operation cannot restart the driver
/// after this reset has committed.
pub(super) async fn shutdown_for_stop() {
    let Some(runtime) = DRIVER_RUNTIME.get() else {
        return;
    };
    runtime.shutdown_for_stop().await;
}

/// Process-exit fallback. The async browser lifecycle must use
/// [`shutdown_for_stop`]; exit cannot await and only makes a best effort to
/// terminate the child before the OS tears down the process.
pub(super) fn shutdown_for_exit() {
    let Some(runtime) = DRIVER_RUNTIME.get() else {
        return;
    };
    runtime.reset_driver(DriverSessionState::Idle);
}

fn runtime() -> Result<Arc<WebDriverRuntime>, String> {
    DRIVER_RUNTIME
        .get()
        .cloned()
        .ok_or_else(|| "browser/webkit-webdriver-unavailable".to_string())
}

fn reserve_loopback_port() -> Result<u16, String> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("reserve loopback port: {error}"))?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|error| format!("read loopback port: {error}"))
}

fn find_driver_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(DRIVER_BIN_ENV).map(PathBuf::from) {
        if crate::platform::filesystem::is_executable_file(&path) {
            return Some(path);
        }
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join("WebKitWebDriver"))
        .find(|candidate| crate::platform::filesystem::is_executable_file(candidate))
}

impl WebDriverRuntime {
    fn new(inspector_port: u16) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            inspector_port,
            session: Mutex::new(DriverSessionState::Idle),
            child: Mutex::new(None),
            handles: Mutex::new(HashMap::new()),
            operations: WebDriverOperationGate::default(),
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(|error| format!("create WebDriver HTTP client: {error}"))?,
        }))
    }

    fn child_alive(&self) -> bool {
        let mut child = self.child.lock();
        let alive = match child.as_mut() {
            Some(process) => match process.try_wait() {
                Ok(None) => true,
                Ok(Some(status)) => {
                    eprintln!("[browser] Linux WebKitWebDriver exited: {status}");
                    false
                }
                Err(error) => {
                    eprintln!("[browser] inspect Linux WebKitWebDriver process failed: {error}");
                    false
                }
            },
            None => false,
        };
        if !alive {
            child.take();
        }
        alive
    }

    /// Read the already-selected live session without starting or recovering
    /// anything. Once an operation has selected its WebView, every resolve and
    /// mutation must remain on this exact generation; a crash is fail-closed
    /// and the next top-level tool may perform a fresh bind.
    fn current_session_locked(&self) -> Result<DriverSession, String> {
        let state = self.session.lock().clone();
        let child_alive = self.child_alive();
        if let Some(session) = ready_session_for_live_process(&state, child_alive) {
            return Ok(session);
        }
        let error = "browser/webkit-session-lost-before-dispatch".to_string();
        if matches!(state, DriverSessionState::Ready { .. }) {
            self.reset_driver(DriverSessionState::Failed(error.clone()));
        }
        Err(error)
    }

    fn reset_driver(&self, state: DriverSessionState) {
        *self.session.lock() = state;
        self.handles.lock().clear();
        if let Some(mut child) = self.child.lock().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    async fn shutdown_for_stop(&self) {
        self.operations
            .run(async {
                self.reset_driver(DriverSessionState::Idle);
            })
            .await;
    }

    async fn ensure_session_locked(&self, probe_ready: bool) -> Result<DriverSession, String> {
        let state = self.session.lock().clone();
        if let DriverSessionState::Failed(error) = &state {
            eprintln!("[browser] restarting Linux WebKitWebDriver after previous failure: {error}");
        }
        let child_alive = self.child_alive();
        if let Some(ready) = ready_session_for_live_process(&state, child_alive) {
            if !probe_ready {
                return Ok(ready);
            }
            match self
                .raw_request(&ready, Method::GET, "window/handles", None)
                .await
            {
                Ok(_) => return Ok(ready),
                Err(error) => {
                    eprintln!(
                        "[browser] Linux WebKitWebDriver session probe failed; restarting on demand: {}",
                        error.message
                    );
                    self.reset_driver(DriverSessionState::Failed(error.message));
                }
            }
        } else if child_alive {
            // `Starting` can remain after cancellation of a prepare future.
            // No background bootstrap owns it, so reclaim it before retrying.
            self.reset_driver(DriverSessionState::Idle);
        } else if matches!(state, DriverSessionState::Ready { .. }) {
            self.reset_driver(DriverSessionState::Failed(
                "WebKitWebDriver process exited".to_string(),
            ));
        }
        self.start_session_locked().await
    }

    async fn start_session_locked(&self) -> Result<DriverSession, String> {
        self.reset_driver(DriverSessionState::Idle);
        let binary = find_driver_binary().ok_or_else(|| {
            format!(
                "browser/webkit-webdriver-not-found: install webkit2gtk-driver or set {DRIVER_BIN_ENV}"
            )
        })?;
        let driver_port = reserve_loopback_port()?;
        let endpoint = format!("http://127.0.0.1:{driver_port}");
        let child = Command::new(&binary)
            .arg(format!("--port={driver_port}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("start {}: {error}", binary.display()))?;
        *self.child.lock() = Some(child);
        *self.session.lock() = DriverSessionState::Starting;

        let deadline = tokio::time::Instant::now() + SESSION_READY_TIMEOUT;
        loop {
            if !self.child_alive() {
                let error = "WebKitWebDriver exited before creating a session".to_string();
                self.reset_driver(DriverSessionState::Failed(error.clone()));
                return Err(error);
            }
            let last_error = match self.create_session_attempt(&endpoint).await {
                Ok(session_id) => {
                    let ready = DriverSession {
                        endpoint: endpoint.clone(),
                        session_id,
                    };
                    *self.session.lock() = DriverSessionState::Ready {
                        endpoint: ready.endpoint.clone(),
                        session_id: ready.session_id.clone(),
                    };
                    self.handles.lock().clear();
                    eprintln!("[browser] Linux WebKitWebDriver session ready");
                    return Ok(ready);
                }
                Err(error) if error.retryable => error.message,
                Err(error) => {
                    self.reset_driver(DriverSessionState::Failed(error.message.clone()));
                    return Err(error.message);
                }
            };
            if tokio::time::Instant::now() >= deadline {
                let error = format!("browser/webkit-webdriver-session-timeout: {last_error}");
                self.reset_driver(DriverSessionState::Failed(error.clone()));
                return Err(error);
            }
            tokio::time::sleep(DRIVER_START_RETRY_INTERVAL).await;
        }
    }

    async fn create_session_attempt(
        &self,
        endpoint: &str,
    ) -> Result<String, DriverSessionStartError> {
        let response = self
            .client
            .post(format!("{endpoint}/session"))
            .timeout(DRIVER_SESSION_ATTEMPT_TIMEOUT)
            .json(&json!({
                "capabilities": {
                    "alwaysMatch": {
                        // Never let a setup/selection command silently dismiss a dialog before
                        // handle_dialog reaches the W3C alert endpoint.
                        "unhandledPromptBehavior": "ignore",
                        "webkitgtk:browserOptions": {
                            "targetAddress": format!("127.0.0.1:{}", self.inspector_port)
                        }
                    }
                }
            }))
            .send()
            .await
            .map_err(|error| DriverSessionStartError {
                message: format!("connect WebKitWebDriver session: {error}"),
                retryable: true,
            })?;
        let status = response.status();
        let value = response
            .json::<Value>()
            .await
            .map_err(|error| DriverSessionStartError {
                message: format!("decode WebKitWebDriver session: {error}"),
                retryable: true,
            })?;
        if status.is_success() && value.pointer("/value/error").is_none() {
            return value
                .pointer("/value/sessionId")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| DriverSessionStartError {
                    message: "WebKitWebDriver session response omitted sessionId".to_string(),
                    retryable: false,
                });
        }
        let message = webdriver_error(&value, status.as_u16());
        Err(DriverSessionStartError {
            retryable: is_transient_session_start_error(&value, &message),
            message,
        })
    }

    async fn request_locked(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let session = self.ensure_session_locked(false).await?;
        self.request_in_session_locked(&session, method, path, body)
            .await
    }

    async fn request_in_session_locked(
        &self,
        session: &DriverSession,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        if &self.current_session_locked()? != session {
            return Err("browser/webkit-session-changed-before-dispatch".to_string());
        }
        self.finish_request(self.raw_request(session, method, path, body).await)
    }

    fn finish_request(
        &self,
        result: Result<Value, WebDriverRequestError>,
    ) -> Result<Value, String> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                if error.restart_session {
                    self.reset_driver(DriverSessionState::Failed(error.message.clone()));
                }
                Err(error.message)
            }
        }
    }

    fn finish_authorized_request(
        &self,
        result: Result<Value, WebDriverRequestError>,
    ) -> Result<Value, String> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                let commit_state = webdriver_mutation_commit_state(&error);
                if error.restart_session {
                    self.reset_driver(DriverSessionState::Failed(error.message.clone()));
                }
                match commit_state {
                    WebDriverMutationCommitState::NotCommitted => Err(error.message),
                    WebDriverMutationCommitState::Unknown => Err(format!(
                        "{ACTION_COMMIT_UNKNOWN_WEBDRIVER}: {}",
                        error.message
                    )),
                }
            }
        }
    }

    /// The authorization check deliberately sits in the same helper as the
    /// WebDriver POST. Callers may perform slow selection/DOM resolution first,
    /// but cannot accidentally add a page mutation without the final exact
    /// host-lease check and trusted-input provenance refresh.
    async fn request_authorized_locked(
        &self,
        session: &DriverSession,
        label: &str,
        authorization: &NativeTabLease,
        emits_takeover_signal: bool,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        if method != Method::POST {
            return Err("browser/webkit-authorized-request-must-be-post".to_string());
        }
        if &self.current_session_locked()? != session {
            return Err("browser/webkit-session-changed-before-dispatch".to_string());
        }
        authorize_registered_mutation(label, authorization, emits_takeover_signal)?;
        self.finish_authorized_request(self.raw_request(session, method, path, body).await)
    }

    async fn raw_request(
        &self,
        session: &DriverSession,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, WebDriverRequestError> {
        let request_description = format!("{} {}", method.as_str(), path.trim_start_matches('/'));
        let url = format!(
            "{}/session/{}/{}",
            session.endpoint,
            session.session_id,
            path.trim_start_matches('/')
        );
        let mut request = self.client.request(method, url);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| WebDriverRequestError {
                message: format!("WebKitWebDriver {request_description} request failed: {error}"),
                restart_session: true,
                failure: WebDriverRequestFailure::Transport,
            })?;
        let status = response.status();
        let value = response
            .json::<Value>()
            .await
            .map_err(|error| WebDriverRequestError {
                message: format!("decode WebKitWebDriver {request_description} response: {error}"),
                restart_session: true,
                failure: WebDriverRequestFailure::ResponseDecode,
            })?;
        if status.is_success() && value.pointer("/value/error").is_none() {
            Ok(value.get("value").cloned().unwrap_or(Value::Null))
        } else {
            Err(WebDriverRequestError {
                message: format!(
                    "WebKitWebDriver {request_description}: {}",
                    webdriver_error(&value, status.as_u16())
                ),
                restart_session: webdriver_error_requires_restart(&value),
                failure: WebDriverRequestFailure::Protocol {
                    code: webdriver_error_code(&value).map(str::to_string),
                },
            })
        }
    }

    async fn handles_locked(&self) -> Result<Vec<String>, String> {
        let value = self
            .request_locked(Method::GET, "window/handles", None)
            .await?;
        let values = value
            .as_array()
            .ok_or_else(|| "WebKitWebDriver returned invalid window handles".to_string())?;
        values
            .iter()
            .map(|value| {
                value.as_str().map(str::to_string).ok_or_else(|| {
                    "WebKitWebDriver returned a non-string window handle".to_string()
                })
            })
            .collect()
    }

    async fn switch_to_locked(&self, handle: &str) -> Result<(), String> {
        self.request_locked(Method::POST, "window", Some(json!({ "handle": handle })))
            .await
            .map(|_| ())
    }

    async fn ensure_current_locked(&self, handle: &str) -> Result<(), String> {
        // Every caller has just observed this handle in `window/handles` (or
        // retained a cached binding against that same authoritative list)
        // while holding the process-wide operation gate. WebKitWebDriver can
        // leave its implicit current handle pointing at a WebView that Tauri
        // has already closed; querying `GET window` then fails with
        // `no such window` and prevents selection of the known-live handle.
        // Linearize top-level selection with the idempotent W3C switch itself.
        self.switch_to_locked(handle).await
    }

    async fn current_url_locked(&self) -> Result<String, String> {
        self.request_locked(Method::GET, "url", None)
            .await?
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "WebKitWebDriver returned a non-string current URL".to_string())
    }

    async fn locate_binding_marker_locked(
        &self,
        label: &str,
        marker: &str,
    ) -> Result<String, String> {
        let deadline = tokio::time::Instant::now() + SESSION_READY_TIMEOUT;
        loop {
            let handles = self.handles_locked().await?;
            self.handles
                .lock()
                .retain(|_, mapped| handles.contains(mapped));
            let mapped = self
                .handles
                .lock()
                .values()
                .cloned()
                .collect::<HashSet<_>>();
            let unused = handles
                .into_iter()
                .filter(|handle| !mapped.contains(handle))
                .collect::<Vec<_>>();
            let mut observed = Vec::with_capacity(unused.len());
            for handle in &unused {
                self.ensure_current_locked(handle).await?;
                observed.push((handle.clone(), self.current_url_locked().await?));
            }
            if let Some(handle) = unique_marker_match(&observed, marker)? {
                return Ok(handle);
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "browser/webkit-window-binding-failed: label={label} unused={}",
                    unused.len()
                ));
            }
            tokio::time::sleep(DRIVER_START_RETRY_INTERVAL).await;
        }
    }

    async fn select_webview_locked(&self, webview: &Webview) -> Result<String, String> {
        let label = webview.label().to_string();
        if expected_binding_nonce(&label).is_none() {
            return Err("browser/webkit-binding-not-registered".to_string());
        }
        let handles = self.handles_locked().await?;
        self.handles
            .lock()
            .retain(|_, mapped| handles.contains(mapped));
        let existing_handle = { self.handles.lock().get(&label).cloned() };
        if let Some(handle) = existing_handle {
            self.ensure_current_locked(&handle).await?;
            return Ok(handle);
        }

        // A remote page is an adversarial principal: a main-world nonce can be read and relayed
        // to another task through same-origin storage. Rebinding therefore navigates only this
        // host-owned WebView to a random internal marker, resolves the exact WebDriver handle,
        // and then reloads the prior URL. This is deliberately a reload after driver recovery;
        // preserving in-page state is less important than maintaining task isolation.
        let original_url = webview
            .url()
            .map_err(|error| format!("browser/webkit-binding-read-url-failed: {error}"))?;
        let expected_nonce = rotate_binding_nonce(&label)?;
        let marker_url = match binding_marker_url(&expected_nonce) {
            Ok(marker_url) => marker_url,
            Err(error) => {
                cancel_binding_navigation(&label, &expected_nonce);
                return Err(error);
            }
        };
        let marker = marker_url.to_string();
        if let Err(error) = webview.navigate(marker_url) {
            cancel_binding_navigation(&label, &expected_nonce);
            return Err(format!(
                "browser/webkit-binding-marker-navigation-failed: {error}"
            ));
        }

        let bind_result = async {
            let handle = self.locate_binding_marker_locked(&label, &marker).await?;
            self.ensure_current_locked(&handle).await?;
            if original_url.as_str() != marker {
                self.request_locked(
                    Method::POST,
                    "url",
                    Some(json!({ "url": original_url.as_str() })),
                )
                .await?;
            }
            Ok::<String, String>(handle)
        }
        .await;

        match bind_result {
            Ok(handle) => {
                self.ensure_current_locked(&handle).await?;
                self.handles.lock().insert(label.clone(), handle.clone());
                Ok(handle)
            }
            Err(error) => {
                // The authoritative mapping was never published. Restore the user's prior page
                // best-effort and fail closed rather than guessing a WebDriver handle by URL/order.
                if original_url.as_str() != marker {
                    let _ = webview.navigate(original_url);
                }
                Err(error)
            }
        }
    }

    async fn element_for_uid_locked(
        &self,
        session: &DriverSession,
        uid: &str,
    ) -> Result<String, String> {
        let value = self
            .request_in_session_locked(
                session,
                Method::POST,
                "execute/sync",
                Some(json!({
                    "script": "const core = window.__PINVOU_BROWSER_CORE_V1__; if (!core) throw new Error('browser/core-runtime-unavailable'); return core.element(arguments[0]);",
                    "args": [uid],
                })),
            )
            .await?;
        element_id(&value)
    }

    async fn active_element_locked(&self, session: &DriverSession) -> Result<String, String> {
        let value = self
            .request_in_session_locked(session, Method::GET, "element/active", None)
            .await?;
        element_id(&value)
    }

    async fn send_keys_to_element_locked(
        &self,
        session: &DriverSession,
        label: &str,
        authorization: &NativeTabLease,
        element: &str,
        text: &str,
    ) -> Result<(), String> {
        self.request_authorized_locked(
            session,
            label,
            authorization,
            true,
            Method::POST,
            &format!("element/{element}/value"),
            Some(json!({
                "text": text,
                "value": text.chars().map(|character| character.to_string()).collect::<Vec<_>>(),
            })),
        )
        .await
        .map(|_| ())
    }
}

fn expected_binding_nonce(label: &str) -> Option<String> {
    WEBVIEW_BINDINGS.get().and_then(|registry| {
        registry
            .lock()
            .get(label)
            .map(|binding| binding.nonce.clone())
    })
}

fn ready_session_for_live_process(
    state: &DriverSessionState,
    child_alive: bool,
) -> Option<DriverSession> {
    match (state, child_alive) {
        (
            DriverSessionState::Ready {
                endpoint,
                session_id,
            },
            true,
        ) => Some(DriverSession {
            endpoint: endpoint.clone(),
            session_id: session_id.clone(),
        }),
        _ => None,
    }
}

fn unique_marker_match(
    observed: &[(String, String)],
    expected_marker: &str,
) -> Result<Option<String>, String> {
    let mut matching = observed
        .iter()
        .filter(|(_, url)| url == expected_marker)
        .map(|(handle, _)| handle.clone());
    let first = matching.next();
    if matching.next().is_some() {
        return Err("browser/webkit-binding-marker-not-unique".to_string());
    }
    Ok(first)
}

fn webdriver_error_code(value: &Value) -> Option<&str> {
    value.pointer("/value/error").and_then(Value::as_str)
}

fn webdriver_error_requires_restart(value: &Value) -> bool {
    matches!(
        webdriver_error_code(value),
        Some("invalid session id" | "session not created")
    )
}

/// Once an authorized mutation POST has entered `raw_request`, transport loss
/// or an undecodable acknowledgement cannot establish that the page action did
/// not run. A complete W3C response is retryable only for error codes whose
/// specified checks reject the command before its intended page mutation.
fn webdriver_mutation_commit_state(error: &WebDriverRequestError) -> WebDriverMutationCommitState {
    match &error.failure {
        WebDriverRequestFailure::Transport | WebDriverRequestFailure::ResponseDecode => {
            WebDriverMutationCommitState::Unknown
        }
        WebDriverRequestFailure::Protocol { code }
            if webdriver_error_proves_no_page_mutation(code.as_deref()) =>
        {
            WebDriverMutationCommitState::NotCommitted
        }
        WebDriverRequestFailure::Protocol { .. } => WebDriverMutationCommitState::Unknown,
    }
}

fn webdriver_error_proves_no_page_mutation(code: Option<&str>) -> bool {
    matches!(
        code,
        Some(
            "invalid argument"
                | "invalid element state"
                | "invalid session id"
                | "element click intercepted"
                | "element not interactable"
                | "no such alert"
                | "no such element"
                | "no such window"
                | "stale element reference"
                | "unsupported operation"
        )
    )
}

fn is_transient_session_start_error(value: &Value, rendered: &str) -> bool {
    let message = value
        .pointer("/value/message")
        .and_then(Value::as_str)
        .unwrap_or(rendered)
        .to_ascii_lowercase();
    message.contains("failed to create a new browsing context")
        || message.contains("no browsing context")
}

fn element_id(value: &Value) -> Result<String, String> {
    value
        .get("element-6066-11e4-a52e-4f735466cecf")
        .or_else(|| value.get("ELEMENT"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "WebKitWebDriver returned an invalid element reference".to_string())
}

fn webdriver_error(value: &Value, status: u16) -> String {
    let error_code = value
        .pointer("/value/error")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let message = value
        .pointer("/value/message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let detail = match (error_code, message) {
        (Some(code), Some(message)) if code != message => format!("{code}: {message}"),
        (Some(code), _) => code.to_string(),
        (None, Some(message)) => message.to_string(),
        (None, None) => "unknown WebDriver error".to_string(),
    };
    format!("WebKitWebDriver HTTP {status}: {detail}")
}

fn finite_coordinate(value: f64) -> Result<i64, String> {
    if !value.is_finite() {
        return Err("browser/invalid-input-coordinate".to_string());
    }
    Ok(value.round() as i64)
}

fn pointer_move(x: f64, y: f64, duration: u64) -> Result<Value, String> {
    Ok(json!({
        "type": "pointerMove",
        "duration": duration,
        "origin": "viewport",
        "x": finite_coordinate(x)?,
        "y": finite_coordinate(y)?,
    }))
}

fn pointer_source(actions: Vec<Value>) -> Value {
    json!({
        "type": "pointer",
        "id": "pinvou-pointer",
        "parameters": { "pointerType": "mouse" },
        "actions": actions,
    })
}

fn key_source(actions: Vec<Value>) -> Value {
    json!({ "type": "key", "id": "pinvou-keyboard", "actions": actions })
}

fn key_value(value: &str) -> &str {
    match value {
        "Enter" => "\u{E007}",
        "Escape" | "Esc" => "\u{E00C}",
        "Backspace" => "\u{E003}",
        "Delete" => "\u{E017}",
        "Tab" => "\u{E004}",
        "ArrowUp" => "\u{E013}",
        "ArrowDown" => "\u{E015}",
        "ArrowLeft" => "\u{E012}",
        "ArrowRight" => "\u{E014}",
        "Space" => " ",
        other => other,
    }
}

fn webdriver_key_sequence(sequence: &str) -> Result<String, String> {
    let mut modifiers = String::new();
    let mut key = None;
    for part in sequence.split('+').filter(|part| !part.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "control" | "ctrl" => modifiers.push('\u{E009}'),
            "shift" => modifiers.push('\u{E008}'),
            "alt" => modifiers.push('\u{E00A}'),
            "meta" | "super" => modifiers.push('\u{E03D}'),
            _ if key.is_none() => key = Some(key_value(part)),
            _ => return Err(format!("browser/unsupported-key-sequence: {sequence}")),
        }
    }
    let key = key.ok_or_else(|| "browser/empty-key-sequence".to_string())?;
    modifiers.push_str(key);
    if !modifiers.is_empty() && sequence.contains('+') {
        modifiers.push('\u{E000}');
    }
    Ok(modifiers)
}

fn key_actions(sequence: &str) -> Result<Vec<Value>, String> {
    let mut modifiers = Vec::new();
    let mut key = None;
    for part in sequence.split('+').filter(|part| !part.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "control" | "ctrl" => modifiers.push("\u{E009}"),
            "shift" => modifiers.push("\u{E008}"),
            "alt" => modifiers.push("\u{E00A}"),
            "meta" | "super" => modifiers.push("\u{E03D}"),
            _ if key.is_none() => key = Some(key_value(part).to_string()),
            _ => return Err(format!("browser/unsupported-key-sequence: {sequence}")),
        }
    }
    let key = key.ok_or_else(|| "browser/empty-key-sequence".to_string())?;
    let mut actions = modifiers
        .iter()
        .map(|value| json!({ "type": "keyDown", "value": value }))
        .collect::<Vec<_>>();
    actions.push(json!({ "type": "keyDown", "value": key }));
    actions.push(json!({ "type": "keyUp", "value": key }));
    actions.extend(
        modifiers
            .iter()
            .rev()
            .map(|value| json!({ "type": "keyUp", "value": value })),
    );
    Ok(actions)
}

fn text_actions(text: &str) -> Vec<Value> {
    text.chars()
        .flat_map(|character| {
            let value = character.to_string();
            [
                json!({ "type": "keyDown", "value": value }),
                json!({ "type": "keyUp", "value": value }),
            ]
        })
        .collect()
}

fn actions_for_input(input: NativeInput) -> Result<Vec<Value>, String> {
    match input {
        NativeInput::MouseMove { x, y } => Ok(vec![pointer_source(vec![pointer_move(x, y, 0)?])]),
        NativeInput::MouseClick {
            x,
            y,
            button,
            click_count,
        } => {
            let button = match button {
                1 => 0,
                2 => 1,
                3 => 2,
                _ => return Err("browser/unsupported-pointer-button".to_string()),
            };
            if !(1..=2).contains(&click_count) {
                return Err("browser/unsupported-click-count".to_string());
            }
            let mut actions = vec![pointer_move(x, y, 0)?];
            for _ in 0..click_count {
                actions.push(json!({ "type": "pointerDown", "button": button }));
                actions.push(json!({ "type": "pointerUp", "button": button }));
            }
            Ok(vec![pointer_source(actions)])
        }
        NativeInput::Drag {
            from_x,
            from_y,
            to_x,
            to_y,
        } => Ok(vec![pointer_source(vec![
            pointer_move(from_x, from_y, 0)?,
            json!({ "type": "pointerDown", "button": 0 }),
            pointer_move(to_x, to_y, 250)?,
            json!({ "type": "pointerUp", "button": 0 }),
        ])]),
        NativeInput::Key { key } => Ok(vec![key_source(key_actions(&key)?)]),
        NativeInput::Text { text } => Ok(vec![key_source(text_actions(&text))]),
        NativeInput::Scroll {
            x,
            y,
            delta_x,
            delta_y,
        } => Ok(vec![json!({
            "type": "wheel",
            "id": "pinvou-wheel",
            "actions": [{
                "type": "scroll",
                "duration": 0,
                "origin": "viewport",
                "x": finite_coordinate(x)?,
                "y": finite_coordinate(y)?,
                "deltaX": finite_coordinate(delta_x)?,
                "deltaY": finite_coordinate(delta_y)?,
            }],
        })]),
    }
}

struct BrowserAutomationContextPlugin {
    profile: PathBuf,
}

struct BrowserAutomationContextPluginInstance;

impl PluginBuilder<tauri::EventLoopMessage> for BrowserAutomationContextPlugin {
    type Plugin = BrowserAutomationContextPluginInstance;

    fn build(self, context: Context<tauri::EventLoopMessage>) -> Self::Plugin {
        let result = context.run_threaded(|main| {
            let main =
                main.ok_or_else(|| "Wry plugin was not installed on the main thread".to_string())?;
            let mut contexts = main
                .web_context
                .lock()
                .map_err(|_| "Wry WebContext store is poisoned".to_string())?;
            match contexts.entry(Some(self.profile.clone())) {
                Entry::Vacant(entry) => {
                    let mut inner = tauri_runtime_wry::wry::WebContext::new(Some(self.profile));
                    inner.set_allows_automation(true);
                    entry.insert(WebContext {
                        inner,
                        referenced_by_webviews: HashSet::new(),
                        registered_custom_protocols: HashSet::new(),
                    });
                    Ok(())
                }
                Entry::Occupied(mut entry) => {
                    if !entry.get().referenced_by_webviews.is_empty() {
                        return Err(
                            "browser WebContext already owns WebViews before automation setup"
                                .to_string(),
                        );
                    }
                    entry.get_mut().inner.set_allows_automation(true);
                    Ok(())
                }
            }
        });

        match result {
            Ok(()) => AUTOMATION_CONTEXT_READY.store(true, Ordering::SeqCst),
            Err(error) => {
                let _ = AUTOMATION_CONTEXT_ERROR.set(error);
            }
        }
        BrowserAutomationContextPluginInstance
    }
}

impl Plugin<tauri::EventLoopMessage> for BrowserAutomationContextPluginInstance {
    fn on_event(
        &mut self,
        _event: &Event<Message<tauri::EventLoopMessage>>,
        _event_loop: &EventLoopWindowTarget<Message<tauri::EventLoopMessage>>,
        _proxy: &EventLoopProxy<Message<tauri::EventLoopMessage>>,
        _control_flow: &mut ControlFlow,
        _context: EventLoopIterationContext<'_, tauri::EventLoopMessage>,
        _web_context: &WebContextStore,
    ) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::super::state::NativeControlOwner;
    use super::*;
    use std::io::{Read, Write};

    fn spawn_one_shot_webdriver_with_responder<F>(
        responder: F,
    ) -> (String, std::thread::JoinHandle<String>)
    where
        F: FnOnce(&str) -> Option<Vec<u8>> + Send + 'static,
    {
        let listener =
            std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind fake WebDriver");
        let address = listener.local_addr().expect("fake WebDriver address");
        let endpoint = format!("http://{address}");
        let request = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept WebDriver request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set fake WebDriver read timeout");
            let mut bytes = Vec::new();
            let mut expected_len = None;
            loop {
                let mut chunk = [0_u8; 1024];
                let read = stream.read(&mut chunk).expect("read WebDriver request");
                if read == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..read]);
                if expected_len.is_none() {
                    if let Some(header_end) =
                        bytes.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        let headers = String::from_utf8_lossy(&bytes[..header_end]);
                        let content_length = headers
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        expected_len = Some(header_end + 4 + content_length);
                    }
                }
                if expected_len.is_some_and(|expected| bytes.len() >= expected) {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&bytes).into_owned();
            if let Some(response) = responder(&request) {
                stream
                    .write_all(&response)
                    .expect("write WebDriver response");
            }
            request
        });
        (endpoint, request)
    }

    fn spawn_one_shot_webdriver(
        response: Option<&'static [u8]>,
    ) -> (String, std::thread::JoinHandle<String>) {
        spawn_one_shot_webdriver_with_responder(move |_| response.map(<[u8]>::to_vec))
    }

    fn request_error(failure: WebDriverRequestFailure) -> WebDriverRequestError {
        WebDriverRequestError {
            message: "synthetic WebDriver failure".to_string(),
            restart_session: false,
            failure,
        }
    }

    fn active_authorization(
        control: &WorkspaceControl,
        session_id: &str,
        tab_token: &str,
        target_id: &str,
    ) -> NativeTabLease {
        let (snapshot, opaque_lease) = control.issue_agent_lease();
        let authorization = NativeTabLease::from_assertion(
            session_id,
            tab_token,
            target_id,
            snapshot.revision,
            opaque_lease,
        )
        .expect("construct native authorization");
        assert!(control.begin_agent_operation(&authorization, false));
        authorization
    }

    #[test]
    fn runtime_is_idle_until_the_first_prepare() {
        let runtime = WebDriverRuntime::new(31_337).expect("create runtime");

        assert_eq!(&*runtime.session.lock(), &DriverSessionState::Idle);
        assert!(runtime.child.lock().is_none());
        assert!(runtime.handles.lock().is_empty());
    }

    #[tokio::test]
    async fn authorized_post_disconnect_after_receive_is_commit_unknown() {
        let runtime = WebDriverRuntime::new(31_339).expect("create runtime");
        let (endpoint, request) = spawn_one_shot_webdriver(None);
        let session = DriverSession {
            endpoint,
            session_id: "session-commit-unknown".to_string(),
        };

        let result = runtime
            .raw_request(
                &session,
                Method::POST,
                "element/button/click",
                Some(json!({})),
            )
            .await;
        let error = runtime
            .finish_authorized_request(result)
            .expect_err("dropped acknowledgement must fail");
        let received = request.join().expect("fake WebDriver request thread");

        assert!(received
            .starts_with("POST /session/session-commit-unknown/element/button/click HTTP/1.1"));
        assert!(error.starts_with(ACTION_COMMIT_UNKNOWN_WEBDRIVER));
        assert!(error.contains("request failed"));
    }

    #[tokio::test]
    async fn authorized_post_with_malformed_ack_is_commit_unknown() {
        let runtime = WebDriverRuntime::new(31_340).expect("create runtime");
        let (endpoint, request) = spawn_one_shot_webdriver(Some(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1\r\nConnection: close\r\n\r\n{",
        ));
        let session = DriverSession {
            endpoint,
            session_id: "session-malformed-ack".to_string(),
        };

        let result = runtime
            .raw_request(
                &session,
                Method::POST,
                "element/button/click",
                Some(json!({})),
            )
            .await;
        let error = runtime
            .finish_authorized_request(result)
            .expect_err("malformed acknowledgement must fail");
        let received = request.join().expect("fake WebDriver request thread");

        assert!(received
            .starts_with("POST /session/session-malformed-ack/element/button/click HTTP/1.1"));
        assert!(error.starts_with(ACTION_COMMIT_UNKNOWN_WEBDRIVER));
        assert!(error.contains("decode WebKitWebDriver"));
    }

    #[tokio::test]
    async fn closed_current_window_does_not_block_switch_to_known_live_handle() {
        let runtime = WebDriverRuntime::new(31_341).expect("create runtime");
        let (endpoint, request) = spawn_one_shot_webdriver_with_responder(|request| {
            let (status, body) = if request.starts_with("GET ") {
                (
                    "404 Not Found",
                    r#"{"value":{"error":"no such window","message":"closed current"}}"#,
                )
            } else {
                ("200 OK", r#"{"value":null}"#)
            };
            Some(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .into_bytes(),
            )
        });
        let session_id = "session-closed-current";
        let child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("start fake live driver process");
        *runtime.child.lock() = Some(child);
        *runtime.session.lock() = DriverSessionState::Ready {
            endpoint,
            session_id: session_id.to_string(),
        };

        let result = runtime.ensure_current_locked("known-live-handle").await;
        runtime.reset_driver(DriverSessionState::Idle);
        let received = request.join().expect("fake WebDriver request thread");

        assert_eq!(result, Ok(()));
        assert!(
            received.starts_with(&format!("POST /session/{session_id}/window HTTP/1.1")),
            "selection consulted the possibly closed current handle first: {received}"
        );
        assert!(received.contains(r#""handle":"known-live-handle""#));
    }

    #[test]
    fn w3c_mutation_errors_are_retryable_only_when_non_commit_is_proven() {
        for code in [
            "invalid argument",
            "invalid element state",
            "invalid session id",
            "element click intercepted",
            "element not interactable",
            "no such alert",
            "no such element",
            "no such window",
            "stale element reference",
            "unsupported operation",
        ] {
            assert_eq!(
                webdriver_mutation_commit_state(&request_error(
                    WebDriverRequestFailure::Protocol {
                        code: Some(code.to_string()),
                    }
                )),
                WebDriverMutationCommitState::NotCommitted,
                "{code}"
            );
        }

        for failure in [
            WebDriverRequestFailure::Transport,
            WebDriverRequestFailure::ResponseDecode,
            WebDriverRequestFailure::Protocol {
                code: Some("timeout".to_string()),
            },
            WebDriverRequestFailure::Protocol {
                code: Some("unknown error".to_string()),
            },
            WebDriverRequestFailure::Protocol { code: None },
        ] {
            assert_eq!(
                webdriver_mutation_commit_state(&request_error(failure)),
                WebDriverMutationCommitState::Unknown
            );
        }
    }

    #[test]
    fn only_a_ready_session_with_a_live_driver_is_reused() {
        let ready = DriverSessionState::Ready {
            endpoint: "http://127.0.0.1:4444".to_string(),
            session_id: "session-1".to_string(),
        };

        assert_eq!(
            ready_session_for_live_process(&ready, true),
            Some(DriverSession {
                endpoint: "http://127.0.0.1:4444".to_string(),
                session_id: "session-1".to_string(),
            })
        );
        assert_eq!(ready_session_for_live_process(&ready, false), None);
        assert_eq!(
            ready_session_for_live_process(&DriverSessionState::Idle, true),
            None
        );
        assert_eq!(
            ready_session_for_live_process(&DriverSessionState::Starting, true),
            None
        );
        assert_eq!(
            ready_session_for_live_process(
                &DriverSessionState::Failed("driver crashed".to_string()),
                false,
            ),
            None
        );
    }

    #[test]
    fn binding_marker_is_opaque_rotating_and_never_injected_into_remote_script() {
        let suffix = format!("{:032x}", rand::random::<u128>());
        let first_label = format!("browser-webview-a-{suffix}");
        let second_label = format!("browser-webview-b-{suffix}");
        let control = Arc::new(WorkspaceControl::new(0, NativeControlOwner::Agent));

        register_webview_binding(&first_label, "0000000000000001", &control)
            .expect("register first binding");
        let first_nonce = expected_binding_nonce(&first_label).expect("first nonce");
        register_webview_binding(&first_label, "0000000000000001", &control)
            .expect("repeat first binding");
        assert_eq!(
            expected_binding_nonce(&first_label).as_deref(),
            Some(first_nonce.as_str()),
            "registration is idempotent until an actual bind rotates the challenge"
        );
        register_webview_binding(&second_label, "0000000000000002", &control)
            .expect("register second binding");
        let second_nonce = expected_binding_nonce(&second_label).expect("second nonce");

        assert_eq!(first_nonce.len(), 64);
        assert!(first_nonce.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first_nonce, second_nonce);
        let rotated = rotate_binding_nonce(&first_label).expect("rotate challenge");
        assert_ne!(first_nonce, rotated);
        let marker = binding_marker_url(&rotated).expect("encode marker");
        assert!(!marker.as_str().contains(&first_label));
        assert_eq!(marker.as_str(), format!("{BINDING_MARKER_PREFIX}{rotated}"));

        unregister_webview_binding(&first_label);
        unregister_webview_binding(&second_label);
        assert!(expected_binding_nonce(&first_label).is_none());
        assert!(expected_binding_nonce(&second_label).is_none());
    }

    #[test]
    fn native_mutation_requires_the_exact_registered_tab_and_active_operation() {
        let suffix = format!("{:032x}", rand::random::<u128>());
        let label_a = format!("browser-webview-auth-a-{suffix}");
        let label_b = format!("browser-webview-auth-b-{suffix}");
        let tab_a = "aaaaaaaaaaaaaaaa";
        let tab_b = "bbbbbbbbbbbbbbbb";
        let control = Arc::new(WorkspaceControl::new(0, NativeControlOwner::Agent));

        register_webview_binding(&label_a, tab_a, &control).expect("register tab a");
        let authorization_a = active_authorization(&control, "session-a", tab_a, "target-a");
        assert_eq!(
            authorize_registered_mutation(&label_a, &authorization_a, false),
            Ok(())
        );
        assert!(
            !control.agent_input_in_progress(),
            "dialog/clear authorization must not suppress real user input"
        );
        assert_eq!(
            authorize_registered_mutation(&label_a, &authorization_a, true),
            Ok(())
        );
        assert!(control.agent_input_in_progress());

        let authorization_b = active_authorization(&control, "session-a", tab_b, "target-b");
        assert_eq!(
            authorize_registered_mutation(&label_a, &authorization_a, false),
            Err("browser/webkit-control-lease-lost".to_string()),
            "a stale operation cannot borrow the current operation"
        );
        assert_eq!(
            authorize_registered_mutation(&label_a, &authorization_b, false),
            Err("browser/webkit-tab-binding-mismatch".to_string()),
            "a current operation for another tab cannot borrow this label"
        );

        register_webview_binding(&label_b, tab_b, &control).expect("register tab b");
        assert_eq!(
            authorize_registered_mutation(&label_b, &authorization_b, false),
            Ok(())
        );

        unregister_webview_binding(&label_a);
        unregister_webview_binding(&label_b);
    }

    #[test]
    fn takeover_after_slow_resolution_fails_before_the_post_boundary() {
        let suffix = format!("{:032x}", rand::random::<u128>());
        let label = format!("browser-webview-slow-resolve-{suffix}");
        let tab = "cccccccccccccccc";
        let control = Arc::new(WorkspaceControl::new(0, NativeControlOwner::Agent));
        register_webview_binding(&label, tab, &control).expect("register binding");
        let authorization = active_authorization(&control, "session-c", tab, "target-c");

        // Model arbitrary selection/DOM-resolution latency: the user takes
        // control after resolution but before the mutation helper is entered.
        control.bump(Some(NativeControlOwner::User));
        let mut post_count = 0;
        let result = authorize_registered_mutation(&label, &authorization, true).map(|()| {
            post_count += 1;
        });

        assert_eq!(result, Err("browser/webkit-control-lease-lost".to_string()));
        assert_eq!(post_count, 0, "no mutation may cross the POST boundary");
        unregister_webview_binding(&label);
    }

    #[test]
    fn multi_step_fill_stops_when_the_operation_is_revoked_between_posts() {
        let suffix = format!("{:032x}", rand::random::<u128>());
        let label = format!("browser-webview-fill-revoke-{suffix}");
        let tab = "dddddddddddddddd";
        let control = Arc::new(WorkspaceControl::new(0, NativeControlOwner::Agent));
        register_webview_binding(&label, tab, &control).expect("register binding");
        let authorization = active_authorization(&control, "session-d", tab, "target-d");

        let mut dispatched_steps = 0;
        authorize_registered_mutation(&label, &authorization, false).expect("authorize clear");
        dispatched_steps += 1;
        control.end_agent_operation(&authorization);
        let second = authorize_registered_mutation(&label, &authorization, true).map(|()| {
            dispatched_steps += 1;
        });

        assert_eq!(second, Err("browser/webkit-control-lease-lost".to_string()));
        assert_eq!(
            dispatched_steps, 1,
            "send-keys must not run after clear when cancellation revoked the operation"
        );
        unregister_webview_binding(&label);
    }

    #[test]
    fn compound_mutation_failure_uses_non_retryable_machine_prefix() {
        let error =
            partially_committed_error("fill", 1, "browser/webkit-control-lease-lost".to_string());
        assert!(error.starts_with("browser/action-partially-committed:"));
        assert!(error.contains("fill completed 1 native step(s)"));
        assert!(error.contains("browser/webkit-control-lease-lost"));
    }

    #[test]
    fn window_binding_requires_exactly_one_internal_marker_match() {
        let ordinary_unbound_windows = vec![
            ("handle-a".to_string(), "https://example.com/a".to_string()),
            ("handle-b".to_string(), "https://example.com/b".to_string()),
        ];
        assert_eq!(
            unique_marker_match(
                &ordinary_unbound_windows,
                "about:blank#pinvou-webdriver-bind-expected"
            )
            .expect("no ordinary URL fallback"),
            None
        );

        let expected = "about:blank#pinvou-webdriver-bind-expected";
        let one_match = vec![
            (
                "handle-a".to_string(),
                "about:blank#pinvou-webdriver-bind-other".to_string(),
            ),
            ("handle-b".to_string(), expected.to_string()),
        ];
        assert_eq!(
            unique_marker_match(&one_match, expected).expect("unique binding"),
            Some("handle-b".to_string())
        );

        let duplicate = vec![
            ("handle-a".to_string(), expected.to_string()),
            ("handle-b".to_string(), expected.to_string()),
        ];
        assert_eq!(
            unique_marker_match(&duplicate, expected),
            Err("browser/webkit-binding-marker-not-unique".to_string())
        );
    }

    #[test]
    fn binding_navigation_is_internal_only_for_the_active_exact_challenge() {
        let label = format!(
            "browser-webview-binding-nav-{:032x}",
            rand::random::<u128>()
        );
        let control = Arc::new(WorkspaceControl::new(0, NativeControlOwner::Unclaimed));
        register_webview_binding(&label, "eeeeeeeeeeeeeeee", &control).expect("register binding");
        let nonce = rotate_binding_nonce(&label).expect("rotate binding nonce");
        let marker = format!("{BINDING_MARKER_PREFIX}{nonce}");

        assert!(!classify_binding_navigation(
            &label,
            &format!("{BINDING_MARKER_PREFIX}{nonce}0")
        ));
        assert!(classify_binding_navigation(&label, &marker));
        assert!(classify_binding_navigation(&label, &marker));
        assert!(!classify_binding_navigation(&label, "about:blank"));
        assert!(!classify_binding_navigation(&label, &marker));

        let second = rotate_binding_nonce(&label).expect("rotate second binding nonce");
        let second_marker = format!("{BINDING_MARKER_PREFIX}{second}");
        assert!(classify_binding_navigation(&label, &second_marker));
        assert!(!classify_binding_navigation(&label, "https://example.com/"));
        assert!(!classify_binding_navigation(&label, &second_marker));
        unregister_webview_binding(&label);
    }

    #[tokio::test]
    async fn operation_gate_keeps_selection_and_action_atomic() {
        let gate = Arc::new(WebDriverOperationGate::default());
        let events = Arc::new(Mutex::new(Vec::new()));
        let release_first = Arc::new(tokio::sync::Notify::new());
        let (first_selected_tx, first_selected_rx) = tokio::sync::oneshot::channel();

        let first = {
            let gate = Arc::clone(&gate);
            let events = Arc::clone(&events);
            let release_first = Arc::clone(&release_first);
            tokio::spawn(async move {
                gate.run(async {
                    events.lock().push("select-a");
                    first_selected_tx.send(()).expect("signal first selection");
                    release_first.notified().await;
                    events.lock().push("action-a");
                })
                .await;
            })
        };
        first_selected_rx.await.expect("first selection");

        let (second_entered_tx, mut second_entered_rx) = tokio::sync::oneshot::channel();
        let second = {
            let gate = Arc::clone(&gate);
            let events = Arc::clone(&events);
            tokio::spawn(async move {
                gate.run(async {
                    events.lock().push("select-b");
                    second_entered_tx.send(()).expect("signal second selection");
                    events.lock().push("action-b");
                })
                .await;
            })
        };

        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut second_entered_rx)
                .await
                .is_err(),
            "second operation entered before the first action completed"
        );
        assert_eq!(&*events.lock(), &["select-a"]);
        release_first.notify_one();
        second_entered_rx
            .await
            .expect("second operation selected after first action");
        first.await.expect("first task");
        second.await.expect("second task");

        assert_eq!(
            &*events.lock(),
            &["select-a", "action-a", "select-b", "action-b"]
        );
    }

    #[tokio::test]
    async fn stop_waits_for_inflight_operation_and_commits_idle_last() {
        let runtime = WebDriverRuntime::new(31_338).expect("create runtime");
        let (selected_tx, selected_rx) = tokio::sync::oneshot::channel();
        let release_action = Arc::new(tokio::sync::Notify::new());

        let operation = {
            let runtime = Arc::clone(&runtime);
            let release_action = Arc::clone(&release_action);
            tokio::spawn(async move {
                runtime
                    .operations
                    .run(async {
                        *runtime.session.lock() = DriverSessionState::Starting;
                        selected_tx.send(()).expect("signal selection");
                        release_action.notified().await;
                        *runtime.session.lock() = DriverSessionState::Ready {
                            endpoint: "http://127.0.0.1:4444".to_string(),
                            session_id: "inflight-session".to_string(),
                        };
                        runtime
                            .handles
                            .lock()
                            .insert("webview".to_string(), "handle".to_string());
                    })
                    .await;
            })
        };
        selected_rx.await.expect("operation selected a window");

        let (stop_started_tx, stop_started_rx) = tokio::sync::oneshot::channel();
        let stop = {
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                stop_started_tx.send(()).expect("signal stop start");
                runtime.shutdown_for_stop().await;
            })
        };
        stop_started_rx.await.expect("stop started");
        assert!(
            tokio::time::timeout(Duration::from_millis(25), async {
                while !stop.is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .is_err(),
            "stop completed while the selected operation was still in flight"
        );

        release_action.notify_one();
        operation.await.expect("operation task");
        stop.await.expect("stop task");

        assert_eq!(&*runtime.session.lock(), &DriverSessionState::Idle);
        assert!(runtime.handles.lock().is_empty());
        assert!(runtime.child.lock().is_none());
    }

    #[test]
    fn webdriver_failures_restart_only_for_session_loss() {
        assert!(webdriver_error_requires_restart(&json!({
            "value": { "error": "invalid session id" }
        })));
        assert!(webdriver_error_requires_restart(&json!({
            "value": { "error": "session not created" }
        })));
        assert!(!webdriver_error_requires_restart(&json!({
            "value": { "error": "no such window" }
        })));

        let no_context = json!({
            "value": {
                "error": "session not created",
                "message": "Failed to create a new browsing context"
            }
        });
        assert!(is_transient_session_start_error(
            &no_context,
            "unrelated rendered text"
        ));
        assert!(!is_transient_session_start_error(
            &json!({ "value": { "message": "unsupported capability" } }),
            "unrelated rendered text"
        ));
    }

    #[test]
    fn webdriver_error_keeps_the_standard_code_when_message_is_empty() {
        assert_eq!(
            webdriver_error(
                &json!({
                    "value": {
                        "error": "unexpected alert open",
                        "message": ""
                    }
                }),
                400,
            ),
            "WebKitWebDriver HTTP 400: unexpected alert open"
        );
        assert_eq!(
            webdriver_error(
                &json!({
                    "value": {
                        "error": "no such alert",
                        "message": "No JavaScript dialog is open"
                    }
                }),
                404,
            ),
            "WebKitWebDriver HTTP 404: no such alert: No JavaScript dialog is open"
        );
    }

    #[test]
    fn dialog_actions_map_only_to_w3c_alert_endpoints() {
        assert_eq!(
            dialog_action_endpoint("accept").expect("accept endpoint"),
            "alert/accept"
        );
        assert_eq!(
            dialog_action_endpoint("dismiss").expect("dismiss endpoint"),
            "alert/dismiss"
        );
        assert_eq!(
            dialog_action_endpoint("confirm"),
            Err("browser/invalid-argument: action".to_string())
        );
    }

    #[test]
    fn inspector_is_loopback_only() {
        prepare_process_environment().expect("prepare inspector");
        let endpoint = std::env::var("WEBKIT_INSPECTOR_SERVER").expect("inspector endpoint");
        assert!(endpoint.starts_with("127.0.0.1:"));
    }

    #[test]
    fn webdriver_actions_use_page_local_sources() {
        let actions = actions_for_input(NativeInput::MouseClick {
            x: 12.0,
            y: 34.0,
            button: 1,
            click_count: 1,
        })
        .expect("click actions");
        assert_eq!(actions[0]["type"], "pointer");
        assert_eq!(actions[0]["actions"][0]["origin"], "viewport");
        assert_eq!(key_actions("Control+A").expect("key actions").len(), 4);
    }

    #[test]
    fn driver_override_must_name_a_real_file() {
        let missing = std::path::Path::new("/pinvou/definitely-missing/WebKitWebDriver");
        assert!(!missing.is_file());
    }
}
