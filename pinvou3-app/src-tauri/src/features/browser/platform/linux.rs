//! Linux WebKitGTK BrowserCore driver.
//!
//! Page interaction stays inside the task-owned `WebView` and is driven
//! through WebKitGTK's loopback WebDriver endpoint; trusted element-scoped
//! input dispatch lives in `linux_automation`.

use serde_json::Value;
use std::sync::Arc;
use tauri::Webview;

use webkit2gtk::WebViewExt;

use super::state::NativeTabLease;
use super::{
    ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION, AsyncDispatchState, BrowserCoreEvaluationMode,
    linux_automation,
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
                            .map_err(|error| format!("browser/webkit-javascript-failed: {error}"))
                            .and_then(|value| {
                                use javascriptcore::ValueExt;
                                value
                                    .to_json(0)
                                    .map(|json| json.to_string())
                                    .ok_or_else(|| {
                                        "browser/webkit-javascript-result-not-json".to_string()
                                    })
                            })
                            .and_then(|json| {
                                serde_json::from_str(&json).map_err(|error| {
                                    format!("browser/webkit-javascript-json-invalid: {error}")
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
    // Machine-readable error codes, matching the macOS BrowserCore adapter
    // style; prose timeout text is not part of any contract.
    dispatch_state
        .wait(
            rx,
            EVALUATION_TIMEOUT,
            "browser/webkit-javascript-timeout",
            "browser/webkit-javascript-callback-closed",
            commit_unknown_prefix,
        )
        .await
}
