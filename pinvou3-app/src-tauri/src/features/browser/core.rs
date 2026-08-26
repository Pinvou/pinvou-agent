//! Platform-neutral BrowserCore page operations.
//!
//! The page runtime owns DOM discovery and hit testing. Platform drivers own
//! trusted input. This module composes both without exposing a host bridge to
//! remote pages. The first production adapter is Linux/WebKitGTK; Windows can
//! continue using the same MCP contract while its advanced diagnostics stay
//! behind the private CDP adapter. macOS uses the same BrowserCore contract
//! with a WKWebView/AppKit platform driver.

use serde_json::{json, Value};
use tauri::Webview;

use super::platform::{self, state::NativeTabLease};

const CORE_GLOBAL: &str = "__PINVOU_BROWSER_CORE_V1__";
const ACTION_COMMITTED_FOCUS_RESTORE_FAILED: &str = "browser/action-committed-focus-restore-failed";
const ACTION_COMMIT_UNKNOWN_FOCUS_RESTORE_FAILED: &str =
    "browser/action-commit-unknown-focus-restore-failed";
const ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION: &str =
    "browser/action-commit-unknown-after-input-interruption";
const ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION: &str =
    platform::ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION;
const ACTION_COMMIT_UNKNOWN_WEBDRIVER: &str = "browser/action-commit-unknown-webdriver";
const ACTION_COMMIT_UNKNOWN_NAVIGATION_DISPATCH: &str =
    "browser/action-commit-unknown-after-navigation-dispatch";
const ACTION_COMMIT_UNKNOWN_TAB_CLOSE: &str = "browser/action-commit-unknown-after-tab-close";
const ACTION_PARTIALLY_COMMITTED: &str = "browser/action-partially-committed";

fn core_call(method: &str, arguments: Value) -> Result<String, String> {
    let encoded = serde_json::to_string(&arguments)
        .map_err(|error| format!("browser/invalid-arguments: {error}"))?;
    Ok(format!(
        "const core = globalThis.{CORE_GLOBAL};\n\
         if (!core || core.version !== 1) throw new Error('browser/core-runtime-unavailable');\n\
         return await core.{method}(...{encoded});"
    ))
}

async fn call(webview: &Webview, method: &str, arguments: Value) -> Result<Value, String> {
    platform::evaluate_browser_core_json(
        webview,
        core_call(method, arguments)?,
        platform::BrowserCoreEvaluationMode::ReadOnly,
        None,
    )
    .await
}

async fn call_mutating(
    webview: &Webview,
    authorization: &NativeTabLease,
    method: &str,
    arguments: Value,
) -> Result<Value, String> {
    platform::evaluate_browser_core_json(
        webview,
        core_call(method, arguments)?,
        platform::BrowserCoreEvaluationMode::MayMutate,
        Some(authorization),
    )
    .await
}

fn required_string<'a>(arguments: &'a Value, field: &str) -> Result<&'a str, String> {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("browser/missing-argument: {field}"))
}

fn tool_text(text: String, structured: Option<Value>) -> Value {
    let mut result = json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false,
    });
    if let Some(structured) = structured {
        result["structuredContent"] = structured;
    }
    result
}

fn tool_error_text(text: String, structured: Value) -> Value {
    let mut result = tool_text(text, Some(structured));
    result["isError"] = json!(true);
    result
}

async fn snapshot_value(webview: &Webview, verbose: bool) -> Result<Value, String> {
    call(webview, "snapshot", json!([{ "verbose": verbose }])).await
}

async fn maybe_snapshot(webview: &Webview, arguments: &Value) -> Result<Option<Value>, String> {
    if arguments
        .get("includeSnapshot")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        Ok(Some(snapshot_value(webview, false).await?))
    } else {
        Ok(None)
    }
}

async fn input_result(
    webview: &Webview,
    arguments: &Value,
    message: &str,
) -> Result<Value, String> {
    match maybe_snapshot(webview, arguments).await {
        Ok(Some(snapshot)) => {
            let text = snapshot
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Ok(tool_text(format!("{message}\n{text}"), Some(snapshot)))
        }
        Ok(None) => Ok(tool_text(message.to_string(), None)),
        Err(error) => Ok(committed_input_observation_warning(message, &error)),
    }
}

/// A native mutation has already crossed the platform commit boundary by the
/// time its optional snapshot runs. A failed observation must therefore not
/// turn the committed action into a retryable tool/transport error.
fn committed_input_observation_warning(message: &str, error: &str) -> Value {
    let warning = format!("browser/post-action-observation-failed: {error}");
    tool_text(
        format!(
            "{message}. The action was committed, but the requested post-action snapshot failed. \
             Do not retry the action; call take_snapshot separately. Observation warning: {warning}"
        ),
        Some(json!({
            "actionCommitted": true,
            "retryable": false,
            "observationWarning": warning,
        })),
    )
}

/// Native/script dispatch and focus restoration are separate commit
/// boundaries. A platform error carrying one of the prefixes below must stay a
/// tool-level outcome:
/// returning it as `Err` would let the host or Agent retry an input that has
/// already happened (or whose commit state cannot be proven false).
pub(super) fn committed_platform_outcome(action: &str, error: &str) -> Option<Value> {
    let (
        error_code,
        outcome,
        commit_state,
        action_may_have_committed,
        focus_restore_failed,
        sub_action_commit_state,
        explanation,
    ) = if error.starts_with(ACTION_COMMITTED_FOCUS_RESTORE_FAILED) {
        (
            ACTION_COMMITTED_FOCUS_RESTORE_FAILED,
            "committed",
            "committed",
            false,
            true,
            None,
            format!(
                "{action} was committed, but Pinvou could not restore the previous application focus"
            ),
        )
    } else if error.starts_with(ACTION_COMMIT_UNKNOWN_FOCUS_RESTORE_FAILED) {
        (
            ACTION_COMMIT_UNKNOWN_FOCUS_RESTORE_FAILED,
            "committed",
            "unknown",
            true,
            true,
            None,
            format!(
                "{action} may already have been committed, and Pinvou could not restore the previous application focus"
            ),
        )
    } else if error.starts_with(ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION) {
        (
            ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION,
            "committed",
            "unknown",
            true,
            false,
            None,
            format!(
                "{action} may already have been committed because the native input callback was interrupted"
            ),
        )
    } else if error.starts_with(ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION) {
        (
            ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION,
            "committed",
            "unknown",
            true,
            false,
            None,
            format!(
                "{action} may already have mutated the page because the system-WebView script callback was interrupted"
            ),
        )
    } else if error.starts_with(ACTION_COMMIT_UNKNOWN_WEBDRIVER) {
        (
            ACTION_COMMIT_UNKNOWN_WEBDRIVER,
            "committed",
            "unknown",
            true,
            false,
            None,
            format!(
                "{action} may already have been committed because WebDriver did not return a conclusive acknowledgement"
            ),
        )
    } else if error.starts_with(ACTION_COMMIT_UNKNOWN_NAVIGATION_DISPATCH) {
        (
            ACTION_COMMIT_UNKNOWN_NAVIGATION_DISPATCH,
            "committed",
            "unknown",
            true,
            false,
            None,
            format!(
                "{action} may already have been committed because the native WebView did not return a conclusive navigation acknowledgement"
            ),
        )
    } else if error.starts_with(ACTION_COMMIT_UNKNOWN_TAB_CLOSE) {
        (
            ACTION_COMMIT_UNKNOWN_TAB_CLOSE,
            "committed",
            "unknown",
            true,
            false,
            None,
            format!(
                "{action} may already have been committed because the native WebView did not return a conclusive close acknowledgement"
            ),
        )
    } else if error.starts_with(ACTION_PARTIALLY_COMMITTED) {
        let nested_unknown = error.contains(ACTION_COMMIT_UNKNOWN_FOCUS_RESTORE_FAILED)
            || error.contains(ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION)
            || error.contains(ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION)
            || error.contains(ACTION_COMMIT_UNKNOWN_WEBDRIVER)
            || error.contains(ACTION_COMMIT_UNKNOWN_NAVIGATION_DISPATCH)
            || error.contains(ACTION_COMMIT_UNKNOWN_TAB_CLOSE);
        let nested_committed = error.contains(ACTION_COMMITTED_FOCUS_RESTORE_FAILED);
        let nested_focus_restore_failed =
            error.contains(ACTION_COMMIT_UNKNOWN_FOCUS_RESTORE_FAILED) || nested_committed;
        (
            ACTION_PARTIALLY_COMMITTED,
            "partial",
            "partial",
            nested_unknown,
            nested_focus_restore_failed,
            if nested_unknown {
                Some("unknown")
            } else if nested_committed {
                Some("committed")
            } else {
                None
            },
            format!("{action} stopped after at least one native sub-action was committed"),
        )
    } else {
        return None;
    };

    Some(tool_error_text(
        format!(
            "{explanation}. This is a non-retryable {outcome} outcome. Do not repeat the whole \
             action; inspect the page state before continuing. Platform error: {error}"
        ),
        json!({
            "errorCode": error_code,
            "outcome": outcome,
            // `unknown` is deliberately treated as committed for retry policy:
            // repeating a possibly-delivered click or keystroke is less safe.
            "actionCommitted": true,
            "actionCommitState": commit_state,
            "actionMayHaveCommitted": action_may_have_committed,
            "subActionCommitState": sub_action_commit_state,
            "retryable": false,
            "focusRestoreFailed": focus_restore_failed,
            "error": error,
        }),
    ))
}

fn native_input_dispatch_result(
    action: &str,
    result: Result<(), String>,
) -> Result<Option<Value>, String> {
    match result {
        Ok(()) => Ok(None),
        Err(error) => match committed_platform_outcome(action, &error) {
            Some(outcome) => Ok(Some(outcome)),
            None => Err(error),
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ValidatedFormField<'a> {
    uid: &'a str,
    value: &'a str,
}

/// Validate the complete batch before the first native write. This prevents a
/// malformed later entry from being discovered only after earlier fields have
/// already been committed.
fn validated_form_fields(arguments: &Value) -> Result<Vec<ValidatedFormField<'_>>, String> {
    let elements = arguments
        .get("elements")
        .and_then(Value::as_array)
        .ok_or_else(|| "browser/missing-argument: elements".to_string())?;
    elements
        .iter()
        .map(|element| {
            let uid = required_string(element, "uid")?;
            let value = element
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| "browser/missing-argument: value".to_string())?;
            Ok(ValidatedFormField { uid, value })
        })
        .collect()
}

fn partial_fill_form_result(
    completed_count: usize,
    failed_index: usize,
    total_count: usize,
    error: &str,
) -> Value {
    tool_error_text(
        format!(
            "Form fill stopped after {completed_count} of {total_count} fields; zero-based field \
             index {failed_index} failed: {error}. This is a non-retryable partial outcome. Do \
             not retry the whole form; inspect the page and continue only with unfinished fields."
        ),
        json!({
            "errorCode": "browser/partial-form-fill",
            "outcome": "partial",
            "retryable": false,
            "completedCount": completed_count,
            "failedIndex": failed_index,
            "totalCount": total_count,
            "error": error,
        }),
    )
}

async fn fill_one(
    webview: &Webview,
    authorization: &NativeTabLease,
    uid: &str,
    value: &str,
) -> Result<(), String> {
    platform::fill_browser_core_element(webview, authorization, uid, value).await
}

fn unavailable_embedded_tool(name: &str) -> String {
    let backend = match platform::browser_core_backend_name() {
        "browser-core-wkwebview" => "wkwebview",
        // Preserve the established Linux error contract. This branch is also
        // the conservative fallback on platforms that cannot run BrowserCore.
        _ => "webkitgtk",
    };
    format!("browser/core-tool-unavailable-on-{backend}: {name}")
}

/// Execute a page-scoped BrowserCore tool after BrowserManager has atomically
/// validated and begun the task lease.
pub(crate) async fn execute_page_tool(
    webview: &Webview,
    authorization: &NativeTabLease,
    name: &str,
    arguments: &Value,
) -> Result<Value, String> {
    match name {
        "take_snapshot" => {
            let snapshot = snapshot_value(
                webview,
                arguments
                    .get("verbose")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )
            .await?;
            let text = snapshot
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Ok(tool_text(text, Some(snapshot)))
        }
        "click" => {
            let uid = required_string(arguments, "uid")?;
            let click_count = if arguments
                .get("dblClick")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                2
            } else {
                1
            };
            if let Some(outcome) = native_input_dispatch_result(
                "Click",
                platform::click_browser_core_element(webview, authorization, uid, click_count)
                    .await,
            )? {
                return Ok(outcome);
            }
            input_result(webview, arguments, "Clicked element").await
        }
        "hover" | "drag" => Err(unavailable_embedded_tool(name)),
        "fill" => {
            let uid = required_string(arguments, "uid")?;
            let value = arguments
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| "browser/missing-argument: value".to_string())?;
            if let Some(outcome) = native_input_dispatch_result(
                "Fill",
                fill_one(webview, authorization, uid, value).await,
            )? {
                return Ok(outcome);
            }
            input_result(webview, arguments, "Filled element").await
        }
        "fill_form" => {
            let fields = validated_form_fields(arguments)?;
            let total_count = fields.len();
            for (failed_index, field) in fields.into_iter().enumerate() {
                if let Err(error) = fill_one(webview, authorization, field.uid, field.value).await {
                    if let Some(mut outcome) = committed_platform_outcome("Form field fill", &error)
                    {
                        outcome["structuredContent"]["completedBeforeCurrent"] =
                            json!(failed_index);
                        outcome["structuredContent"]["currentFieldIndex"] = json!(failed_index);
                        outcome["structuredContent"]["totalCount"] = json!(total_count);
                        return Ok(outcome);
                    }
                    return Ok(partial_fill_form_result(
                        failed_index,
                        failed_index,
                        total_count,
                        &error,
                    ));
                }
            }
            input_result(webview, arguments, "Filled form").await
        }
        "type_text" => {
            let text = arguments
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| "browser/missing-argument: text".to_string())?;
            if let Some(outcome) = native_input_dispatch_result(
                "Text input",
                platform::type_browser_core_text(
                    webview,
                    authorization,
                    text,
                    arguments.get("submitKey").and_then(Value::as_str),
                )
                .await,
            )? {
                return Ok(outcome);
            }
            Ok(tool_text("Typed text".to_string(), None))
        }
        "press_key" => {
            let key = required_string(arguments, "key")?;
            if let Some(outcome) = native_input_dispatch_result(
                "Key press",
                platform::press_browser_core_key(webview, authorization, key).await,
            )? {
                return Ok(outcome);
            }
            input_result(webview, arguments, "Pressed key").await
        }
        "wait_for" => {
            let texts = arguments
                .get("text")
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| "browser/missing-argument: text".to_string())?;
            let timeout = arguments
                .get("timeout")
                .and_then(Value::as_u64)
                .unwrap_or(10_000)
                .min(12_000);
            let matched = call(webview, "waitFor", json!([texts, timeout])).await?;
            Ok(tool_text(
                format!(
                    "Found text: {}",
                    matched
                        .get("match")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                ),
                Some(matched),
            ))
        }
        "evaluate_script" => {
            let function = required_string(arguments, "function")?;
            let args = arguments
                .get("args")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let output =
                match call_mutating(webview, authorization, "evaluate", json!([function, args]))
                    .await
                {
                    Ok(output) => output,
                    Err(error) => {
                        if let Some(outcome) =
                            committed_platform_outcome("Script evaluation", &error)
                        {
                            return Ok(outcome);
                        }
                        return Err(error);
                    }
                };
            Ok(tool_text(
                serde_json::to_string_pretty(&output).unwrap_or_else(|_| "null".to_string()),
                Some(json!({ "result": output })),
            ))
        }
        "handle_dialog" => {
            let action = required_string(arguments, "action")?;
            if !matches!(action, "accept" | "dismiss") {
                return Err("browser/invalid-argument: action".to_string());
            }
            let prompt_text = match arguments.get("promptText") {
                Some(Value::String(value)) => Some(value.as_str()),
                Some(_) => return Err("browser/invalid-argument: promptText".to_string()),
                None => None,
            };
            let dialog_text = match platform::handle_browser_core_dialog(
                webview,
                authorization,
                action,
                prompt_text,
            )
            .await
            {
                Ok(dialog_text) => dialog_text,
                Err(error) => {
                    if let Some(outcome) = committed_platform_outcome("Dialog action", &error) {
                        return Ok(outcome);
                    }
                    return Err(error);
                }
            };
            Ok(tool_text(
                format!(
                    "Successfully {} the dialog: {}",
                    if action == "accept" {
                        "accepted"
                    } else {
                        "dismissed"
                    },
                    dialog_text
                ),
                Some(json!({
                    "action": action,
                    "dialogText": dialog_text,
                })),
            ))
        }
        // WebDriver's Set Window Rect targets a top-level native window. Pinvou's Linux
        // browser is an embedded child surface whose bounds belong to the right Dock, so
        // emulating this tool would either resize the app or be immediately overwritten.
        "resize_page" => Err(unavailable_embedded_tool("resize_page")),
        _ => Err(format!("browser/core-tool-unsupported: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_mutation_keeps_success_when_post_observation_fails() {
        let result = committed_input_observation_warning("Clicked element", "snapshot unavailable");

        assert_eq!(result["isError"], json!(false));
        assert_eq!(result["structuredContent"]["actionCommitted"], json!(true));
        assert_eq!(result["structuredContent"]["retryable"], json!(false));
        assert_eq!(
            result["structuredContent"]["observationWarning"],
            json!("browser/post-action-observation-failed: snapshot unavailable")
        );
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("Do not retry the action"));
    }

    #[test]
    fn committed_focus_restore_failure_stays_a_non_retryable_tool_outcome() {
        let error =
            "browser/action-committed-focus-restore-failed: browser/wkwebview-focus-restore-failed";
        let result = committed_platform_outcome("Click", error).expect("committed outcome");

        assert_eq!(result["isError"], json!(true));
        assert_eq!(
            result["structuredContent"]["errorCode"],
            json!(ACTION_COMMITTED_FOCUS_RESTORE_FAILED)
        );
        assert_eq!(result["structuredContent"]["actionCommitted"], json!(true));
        assert_eq!(
            result["structuredContent"]["actionCommitState"],
            json!("committed")
        );
        assert_eq!(result["structuredContent"]["retryable"], json!(false));
        assert_eq!(
            result["structuredContent"]["focusRestoreFailed"],
            json!(true)
        );
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("Do not repeat the whole action"));
    }

    #[test]
    fn unknown_commit_is_conservatively_non_retryable_but_normal_errors_propagate() {
        let unknown = native_input_dispatch_result(
            "Key press",
            Err(
                "browser/action-commit-unknown-focus-restore-failed: action=event; restore=focus"
                    .into(),
            ),
        )
        .expect("tool-level outcome")
        .expect("structured outcome");
        assert_eq!(
            unknown["structuredContent"]["actionCommitState"],
            json!("unknown")
        );
        assert_eq!(unknown["structuredContent"]["actionCommitted"], json!(true));
        assert_eq!(
            unknown["structuredContent"]["actionMayHaveCommitted"],
            json!(true)
        );
        assert_eq!(unknown["structuredContent"]["retryable"], json!(false));

        let webdriver_unknown = native_input_dispatch_result(
            "Click",
            Err(
                "browser/action-commit-unknown-webdriver: connection closed before response".into(),
            ),
        )
        .expect("tool-level WebDriver outcome")
        .expect("structured WebDriver outcome");
        assert_eq!(
            webdriver_unknown["structuredContent"]["errorCode"],
            json!(ACTION_COMMIT_UNKNOWN_WEBDRIVER)
        );
        assert_eq!(
            webdriver_unknown["structuredContent"]["actionCommitState"],
            json!("unknown")
        );
        assert_eq!(
            webdriver_unknown["structuredContent"]["actionMayHaveCommitted"],
            json!(true)
        );
        assert_eq!(
            webdriver_unknown["structuredContent"]["focusRestoreFailed"],
            json!(false)
        );
        assert_eq!(
            webdriver_unknown["structuredContent"]["retryable"],
            json!(false)
        );

        let input_interrupted = native_input_dispatch_result(
            "Key press",
            Err("browser/action-commit-unknown-after-input-interruption: callback closed".into()),
        )
        .expect("tool-level interrupted input outcome")
        .expect("structured interrupted input outcome");
        assert_eq!(
            input_interrupted["structuredContent"]["errorCode"],
            json!(ACTION_COMMIT_UNKNOWN_INPUT_INTERRUPTION)
        );
        assert_eq!(
            input_interrupted["structuredContent"]["actionCommitState"],
            json!("unknown")
        );
        assert_eq!(
            input_interrupted["structuredContent"]["retryable"],
            json!(false)
        );

        let script_interrupted = committed_platform_outcome(
            "Script evaluation",
            "browser/action-commit-unknown-after-script-interruption: callback closed",
        )
        .expect("tool-level interrupted script outcome");
        assert_eq!(
            script_interrupted["structuredContent"]["errorCode"],
            json!(ACTION_COMMIT_UNKNOWN_SCRIPT_INTERRUPTION)
        );
        assert_eq!(
            script_interrupted["structuredContent"]["actionCommitState"],
            json!("unknown")
        );
        assert_eq!(
            script_interrupted["structuredContent"]["actionMayHaveCommitted"],
            json!(true)
        );
        assert_eq!(
            script_interrupted["structuredContent"]["retryable"],
            json!(false)
        );

        assert_eq!(
            native_input_dispatch_result(
                "Key press",
                Err("browser/wkwebview-focus-rejected".into())
            ),
            Err("browser/wkwebview-focus-rejected".into())
        );
        assert_eq!(
            native_input_dispatch_result("Key press", Ok(())).expect("success"),
            None
        );
    }

    #[test]
    fn partial_platform_commit_uses_the_same_non_retryable_boundary() {
        let result = committed_platform_outcome(
            "Text input",
            "browser/action-partially-committed: text inserted; submit key failed",
        )
        .expect("partial outcome");

        assert_eq!(result["isError"], json!(true));
        assert_eq!(
            result["structuredContent"]["errorCode"],
            json!(ACTION_PARTIALLY_COMMITTED)
        );
        assert_eq!(result["structuredContent"]["outcome"], json!("partial"));
        assert_eq!(
            result["structuredContent"]["actionCommitState"],
            json!("partial")
        );
        assert_eq!(result["structuredContent"]["actionCommitted"], json!(true));
        assert_eq!(result["structuredContent"]["retryable"], json!(false));
        assert_eq!(
            result["structuredContent"]["focusRestoreFailed"],
            json!(false)
        );
    }

    #[test]
    fn partial_compound_outcome_preserves_nested_focus_restore_evidence() {
        let result = committed_platform_outcome(
            "Fill",
            "browser/action-partially-committed: clicked: browser/action-commit-unknown-focus-restore-failed: restore",
        )
        .expect("partial outcome");

        assert_eq!(result["structuredContent"]["outcome"], json!("partial"));
        assert_eq!(
            result["structuredContent"]["actionCommitState"],
            json!("partial")
        );
        assert_eq!(
            result["structuredContent"]["subActionCommitState"],
            json!("unknown")
        );
        assert_eq!(
            result["structuredContent"]["actionMayHaveCommitted"],
            json!(true)
        );
        assert_eq!(
            result["structuredContent"]["focusRestoreFailed"],
            json!(true)
        );

        let webdriver_result = committed_platform_outcome(
            "Fill",
            "browser/action-partially-committed: cleared: browser/action-commit-unknown-webdriver: connection reset",
        )
        .expect("partial WebDriver outcome");
        assert_eq!(
            webdriver_result["structuredContent"]["subActionCommitState"],
            json!("unknown")
        );
        assert_eq!(
            webdriver_result["structuredContent"]["actionMayHaveCommitted"],
            json!(true)
        );
        assert_eq!(
            webdriver_result["structuredContent"]["focusRestoreFailed"],
            json!(false)
        );
    }

    #[test]
    fn form_validation_rejects_a_late_invalid_field_before_dispatch() {
        let arguments = json!({
            "elements": [
                { "uid": "p1", "value": "first" },
                { "uid": "p2" },
            ]
        });

        assert_eq!(
            validated_form_fields(&arguments).unwrap_err(),
            "browser/missing-argument: value"
        );
    }

    #[test]
    fn partial_form_failure_is_a_non_retryable_structured_tool_result() {
        let result = partial_fill_form_result(2, 2, 4, "browser/stale-ref: p3");

        assert_eq!(result["isError"], json!(true));
        assert_eq!(
            result["structuredContent"]["errorCode"],
            json!("browser/partial-form-fill")
        );
        assert_eq!(result["structuredContent"]["outcome"], json!("partial"));
        assert_eq!(result["structuredContent"]["retryable"], json!(false));
        assert_eq!(result["structuredContent"]["completedCount"], json!(2));
        assert_eq!(result["structuredContent"]["failedIndex"], json!(2));
        assert_eq!(result["structuredContent"]["totalCount"], json!(4));
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("Do not retry the whole form"));
    }
}
