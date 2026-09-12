//! `feedback` family: user feedback submission, mirroring
//! `pinvou3-app/src-tauri/src/app/commands/settings.rs::submit_feedback` →
//! `features::feedback::submit_feedback`.
//!
//! The community feature function never uploads anything: it validates the
//! request and returns a receipt that points at the public GitHub issue
//! tracker. The CLI calls exactly that function and discloses two surface
//! deviations:
//! - the GUI opens the issues URL in a browser; the CLI prints it;
//! - the GUI keeps the receipt in-app; the CLI additionally persists the
//!   request bundle under `$PINVOU3_HOME/feedback/pending/` and the returned
//!   receipt under `$PINVOU3_HOME/feedback/receipts/` — the same directory
//!   contract `platform::paths` defines for feedback artifacts. Nothing
//!   leaves the machine.
//!
//! The CLI fixes the request `entry_point` to `"settings"` (the only two
//! values the feature accepts are the GUI's settings page and error banner)
//! because a terminal invocation has no banner context.

use std::path::{Path, PathBuf};

use pinvou3_lib::features::feedback::{
    FeedbackAttachmentRequest, FeedbackStatus, FeedbackSubmitRequest, FeedbackType,
};

use crate::support::{render, sandbox_home, success};
use crate::{CliError, CliOutcome, OutputMode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedbackCommand {
    submit: SubmitArgs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SubmitArgs {
    feedback_type: FeedbackType,
    title: String,
    body_file: PathBuf,
    attachments: Vec<PathBuf>,
}

const USAGE: &str = "usage: pinvou feedback submit --type issue|suggestion --title T --body-file F [--attach PATH...]";

/// The community feature function's message points here; the const is
/// crate-private upstream, so the URL is mirrored verbatim.
const COMMUNITY_ISSUES_URL: &str = "https://github.com/Pinvou/pinvou-agent/issues";

pub fn parse(values: &[String]) -> Result<FeedbackCommand, CliError> {
    let subcommand = values.get(1).ok_or_else(|| CliError::usage(USAGE))?;
    if subcommand != "submit" {
        return Err(CliError::usage(USAGE));
    }
    let mut feedback_type: Option<FeedbackType> = None;
    let mut title: Option<String> = None;
    let mut body_file: Option<PathBuf> = None;
    let mut attachments = Vec::new();
    let index = &values[2..];
    let mut position = 0;
    while position < index.len() {
        let token = index[position].as_str();
        let value = |position: usize, name: &str| -> Result<&str, CliError> {
            index
                .get(position + 1)
                .map(String::as_str)
                .filter(|value| !value.is_empty() && !value.starts_with("--"))
                .ok_or_else(|| CliError::usage(format!("feedback option {name} requires a value")))
        };
        match token {
            "--type" => {
                if feedback_type.is_some() {
                    return Err(CliError::usage("duplicate feedback option --type"));
                }
                feedback_type = Some(match value(position, "--type")? {
                    "issue" => FeedbackType::Issue,
                    "suggestion" => FeedbackType::Suggestion,
                    other => {
                        return Err(CliError::usage(format!(
                            "feedback submit --type must be issue or suggestion (got {other})"
                        )));
                    }
                });
                position += 2;
            }
            "--title" => {
                if title.is_some() {
                    return Err(CliError::usage("duplicate feedback option --title"));
                }
                title = Some(value(position, "--title")?.to_owned());
                position += 2;
            }
            "--body-file" => {
                if body_file.is_some() {
                    return Err(CliError::usage("duplicate feedback option --body-file"));
                }
                body_file = Some(PathBuf::from(value(position, "--body-file")?));
                position += 2;
            }
            "--attach" => {
                attachments.push(PathBuf::from(value(position, "--attach")?));
                position += 2;
            }
            other => {
                return Err(CliError::usage(format!(
                    "unsupported feedback option: {other}"
                )));
            }
        }
    }
    let submit = SubmitArgs {
        feedback_type: feedback_type
            .ok_or_else(|| CliError::usage("feedback submit requires --type issue|suggestion"))?,
        title: title.ok_or_else(|| CliError::usage("feedback submit requires --title T"))?,
        body_file: body_file
            .ok_or_else(|| CliError::usage("feedback submit requires --body-file F"))?,
        attachments,
    };
    Ok(FeedbackCommand { submit })
}

pub fn execute(command: FeedbackCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    // Validates the sandbox-home contract (absolute PINVOU3_HOME) before any
    // feedback paths are resolved.
    sandbox_home()?;
    let submit = command.submit;
    let description = std::fs::read_to_string(&submit.body_file).map_err(|error| {
        CliError::failed(format!(
            "feedback submit: cannot read body file {}: {error}",
            submit.body_file.display()
        ))
    })?;
    let mut attachments = Vec::new();
    for path in &submit.attachments {
        let size = std::fs::metadata(path)
            .map_err(|error| {
                CliError::failed(format!(
                    "feedback submit: cannot read attachment {}: {error}",
                    path.display()
                ))
            })?
            .len();
        attachments.push(FeedbackAttachmentRequest {
            path: path.display().to_string(),
            name: path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("attachment")
                .to_owned(),
            media_type: media_type(path).to_owned(),
            mime: None,
            size_bytes: Some(size),
        });
    }
    let request = FeedbackSubmitRequest {
        feedback_type: submit.feedback_type,
        title: Some(submit.title),
        description,
        // See module docs: the CLI has no error-banner context.
        entry_point: "settings".to_owned(),
        error_summary: None,
        attachments,
        privacy_notice_version: "community-v1".to_owned(),
    };
    pinvou3_lib::features::feedback::validate_feedback_request(&request).map_err(|error| {
        CliError::failed(format!(
            "feedback submit: {}",
            translate_feedback_text(&error.to_string())
        ))
    })?;
    let feedback_id = new_feedback_id();
    let pending_dir = pinvou3_lib::platform::paths::feedback_pending_dir();
    let receipts_dir = pinvou3_lib::platform::paths::feedback_receipts_dir();
    let pending_path = pending_dir.join(format!("{feedback_id}.json"));
    let receipt_path = receipts_dir.join(format!("{feedback_id}.json"));
    // Persist the request bundle before the (consuming) feature call so the
    // submitted content survives even on the failure paths.
    write_json(
        &pending_path,
        serde_json::to_value(&request)
            .map_err(|error| CliError::failed(format!("feedback submit: serialize: {error}")))?,
    )?;
    // The community `submit_feedback` body is validation + a fixed receipt
    // (no awaits); poll the real future once on this thread. If a future
    // version ever awaits (e.g. an upload path), this fails cleanly instead
    // of hanging or guessing at the receipt. A feature-level `Err` is a real
    // failure (validation) and must surface its message instead of being
    // collapsed into the generic not-synchronous error.
    let receipt = match poll_once(pinvou3_lib::features::feedback::submit_feedback(request)) {
        Some(Ok(receipt)) => receipt,
        Some(Err(error)) => {
            return Err(CliError::failed(format!(
                "feedback submit failed: {}",
                translate_feedback_text(&error.to_string())
            )));
        }
        None => {
            return Err(CliError::failed(
                "feedback submit: the feature did not complete synchronously; \
                 use the GitHub issue tracker directly",
            ));
        }
    };
    write_json(
        &receipt_path,
        serde_json::to_value(&receipt)
            .map_err(|error| CliError::failed(format!("feedback submit: serialize: {error}")))?,
    )?;

    let status = status_label(receipt.status);
    let message = translate_feedback_text(&receipt.message);
    let value = serde_json::json!({
        "feedback_id": feedback_id,
        "status": status,
        "pending_path": pending_path.display().to_string(),
        "receipt_path": receipt_path.display().to_string(),
        "issue_url": COMMUNITY_ISSUES_URL,
        "message": message,
    });
    let mut human = format!(
        "Feedback: {feedback_id}\nStatus: {status}\nPending: {}\nReceipt: {}\nCommunity edition does not upload feedback. Submit at: {COMMUNITY_ISSUES_URL}",
        pending_path.display(),
        receipt_path.display(),
    );
    // The community build's only receipt is `failed_validation` carrying the
    // no-upload notice — that is the designed success path here (validate
    // locally, persist the bundle, point at the tracker), not a caller
    // error, so it keeps exit 0. A genuine feature `Err` is rejected above.
    if receipt.status == FeedbackStatus::FailedValidation {
        human.push_str(&format!("\n{message}"));
    }
    Ok(success(render(output, human, &value)))
}

/// The feature layer's user-facing strings are Chinese (GUI copy); the CLI is
/// an English tool, so the known messages are translated at this boundary and
/// anything unrecognized passes through unchanged rather than being dropped.
fn translate_feedback_text(text: &str) -> String {
    if text.contains("请填写反馈说明") {
        "a feedback description is required".to_owned()
    } else if text.contains("反馈说明最多") {
        // The character count is embedded in the original message.
        let count = text
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>();
        format!("the feedback description allows at most {count} characters")
    } else if text.contains("反馈标题最多") {
        let count = text
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>();
        format!("the feedback title allows at most {count} characters")
    } else if text.contains("反馈入口来源无效") {
        "invalid feedback entry point".to_owned()
    } else if text.contains("社区版不会上传反馈") {
        format!(
            "The community edition does not upload feedback, logs, or attachments. Submit an issue at {COMMUNITY_ISSUES_URL}."
        )
    } else {
        text.to_owned()
    }
}

fn status_label(status: FeedbackStatus) -> &'static str {
    match status {
        FeedbackStatus::Submitted => "submitted",
        FeedbackStatus::FailedRetryable => "failed_retryable",
        FeedbackStatus::FailedValidation => "failed_validation",
    }
}

fn media_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "md" | "markdown" => "text/markdown",
        "txt" | "log" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

fn write_json(path: &Path, value: serde_json::Value) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CliError::failed(format!(
                "feedback submit: cannot create {}: {error}",
                parent.display()
            ))
        })?;
    }
    let mut bytes = serde_json::to_vec_pretty(&value)
        .map_err(|error| CliError::failed(format!("feedback submit: serialize: {error}")))?;
    bytes.push(b'\n');
    std::fs::write(path, bytes).map_err(|error| {
        CliError::failed(format!(
            "feedback submit: cannot write {}: {error}",
            path.display()
        ))
    })
}

fn new_feedback_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("feedback-{millis}-{}", std::process::id())
}

/// Polls an await-free future once on the current thread with a no-op waker.
/// Only valid for futures that finish on the first poll (the community
/// `submit_feedback` body contains no `.await`); `Pending` is surfaced as
/// `None` so the caller fails cleanly instead of hanging.
fn poll_once<F: std::future::Future>(future: F) -> Option<F::Output> {
    use std::task::{Context, Poll, Waker};

    let mut cx = Context::from_waker(Waker::noop());
    let mut pinned = std::pin::pin!(future);
    match pinned.as_mut().poll(&mut cx) {
        Poll::Ready(output) => Some(output),
        Poll::Pending => None,
    }
}
