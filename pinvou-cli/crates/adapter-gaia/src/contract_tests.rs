use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent_backend_api::{
    AttachmentHandle, PrivateInputHandle, ResolvedAttachmentSource, SecretOutput, SecretText,
};
use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray, StructArray};
use arrow_schema::{DataType, Field, Fields, Schema};
use async_trait::async_trait;
use benchmark_core::{
    BenchmarkAdapter, BenchmarkService, ExecutionRequest, ModelIdentity, RunContext, RunManifest,
    RunStore, Split, TaskOutcome, TaskRunner, TaskSelection, TaskStatus, ToolPolicyId,
    VerifiedDataset,
};
use parquet::arrow::ArrowWriter;
use sha2::{Digest, Sha256};

use crate::{
    GAIA_DATASET_REVISION, GAIA_REVISION_MARKER, GaiaAdapter, GaiaDataset, GaiaPrivateInputs,
};

const PARQUET_PATH: &str = "2023/validation/metadata.level1.parquet";

struct TempSnapshot(PathBuf);

impl TempSnapshot {
    fn new() -> Self {
        // 同进程并行测试线程在粗粒度时钟(如 macOS)下纳秒时间戳可能同 tick
        // 撞名,互相 remove_dir_all;用进程内原子计数保证唯一。
        static SNAPSHOT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let unique = SNAPSHOT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pinvou-gaia-unit-contract-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempSnapshot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn verified_fixture() -> (TempSnapshot, GaiaDataset) {
    let snapshot = TempSnapshot::new();
    let root = snapshot.path();
    fs::create_dir_all(root.join("2023/validation")).unwrap();
    fs::write(root.join(GAIA_REVISION_MARKER), GAIA_DATASET_REVISION).unwrap();
    fs::write(
        root.join("2023/validation/attachment.txt"),
        b"FROZEN_SYNTHETIC_ATTACHMENT",
    )
    .unwrap();

    let metadata_names = [
        "Steps",
        "Number of steps",
        "How long did this take?",
        "Tools",
        "Number of tools",
    ];
    let metadata_fields = Fields::from(
        metadata_names
            .iter()
            .map(|name| Field::new(*name, DataType::Utf8, true))
            .collect::<Vec<_>>(),
    );
    let metadata: ArrayRef = Arc::new(StructArray::new(
        metadata_fields,
        metadata_names
            .iter()
            .map(|_| Arc::new(StringArray::from(vec![Some("synthetic")])) as ArrayRef)
            .collect(),
        None,
    ));
    let arrays: Vec<(&str, ArrayRef)> = vec![
        ("task_id", Arc::new(StringArray::from(vec!["safe-task-1"]))),
        (
            "Question",
            Arc::new(StringArray::from(vec!["PRIVATE_QUESTION_SENTINEL"])),
        ),
        ("Level", Arc::new(Int64Array::from(vec![1]))),
        (
            "Final answer",
            Arc::new(StringArray::from(vec!["PRIVATE_REFERENCE_SENTINEL"])),
        ),
        (
            "file_name",
            Arc::new(StringArray::from(vec![Some("attachment.txt")])),
        ),
        (
            "file_path",
            Arc::new(StringArray::from(vec![Some(
                "2023/validation/attachment.txt",
            )])),
        ),
        ("Annotator Metadata", metadata),
    ];
    let schema = Arc::new(Schema::new(
        arrays
            .iter()
            .map(|(name, array)| Field::new(*name, array.data_type().clone(), true))
            .collect::<Vec<_>>(),
    ));
    let batch =
        RecordBatch::try_new(schema, arrays.into_iter().map(|(_, array)| array).collect()).unwrap();
    let file = fs::File::create(root.join(PARQUET_PATH)).unwrap();
    let mut writer = ArrowWriter::try_new(file, batch.schema(), None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    let parquet = fs::read(root.join(PARQUET_PATH)).unwrap();
    let mut dataset = GaiaDataset::verify_with_expected_parquet_for_tests(
        root,
        parquet.len() as u64,
        Sha256::digest(&parquet).into(),
    )
    .unwrap();
    dataset
        .bind_attachment_sha256(&BTreeMap::from([(
            PathBuf::from("2023/validation/attachment.txt"),
            Sha256::digest(b"FROZEN_SYNTHETIC_ATTACHMENT").into(),
        )]))
        .unwrap();
    (snapshot, dataset)
}

struct SubmissionRunner;

#[async_trait]
impl TaskRunner for SubmissionRunner {
    async fn run_task(
        &self,
        task: &benchmark_core::BenchmarkTask,
        _context: &RunContext,
    ) -> benchmark_core::Result<TaskOutcome> {
        Ok(
            TaskOutcome::new(task.task_id(), TaskStatus::Completed, None, vec![], 1)
                .with_private_output(SecretOutput::new(SecretText::new("candidate answer"))),
        )
    }
}

fn submission_completed_run(
    snapshot: &TempSnapshot,
    dataset: Arc<GaiaDataset>,
) -> (GaiaAdapter, PathBuf, benchmark_core::CompletedRun) {
    let adapter = GaiaAdapter::with_dataset(dataset);
    let runtime = snapshot.path().join("submission-runtime");
    fs::create_dir(&runtime).unwrap();
    let manifest = RunManifest::new(
        "gaia-submission-run",
        adapter.descriptor(),
        Split::new(crate::GAIA_SPLIT),
        ModelIdentity::new("synthetic", "model").unwrap(),
        ToolPolicyId::new("pinvou-gaia-public-web/v1"),
        1,
    )
    .unwrap();
    let service = BenchmarkService::with_runner(&runtime, Arc::new(SubmissionRunner)).unwrap();
    futures::executor::block_on(service.run_adapter(
        manifest,
        &adapter,
        &VerifiedDataset::new(crate::GAIA_DATASET_REVISION, snapshot.path()),
        &TaskSelection::all(),
    ))
    .unwrap();
    let run = RunStore::open(&runtime, "gaia-submission-run")
        .unwrap()
        .completed_run()
        .unwrap();
    (adapter, runtime, run)
}

#[test]
fn submission_reopened_run_writes_compact_official_jsonl_without_private_inputs() {
    let (snapshot, dataset) = verified_fixture();
    let (adapter, _runtime, run) = submission_completed_run(&snapshot, Arc::new(dataset));
    let destination = snapshot.path().join("submission.jsonl");

    let artifact = adapter.write_submission(&run, &destination).unwrap();
    assert_eq!(artifact.path(), destination);
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "{\"task_id\":\"safe-task-1\",\"model_answer\":\"candidate answer\"}\n"
    );
    let contents = fs::read_to_string(&destination).unwrap();
    for sentinel in [
        "PRIVATE_QUESTION_SENTINEL",
        "PRIVATE_REFERENCE_SENTINEL",
        "FROZEN_SYNTHETIC_ATTACHMENT",
        "pinvou-gaia-public-web/v1",
        "gaia-submission-run",
    ] {
        assert!(!contents.contains(sentinel));
    }
    assert!(!format!("{artifact:?}").contains(destination.to_string_lossy().as_ref()));
}

#[test]
fn submission_enforces_terminal_complete_runs_and_never_overwrites() {
    let (snapshot, dataset) = verified_fixture();
    let (adapter, runtime, run) = submission_completed_run(&snapshot, Arc::new(dataset));
    let destination = snapshot.path().join("submission.jsonl");
    fs::write(&destination, b"preserve").unwrap();
    let error = adapter.write_submission(&run, &destination).unwrap_err();
    assert_eq!(error.code(), "gaia_submission_target_exists");
    assert_eq!(fs::read(&destination).unwrap(), b"preserve");

    fs::remove_file(&destination).unwrap();
    let partial = benchmark_core::CompletedRun::new("partial", vec![]);
    let error = adapter
        .write_submission(&partial, &destination)
        .unwrap_err();
    assert_eq!(error.code(), "gaia_submission_incomplete");
    assert!(!destination.exists());
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains(snapshot.path().to_string_lossy().as_ref()));
    assert!(!rendered.contains("candidate answer"));

    for status in [TaskStatus::Planned, TaskStatus::Running] {
        let nonterminal = benchmark_core::CompletedRun::new(
            "nonterminal",
            vec![TaskOutcome::new("safe-task-1", status, None, vec![], 1)],
        );
        assert_eq!(
            adapter
                .write_submission(&nonterminal, &destination)
                .unwrap_err()
                .code(),
            "gaia_submission_not_completed"
        );
        assert!(!destination.exists());
    }

    let unknown = benchmark_core::CompletedRun::new(
        "unknown",
        vec![TaskOutcome::new(
            "unknown-task",
            TaskStatus::Completed,
            None,
            vec![],
            1,
        )],
    );
    assert_eq!(
        adapter
            .write_submission(&unknown, &destination)
            .unwrap_err()
            .code(),
        "gaia_submission_unknown_task"
    );
    let failed = benchmark_core::CompletedRun::new(
        "failed",
        vec![TaskOutcome::new(
            "safe-task-1",
            TaskStatus::Failed,
            None,
            vec![],
            1,
        )],
    );
    adapter.write_submission(&failed, &destination).unwrap();
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "{\"task_id\":\"safe-task-1\",\"model_answer\":\"\"}\n"
    );
    fs::remove_file(&destination).unwrap();

    let prediction_dir = runtime.join("eval/runs/gaia-submission-run/private/predictions");
    let blob = fs::read_dir(prediction_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(blob, b"CORRUPT_PRIVATE_SENTINEL").unwrap();
    let error = adapter.write_submission(&run, &destination).unwrap_err();
    assert_eq!(error.code(), "gaia_submission_prediction_unavailable");
    assert!(!destination.exists());
    assert!(!format!("{error:?} {error}").contains("CORRUPT_PRIVATE_SENTINEL"));

    let missing_prediction = benchmark_core::CompletedRun::new(
        "missing-prediction",
        vec![TaskOutcome::new(
            "safe-task-1",
            TaskStatus::Completed,
            None,
            vec![],
            1,
        )],
    );
    assert_eq!(
        adapter
            .write_submission(&missing_prediction, &destination)
            .unwrap_err()
            .code(),
        "gaia_submission_prediction_unavailable"
    );
    let duplicate = benchmark_core::CompletedRun::new(
        "duplicate",
        vec![
            TaskOutcome::new("safe-task-1", TaskStatus::Completed, None, vec![], 1),
            TaskOutcome::new("safe-task-1", TaskStatus::Completed, None, vec![], 1),
        ],
    );
    assert_eq!(
        adapter
            .write_submission(&duplicate, &destination)
            .unwrap_err()
            .code(),
        "gaia_submission_duplicate_task"
    );
    assert!(!destination.exists());
}

#[test]
fn submission_rejects_symlink_destination_or_ancestor_and_accepts_bare_filename_parent() {
    let (snapshot, dataset) = verified_fixture();
    let (adapter, _runtime, run) = submission_completed_run(&snapshot, Arc::new(dataset));
    let real_parent = snapshot.path().join("real-parent");
    fs::create_dir(&real_parent).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let destination_link = snapshot.path().join("destination-link.jsonl");
        symlink(snapshot.path().join("missing-target"), &destination_link).unwrap();
        assert_eq!(
            adapter
                .write_submission(&run, &destination_link)
                .unwrap_err()
                .code(),
            "gaia_submission_target_exists"
        );
        let parent_link = snapshot.path().join("parent-link");
        symlink(&real_parent, &parent_link).unwrap();
        assert_eq!(
            adapter
                .write_submission(&run, &parent_link.join("submission.jsonl"))
                .unwrap_err()
                .code(),
            "gaia_submission_target_unsafe"
        );
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::{symlink_dir, symlink_file};
        let destination_link = snapshot.path().join("destination-link.jsonl");
        if symlink_file(snapshot.path().join("missing-target"), &destination_link).is_ok() {
            assert_eq!(
                adapter
                    .write_submission(&run, &destination_link)
                    .unwrap_err()
                    .code(),
                "gaia_submission_target_exists"
            );
        }
        let parent_link = snapshot.path().join("parent-link");
        if symlink_dir(&real_parent, &parent_link).is_ok() {
            assert_eq!(
                adapter
                    .write_submission(&run, &parent_link.join("submission.jsonl"))
                    .unwrap_err()
                    .code(),
                "gaia_submission_target_unsafe"
            );
        }
    }
}

#[test]
fn default_unit_contract_covers_selection_native_turn_and_privacy() {
    let (_snapshot, dataset) = verified_fixture();
    let adapter = GaiaAdapter::new();
    let plan = adapter.plan(&dataset, &TaskSelection::all()).unwrap();

    assert_eq!(plan.tasks().len(), 1);
    let task = &plan.tasks()[0];
    let ExecutionRequest::NativeTurn {
        prompt_handle,
        attachments,
        timeout,
        tool_policy,
        output_contract,
    } = task.execution()
    else {
        panic!("GAIA must use NativeTurn");
    };
    assert_eq!(*timeout, Duration::from_secs(600));
    assert_eq!(tool_policy.as_str(), "pinvou-gaia-public-web/v1");
    assert_eq!(output_contract.as_str(), "gaia-final/v1");
    assert_eq!(attachments.len(), 1);
    assert_eq!(format!("{prompt_handle:?}"), "PrivateInputHandle([opaque])");
    let debug = format!("{task:?}");
    assert!(!debug.contains("PRIVATE_QUESTION_SENTINEL"));
    assert!(!debug.contains("PRIVATE_REFERENCE_SENTINEL"));

    assert_eq!(
        adapter
            .plan(
                &dataset,
                &TaskSelection::from_task_ids(vec!["safe-task-1".into()]),
            )
            .unwrap()
            .tasks()
            .len(),
        1
    );
    for invalid in ["", "unknown-task", "safe-task-1 "] {
        assert_eq!(
            adapter
                .plan(
                    &dataset,
                    &TaskSelection::from_task_ids(vec![invalid.into()]),
                )
                .unwrap_err()
                .code(),
            "gaia_task_selection_invalid"
        );
    }
}

#[test]
fn default_unit_resolver_freezes_attachment_and_uses_safe_errors() {
    let (snapshot, dataset) = verified_fixture();
    let inputs = GaiaPrivateInputs::new(Arc::new(dataset));
    let resolved = inputs
        .resolve_handle(&PrivateInputHandle::new("gaia:safe-task-1:prompt"))
        .unwrap();
    let prompt = resolved.prompt().expose_to_backend();
    assert!(prompt.starts_with("PRIVATE_QUESTION_SENTINEL"));
    assert!(prompt.contains("FINAL ANSWER: <answer>"));
    assert!(!format!("{inputs:?} {resolved:?}").contains("PRIVATE_QUESTION_SENTINEL"));

    let source = inputs
        .resolve_attachment_handle(&AttachmentHandle::new("gaia:safe-task-1:attachment"))
        .unwrap();
    let path = snapshot.path().join("2023/validation/attachment.txt");
    fs::write(&path, b"MUTATED_IN_PLACE_AND_GROWN").unwrap();
    fs::rename(&path, snapshot.path().join("mutated-original.txt")).unwrap();
    fs::write(&path, b"REPLACEMENT_PATH_BYTES").unwrap();
    let mut bytes = Vec::new();
    source
        .try_read_verified_file(|reader| reader.read_to_end(&mut bytes))
        .unwrap()
        .unwrap();
    assert_eq!(bytes, b"FROZEN_SYNTHETIC_ATTACHMENT");
    assert_eq!(
        source.verified_file_size().unwrap(),
        Some(bytes.len() as u64)
    );
    assert_eq!(
        format!("{source:?}"),
        "ResolvedAttachmentSource([redacted])"
    );

    let unknown = inputs
        .resolve_handle(&PrivateInputHandle::new("gaia:unknown-task:prompt"))
        .unwrap_err();
    assert_eq!(
        unknown.to_string(),
        "agent backend operation failed: gaia_private_input_unknown"
    );
    assert!(!unknown.to_string().contains("unknown-task"));
}

#[test]
fn immutable_attachment_check_rejects_content_changed_after_reopen_before_freeze() {
    let (snapshot, dataset) = verified_fixture();
    let attachment = dataset.rows()[0].attachment().unwrap();
    let verified_file = attachment.reopen_verified().unwrap();
    fs::write(
        snapshot.path().join("2023/validation/attachment.txt"),
        b"BROKEN_SYNTHETIC_ATTACHMENT",
    )
    .unwrap();
    let frozen = ResolvedAttachmentSource::from_verified_file(
        attachment.path(),
        "attachment.txt",
        verified_file,
    )
    .unwrap();
    assert!(attachment.verify_immutable_source(&frozen).is_err());
}
