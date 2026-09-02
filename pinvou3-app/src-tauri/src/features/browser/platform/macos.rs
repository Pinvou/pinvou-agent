//! macOS WKWebView BrowserCore driver.
//!
//! DOM discovery and script evaluation execute in the exact task-owned
//! `WKWebView`.  Pointer and keyboard input is delivered as app-scoped AppKit
//! events directly to that WebView; the adapter never posts a CoreGraphics
//! event, moves the desktop cursor, or installs a global keyboard hook.

use std::collections::HashMap;
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use objc2::MainThreadMarker;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSDeleteFunctionKey, NSDownArrowFunctionKey, NSEndFunctionKey, NSEvent, NSEventModifierFlags,
    NSEventType, NSHomeFunctionKey, NSLeftArrowFunctionKey, NSPageDownFunctionKey,
    NSPageUpFunctionKey, NSResponder, NSRightArrowFunctionKey, NSStandardKeyBindingResponding,
    NSUpArrowFunctionKey, NSView, NSWindow,
};
use objc2_foundation::{NSError, NSPoint, NSString};
use objc2_web_kit::{WKContentWorld, WKWebView};
use parking_lot::Mutex;
use serde_json::Value;
use tauri::Webview;

use super::state::{NativeTabLease, WorkspaceControl};
use super::{
    ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION, AsyncDispatchState, BrowserCoreEvaluationMode,
    NativeInput,
};

const EVALUATION_TIMEOUT: Duration = Duration::from_secs(15);
const BIND_TIMEOUT: Duration = Duration::from_secs(5);
const BIND_RETRY_INTERVAL: Duration = Duration::from_millis(25);
const CORE_GLOBAL: &str = "__PINVOU_BROWSER_CORE_V1__";
const ACTION_COMMITTED_FOCUS_RESTORE_FAILED: &str = "browser/action-committed-focus-restore-failed";
const ACTION_COMMIT_UNKNOWN_FOCUS_RESTORE_FAILED: &str =
    "browser/action-commit-unknown-focus-restore-failed";
const ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION: &str =
    "browser/action-commit-unknown-after-input-interruption";
const ACTION_PARTIALLY_COMMITTED: &str = "browser/action-partially-committed";

struct WebviewBinding {
    tab_token: String,
    control: Weak<WorkspaceControl>,
}

static REGISTERED_WEBVIEWS: OnceLock<Mutex<HashMap<String, WebviewBinding>>> = OnceLock::new();
static INPUT_GATE: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

fn registered_webviews() -> &'static Mutex<HashMap<String, WebviewBinding>> {
    REGISTERED_WEBVIEWS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn input_gate() -> &'static tokio::sync::Mutex<()> {
    INPUT_GATE.get_or_init(|| tokio::sync::Mutex::new(()))
}

pub(super) fn backend_available() -> bool {
    true
}

/// Register the host-owned Tauri label and its expected tab identity in Rust
/// process memory. Unlike Linux, macOS does not need to discover an external
/// WebDriver target: the Tauri `Webview` handle already identifies the exact
/// WKWebView. No identity, lease, nonce, or capability is injected into the
/// remote document.
pub(super) fn register_webview_binding(
    label: &str,
    tab_token: &str,
    control: &Arc<WorkspaceControl>,
) -> Result<(), String> {
    if label.is_empty() || label.len() > 256 {
        return Err("browser/wkwebview-binding-label-invalid".to_string());
    }
    if tab_token.is_empty() || tab_token.len() > 256 {
        return Err("browser/wkwebview-binding-tab-invalid".to_string());
    }
    registered_webviews().lock().insert(
        label.to_string(),
        WebviewBinding {
            tab_token: tab_token.to_string(),
            control: Arc::downgrade(control),
        },
    );
    Ok(())
}

pub(super) fn unregister_webview_binding(label: &str) {
    registered_webviews().lock().remove(label);
}

pub(super) async fn wait_until_ready() -> Result<(), String> {
    Ok(())
}

/// Bind the host's stable Tauri handle to BrowserCore.  The retry covers the
/// short interval between `add_child` returning and the document-start script
/// becoming callable in a newly-created, hidden WKWebView.
pub(super) async fn bind_webview(webview: &Webview) -> Result<(), String> {
    let label = webview.label().to_string();
    if !registered_webviews().lock().contains_key(&label) {
        return Err("browser/wkwebview-binding-not-registered".to_string());
    }

    let deadline = tokio::time::Instant::now() + BIND_TIMEOUT;
    let script = format!("return {{ ready: globalThis.{CORE_GLOBAL}?.version === 1 }};");
    loop {
        match evaluate_json(
            webview,
            script.clone(),
            BrowserCoreEvaluationMode::ReadOnly,
            None,
        )
        .await
        {
            Ok(value) if value.get("ready").and_then(Value::as_bool) == Some(true) => {
                return Ok(());
            }
            Ok(_) | Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(BIND_RETRY_INTERVAL).await;
            }
            Ok(_) => return Err("browser/wkwebview-core-runtime-unavailable".to_string()),
            Err(error) => return Err(error),
        }
    }
}

/// Evaluate an async BrowserCore function body in WKWebView's page world and
/// copy the JSON result out of the Objective-C callback before it returns.
pub(super) async fn evaluate_json(
    webview: &Webview,
    script: String,
    mode: BrowserCoreEvaluationMode,
    authorization: Option<&NativeTabLease>,
) -> Result<Value, String> {
    let authorization = super::evaluation_authorization(mode, authorization)?.cloned();
    let label = webview.label().to_string();
    let function_body = wrap_json_evaluation(&script);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(tx)));
    let dispatch_state = AsyncDispatchState::new();
    let callback_state = dispatch_state.clone();

    webview
        .with_webview(move |platform| {
            let result = if platform.inner().is_null() {
                Err("browser/wkwebview-native-handle-null".to_string())
            } else {
                let Some(main_thread) = MainThreadMarker::new() else {
                    if let Some(sender) = sender.lock().take() {
                        let _ =
                            sender.send(Err("browser/wkwebview-not-on-main-thread".to_string()));
                    }
                    callback_state.cancel_pending();
                    return;
                };
                let body = NSString::from_str(&function_body);
                // SAFETY: MainThreadMarker::new() above proves this code runs
                // on the main thread, which pageWorld requires; it returns the
                // shared process-wide page world.
                let world = unsafe { WKContentWorld::pageWorld(main_thread) };
                let callback_sender = Arc::clone(&sender);
                let completion_state = callback_state.clone();
                let block =
                    block2::RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
                        let result = copy_json_callback(value, error);
                        if let Some(sender) = callback_sender.lock().take() {
                            let _ = sender.send(result);
                        }
                        completion_state.finish();
                    });
                // SAFETY: on macOS, wry's WebviewInner is exactly a WKWebView
                // and with_webview guarantees main-thread access while the
                // closure runs, keeping the reference alive for this scope.
                let view = unsafe { &*platform.inner().cast::<WKWebView>() };
                let dispatch = || {
                    if !callback_state.begin() {
                        return Ok(());
                    }
                    // SAFETY: view is a valid &WKWebView obtained above under
                    // with_webview's main-thread guarantee; body and world are
                    // valid objects alive for the call; block is an RcBlock
                    // matching the completion-handler signature, borrowed for
                    // the call, and the handler copies its result out before
                    // returning.
                    unsafe {
                        view.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                            &body,
                            None,
                            None,
                            &world,
                            Some(&block),
                        );
                    }
                    Ok(())
                };
                if let Some(authorization) = authorization.as_ref() {
                    dispatch_script_mutation_if_authorized(&label, authorization, dispatch)
                } else {
                    dispatch()
                }
            };

            if let Err(error) = result {
                if let Some(sender) = sender.lock().take() {
                    let _ = sender.send(Err(error));
                }
                callback_state.cancel_pending();
            }
        })
        .map_err(|error| format!("browser/wkwebview-access-failed: {error}"))?;

    let commit_unknown_prefix = matches!(mode, BrowserCoreEvaluationMode::MayMutate)
        .then_some(ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION);
    dispatch_state
        .wait(
            rx,
            EVALUATION_TIMEOUT,
            "browser/wkwebview-javascript-timeout",
            "browser/wkwebview-javascript-callback-closed",
            commit_unknown_prefix,
        )
        .await
}

fn wrap_json_evaluation(script: &str) -> String {
    format!(
        "const __pinvouValue = await (async () => {{\n{script}\n}})();\n\
         const __pinvouJson = JSON.stringify(__pinvouValue);\n\
         return __pinvouJson === undefined ? 'null' : __pinvouJson;"
    )
}

fn copy_json_callback(value: *mut AnyObject, error: *mut NSError) -> Result<Value, String> {
    if let Some(error) = NonNull::new(error) {
        // SAFETY: error is non-null (checked above) and points to an NSError
        // owned by the WKWebView completion callback, valid for the duration
        // of this callback; the description is copied out as a String.
        let description = unsafe { error.as_ref() }.localizedDescription().to_string();
        return Err(format!(
            "browser/wkwebview-javascript-failed: {description}"
        ));
    }
    let value = NonNull::new(value)
        .ok_or_else(|| "browser/wkwebview-javascript-result-null".to_string())?;
    // SAFETY: value is non-null (checked above) and points to the result
    // object owned by the WKWebView completion callback, valid for the
    // duration of this callback; the string is copied out below.
    let value = unsafe { value.as_ref() };
    let json_string = value
        .downcast_ref::<NSString>()
        .ok_or_else(|| "browser/wkwebview-javascript-result-not-json-string".to_string())?
        .to_string();
    serde_json::from_str(&json_string)
        .map_err(|error| format!("browser/wkwebview-json-invalid: {error}"))
}

pub(super) async fn dispatch_input(
    webview: &Webview,
    authorization: &NativeTabLease,
    input: NativeInput,
) -> Result<(), String> {
    let _guard = input_gate().lock().await;
    match input {
        NativeInput::MouseClick {
            x,
            y,
            button,
            click_count,
        } => dispatch_mouse_click(webview, authorization, x, y, button, click_count).await,
        NativeInput::Key { key } => dispatch_key(webview, authorization, &key).await,
        NativeInput::Text { text } => insert_text(webview, authorization, &text).await,
        NativeInput::MouseMove { .. } | NativeInput::Drag { .. } | NativeInput::Scroll { .. } => {
            Err("browser/trusted-input-gesture-unavailable-on-wkwebview".to_string())
        }
    }
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
    let _guard = input_gate().lock().await;
    let (x, y) = resolve_element_point(webview, uid).await?;
    dispatch_mouse_click(webview, authorization, x, y, 1, click_count).await
}

pub(super) async fn fill_element(
    webview: &Webview,
    authorization: &NativeTabLease,
    uid: &str,
    value: &str,
) -> Result<(), String> {
    let _guard = input_gate().lock().await;
    let (x, y) = resolve_element_point(webview, uid).await?;
    if let Err(error) = dispatch_mouse_click(webview, authorization, x, y, 1, 1).await {
        return Err(mark_incomplete_after_possible_commit(
            "the field was clicked before text replacement began",
            error,
        ));
    }
    if let Err(error) = select_all(webview, authorization).await {
        return Err(mark_action_partially_committed(
            "click succeeded before select-all",
            error,
        ));
    }
    let backspace = KeyStroke::named("Backspace").map_err(|error| {
        mark_action_partially_committed("click and select-all succeeded before clear", error)
    })?;
    if let Err(error) = dispatch_key_event(webview, authorization, &backspace).await {
        return Err(mark_action_partially_committed(
            "click and select-all succeeded before clear",
            error,
        ));
    }
    if !value.is_empty() {
        if let Err(error) = insert_text(webview, authorization, value).await {
            return Err(mark_action_partially_committed(
                "the previous field value was cleared before replacement text",
                error,
            ));
        }
    }
    Ok(())
}

pub(super) async fn type_text(
    webview: &Webview,
    authorization: &NativeTabLease,
    text: &str,
    submit_key: Option<&str>,
) -> Result<(), String> {
    let _guard = input_gate().lock().await;
    let mut text_committed = false;
    if !text.is_empty() {
        if let Err(error) = insert_text(webview, authorization, text).await {
            return Err(if submit_key.is_some() {
                mark_incomplete_after_possible_commit(
                    "text may have been inserted before submit-key dispatch began",
                    error,
                )
            } else {
                error
            });
        }
        text_committed = true;
    }
    if let Some(key) = submit_key {
        if let Err(error) = dispatch_key(webview, authorization, key).await {
            if text_committed {
                return Err(mark_action_partially_committed(
                    "text was inserted before submit-key dispatch",
                    error,
                ));
            }
            return Err(error);
        }
    }
    Ok(())
}

pub(super) async fn press_key(
    webview: &Webview,
    authorization: &NativeTabLease,
    key: &str,
) -> Result<(), String> {
    let _guard = input_gate().lock().await;
    dispatch_key(webview, authorization, key).await
}

pub(super) async fn handle_dialog(
    webview: &Webview,
    authorization: &NativeTabLease,
    _action: &str,
    _prompt_text: Option<&str>,
) -> Result<String, String> {
    // WKWebView dialog handling is intentionally unsupported until a native
    // UI delegate is owned by BrowserCore. Still validate the exact host lease
    // so a stale/background task cannot probe this page-scoped route. This is
    // authorize-only: no native event is emitted and no takeover grace window
    // may be opened.
    authorize_agent_input_for_label(webview.label(), authorization, false)?;
    Err("browser/dialog-backend-unavailable-on-wkwebview".to_string())
}

async fn resolve_element_point(webview: &Webview, uid: &str) -> Result<(f64, f64), String> {
    let uid = serde_json::to_string(uid)
        .map_err(|error| format!("browser/invalid-element-uid: {error}"))?;
    let value = evaluate_json(
        webview,
        format!(
            "const core = globalThis.{CORE_GLOBAL};\n\
             if (!core || core.version !== 1) throw new Error('browser/core-runtime-unavailable');\n\
             return await core.point({uid});"
        ),
        BrowserCoreEvaluationMode::ReadOnly,
        None,
    )
    .await?;
    let x = finite_coordinate(value.get("x"), "x")?;
    let y = finite_coordinate(value.get("y"), "y")?;
    Ok((x, y))
}

fn finite_coordinate(value: Option<&Value>, field: &str) -> Result<f64, String> {
    let value = value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("browser/invalid-element-coordinate: {field}"))?;
    if value.abs() > 1_000_000.0 {
        return Err(format!("browser/invalid-element-coordinate: {field}"));
    }
    Ok(value)
}

async fn dispatch_mouse_click(
    webview: &Webview,
    authorization: &NativeTabLease,
    x: f64,
    y: f64,
    button: u32,
    click_count: u8,
) -> Result<(), String> {
    if button != 1 {
        return Err("browser/unsupported-pointer-button".to_string());
    }
    if !(1..=2).contains(&click_count) || !x.is_finite() || !y.is_finite() {
        return Err("browser/invalid-pointer-input".to_string());
    }
    with_native_webview(webview, authorization, true, move |view, window| {
        let bounds = view.bounds();
        if x < 0.0 || y < 0.0 || x > bounds.size.width || y > bounds.size.height {
            return Err("browser/pointer-outside-wkwebview".to_string());
        }
        let local_y = if view.isFlipped() {
            y
        } else {
            bounds.size.height - y
        };
        let local_point = NSPoint::new(x, local_y);
        let window_point = view.convertPoint_toView(local_point, None);
        // WKWebView is a container. Dispatch to the private content NSView
        // selected by AppKit hit-testing so the event follows WebKit's native
        // responder path without ever entering the rest of the application UI.
        let target =
            hit_test_webview_local_point(view, local_point, "browser/wkwebview-hit-test-failed")?;
        let webview_view: &NSView = view;
        if !std::ptr::eq(&*target, webview_view) && !target.isDescendantOf(webview_view) {
            return Err("browser/wkwebview-hit-test-escaped-surface".to_string());
        }
        let focus_target: &NSResponder = &target;
        with_temporary_browser_focus(view, window, focus_target, |_responder| {
            // Construct the complete sequence before the first mouseDown. A
            // double-click cannot therefore report a pre-commit creation
            // error after its first click has already been delivered.
            let mut events = Vec::with_capacity(usize::from(click_count) * 2);
            for count in 1..=click_count {
                let down = mouse_event(
                    NSEventType::LeftMouseDown,
                    window_point,
                    window.windowNumber(),
                    count,
                    1.0,
                )?;
                let up = mouse_event(
                    NSEventType::LeftMouseUp,
                    window_point,
                    window.windowNumber(),
                    count,
                    0.0,
                )?;
                events.push((down, up));
            }
            for (down, up) in events {
                target.mouseDown(&down);
                target.mouseUp(&up);
            }
            Ok(())
        })
    })
    .await
}

fn mouse_event(
    event_type: NSEventType,
    location: NSPoint,
    window_number: isize,
    click_count: u8,
    pressure: f32,
) -> Result<objc2::rc::Retained<NSEvent>, String> {
    NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        event_type,
        location,
        NSEventModifierFlags::empty(),
        0.0,
        window_number,
        None,
        0,
        isize::from(click_count),
        pressure,
    )
    .ok_or_else(|| "browser/wkwebview-mouse-event-creation-failed".to_string())
}

async fn insert_text(
    webview: &Webview,
    authorization: &NativeTabLease,
    text: &str,
) -> Result<(), String> {
    let text = text.to_string();
    with_native_webview(webview, authorization, false, move |view, window| {
        let target = browser_content_focus_target(view)?;
        let focus_target: &NSResponder = &target;
        with_temporary_browser_focus(view, window, focus_target, |responder| {
            let text = NSString::from_str(&text);
            let object: &AnyObject = &text;
            // SAFETY: responder is the WKContentView-derived NSResponder made
            // first responder by with_temporary_browser_focus and is alive for
            // this call; object is a valid &NSString alive for the call.
            unsafe { responder.insertText(object) };
            Ok(())
        })
    })
    .await
}

async fn select_all(webview: &Webview, authorization: &NativeTabLease) -> Result<(), String> {
    with_native_webview(webview, authorization, false, move |view, window| {
        let target = browser_content_focus_target(view)?;
        let focus_target: &NSResponder = &target;
        with_temporary_browser_focus(view, window, focus_target, |responder| {
            // `performKeyEquivalent:` depends on the application/window being
            // active and can reduce Command-A to a keyUp-only DOM sequence in
            // a non-key Pinvou window. Invoke AppKit's native editing command
            // on the exact WKContentView responder so background fill keeps
            // the user's frontmost application unchanged.
            // SAFETY: responder is the WKContentView-derived NSResponder made
            // first responder by with_temporary_browser_focus and is alive for
            // this call; a nil sender is accepted by AppKit's selectAll:.
            unsafe { responder.selectAll(None) };
            Ok(())
        })
    })
    .await
}

async fn dispatch_key(
    webview: &Webview,
    authorization: &NativeTabLease,
    key: &str,
) -> Result<(), String> {
    let stroke = KeyStroke::parse(key)?;
    dispatch_key_event(webview, authorization, &stroke).await
}

async fn dispatch_key_event(
    webview: &Webview,
    authorization: &NativeTabLease,
    stroke: &KeyStroke,
) -> Result<(), String> {
    let stroke = stroke.clone();
    with_native_webview(webview, authorization, true, move |view, window| {
        let target = browser_content_focus_target(view)?;
        let focus_target: &NSResponder = &target;
        with_temporary_browser_focus(view, window, focus_target, |responder| {
            let characters = NSString::from_str(&stroke.characters);
            let ignoring_modifiers = NSString::from_str(&stroke.characters_ignoring_modifiers);
            let down = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                NSEventType::KeyDown,
                NSPoint::new(0.0, 0.0),
                stroke.modifiers,
                0.0,
                window.windowNumber(),
                None,
                &characters,
                &ignoring_modifiers,
                false,
                stroke.key_code,
            )
            .ok_or_else(|| "browser/wkwebview-key-event-creation-failed".to_string())?;
            let up = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                NSEventType::KeyUp,
                NSPoint::new(0.0, 0.0),
                stroke.modifiers,
                0.0,
                window.windowNumber(),
                None,
                &characters,
                &ignoring_modifiers,
                false,
                stroke.key_code,
            )
            .ok_or_else(|| "browser/wkwebview-key-event-creation-failed".to_string())?;

            if stroke.modifiers.contains(NSEventModifierFlags::Command) {
                let _ = responder.performKeyEquivalent(&down);
            } else {
                responder.keyDown(&down);
            }
            responder.keyUp(&up);
            Ok(())
        })
    })
    .await
}

/// Return a responder that is physically owned by this task's WKWebView.
/// This prevents a stale focus in the chat composer (or any sibling AppKit
/// view) from receiving text when an Agent tool is routed to the browser.
fn browser_first_responder(
    view: &WKWebView,
    window: &NSWindow,
) -> Result<objc2::rc::Retained<NSResponder>, String> {
    let responder = window
        .firstResponder()
        .ok_or_else(|| "browser/wkwebview-first-responder-missing".to_string())?;
    let responder_view = responder
        .downcast_ref::<NSView>()
        .ok_or_else(|| "browser/wkwebview-first-responder-not-view".to_string())?;
    let webview_view: &NSView = view;
    if !std::ptr::eq(responder_view, webview_view) && !responder_view.isDescendantOf(webview_view) {
        return Err("browser/wkwebview-first-responder-outside-surface".to_string());
    }
    Ok(responder)
}

/// Resolve WebKit's app-scoped native text responder without depending on a
/// private Objective-C class name. On macOS WebKit may expose no hittable
/// content descendant: `hitTest(_:)` then returns the exact `WKWebView`, which
/// itself accepts first-responder status and forwards native text/key events
/// to the focused DOM element. Both that exact container and its descendants
/// remain confined to this task-owned surface.
fn browser_content_focus_target(view: &WKWebView) -> Result<objc2::rc::Retained<NSView>, String> {
    let bounds = view.bounds();
    let centre = NSPoint::new(bounds.size.width / 2.0, bounds.size.height / 2.0);
    let target =
        hit_test_webview_local_point(view, centre, "browser/wkwebview-content-hit-test-failed")?;
    let webview_view: &NSView = view;
    if !std::ptr::eq(&*target, webview_view) && !target.isDescendantOf(webview_view) {
        return Err("browser/wkwebview-content-hit-test-escaped-surface".to_string());
    }
    Ok(target)
}

/// AppKit's `hitTest(_:)` accepts a point in the receiver's *superview*
/// coordinate system, while BrowserCore resolves DOM points in WKWebView-local
/// coordinates. Convert at this single boundary so a docked WebView with a
/// non-zero frame origin cannot miss its own content view or escape into a
/// sibling application surface.
fn hit_test_webview_local_point(
    view: &WKWebView,
    local_point: NSPoint,
    missing_error: &str,
) -> Result<objc2::rc::Retained<NSView>, String> {
    // SAFETY: every caller runs inside `with_native_webview`, on Tauri's main
    // thread, and AppKit view hierarchy access is confined to that closure.
    let superview = unsafe { view.superview() }
        .ok_or_else(|| "browser/wkwebview-superview-missing".to_string())?;
    let point_in_superview = view.convertPoint_toView(local_point, Some(&*superview));
    view.hitTest(point_in_superview)
        .ok_or_else(|| missing_error.to_string())
}

/// Execute one app-scoped native action without transferring persistent AppKit
/// focus away from the rest of Pinvou.  A task-owned child WebView may only be
/// focused while it is physically visible and attached to this exact window.
/// `makeFirstResponder` is advisory on AppKit, so the actual responder is read
/// back and checked before the action is allowed to run.
fn with_temporary_browser_focus(
    view: &WKWebView,
    window: &NSWindow,
    focus_target: &NSResponder,
    operation: impl FnOnce(&NSResponder) -> Result<(), String>,
) -> Result<(), String> {
    if let Err(error) = ensure_interactive_webview(view, window) {
        return reject_after_clearing_unsafe_responder(window, error);
    }
    ensure_responder_belongs_to_webview(focus_target, view, "focus-target")?;

    let previous = window.firstResponder();
    if let Some(previous) = previous.as_deref() {
        if let Err(error) = ensure_responder_is_restorable(previous, window) {
            return reject_after_clearing_unsafe_responder(window, error);
        }
    }

    // Do not use `?` in this block: every path after the saved responder must
    // run the restoration step, including focus rejection and action failure.
    let action_result = if !window.makeFirstResponder(Some(focus_target)) {
        Err("browser/wkwebview-focus-rejected".to_string())
    } else {
        match browser_first_responder(view, window) {
            Ok(actual) => operation(&actual),
            Err(error) => Err(error),
        }
    };

    let restore_result = restore_first_responder(view, window, previous.as_deref());
    combine_action_and_restore(action_result, restore_result)
}

fn ensure_interactive_webview(view: &WKWebView, window: &NSWindow) -> Result<(), String> {
    let attached_window = view
        .window()
        .ok_or_else(|| "browser/wkwebview-surface-detached".to_string())?;
    if !std::ptr::eq(&*attached_window, window) {
        return Err("browser/wkwebview-window-mismatch".to_string());
    }
    if !window.isVisible() || window.isMiniaturized() {
        return Err("browser/wkwebview-window-not-interactive".to_string());
    }
    if view.isHiddenOrHasHiddenAncestor() {
        return Err("browser/wkwebview-surface-hidden".to_string());
    }
    let bounds = view.bounds();
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return Err("browser/wkwebview-surface-empty".to_string());
    }
    let visible = view.visibleRect();
    if visible.size.width <= 0.0 || visible.size.height <= 0.0 {
        return Err("browser/wkwebview-surface-clipped".to_string());
    }
    Ok(())
}

fn ensure_responder_belongs_to_webview(
    responder: &NSResponder,
    view: &WKWebView,
    role: &str,
) -> Result<(), String> {
    let responder_view = responder
        .downcast_ref::<NSView>()
        .ok_or_else(|| format!("browser/wkwebview-{role}-not-view"))?;
    let webview_view: &NSView = view;
    if !std::ptr::eq(responder_view, webview_view) && !responder_view.isDescendantOf(webview_view) {
        return Err(format!("browser/wkwebview-{role}-outside-surface"));
    }
    Ok(())
}

/// A saved sibling responder is safe to restore only while it remains visible
/// in the same NSWindow.  In particular, never restore a responder belonging
/// to a tab or task that became hidden while the Agent action was running.
fn ensure_responder_is_restorable(
    responder: &NSResponder,
    window: &NSWindow,
) -> Result<(), String> {
    let Some(responder_view) = responder.downcast_ref::<NSView>() else {
        // NSWindow and other non-view responders are owned by AppKit rather
        // than a hidden child surface and can be restored normally.
        return Ok(());
    };
    let responder_window = responder_view
        .window()
        .ok_or_else(|| "browser/wkwebview-previous-responder-detached".to_string())?;
    if !std::ptr::eq(&*responder_window, window) {
        return Err("browser/wkwebview-previous-responder-window-mismatch".to_string());
    }
    if responder_view.isHiddenOrHasHiddenAncestor() {
        return Err("browser/wkwebview-previous-responder-hidden".to_string());
    }
    Ok(())
}

/// Reject a hidden/background dispatch without preserving an already-stale
/// responder from that hidden task. A visible sibling responder (for example
/// the chat composer) is not touched.
fn reject_after_clearing_unsafe_responder(window: &NSWindow, error: String) -> Result<(), String> {
    let unsafe_responder = window
        .firstResponder()
        .as_deref()
        .is_some_and(|responder| ensure_responder_is_restorable(responder, window).is_err());
    if !unsafe_responder {
        return Err(error);
    }

    let _ = window.makeFirstResponder(None);
    let still_unsafe = window
        .firstResponder()
        .as_deref()
        .is_some_and(|responder| ensure_responder_is_restorable(responder, window).is_err());
    if still_unsafe {
        Err(format!(
            "browser/wkwebview-rejected-and-unsafe-focus-clear-failed: {error}"
        ))
    } else {
        Err(error)
    }
}

fn restore_first_responder(
    view: &WKWebView,
    window: &NSWindow,
    previous: Option<&NSResponder>,
) -> Result<(), String> {
    if let Some(previous) = previous {
        if let Err(error) = ensure_responder_is_restorable(previous, window) {
            clear_browser_focus(view, window)?;
            return Err(format!("browser/wkwebview-focus-restore-unsafe: {error}"));
        }
    }

    let accepted = window.makeFirstResponder(previous);
    let actual = window.firstResponder();
    let restored = match previous {
        Some(previous) => actual
            .as_deref()
            .is_some_and(|actual| std::ptr::eq(actual, previous)),
        // AppKit may represent `makeFirstResponder(nil)` by making NSWindow
        // itself first responder. Either state is safe as long as the scoped
        // browser surface no longer owns focus.
        None => actual.as_deref().is_none_or(|actual| {
            ensure_responder_belongs_to_webview(actual, view, "restored").is_err()
        }),
    };
    if accepted && restored {
        return Ok(());
    }

    clear_browser_focus(view, window)?;
    Err("browser/wkwebview-focus-restore-failed".to_string())
}

fn clear_browser_focus(view: &WKWebView, window: &NSWindow) -> Result<(), String> {
    let _ = window.makeFirstResponder(None);
    let browser_still_focused = window
        .firstResponder()
        .as_deref()
        .is_some_and(|actual| ensure_responder_belongs_to_webview(actual, view, "cleared").is_ok());
    if browser_still_focused {
        Err("browser/wkwebview-focus-clear-failed".to_string())
    } else {
        Ok(())
    }
}

fn combine_action_and_restore(
    action: Result<(), String>,
    restore: Result<(), String>,
) -> Result<(), String> {
    match (action, restore) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(action), Ok(())) => Err(action),
        (Ok(()), Err(restore)) => Err(format!(
            "{ACTION_COMMITTED_FOCUS_RESTORE_FAILED}: {restore}"
        )),
        (Err(action), Err(restore)) => Err(format!(
            "{ACTION_COMMIT_UNKNOWN_FOCUS_RESTORE_FAILED}: action={action}; restore={restore}"
        )),
    }
}

async fn with_native_webview(
    webview: &Webview,
    authorization: &NativeTabLease,
    emits_takeover_signal: bool,
    operation: impl FnOnce(&WKWebView, &NSWindow) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let dispatch_state = AsyncDispatchState::new();
    let callback_state = dispatch_state.clone();
    let label = webview.label().to_string();
    let authorization = authorization.clone();
    webview
        .with_webview(move |platform| {
            let result = if platform.inner().is_null() || platform.ns_window().is_null() {
                Err("browser/wkwebview-native-handle-null".to_string())
            } else if MainThreadMarker::new().is_none() {
                Err("browser/wkwebview-not-on-main-thread".to_string())
            } else {
                // SAFETY: on macOS, wry's WebviewInner is exactly a WKWebView
                // and ns_window() an NSWindow; with_webview guarantees
                // main-thread access and keeps both alive for this closure.
                let view = unsafe { &*platform.inner().cast::<WKWebView>() };
                // SAFETY: same wry identity, null-check, and lifetime
                // guarantees as the WKWebView cast directly above;
                // ns_window() is exactly an NSWindow on macOS.
                let window = unsafe { &*platform.ns_window().cast::<NSWindow>() };
                // DOM resolution can be delayed by the page. Revalidate the
                // full host lease on the main thread immediately before AppKit
                // dispatch. Only pointer/key events refresh the takeover
                // provenance window; insertText emits beforeinput/input, which
                // the host takeover listener does not observe.
                if !callback_state.begin() {
                    return;
                }
                match authorize_agent_input_for_label(&label, &authorization, emits_takeover_signal)
                {
                    Ok(()) => operation(view, window),
                    Err(error) => Err(error),
                }
            };
            let _ = tx.send(result);
            callback_state.finish();
        })
        .map_err(|error| format!("browser/wkwebview-access-failed: {error}"))?;
    dispatch_state
        .wait(
            rx,
            EVALUATION_TIMEOUT,
            "browser/wkwebview-input-timeout",
            "browser/wkwebview-input-callback-closed",
            Some(ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION),
        )
        .await?;

    // WKWebView forwards AppKit responder calls to its WebContent process.
    // The responder method can return before that process has applied the
    // trusted event; without an IPC fence a following compound sub-action can
    // overtake it (for example insertText racing the preceding Backspace).
    // A page-world round trip is ordered after the native event while keeping
    // all routing inside this exact task-owned WKWebView.
    evaluate_json(
        webview,
        "return true;".to_string(),
        BrowserCoreEvaluationMode::ReadOnly,
        None,
    )
    .await
    .map(|_| ())
    .map_err(|error| format!("{ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION}: {error}"))
}

fn registered_control_for_authorization(
    label: &str,
    authorization: &NativeTabLease,
) -> Result<Arc<WorkspaceControl>, String> {
    let control = {
        let bindings = registered_webviews().lock();
        let binding = bindings
            .get(label)
            .ok_or_else(|| "browser/wkwebview-binding-stale".to_string())?;
        if binding.tab_token != authorization.tab_token {
            return Err("browser/wkwebview-tab-binding-mismatch".to_string());
        }
        Weak::upgrade(&binding.control)
    }
    .ok_or_else(|| "browser/wkwebview-binding-stale".to_string())?;
    Ok(control)
}

/// Keep the exact control lock held through WKWebView's asynchronous script
/// enqueue. A user takeover therefore either revokes this operation first or
/// is ordered after this native dispatch; there is no check/enqueue gap.
fn dispatch_script_mutation_if_authorized<T, F>(
    label: &str,
    authorization: &NativeTabLease,
    dispatch: F,
) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String>,
{
    registered_control_for_authorization(label, authorization)?
        .dispatch_if_agent_authorized(authorization, dispatch)?
        .ok_or_else(|| "browser/wkwebview-control-lease-lost".to_string())
}

fn authorize_agent_input_for_label(
    label: &str,
    authorization: &NativeTabLease,
    emits_takeover_signal: bool,
) -> Result<(), String> {
    let control = registered_control_for_authorization(label, authorization)?;
    let authorized = if emits_takeover_signal {
        control.refresh_agent_input_window(authorization)
    } else {
        control.authorize_agent_dispatch(authorization)
    };
    if !authorized {
        return Err("browser/wkwebview-control-lease-lost".to_string());
    }
    Ok(())
}

fn mark_action_partially_committed(context: &str, error: String) -> String {
    if error.starts_with(ACTION_COMMITTED_FOCUS_RESTORE_FAILED)
        || error.starts_with(ACTION_COMMIT_UNKNOWN_FOCUS_RESTORE_FAILED)
        || error.starts_with(ACTION_PARTIALLY_COMMITTED)
    {
        return error;
    }
    format!("{ACTION_PARTIALLY_COMMITTED}: {context}: {error}")
}

/// Keep the first sub-action's ordinary pre-dispatch failure retryable. If the
/// platform says that sub-action committed (or may have committed), the larger
/// compound operation is incomplete and retrying it from the start is unsafe.
fn mark_incomplete_after_possible_commit(context: &str, error: String) -> String {
    if error.starts_with(ACTION_PARTIALLY_COMMITTED) {
        return error;
    }
    if error.starts_with(ACTION_COMMITTED_FOCUS_RESTORE_FAILED)
        || error.starts_with(ACTION_COMMIT_UNKNOWN_FOCUS_RESTORE_FAILED)
    {
        return format!("{ACTION_PARTIALLY_COMMITTED}: {context}: {error}");
    }
    error
}

#[derive(Clone)]
struct KeyStroke {
    characters: String,
    characters_ignoring_modifiers: String,
    modifiers: NSEventModifierFlags,
    key_code: u16,
}

impl KeyStroke {
    fn named(name: &str) -> Result<Self, String> {
        let (characters, key_code, modifiers) =
            named_key(name).ok_or_else(|| format!("browser/unsupported-key: {name}"))?;
        Ok(Self {
            characters: characters.clone(),
            characters_ignoring_modifiers: characters,
            modifiers,
            key_code,
        })
    }

    fn parse(key: &str) -> Result<Self, String> {
        let parts = key.split('+').map(str::trim).collect::<Vec<_>>();
        let (name, modifiers) = parts
            .split_last()
            .ok_or_else(|| "browser/unsupported-key: empty".to_string())?;
        if name.is_empty() {
            return Err("browser/unsupported-key: empty".to_string());
        }
        let mut flags = NSEventModifierFlags::empty();
        for modifier in modifiers {
            flags |= match modifier.to_ascii_lowercase().as_str() {
                "shift" => NSEventModifierFlags::Shift,
                "control" | "ctrl" => NSEventModifierFlags::Control,
                "alt" | "option" => NSEventModifierFlags::Option,
                "meta" | "command" | "cmd" => NSEventModifierFlags::Command,
                _ => return Err(format!("browser/unsupported-key-modifier: {modifier}")),
            };
        }

        let (characters, key_code) =
            if let Some((characters, key_code, named_flags)) = named_key(name) {
                flags |= named_flags;
                (characters, key_code)
            } else if name.chars().count() == 1 {
                let lower = name.to_lowercase();
                (
                    if flags.contains(NSEventModifierFlags::Shift) {
                        name.to_uppercase()
                    } else {
                        lower.clone()
                    },
                    key_code_for_character(&lower).unwrap_or(0),
                )
            } else {
                return Err(format!("browser/unsupported-key: {name}"));
            };

        Ok(Self {
            characters: characters.clone(),
            characters_ignoring_modifiers: characters.to_lowercase(),
            modifiers: flags,
            key_code,
        })
    }
}

fn named_key(name: &str) -> Option<(String, u16, NSEventModifierFlags)> {
    let function_character = |value: u32| char::from_u32(value).map(|value| value.to_string());
    let plain = |characters: &str, key_code| {
        Some((
            characters.to_string(),
            key_code,
            NSEventModifierFlags::empty(),
        ))
    };
    let function = |value, key_code, numeric_pad| {
        function_character(value).map(|characters| {
            let mut modifiers = NSEventModifierFlags::Function;
            if numeric_pad {
                modifiers |= NSEventModifierFlags::NumericPad;
            }
            (characters, key_code, modifiers)
        })
    };
    match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => plain("\r", 36),
        "tab" => plain("\t", 48),
        "space" => plain(" ", 49),
        // macOS reports the physical backward-delete key as U+007F with
        // virtual key code 51. U+0008 would not follow Cocoa's standard text
        // editing path and can leave fill() unable to clear the selection.
        "backspace" => plain("\u{7f}", 51),
        "escape" | "esc" => plain("\u{1b}", 53),
        "delete" => function(NSDeleteFunctionKey, 117, false),
        "arrowleft" | "left" => function(NSLeftArrowFunctionKey, 123, true),
        "arrowright" | "right" => function(NSRightArrowFunctionKey, 124, true),
        "arrowdown" | "down" => function(NSDownArrowFunctionKey, 125, true),
        "arrowup" | "up" => function(NSUpArrowFunctionKey, 126, true),
        "home" => function(NSHomeFunctionKey, 115, false),
        "end" => function(NSEndFunctionKey, 119, false),
        "pageup" => function(NSPageUpFunctionKey, 116, false),
        "pagedown" => function(NSPageDownFunctionKey, 121, false),
        _ => None,
    }
}

fn key_code_for_character(character: &str) -> Option<u16> {
    Some(match character {
        "a" => 0,
        "s" => 1,
        "d" => 2,
        "f" => 3,
        "h" => 4,
        "g" => 5,
        "z" => 6,
        "x" => 7,
        "c" => 8,
        "v" => 9,
        "b" => 11,
        "q" => 12,
        "w" => 13,
        "e" => 14,
        "r" => 15,
        "y" => 16,
        "t" => 17,
        "1" => 18,
        "2" => 19,
        "3" => 20,
        "4" => 21,
        "6" => 22,
        "5" => 23,
        "=" => 24,
        "9" => 25,
        "7" => 26,
        "-" => 27,
        "8" => 28,
        "0" => 29,
        "]" => 30,
        "o" => 31,
        "u" => 32,
        "[" => 33,
        "i" => 34,
        "p" => 35,
        "l" => 37,
        "j" => 38,
        "'" => 39,
        "k" => 40,
        ";" => 41,
        "\\" => 42,
        "," => 43,
        "/" => 44,
        "n" => 45,
        "m" => 46,
        "." => 47,
        "`" => 50,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::browser::platform::state::NativeControlOwner;
    use serde_json::json;

    #[test]
    fn json_wrapper_awaits_and_serializes_the_result() {
        let wrapped = wrap_json_evaluation("return await core.snapshot();");
        assert!(wrapped.contains("await (async () =>"));
        assert!(wrapped.contains("JSON.stringify"));
        assert!(wrapped.contains("return await core.snapshot();"));
    }

    #[test]
    fn key_parser_accepts_browser_core_chords_and_rejects_unknown_modifiers() {
        let command = KeyStroke::parse("Meta+A").expect("command-a");
        assert!(command.modifiers.contains(NSEventModifierFlags::Command));
        assert_eq!(command.key_code, 0);
        let enter = KeyStroke::parse("Enter").expect("enter");
        assert_eq!(enter.characters, "\r");
        let backspace = KeyStroke::parse("Backspace").expect("backspace");
        assert_eq!(backspace.characters, "\u{7f}");
        assert_eq!(backspace.key_code, 51);
        assert!(backspace.modifiers.is_empty());
        let end = KeyStroke::parse("End").expect("end");
        assert!(end.modifiers.contains(NSEventModifierFlags::Function));
        let left = KeyStroke::parse("ArrowLeft").expect("left");
        assert!(left.modifiers.contains(NSEventModifierFlags::Function));
        assert!(left.modifiers.contains(NSEventModifierFlags::NumericPad));
        assert!(KeyStroke::parse("Hyper+A").is_err());
    }

    #[test]
    fn host_binding_registry_contains_no_remote_page_identity() {
        let label = "agent-browser-static-contract";
        let tab_token = "0123456789abcdef";
        let control = Arc::new(WorkspaceControl::new(1, NativeControlOwner::Agent));
        register_webview_binding(label, tab_token, &control).expect("register host label");
        let bindings = registered_webviews().lock();
        let binding = bindings.get(label).expect("registered binding");
        assert_eq!(binding.tab_token, tab_token);
        let registered = Weak::upgrade(&binding.control).expect("registered workspace control");
        assert!(Arc::ptr_eq(&registered, &control));
        drop(bindings);
        unregister_webview_binding(label);
        assert!(!registered_webviews().lock().contains_key(label));
    }

    #[test]
    fn stale_dispatch_cannot_borrow_a_new_active_operation() {
        let label = "agent-browser-stale-dispatch-contract";
        let tab_a = "0123456789abcdef";
        let tab_b = "fedcba9876543210";
        let control = Arc::new(WorkspaceControl::new(1, NativeControlOwner::Agent));
        let (snapshot, opaque_lease) = control.issue_agent_lease();
        let operation_a = NativeTabLease::from_assertion(
            "session-a",
            tab_a,
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .expect("operation a");
        register_webview_binding(label, tab_a, &control).expect("register tab a");
        assert!(control.begin_agent_operation(&operation_a, true));
        control.end_agent_operation(&operation_a);
        let (next_snapshot, next_opaque_lease) = control.issue_agent_lease();
        let operation_b = NativeTabLease::from_assertion(
            "session-a",
            tab_b,
            "target-b",
            next_snapshot.revision,
            next_opaque_lease,
        )
        .expect("operation b");
        assert!(control.begin_agent_operation(&operation_b, true));

        assert_eq!(
            authorize_agent_input_for_label(label, &operation_a, true),
            Err("browser/wkwebview-control-lease-lost".to_string())
        );
        assert_eq!(
            authorize_agent_input_for_label(label, &operation_b, true),
            Err("browser/wkwebview-tab-binding-mismatch".to_string())
        );
        unregister_webview_binding(label);
    }

    #[test]
    fn insert_text_authorization_does_not_open_the_takeover_suppression_window() {
        let label = "agent-browser-text-provenance-contract";
        let tab = "0123456789abcdef";
        let control = Arc::new(WorkspaceControl::new(1, NativeControlOwner::Agent));
        let (snapshot, opaque_lease) = control.issue_agent_lease();
        let operation = NativeTabLease::from_assertion(
            "session-a",
            tab,
            "target-a",
            snapshot.revision,
            opaque_lease,
        )
        .expect("operation");

        register_webview_binding(label, tab, &control).expect("register tab");
        assert!(control.begin_agent_operation(&operation, false));
        assert_eq!(
            authorize_agent_input_for_label(label, &operation, false),
            Ok(())
        );
        assert!(!control.agent_input_in_progress());
        assert_eq!(
            authorize_agent_input_for_label(label, &operation, true),
            Ok(())
        );
        assert!(control.agent_input_in_progress());
        control.end_agent_operation(&operation);
        unregister_webview_binding(label);
    }

    #[test]
    fn finite_coordinates_fail_closed() {
        assert_eq!(finite_coordinate(Some(&json!(12.5)), "x"), Ok(12.5));
        assert!(finite_coordinate(Some(&json!("12")), "x").is_err());
        assert!(finite_coordinate(Some(&json!(2_000_000)), "x").is_err());
    }

    #[test]
    fn responder_restore_failure_is_never_hidden_by_the_action_result() {
        assert_eq!(combine_action_and_restore(Ok(()), Ok(())), Ok(()));
        assert_eq!(
            combine_action_and_restore(Err("action".into()), Ok(())),
            Err("action".into())
        );
        assert_eq!(
            combine_action_and_restore(Ok(()), Err("restore".into())),
            Err("browser/action-committed-focus-restore-failed: restore".into())
        );
        assert_eq!(
            combine_action_and_restore(Err("action".into()), Err("restore".into())),
            Err(
                "browser/action-commit-unknown-focus-restore-failed: action=action; restore=restore"
                    .into()
            )
        );
    }

    #[test]
    fn later_compound_failure_is_marked_partial_without_hiding_committed_errors() {
        assert_eq!(
            mark_action_partially_committed("text inserted", "submit failed".into()),
            "browser/action-partially-committed: text inserted: submit failed"
        );
        let committed = "browser/action-committed-focus-restore-failed: restore".to_string();
        assert_eq!(
            mark_action_partially_committed("text inserted", committed.clone()),
            committed.clone()
        );
        assert_eq!(
            mark_incomplete_after_possible_commit("fill not started", committed.clone()),
            format!("browser/action-partially-committed: fill not started: {committed}")
        );
        assert_eq!(
            mark_incomplete_after_possible_commit("fill not started", "precommit".into()),
            "precommit"
        );
    }

    #[tokio::test]
    async fn pending_native_input_timeout_cancels_before_dispatch() {
        let (_tx, rx) = tokio::sync::oneshot::channel();
        let state = AsyncDispatchState::new();

        assert_eq!(
            state
                .wait::<()>(
                    rx,
                    Duration::ZERO,
                    "browser/wkwebview-input-timeout",
                    "browser/wkwebview-input-callback-closed",
                    Some(ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION),
                )
                .await,
            Err("browser/wkwebview-input-timeout".to_string())
        );
        assert!(!state.begin());
    }

    #[tokio::test]
    async fn running_native_input_timeout_is_non_retryable_commit_unknown() {
        let (_tx, rx) = tokio::sync::oneshot::channel();
        let state = AsyncDispatchState::new();
        assert!(state.begin());

        let error = state
            .wait::<()>(
                rx,
                Duration::ZERO,
                "browser/wkwebview-input-timeout",
                "browser/wkwebview-input-callback-closed",
                Some(ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION),
            )
            .await
            .expect_err("running dispatch must not become a retryable timeout");
        assert!(error.starts_with(ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION));
        assert!(error.ends_with("browser/wkwebview-input-timeout"));
    }

    #[tokio::test]
    async fn running_native_input_callback_close_is_non_retryable_commit_unknown() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let state = AsyncDispatchState::new();
        assert!(state.begin());
        drop(tx);

        let error = state
            .wait::<()>(
                rx,
                Duration::from_secs(1),
                "browser/wkwebview-input-timeout",
                "browser/wkwebview-input-callback-closed",
                Some(ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION),
            )
            .await
            .expect_err("a lost running callback has unknown commit state");
        assert!(error.starts_with(ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION));
        assert!(error.ends_with("browser/wkwebview-input-callback-closed"));
    }

    #[tokio::test]
    async fn completed_native_input_preserves_its_exact_result() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let state = AsyncDispatchState::new();
        assert!(state.begin());
        tx.send(Err("exact-platform-error".to_string())).unwrap();
        state.finish();

        assert_eq!(
            state
                .wait::<()>(
                    rx,
                    Duration::from_secs(1),
                    "browser/wkwebview-input-timeout",
                    "browser/wkwebview-input-callback-closed",
                    Some(ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION),
                )
                .await,
            Err("exact-platform-error".to_string())
        );
    }
}
