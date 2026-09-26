//! `feedback` family: user feedback submission, mirroring
//! `pinvou3-app/src-tauri/src/app/commands/settings.rs::submit_feedback` →
//! `features::feedback::submit_feedback`.
//!
//! The community feature function never uploads anything: it validates the
//! request and returns a receipt that points at the public GitHub issue
//! tracker. The CLI calls exactly that function and discloses two surface
//! deviations:
//! - the GUI opens the issues URL in a browser; the CLI prints it;
//! - the GUI keeps the receipt in-app; the CLI persists one file per
//!   submission under `$PINVOU3_HOME/feedback/receipts/<id>.json`, holding the
//!   returned receipt plus the request it answers. Nothing leaves the machine.
//!
//! Retention: the request is staged under `$PINVOU3_HOME/feedback/pending/`
//! only for the duration of the (consuming) feature call, so the user's text
//! survives a crash in between, and is removed once the receipt lands.
//! `platform::paths` documents that directory as "packages that failed to
//! upload or are still being prepared"; because the community
//! `submit_feedback` never uploads and no retry lane exists, a bundle left
//! there after a concluded run would be permanent and would claim an in-flight
//! upload that will never happen. Both files are written `0600` on unix: they
//! contain free text the user wrote, and the default umask would publish it to
//! every account on the machine.
//!
//! `--attach` registers a path only — nothing is read and nothing is uploaded —
//! but the path lands in that permanent receipt, so attachments are held to the
//! same credential-path policy the GUI's picker enforces
//! (`artifacts::check_sensitive_path`), and the registered set is echoed back
//! in both output modes so `--attach` is not silently a no-op.
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
    // The body is validated/truncated further down; the cap only stops an
    // unbounded file from being loaded in the first place.
    let description =
        crate::support::read_text_file_capped(&submit.body_file, 64 * 1024, "feedback submit")?;
    let mut attachments = Vec::new();
    // Rendered back to the user below. The request itself is consumed by the
    // feature call, so the summary is built here while the data is still
    // owned locally.
    let mut attachment_rows = Vec::new();
    for path in &submit.attachments {
        let size = std::fs::metadata(path)
            .map_err(|error| {
                CliError::failed(format!(
                    "feedback submit: cannot read attachment {}: {error}",
                    path.display()
                ))
            })?
            .len();
        // Nothing is read or uploaded here — only the path is recorded — but
        // it is recorded into a receipt that stays on disk forever, and the
        // GUI's attachment picker refuses these locations outright. A CLI that
        // happily writes `/home/u/.ssh/id_rsa` into a persisted file is a way
        // around a rule the other surface enforces, so the same path policy
        // applies (`artifacts::check_sensitive_path`, the mirror of
        // `platform::path_policy::check_sensitive_components`). The canonical
        // form is what the policy is defined over, so resolve first.
        let canonical = std::fs::canonicalize(path).map_err(|error| {
            CliError::failed(format!(
                "feedback submit: cannot resolve attachment {}: {error}",
                path.display()
            ))
        })?;
        crate::artifacts::check_sensitive_path(&canonical).map_err(|reason| {
            CliError::failed(format!("feedback submit: refusing attachment: {reason}"))
        })?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("attachment")
            .to_owned();
        let media_type = media_type(path).to_owned();
        let display_path = path.display().to_string();
        attachment_rows.push(serde_json::json!({
            "path": display_path,
            "name": name,
            "media_type": media_type,
            "size_bytes": size,
        }));
        attachments.push(FeedbackAttachmentRequest {
            path: display_path,
            name,
            media_type,
            size_bytes: Some(size),
        });
    }
    let request = FeedbackSubmitRequest {
        feedback_type: submit.feedback_type,
        title: Some(submit.title),
        description,
        // See module docs: the CLI has no error-banner context.
        entry_point: "settings".to_owned(),
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
    let request_value = serde_json::to_value(&request)
        .map_err(|error| CliError::failed(format!("feedback submit: serialize: {error}")))?;
    // Stage the request under `feedback/pending/` before the (consuming)
    // feature call, so the submitted content survives a crash or an error
    // between here and the receipt. That directory means "packages that failed
    // to upload or are still being prepared" (`platform::paths`), which is
    // exactly what this file is *while the call is in flight* — and nothing
    // more. It is removed again below once the run concludes.
    write_json(&pending_path, &request_value)?;
    // The community `submit_feedback` body is validation + a fixed receipt
    // (no awaits); poll the real future once on this thread. If a future
    // version ever awaits (e.g. an upload path), this fails cleanly instead
    // of hanging or guessing at the receipt. A feature-level `Err` is a real
    // failure (validation) and must surface its message instead of being
    // collapsed into the generic not-synchronous error.
    //
    // Kept deliberately, and kept *in addition to* the explicit
    // `validate_feedback_request` above even though `submit_feedback` runs the
    // same validation internally. The two are not redundant: the explicit call
    // rejects a bad request before anything is staged on disk (so an invalid
    // submission leaves no file under `feedback/pending/` at all), while this
    // call is the real feature entry point — the receipt message, including
    // the community no-upload notice, belongs to `features/feedback` and is
    // not reconstructed here. Inlining a `FeedbackReceipt` literal instead
    // would make the CLI the second place that owns that copy.
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
    let object = receipt_value
        .as_object_mut()
        .ok_or_else(|| CliError::failed("feedback submit: the receipt is not a JSON object"))?;
    object.insert(
        "feedback_id".to_owned(),
        serde_json::Value::String(feedback_id.clone()),
    );
    // The receipt carries the request it is a receipt for. Once the staged
    // copy is dropped (below) this is the only record of what the user wrote,
    // and they need that text to paste into the issue tracker.
    object.insert("request".to_owned(), request_value);
    // A failed receipt write used to `?` straight out of here, leaving the
    // staged bundle behind — precisely the permanent, never-retried file under
    // `feedback/pending/` that the module docs argue against, and the error
    // named only the receipt path so nothing pointed at it. The run has
    // concluded either way, so the stage is dropped on both arms. Nothing the
    // user wrote is lost with it: the description came from `--body-file`,
    // which is still on disk, and the attachments were only ever referenced by
    // path.
    let write_result = write_json(&receipt_path, &receipt_value);
    // The run concluded, so nothing is pending: the community `submit_feedback`
    // never uploads and there is no retry lane that would ever pick this file
    // up. Leaving it under `feedback/pending/` claimed an in-flight upload that
    // does not exist and made the user's free text permanent in a directory
    // documented for transient packages. A removal failure is only a note: on
    // the success arm the submission itself succeeded, and failing it here
    // would tell the user their feedback was not recorded when the receipt is
    // on disk.
    if let Err(error) = std::fs::remove_file(&pending_path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            note!(
                "warning: feedback submit: the staged bundle {} could not be removed \
                 after the submission concluded ({error}); delete it manually — \
                 nothing will retry it",
                pending_path.display()
            );
        }
    }
    write_result?;

    let status = status_label(receipt.status);
    let message = translate_feedback_text(&receipt.message);
    // `status` is for humans. Scripts branch on `uploaded`: the community
    // receipt is always `failed_validation` *on the success path* (see below),
    // so `.status == "submitted"` never holds and `.status` cannot be used to
    // detect a real failure either. `uploaded` answers the one question a
    // caller actually has — did this leave the machine — and stays correct if
    // an edition ever does upload.
    let uploaded = receipt.status == FeedbackStatus::Submitted;
    // Built before the payload takes ownership of the rows.
    let attachment_lines = attachment_rows
        .iter()
        .map(|row| {
            format!(
                "\nAttachment: {} ({}, {} bytes) {}",
                row["name"].as_str().unwrap_or_default(),
                row["media_type"].as_str().unwrap_or_default(),
                row["size_bytes"].as_u64().unwrap_or_default(),
                row["path"].as_str().unwrap_or_default(),
            )
        })
        .collect::<String>();
    // No `pending_path`: the staged file is gone by now, and a key naming a
    // path that does not exist is worse than no key.
    let value = serde_json::json!({
        "feedback_id": feedback_id,
        "status": status,
        "uploaded": uploaded,
        "receipt_path": receipt_path.display().to_string(),
        "issue_url": COMMUNITY_ISSUES_URL,
        "message": message,
        // Registered attachments, echoed back: the command stats each one and
        // records it into the persisted request, and without this the user has
        // no evidence that `--attach` did anything at all.
        "attachments": attachment_rows,
    });
    let mut human = format!(
        "Feedback: {feedback_id}\nStatus: {status}\nUploaded: {uploaded}\nReceipt: {}\nCommunity edition does not upload feedback. Submit at: {COMMUNITY_ISSUES_URL}",
        receipt_path.display(),
    );
    human.push_str(&attachment_lines);
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

/// Writes one feedback file atomically, owner-readable only.
///
/// Both files this command produces carry the user's free-text feedback (the
/// staged request holds it directly; the receipt embeds it). On a shared
/// machine the default umask leaves that world-readable for as long as the
/// file exists — which, since nothing ever uploads or prunes it, is forever.
/// `0600` matches how the app stores user-authored content it does not intend
/// to publish.
fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CliError::failed(format!(
                "feedback submit: cannot create {}: {error}",
                parent.display()
            ))
        })?;
    }
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| CliError::failed(format!("feedback submit: serialize: {error}")))?;
    bytes.push(b'\n');
    // Shared stage+rename helper: a crash mid-write must not leave a truncated
    // feedback file behind, and the staging step must not follow a planted
    // symlink or swallow a failing fsync (see `artifacts::atomic_write`).
    crate::artifacts::atomic_write(path, &bytes, crate::artifacts::WriteVisibility::OwnerOnly)
        .map_err(|error| {
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
