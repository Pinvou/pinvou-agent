use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use benchmark_core::{
    BenchmarkError, CompletedRun, PrivatePredictionContentType, Result, SubmissionArtifact,
    TaskStatus,
};
use rand::random;
use serde::Serialize;

use crate::GaiaDataset;
use crate::fetch::{create_private_file, is_link_or_reparse};

/// Durable private prediction type tag shared with the GAIA adapter and scorer.
/// Core persists the resolved candidate answer under this concrete content type,
/// which is the run-bound scorer tag used to reopen predictions offline.
const GAIA_DURABLE_PREDICTION_TYPE: &str = "utf8-text/v1";
const MAX_TASKS: usize = 128;
const MAX_ANSWER_BYTES: usize = 64 * 1024;
const MAX_SUBMISSION_BYTES: u64 = 16 * 1024 * 1024;

/// Exports the official-format GAIA submission JSONL.
///
/// Only complete GAIA runs reopened from the durable private store are
/// accepted: every dataset row must have exactly one terminal outcome. A
/// completed outcome must have a durable `utf8-text/v1` prediction resolvable
/// through the run-bound scorer view; failed, timed-out, and cancelled outcomes
/// are exported with an empty answer and therefore score as incorrect. Each line
/// is a compact JSON object with exactly two keys, `task_id` and `model_answer`,
/// emitted in deterministic dataset row order. The public
/// prediction handle cannot decode the answer. Publication is atomic and
/// no-clobber: a sibling temporary file with private permissions is written,
/// synced, then hard-linked into place, which fails if the destination already
/// exists. No prompt, reference, attachment, tool I/O, session id, or internal
/// handle is ever emitted.
pub(crate) fn write_submission(
    dataset: &GaiaDataset,
    run: &CompletedRun,
    destination: &Path,
) -> Result<SubmissionArtifact> {
    // Reject any existing destination (regular file, directory, or reparse
    // point) before resolving private predictions.
    if fs::symlink_metadata(destination).is_ok() {
        return Err(BenchmarkError::Contract(
            "gaia_submission_target_exists".into(),
        ));
    }
    // The parent must already be a real directory with no reparse leaf.
    let requested_parent = destination
        .parent()
        .ok_or_else(|| BenchmarkError::Contract("gaia_submission_target_unsafe".into()))?;
    let parent = validate_parent(requested_parent)?;

    // Resolve every prediction before touching the filesystem so an incomplete
    // run never leaves a partial file behind.
    let entries = collect_entries(dataset, run)?;
    if entries.len() > MAX_TASKS {
        return Err(BenchmarkError::Contract(
            "gaia_submission_incomplete".into(),
        ));
    }

    let temporary = parent.join(format!(
        ".pinvou-gaia-submission-{:016x}.tmp",
        random::<u64>()
    ));
    let result = (|| {
        let mut file = create_private_file(&temporary)
            .map_err(|_| BenchmarkError::Contract("gaia_submission_publish_failed".into()))?;
        let mut total = 0_u64;
        for entry in &entries {
            let line = serde_json::to_string(&SubmissionLine {
                task_id: entry.task_id.as_str(),
                model_answer: entry.answer.as_str(),
            })
            .map_err(|_| BenchmarkError::Contract("gaia_submission_publish_failed".into()))?;
            let mut bytes = line.into_bytes();
            bytes.push(b'\n');
            total = total
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| BenchmarkError::Contract("gaia_submission_publish_failed".into()))?;
            if total > MAX_SUBMISSION_BYTES {
                return Err(BenchmarkError::Contract(
                    "gaia_submission_publish_failed".into(),
                ));
            }
            file.write_all(&bytes)
                .map_err(|_| BenchmarkError::Contract("gaia_submission_publish_failed".into()))?;
        }
        file.sync_all()
            .map_err(|_| BenchmarkError::Contract("gaia_submission_publish_failed".into()))?;
        drop(file);
        // Publish atomically without overwrite: hard_link fails if the
        // destination already exists, closing the TOCTOU window between the
        // initial existence check and publication.
        fs::hard_link(&temporary, destination)
            .map_err(|_| BenchmarkError::Contract("gaia_submission_target_exists".into()))?;
        // Publication already succeeded. A failed best-effort cleanup must not
        // report failure while leaving a valid official target behind.
        let _ = fs::remove_file(&temporary);
        if sync_parent(&parent).is_err() {
            let _ = fs::remove_file(destination);
            return Err(BenchmarkError::Contract(
                "gaia_submission_publish_failed".into(),
            ));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map(|_| SubmissionArtifact::new(destination))
}

struct SubmissionEntry {
    task_id: String,
    answer: String,
}

#[derive(Serialize)]
struct SubmissionLine<'a> {
    task_id: &'a str,
    model_answer: &'a str,
}

fn collect_entries(dataset: &GaiaDataset, run: &CompletedRun) -> Result<Vec<SubmissionEntry>> {
    let rows = dataset.rows();
    let outcomes = run.outcomes();
    if rows.is_empty() || outcomes.is_empty() {
        return Err(BenchmarkError::Contract(
            "gaia_submission_incomplete".into(),
        ));
    }
    let dataset_ids = rows.iter().map(|row| row.task_id()).collect::<HashSet<_>>();
    // Index outcomes by task id, rejecting unknown, duplicate, and non-terminal
    // statuses with fixed safe codes.
    let mut by_task: HashMap<&str, &benchmark_core::TaskOutcome> =
        HashMap::with_capacity(outcomes.len());
    for outcome in outcomes {
        if !dataset_ids.contains(outcome.task_id()) {
            return Err(BenchmarkError::Contract(
                "gaia_submission_unknown_task".into(),
            ));
        }
        if by_task.insert(outcome.task_id(), outcome).is_some() {
            return Err(BenchmarkError::Contract(
                "gaia_submission_duplicate_task".into(),
            ));
        }
        if matches!(outcome.status(), TaskStatus::Planned | TaskStatus::Running) {
            return Err(BenchmarkError::Contract(
                "gaia_submission_not_completed".into(),
            ));
        }
    }
    // Check coverage after structural validation so duplicate and unknown
    // outcomes retain their stable, actionable safe error codes.
    if outcomes.len() != rows.len() {
        return Err(BenchmarkError::Contract(
            "gaia_submission_incomplete".into(),
        ));
    }
    // Emit in deterministic dataset row order, resolving each prediction
    // through the run-bound scorer view.
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let outcome = by_task
            .get(row.task_id())
            .ok_or_else(|| BenchmarkError::Contract("gaia_submission_incomplete".into()))?;
        if matches!(
            outcome.status(),
            TaskStatus::Failed | TaskStatus::Timeout | TaskStatus::Cancelled
        ) {
            entries.push(SubmissionEntry {
                task_id: outcome.task_id().to_owned(),
                answer: String::new(),
            });
            continue;
        }
        let prediction = outcome.prediction().ok_or_else(|| {
            BenchmarkError::Contract("gaia_submission_prediction_unavailable".into())
        })?;
        if prediction.type_tag() != GAIA_DURABLE_PREDICTION_TYPE {
            return Err(BenchmarkError::Contract(
                "gaia_submission_prediction_unavailable".into(),
            ));
        }
        let payload = run.resolve_private_prediction(outcome).map_err(|_| {
            BenchmarkError::Contract("gaia_submission_prediction_unavailable".into())
        })?;
        if payload.content_type() != PrivatePredictionContentType::Utf8TextV1 {
            return Err(BenchmarkError::Contract(
                "gaia_submission_prediction_unavailable".into(),
            ));
        }
        let answer = std::str::from_utf8(payload.expose_to_scorer()).map_err(|_| {
            BenchmarkError::Contract("gaia_submission_prediction_unavailable".into())
        })?;
        if answer.len() > MAX_ANSWER_BYTES {
            return Err(BenchmarkError::Contract(
                "gaia_submission_incomplete".into(),
            ));
        }
        entries.push(SubmissionEntry {
            task_id: outcome.task_id().to_owned(),
            answer: answer.to_owned(),
        });
    }
    Ok(entries)
}

fn validate_parent(parent: &Path) -> Result<PathBuf> {
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    if parent
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(BenchmarkError::Contract(
            "gaia_submission_target_unsafe".into(),
        ));
    }
    // 只校验叶子父目录本身,不逐级拒绝 symlink 祖先:防覆写由 create_new +
    // hard_link 承担,而 macOS 的 /var($TMPDIR 祖先)等系统 symlink 会让
    // 祖先级检查把所有合法临时目录判为不安全。
    let metadata = fs::symlink_metadata(parent)
        .map_err(|_| BenchmarkError::Contract("gaia_submission_target_unsafe".into()))?;
    if !metadata.is_dir() || is_link_or_reparse(&metadata) {
        return Err(BenchmarkError::Contract(
            "gaia_submission_target_unsafe".into(),
        ));
    }
    parent
        .canonicalize()
        .map_err(|_| BenchmarkError::Contract("gaia_submission_target_unsafe".into()))
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> std::io::Result<()> {
    fs::File::open(parent)?.sync_all()
}

#[cfg(windows)]
fn sync_parent(_parent: &Path) -> std::io::Result<()> {
    // Windows has no reliable directory fsync equivalent. The private file is
    // sync_all'ed before publication; this explicit platform no-op documents
    // the remaining metadata durability limitation.
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn sync_parent(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn submission_bare_filename_uses_current_directory_as_safe_parent() {
        assert_eq!(
            super::validate_parent(Path::new("")).unwrap(),
            Path::new(".").canonicalize().unwrap()
        );
    }

    /// validate_parent 只拒绝叶子层的 symlink;经由 symlink 祖先(如 macOS
    /// /var → /private/var)到达的真实目录必须可用,否则 $TMPDIR 下写
    /// submission 恒失败。
    #[cfg(unix)]
    #[test]
    fn submission_accepts_parent_behind_symlinked_ancestor() {
        let tmp = std::env::temp_dir().join(format!(
            "gaia-submission-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(tmp.join("real").join("inner")).unwrap();
        std::os::unix::fs::symlink(tmp.join("real"), tmp.join("link")).unwrap();

        assert!(super::validate_parent(&tmp.join("link").join("inner")).is_ok());
        // 叶子父目录本身是 symlink 仍拒绝。
        assert!(super::validate_parent(&tmp.join("link")).is_err());

        std::fs::remove_dir_all(tmp).unwrap();
    }
}
