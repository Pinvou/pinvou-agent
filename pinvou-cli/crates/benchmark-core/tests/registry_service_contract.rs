use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_backend_api::PrivateInputHandle;
use agent_backend_api::{SecretOutput, SecretText};
use async_trait::async_trait;
use benchmark_core::{
    BenchmarkAdapter, BenchmarkDescriptor, BenchmarkId, BenchmarkPlan, BenchmarkRegistry,
    BenchmarkService, BenchmarkTask, CompletedRun, ExecutionKind, ExecutionRequest, ModelIdentity,
    OfficialScoreReport, OutputContract, PredictionRetention, PreparedTask, RunContext,
    RunManifest, RunStore, Split, SubmissionArtifact, TaskOutcome, TaskRunner, TaskSelection,
    TaskStatus, ToolPolicyId, VerifiedDataset,
};

fn temp_base(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("pinvou-registry-{name}-{nonce}"));
    std::fs::create_dir(&path).unwrap();
    path
}

#[derive(Default)]
struct Calls {
    planned: usize,
    prepared: usize,
    scored: usize,
    submitted: usize,
    ran: Vec<String>,
}

struct FixtureAdapter {
    descriptor: BenchmarkDescriptor,
    calls: Arc<Mutex<Calls>>,
}

impl FixtureAdapter {
    fn new(id: &str, calls: Arc<Mutex<Calls>>) -> Self {
        Self {
            descriptor: BenchmarkDescriptor::new(
                BenchmarkId::new(id),
                "fixture-adapter/v1",
                "fixture-dataset/v1",
                "fixture-scorer/v1",
                vec![Split::new("validation")],
                ExecutionKind::NativeTurn,
            ),
            calls,
        }
    }
}

impl BenchmarkAdapter for FixtureAdapter {
    fn descriptor(&self) -> &BenchmarkDescriptor {
        &self.descriptor
    }

    fn verify_dataset(&self, dataset_root: &Path) -> benchmark_core::Result<VerifiedDataset> {
        Ok(VerifiedDataset::new("fixture-dataset/v1", dataset_root))
    }

    fn plan(
        &self,
        _dataset: &VerifiedDataset,
        _selection: &TaskSelection,
    ) -> benchmark_core::Result<BenchmarkPlan> {
        self.calls.lock().unwrap().planned += 1;
        Ok(BenchmarkPlan::new(vec![BenchmarkTask::new(
            "raw-task",
            None,
            None,
            ExecutionRequest::native_turn(
                PrivateInputHandle::new("private-input"),
                vec![],
                Duration::from_secs(1),
                ToolPolicyId::new("fixture/v1"),
                OutputContract::new("fixture/v1"),
            ),
            None,
        )]))
    }

    fn prepare_task(
        &self,
        task: &BenchmarkTask,
        _run: &RunContext,
    ) -> benchmark_core::Result<PreparedTask> {
        self.calls.lock().unwrap().prepared += 1;
        Ok(PreparedTask::new(task.clone()))
    }

    fn score(&self, _run: &CompletedRun) -> benchmark_core::Result<OfficialScoreReport> {
        self.calls.lock().unwrap().scored += 1;
        Ok(OfficialScoreReport::new(1, 1))
    }

    fn write_submission(
        &self,
        _run: &CompletedRun,
        destination: &Path,
    ) -> benchmark_core::Result<SubmissionArtifact> {
        self.calls.lock().unwrap().submitted += 1;
        Ok(SubmissionArtifact::new(destination))
    }
}

struct DurableFixtureAdapter(FixtureAdapter);

impl BenchmarkAdapter for DurableFixtureAdapter {
    fn descriptor(&self) -> &BenchmarkDescriptor {
        self.0.descriptor()
    }

    fn private_output_retention(&self) -> PredictionRetention {
        PredictionRetention::DurableUntilPurge
    }

    fn verify_dataset(&self, root: &Path) -> benchmark_core::Result<VerifiedDataset> {
        self.0.verify_dataset(root)
    }

    fn plan(
        &self,
        dataset: &VerifiedDataset,
        selection: &TaskSelection,
    ) -> benchmark_core::Result<BenchmarkPlan> {
        self.0.plan(dataset, selection)
    }

    fn prepare_task(
        &self,
        task: &BenchmarkTask,
        run: &RunContext,
    ) -> benchmark_core::Result<PreparedTask> {
        self.0.prepare_task(task, run)
    }

    fn score(&self, run: &CompletedRun) -> benchmark_core::Result<OfficialScoreReport> {
        self.0.score(run)
    }

    fn write_submission(
        &self,
        run: &CompletedRun,
        destination: &Path,
    ) -> benchmark_core::Result<SubmissionArtifact> {
        self.0.write_submission(run, destination)
    }
}

struct CanonicalJsonFixtureAdapter(DurableFixtureAdapter);

impl BenchmarkAdapter for CanonicalJsonFixtureAdapter {
    fn descriptor(&self) -> &BenchmarkDescriptor {
        self.0.descriptor()
    }
    fn private_output_retention(&self) -> PredictionRetention {
        PredictionRetention::DurableUntilPurge
    }
    fn private_prediction_content_type(&self) -> benchmark_core::PrivatePredictionContentType {
        benchmark_core::PrivatePredictionContentType::CanonicalJsonV1
    }
    fn verify_dataset(&self, root: &Path) -> benchmark_core::Result<VerifiedDataset> {
        self.0.verify_dataset(root)
    }
    fn plan(
        &self,
        dataset: &VerifiedDataset,
        selection: &TaskSelection,
    ) -> benchmark_core::Result<BenchmarkPlan> {
        self.0.plan(dataset, selection)
    }
    fn prepare_task(
        &self,
        task: &BenchmarkTask,
        run: &RunContext,
    ) -> benchmark_core::Result<PreparedTask> {
        self.0.prepare_task(task, run)
    }
    fn score(&self, run: &CompletedRun) -> benchmark_core::Result<OfficialScoreReport> {
        self.0.score(run)
    }
    fn write_submission(
        &self,
        run: &CompletedRun,
        destination: &Path,
    ) -> benchmark_core::Result<SubmissionArtifact> {
        self.0.write_submission(run, destination)
    }
}

struct RecordingRunner(Arc<Mutex<Calls>>);

#[async_trait]
impl TaskRunner for RecordingRunner {
    async fn run_task(
        &self,
        task: &BenchmarkTask,
        _context: &RunContext,
    ) -> benchmark_core::Result<TaskOutcome> {
        self.0.lock().unwrap().ran.push(task.task_id().to_owned());
        Ok(TaskOutcome::new(
            task.task_id(),
            TaskStatus::Completed,
            None,
            vec![],
            1,
        ))
    }
}

struct PrivateOutputRunner;

#[async_trait]
impl TaskRunner for PrivateOutputRunner {
    async fn run_task(
        &self,
        task: &BenchmarkTask,
        _context: &RunContext,
    ) -> benchmark_core::Result<TaskOutcome> {
        Ok(
            TaskOutcome::new(task.task_id(), TaskStatus::Completed, None, vec![], 1)
                .with_private_output(SecretOutput::new(SecretText::new(
                    "PRIVATE_ANSWER_SENTINEL",
                ))),
        )
    }
}

struct CanonicalJsonRunner;

#[async_trait]
impl TaskRunner for CanonicalJsonRunner {
    async fn run_task(
        &self,
        task: &BenchmarkTask,
        _context: &RunContext,
    ) -> benchmark_core::Result<TaskOutcome> {
        Ok(
            TaskOutcome::new(task.task_id(), TaskStatus::Completed, None, vec![], 1)
                .with_private_output(SecretOutput::new(SecretText::new(
                    r#"{"answer":"PRIVATE_JSON_SENTINEL"}"#,
                ))),
        )
    }
}

#[test]
fn canonical_json_durable_run_reopens_without_public_secret_bytes() {
    let base = temp_base("canonical-json-private-output");
    let calls = Arc::new(Mutex::new(Calls::default()));
    let adapter =
        CanonicalJsonFixtureAdapter(DurableFixtureAdapter(FixtureAdapter::new("fixture", calls)));
    let dataset = adapter.verify_dataset(Path::new("fixture-root")).unwrap();
    let manifest = RunManifest::new(
        "run-canonical-json",
        adapter.descriptor(),
        Split::new("validation"),
        ModelIdentity::new("fixture", "model").unwrap(),
        ToolPolicyId::new("fixture/v1"),
        1,
    )
    .unwrap();
    let service = BenchmarkService::with_runner(&base, Arc::new(CanonicalJsonRunner)).unwrap();
    futures::executor::block_on(service.run_adapter(
        manifest,
        &adapter,
        &dataset,
        &TaskSelection::all(),
    ))
    .unwrap();

    let run_dir = base.join("eval/runs/run-canonical-json");
    for public_path in ["manifest.json", "events.jsonl", "predictions.jsonl"] {
        let public = std::fs::read_to_string(run_dir.join(public_path)).unwrap();
        assert!(!public.contains("PRIVATE_JSON_SENTINEL"), "{public_path}");
    }
    let reopened = RunStore::open(&base, "run-canonical-json")
        .unwrap()
        .completed_run()
        .unwrap();
    let payload = reopened
        .resolve_private_prediction(&reopened.outcomes()[0])
        .unwrap();
    assert_eq!(
        payload.content_type(),
        benchmark_core::PrivatePredictionContentType::CanonicalJsonV1
    );
    assert_eq!(
        payload.expose_to_scorer(),
        br#"{"answer":"PRIVATE_JSON_SENTINEL"}"#
    );
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn ephemeral_runs_do_not_create_private_blobs_or_publish_backend_capabilities() {
    let base = temp_base("ephemeral-private-output");
    let calls = Arc::new(Mutex::new(Calls::default()));
    let adapter = FixtureAdapter::new("fixture", calls);
    let dataset = adapter.verify_dataset(Path::new("fixture-root")).unwrap();
    let manifest = RunManifest::new(
        "run-ephemeral",
        adapter.descriptor(),
        Split::new("validation"),
        ModelIdentity::new("fixture", "model").unwrap(),
        ToolPolicyId::new("fixture/v1"),
        1,
    )
    .unwrap();
    let service = BenchmarkService::with_runner(&base, Arc::new(PrivateOutputRunner)).unwrap();

    let summary = futures::executor::block_on(service.run_adapter(
        manifest,
        &adapter,
        &dataset,
        &TaskSelection::all(),
    ))
    .unwrap();

    assert!(summary.outcomes()[0].prediction().is_none());
    let run_dir = base.join("eval/runs/run-ephemeral");
    assert!(!run_dir.join("private/predictions").exists());
    for public_path in ["manifest.json", "events.jsonl", "predictions.jsonl"] {
        let public = std::fs::read_to_string(run_dir.join(public_path)).unwrap();
        assert!(!public.contains("PRIVATE_ANSWER_SENTINEL"), "{public_path}");
    }
    let reopened = RunStore::open(&base, "run-ephemeral")
        .unwrap()
        .completed_run()
        .unwrap();
    assert!(reopened.outcomes()[0].prediction().is_none());
    assert!(!run_dir.join("private/predictions").exists());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn durable_runs_publish_only_core_handles_and_reopen_for_scoring() {
    let base = temp_base("durable-private-output");
    let calls = Arc::new(Mutex::new(Calls::default()));
    let adapter = DurableFixtureAdapter(FixtureAdapter::new("fixture", calls));
    let dataset = adapter.verify_dataset(Path::new("fixture-root")).unwrap();
    let manifest = RunManifest::new(
        "run-durable",
        adapter.descriptor(),
        Split::new("validation"),
        ModelIdentity::new("fixture", "model").unwrap(),
        ToolPolicyId::new("fixture/v1"),
        1,
    )
    .unwrap();
    let service = BenchmarkService::with_runner(&base, Arc::new(PrivateOutputRunner)).unwrap();

    futures::executor::block_on(service.run_adapter(
        manifest,
        &adapter,
        &dataset,
        &TaskSelection::all(),
    ))
    .unwrap();

    let run_dir = base.join("eval/runs/run-durable");
    let public = std::fs::read_to_string(run_dir.join("predictions.jsonl")).unwrap();
    assert!(!public.contains("PRIVATE_ANSWER_SENTINEL"));
    let blobs = std::fs::read_dir(run_dir.join("private/predictions"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(blobs.len(), 1);

    let reopened = RunStore::open(&base, "run-durable")
        .unwrap()
        .completed_run()
        .unwrap();
    let payload = reopened
        .resolve_private_prediction(&reopened.outcomes()[0])
        .unwrap();
    assert_eq!(payload.expose_to_scorer(), b"PRIVATE_ANSWER_SENTINEL");

    std::fs::remove_file(blobs[0].path()).unwrap();
    let missing = reopened
        .resolve_private_prediction(&reopened.outcomes()[0])
        .unwrap_err();
    assert_eq!(missing.code(), "private_prediction_unavailable");
    assert!(!missing.to_string().contains("PRIVATE_ANSWER_SENTINEL"));
    std::fs::write(blobs[0].path(), b"corrupt-private-blob").unwrap();
    let corrupt = reopened
        .resolve_private_prediction(&reopened.outcomes()[0])
        .unwrap_err();
    assert_eq!(corrupt.code(), "private_prediction_unavailable");
    assert!(!format!("{reopened:?}").contains("PRIVATE_ANSWER_SENTINEL"));
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn registry_rejects_duplicate_and_unknown_benchmark_ids() {
    let mut registry = BenchmarkRegistry::new();
    registry
        .register(Arc::new(FixtureAdapter::new(
            "fixture",
            Arc::new(Mutex::new(Calls::default())),
        )))
        .unwrap();
    assert_eq!(
        registry
            .get(&BenchmarkId::new("fixture"))
            .unwrap()
            .descriptor()
            .id()
            .as_str(),
        "fixture"
    );
    let duplicate = registry
        .register(Arc::new(FixtureAdapter::new(
            "fixture",
            Arc::new(Mutex::new(Calls::default())),
        )))
        .unwrap_err();
    assert_eq!(duplicate.code(), "duplicate_benchmark");
    assert_eq!(
        registry
            .get(&BenchmarkId::new("unknown"))
            .err()
            .unwrap()
            .code(),
        "unknown_benchmark"
    );
}

#[test]
fn adapter_driven_service_plans_prepares_then_runs_without_implicit_scoring() {
    let base = temp_base("service");
    let calls = Arc::new(Mutex::new(Calls::default()));
    let adapter = FixtureAdapter::new("fixture", calls.clone());
    let dataset = adapter.verify_dataset(Path::new("fixture-root")).unwrap();
    let manifest = RunManifest::new(
        "run-adapter",
        adapter.descriptor(),
        Split::new("validation"),
        ModelIdentity::new("fixture", "model").unwrap(),
        ToolPolicyId::new("fixture/v1"),
        1,
    )
    .unwrap();
    let service =
        BenchmarkService::with_runner(&base, Arc::new(RecordingRunner(calls.clone()))).unwrap();
    let summary = futures::executor::block_on(service.run_adapter(
        manifest,
        &adapter,
        &dataset,
        &TaskSelection::all(),
    ))
    .unwrap();

    assert_eq!(summary.completed(), 1);
    let snapshot = calls.lock().unwrap();
    assert_eq!(snapshot.planned, 1);
    assert_eq!(snapshot.prepared, 1);
    assert_eq!(snapshot.ran, ["raw-task"]);
    assert_eq!(snapshot.scored, 0);
    assert_eq!(snapshot.submitted, 0);
    drop(snapshot);

    let completed = CompletedRun::new(summary.run_id(), summary.outcomes().to_vec());
    assert_eq!(
        service
            .score_adapter(&adapter, &completed)
            .unwrap()
            .accuracy(),
        1.0
    );
    assert_eq!(
        service
            .write_adapter_submission(&adapter, &completed, Path::new("submission.jsonl"))
            .unwrap()
            .path(),
        Path::new("submission.jsonl")
    );
    let calls = calls.lock().unwrap();
    assert_eq!(calls.scored, 1);
    assert_eq!(calls.submitted, 1);
    drop(calls);

    let expected = RunManifest::new(
        "run-adapter",
        adapter.descriptor(),
        Split::new("validation"),
        ModelIdentity::new("fixture", "model").unwrap(),
        ToolPolicyId::new("fixture/v1"),
        1,
    )
    .unwrap();
    for (field, replacement) in [
        ("schema_version", serde_json::json!(2)),
        ("concurrency", serde_json::json!(2)),
        ("pass", serde_json::json!(2)),
        ("adapter_version", serde_json::json!("fixture-adapter/v2")),
        ("dataset_revision", serde_json::json!("fixture-dataset/v2")),
        ("scorer_revision", serde_json::json!("fixture-scorer/v2")),
        ("split", serde_json::json!("test")),
        (
            "model",
            serde_json::json!({"provider": "fixture", "model": "other"}),
        ),
        ("tool_policy", serde_json::json!("fixture/v2")),
    ] {
        let mut value = serde_json::to_value(&expected).unwrap();
        value[field] = replacement;
        let changed: RunManifest = serde_json::from_value(value).unwrap();
        let error = futures::executor::block_on(service.resume_adapter(
            "run-adapter",
            &changed,
            &adapter,
            &dataset,
            &TaskSelection::all(),
        ))
        .unwrap_err();
        assert_eq!(error.code(), "resume_manifest_mismatch", "{field}");
    }
    std::fs::remove_dir_all(base).unwrap();
}
