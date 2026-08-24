use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_backend_api::{AttachmentHandle, PrivateInputHandle};
use benchmark_core::{
    ArtifactReference, BenchmarkAdapter, BenchmarkDescriptor, BenchmarkId, BenchmarkPlan,
    BenchmarkTask, CompletedRun, ExecutionKind, ExecutionRequest, OfficialScoreReport,
    OutputContract, PredictionRetention, PreparedTask, ReferenceHandle, RunContext, Split,
    SubmissionArtifact, TaskOutcome, TaskSelection, TaskStatus, ToolPolicyId, VerifiedDataset,
};

#[test]
fn submission_artifact_debug_redacts_destination_path() {
    let artifact = SubmissionArtifact::new("PRIVATE_SUBMISSION_PATH_SENTINEL.jsonl");
    assert_eq!(format!("{artifact:?}"), "SubmissionArtifact([redacted])");
}

struct ContractAdapter {
    descriptor: BenchmarkDescriptor,
}

impl BenchmarkAdapter for ContractAdapter {
    fn descriptor(&self) -> &BenchmarkDescriptor {
        &self.descriptor
    }

    fn verify_dataset(&self, dataset_root: &Path) -> benchmark_core::Result<VerifiedDataset> {
        Ok(VerifiedDataset::new("fixture", dataset_root))
    }

    fn plan(
        &self,
        _dataset: &VerifiedDataset,
        _selection: &TaskSelection,
    ) -> benchmark_core::Result<BenchmarkPlan> {
        Ok(BenchmarkPlan::new(vec![native_task()]))
    }

    fn prepare_task(
        &self,
        task: &BenchmarkTask,
        _run: &RunContext,
    ) -> benchmark_core::Result<PreparedTask> {
        Ok(PreparedTask::new(task.clone()))
    }

    fn score(&self, _run: &CompletedRun) -> benchmark_core::Result<OfficialScoreReport> {
        Ok(OfficialScoreReport::new(1, 1))
    }

    fn write_submission(
        &self,
        _run: &CompletedRun,
        destination: &Path,
    ) -> benchmark_core::Result<SubmissionArtifact> {
        Ok(SubmissionArtifact::new(destination))
    }
}

fn native_task() -> BenchmarkTask {
    BenchmarkTask::new(
        "task-1",
        Some("search".into()),
        Some("1".into()),
        ExecutionRequest::native_turn(
            PrivateInputHandle::new("private-prompt-1"),
            vec![AttachmentHandle::new("attachment-1")],
            Duration::from_secs(30),
            ToolPolicyId::new("gaia/v1"),
            OutputContract::new("final-answer/v1"),
        ),
        Some(ReferenceHandle::new("private-reference-1")),
    )
}

#[test]
fn adapter_contract_plans_prepares_scores_and_writes_submission() {
    let adapter = ContractAdapter {
        descriptor: BenchmarkDescriptor::new(
            BenchmarkId::new("fixture"),
            "fixture-adapter/v1",
            "dataset-sha",
            "scorer-sha",
            vec![Split::new("validation")],
            ExecutionKind::NativeTurn,
        ),
    };
    let dataset = adapter.verify_dataset(Path::new("dataset")).unwrap();
    assert_eq!(
        adapter.private_output_retention(),
        PredictionRetention::Ephemeral
    );
    assert_eq!(
        adapter.private_prediction_content_type(),
        benchmark_core::PrivatePredictionContentType::Utf8TextV1
    );
    let plan = adapter.plan(&dataset, &TaskSelection::all()).unwrap();
    let prepared = adapter
        .prepare_task(
            &plan.tasks()[0],
            &RunContext::new("run-1", PathBuf::from("run")),
        )
        .unwrap();
    let run = CompletedRun::new(
        "run-1",
        vec![TaskOutcome::new(
            "task-1",
            TaskStatus::Completed,
            None,
            vec![ArtifactReference::new("artifact-1")],
            7,
        )],
    );

    assert_eq!(
        adapter.descriptor().execution_kind(),
        ExecutionKind::NativeTurn
    );
    assert_eq!(prepared.task().task_id(), "task-1");
    assert_eq!(adapter.score(&run).unwrap().accuracy(), 1.0);
    assert_eq!(
        adapter
            .write_submission(&run, Path::new("submission.jsonl"))
            .unwrap()
            .path(),
        Path::new("submission.jsonl")
    );
}

#[test]
fn execution_contract_distinguishes_native_turn_and_external_harness() {
    let native = native_task();
    let external = ExecutionRequest::external_harness(
        benchmark_core::VerifiedArtifact::new("workspace-1"),
        "sha256:0123456789abcdef",
        vec!["workbuddy".into(), "run".into()],
        Duration::from_secs(90),
    );

    assert_eq!(native.execution().kind(), ExecutionKind::NativeTurn);
    assert_eq!(external.kind(), ExecutionKind::ExternalHarness);
}

#[test]
fn handles_do_not_reveal_private_payloads_through_debug_output() {
    let task = native_task();
    let prediction_handle = benchmark_core::PredictionHandle::new("prediction-secret");
    let task_debug = format!("{task:?}");
    let prediction_debug = format!("{prediction_handle:?}");
    assert!(!task_debug.contains("private-prompt-1"));
    assert!(!task_debug.contains("private-reference-1"));
    assert!(!prediction_debug.contains("prediction-secret"));
}

#[test]
fn official_score_reports_compatible_and_partial_metadata() {
    let complete = OfficialScoreReport::compatible(53, 31, "validation", "1");
    assert_eq!(complete.evaluated(), 53);
    assert_eq!(complete.correct(), 31);
    assert!(complete.is_complete());
    assert!(complete.is_official_dataset_compatible());
    assert_eq!(complete.split(), "validation");
    assert_eq!(complete.level(), "1");
    assert_eq!(complete.comparable_accuracy(), Some(31.0 / 53.0));

    let partial = OfficialScoreReport::partial(4, 2, "validation", "1");
    assert!(!partial.is_complete());
    assert!(!partial.is_official_dataset_compatible());
    assert_eq!(partial.comparable_accuracy(), None);
}

#[test]
fn official_score_rejects_impossible_counts_without_claiming_compatibility() {
    let invalid_compatible = OfficialScoreReport::compatible(4, 5, "validation", "1");
    assert_eq!(invalid_compatible.evaluated(), 4);
    assert_eq!(invalid_compatible.correct(), 4);
    assert!(!invalid_compatible.is_complete());
    assert!(!invalid_compatible.is_official_dataset_compatible());
    assert_eq!(invalid_compatible.comparable_accuracy(), None);

    let invalid_partial = OfficialScoreReport::partial(4, 5, "validation", "1");
    assert_eq!(invalid_partial.evaluated(), 4);
    assert_eq!(invalid_partial.correct(), 4);
    assert_eq!(invalid_partial.comparable_accuracy(), None);
}

#[test]
fn official_score_zero_evaluated_is_not_compatible() {
    let empty = OfficialScoreReport::compatible(0, 0, "validation", "1");
    assert!(!empty.is_complete());
    assert!(!empty.is_official_dataset_compatible());
    assert_eq!(empty.comparable_accuracy(), None);
}

#[test]
fn official_score_legacy_constructor_remains_partial_and_non_comparable() {
    let legacy = OfficialScoreReport::new(2, 1);
    assert_eq!(legacy.evaluated(), 2);
    assert_eq!(legacy.correct(), 1);
    assert_eq!(legacy.accuracy(), 0.5);
    assert!(!legacy.is_complete());
    assert!(!legacy.is_official_dataset_compatible());
    assert_eq!(legacy.split(), "unspecified");
    assert_eq!(legacy.level(), "unspecified");
    assert_eq!(legacy.comparable_accuracy(), None);
}
