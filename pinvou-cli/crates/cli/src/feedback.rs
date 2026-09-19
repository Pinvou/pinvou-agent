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
//!   receipt under `$PINVOU3_HOME/feedback/receipts/`. Those directories
//!   were `platform::paths` helpers on the fork base and were swept on main
//!   once the GUI stopped persisting feedback locally; the CLI reconstructs
//!   the same layout from `paths::pinvou3_home()` so earlier bundles stay
//!   readable. Nothing leaves the machine.
//!
//! The request struct carries only `title`/`description`/`entry_point` on
//! main — `type`/`error_summary`/`attachments`/`privacy_notice_version`
//! were parse-then-drop fields the stub never consumed, and the sweep
//! removed them; the CLI follows the reduced shape, so `feedback submit`
//! takes no `--type` and no `--attach`.
//!
//! The CLI fixes the request `entry_point` to `"settings"` (the only two
//! values the feature accepts are the GUI's settings page and error banner)
//! because a terminal invocation has no banner context.

use std::path::{Path, PathBuf};

use pinvou3_lib::features::feedback::{FeedbackStatus, FeedbackSubmitRequest};

use crate::support::{render, sandbox_home, success};
use crate::{CliError, CliOutcome, OutputMode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedbackCommand {
    submit: SubmitArgs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SubmitArgs {
    title: String,
    body_file: PathBuf,
}

const USAGE: &str = "usage: pinvou feedback submit --title T --body-file F";

/// The community feature function's message points here; the const is
/// crate-private upstream, so the URL is mirrored verbatim.
const COMMUNITY_ISSUES_URL: &str = "https://github.com/Pinvou/pinvou-agent/issues";

pub fn parse(values: &[String]) -> Result<FeedbackCommand, CliError> {
    let subcommand = values.get(1).ok_or_else(|| CliError::usage(USAGE))?;
    if subcommand != "submit" {
        return Err(CliError::usage(USAGE));
    }
    let mut title: Option<String> = None;
    let mut body_file: Option<PathBuf> = None;
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
            other => {
                return Err(CliError::usage(format!(
                    "unsupported feedback option: {other}"
                )));
            }
        }
    }
    let submit = SubmitArgs {
        title: title.ok_or_else(|| CliError::usage("feedback submit requires --title T"))?,
        body_file: body_file
            .ok_or_else(|| CliError::usage("feedback submit requires --body-file F"))?,
    };
    Ok(FeedbackCommand { submit })
}

pub fn execute(command: FeedbackCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    // Validates the sandbox-home contract (absolute PINVOU3_HOME) before any
    // feedback paths are resolved.
    sandbox_home()?;
    let submit = command.submit;
    // The body is validated/truncated further down; the cap only stops an
    // unbounded file from being loaded in the first place.
    let description =
        crate::support::read_text_file_capped(&submit.body_file, 64 * 1024, "feedback submit")?;
    let request = FeedbackSubmitRequest {
        title: Some(submit.title),
        description,
        // See module docs: the CLI has no error-banner context.
        entry_point: "settings".to_owned(),
    };
    pinvou3_lib::features::feedback::validate_feedback_request(&request).map_err(|error| {
        CliError::failed(format!(
            "feedback submit: {}",
            translate_feedback_text(&error.to_string())
        ))
    })?;
    let feedback_id = new_feedback_id();
    // See module docs: the swept `platform::paths` helpers were one-line
    // joins over the same layout; reconstruct it from the pub home path.
    let pending_dir = pinvou3_lib::platform::paths::pinvou3_home()
        .join("feedback")
        .join("pending");
    let receipts_dir = pinvou3_lib::platform::paths::pinvou3_home()
        .join("feedback")
        .join("receipts");
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
    let mut receipt_value = serde_json::to_value(&receipt)
        .map_err(|error| CliError::failed(format!("feedback submit: serialize: {error}")))?;
    // The community feature answers with an empty id (`features/feedback`
    // owns no id generation), while the receipt file is named after the
    // CLI-generated id — persist that id in the payload so scripts reading
    // the file see the same id the filename and stdout carry.
    if let Some(object) = receipt_value.as_object_mut() {
        object.insert(
            "feedback_id".to_owned(),
            serde_json::Value::String(feedback_id.clone()),
        );
    }
    write_json(&receipt_path, receipt_value)?;

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
        FeedbackStatus::FailedValidation => "failed_validation",
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
    // Same tmp+rename discipline as the scheduled registry writer: a crash
    // mid-write must not leave a truncated pending/receipt file that a later
    // submit would read as garbage.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = path.with_extension(format!("json.tmp.{}.{}", std::process::id(), nonce));
    std::fs::write(&tmp, bytes).map_err(|error| {
        // Same cleanup guarantee as the rename-failure path below: a staged
        // file that never lands must not accumulate in the feedback dir.
        let _ = std::fs::remove_file(&tmp);
        CliError::failed(format!(
            "feedback submit: cannot write {}: {error}",
            tmp.display()
        ))
    })?;
    let _ = std::fs::File::open(&tmp).and_then(|file| file.sync_all());
    std::fs::rename(&tmp, path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        CliError::failed(format!(
            "feedback submit: cannot move {} to {}: {error}",
            tmp.display(),
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
