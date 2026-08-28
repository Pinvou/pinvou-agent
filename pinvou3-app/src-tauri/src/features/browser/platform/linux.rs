//! Linux WebKitGTK BrowserCore driver.
//!
//! DOM discovery stays inside the task-owned `WebView`. Trusted pointer and
//! keyboard input is submitted through WebKitGTK's standards-based WebDriver
//! endpoint, so events are scoped to that page and do not take over the
//! desktop-wide mouse or keyboard.

use serde_json::Value;
use std::sync::Arc;
use tauri::Webview;

use webkit2gtk::WebViewExt;

use super::state::NativeTabLease;
use super::{
    linux_automation, AsyncDispatchState, BrowserCoreEvaluationMode, NativeInput,
    ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION,
};

const EVALUATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Call an async BrowserCore function body in the task-owned WebKitGTK page
/// and return a JSON value. The callback originates on the GTK main context
/// and only sends an owned JSON string across the async boundary.
pub(super) async fn evaluate_json(
    webview: &Webview,
    script: String,
    mode: BrowserCoreEvaluationMode,
    authorization: Option<&NativeTabLease>,
) -> Result<Value, String> {
    let authorization = super::evaluation_authorization(mode, authorization)?.cloned();
    let label = webview.label().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let sender = Arc::new(parking_lot::Mutex::new(Some(tx)));
    let dispatch_state = AsyncDispatchState::new();
    let callback_state = dispatch_state.clone();
    webview
        .with_webview(move |platform| {
            let webview = platform.inner();
            let completion_state = callback_state.clone();
            let callback_sender = Arc::clone(&sender);
            let dispatch = || {
                if !callback_state.begin() {
                    return Ok(());
                }
                webview.call_async_javascript_function(
                    &script,
                    None,
                    None,
                    Some("pinvou://browser-core"),
                    None::<&webkit2gtk::gio::Cancellable>,
                    move |result| {
                        let result = result
                            .map_err(|error| {
                                format!("WebKitGTK JavaScript evaluation failed: {error}")
                            })
                            .and_then(|value| {
                                use javascriptcore::ValueExt;
                                value
                                    .to_json(0)
                                    .map(|json| json.to_string())
                                    .ok_or_else(|| {
                                        "WebKitGTK JavaScript result cannot be serialized as JSON"
                                            .to_string()
                                    })
                            })
                            .and_then(|json| {
                                serde_json::from_str(&json).map_err(|error| {
                                    format!("Failed to parse WebKitGTK JavaScript result: {error}")
                                })
                            });
                        if let Some(sender) = callback_sender.lock().take() {
                            let _ = sender.send(result);
                        }
                        completion_state.finish();
                    },
                );
                Ok(())
            };
            let result = if let Some(authorization) = authorization.as_ref() {
                linux_automation::dispatch_script_mutation_if_authorized(
                    &label,
                    authorization,
                    dispatch,
                )
            } else {
                dispatch()
            };
            if let Err(error) = result {
                if let Some(sender) = sender.lock().take() {
                    let _ = sender.send(Err(error));
                }
                callback_state.cancel_pending();
            }
        })
        .map_err(|error| format!("Failed to access WebKitGTK page: {error}"))?;

    let commit_unknown_prefix = matches!(mode, BrowserCoreEvaluationMode::MayMutate)
        .then_some(ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION);
    dispatch_state
        .wait(
            rx,
            EVALUATION_TIMEOUT,
            "WebKitGTK JavaScript evaluation timed out",
            "WebKitGTK JavaScript callback closed",
            commit_unknown_prefix,
        )
        .await
}

pub(super) async fn bind_webview(webview: &Webview) -> Result<(), String> {
    linux_automation::bind_webview(webview).await
}

pub(super) async fn wait_until_ready() -> Result<(), String> {
    linux_automation::wait_until_ready().await
}

pub(super) async fn dispatch_input(
    webview: &Webview,
    authorization: &NativeTabLease,
    input: NativeInput,
) -> Result<(), String> {
    linux_automation::dispatch_input(webview, authorization, input).await
}

pub(super) async fn click_element(
    webview: &Webview,
    authorization: &NativeTabLease,
    uid: &str,
    click_count: u8,
) -> Result<(), String> {
    linux_automation::click_element(webview, authorization, uid, click_count).await
}

pub(super) async fn fill_element(
    webview: &Webview,
    authorization: &NativeTabLease,
    uid: &str,
    value: &str,
) -> Result<(), String> {
    linux_automation::fill_element(webview, authorization, uid, value).await
}

pub(super) async fn type_text(
    webview: &Webview,
    authorization: &NativeTabLease,
    text: &str,
    submit_key: Option<&str>,
) -> Result<(), String> {
    linux_automation::type_text(webview, authorization, text, submit_key).await
}

pub(super) async fn press_key(
    webview: &Webview,
    authorization: &NativeTabLease,
    key: &str,
) -> Result<(), String> {
    linux_automation::press_key(webview, authorization, key).await
}

pub(super) async fn handle_dialog(
    webview: &Webview,
    authorization: &NativeTabLease,
    action: &str,
    prompt_text: Option<&str>,
) -> Result<String, String> {
    linux_automation::handle_dialog(webview, authorization, action, prompt_text).await
}
