use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_backend_api::{
    AgentBackendError, AgentRunObserver, AgentSessionHandle, AgentTaskInput, AgentTaskOutcome,
    AttachmentHandle, HeadlessAgentBackend, PrepareRequest, PrivateInputHandle,
    PrivateInputResolver, PrivateOutputHandle, ResolvedAttachmentSource, ResolvedPrivateInput,
    SafeAgentEvent, SafeUsageMetrics, SecretOutput, SecretText,
};
use async_trait::async_trait;
use benchmark_core::{
    BenchmarkDescriptor, BenchmarkId, BenchmarkPlan, BenchmarkService, BenchmarkTask,
    ExecutionKind, ExecutionRequest, ModelIdentity, NativeAgentRunner, OutputContract, RunContext,
    RunManifest, RunStore, Split, TaskRunner, TaskStatus, ToolPolicyId,
};

fn temp_base(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("pinvou-core-{name}-{nonce}"));
    fs::create_dir(&path).unwrap();
    path
}

fn descriptor() -> BenchmarkDescriptor {
    BenchmarkDescriptor::new(
        BenchmarkId::new("smoke"),
        "smoke/v1",
        "dataset-sha-123",
        "scorer-sha-123",
        vec![Split::new("smoke")],
        ExecutionKind::NativeTurn,
    )
}

fn manifest(run_id: &str) -> RunManifest {
    RunManifest::new(
        run_id,
        &descriptor(),
        Split::new("smoke"),
        ModelIdentity::new("fixture", "mock-model").unwrap(),
        ToolPolicyId::new("smoke/v1"),
        1,
    )
    .unwrap()
}

fn task(id: &str) -> BenchmarkTask {
    BenchmarkTask::new(
        id,
        None,
        None,
        ExecutionRequest::native_turn(
            PrivateInputHandle::new(format!("private-{id}")),
            vec![],
            Duration::from_secs(5),
            ToolPolicyId::new("smoke/v1"),
            OutputContract::new("text/v1"),
        ),
        None,
    )
}

fn attachment_task(id: &str) -> BenchmarkTask {
    BenchmarkTask::new(
        id,
        None,
        None,
        ExecutionRequest::native_turn(
            PrivateInputHandle::new(format!("private-{id}")),
            vec![AttachmentHandle::new(format!("attachment-{id}"))],
            Duration::from_secs(5),
            ToolPolicyId::new("smoke/v1"),
            OutputContract::new("text/v1"),
        ),
        None,
    )
}

#[test]
fn manifest_is_immutable_and_rejects_secret_shaped_model_identity() {
    let base = temp_base("manifest");
    let store = RunStore::create(&base, &manifest("run-1")).unwrap();
    let manifest_text = fs::read_to_string(store.manifest_path()).unwrap();
    assert!(!manifest_text.to_ascii_lowercase().contains("token"));

    let error = RunStore::create(&base, &manifest("run-1")).unwrap_err();
    assert_eq!(error.code(), "run_exists");
    assert_eq!(
        fs::read_to_string(store.manifest_path()).unwrap(),
        manifest_text
    );

    let error = ModelIdentity::new("fixture", "api_key=secret").unwrap_err();
    assert_eq!(error.code(), "unsafe_persistence");
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn resume_manifest_match_rejects_every_pinned_contract_dimension() {
    let expected = manifest("run-resume-contract");
    let expected_descriptor = descriptor();
    let expected_model = ModelIdentity::new("fixture", "mock-model").unwrap();
    assert!(expected.matches_resume(&expected_descriptor, "smoke", &expected_model, "smoke/v1"));

    let mutations = [
        ("schema_version", serde_json::json!(2)),
        ("concurrency", serde_json::json!(2)),
        ("pass", serde_json::json!(2)),
        ("benchmark", serde_json::json!("other")),
        ("adapter_version", serde_json::json!("smoke/v2")),
        ("dataset_revision", serde_json::json!("dataset-sha-456")),
        ("scorer_revision", serde_json::json!("scorer-sha-456")),
        ("split", serde_json::json!("validation")),
        (
            "model",
            serde_json::json!({"provider": "fixture", "model": "other-model"}),
        ),
        ("tool_policy", serde_json::json!("smoke/v2")),
    ];
    for (field, replacement) in mutations {
        let mut stored = serde_json::to_value(&expected).unwrap();
        stored[field] = replacement;
        let stored: RunManifest = serde_json::from_value(stored).unwrap();
        assert!(
            !stored.matches_resume(&expected_descriptor, "smoke", &expected_model, "smoke/v1"),
            "resume unexpectedly accepted changed {field}"
        );
    }
}

#[test]
fn gaia_manifest_contract_rejects_every_non_model_dimension() {
    let expected = manifest("gaia-contract");
    let expected_descriptor = descriptor();
    assert!(expected.matches_contract(&expected_descriptor, "smoke", "smoke/v1", 1));

    let mutations = [
        ("schema_version", serde_json::json!(2)),
        ("concurrency", serde_json::json!(2)),
        ("pass", serde_json::json!(2)),
        ("benchmark", serde_json::json!("other")),
        ("adapter_version", serde_json::json!("smoke/v2")),
        ("dataset_revision", serde_json::json!("dataset-sha-456")),
        ("scorer_revision", serde_json::json!("scorer-sha-456")),
        ("split", serde_json::json!("validation")),
        ("tool_policy", serde_json::json!("smoke/v2")),
    ];
    for (field, replacement) in mutations {
        let mut stored = serde_json::to_value(&expected).unwrap();
        stored[field] = replacement;
        let stored: RunManifest = serde_json::from_value(stored).unwrap();
        assert!(
            !stored.matches_contract(&expected_descriptor, "smoke", "smoke/v1", 1),
            "contract unexpectedly accepted changed {field}"
        );
    }

    let mut changed_model = serde_json::to_value(&expected).unwrap();
    changed_model["model"] = serde_json::json!({"provider": "other", "model": "arbitrary"});
    let changed_model: RunManifest = serde_json::from_value(changed_model).unwrap();
    assert!(changed_model.matches_contract(&expected_descriptor, "smoke", "smoke/v1", 1));
}

#[test]
fn terminal_event_requires_a_durable_outcome_and_resume_skips_it() {
    let base = temp_base("ordering");
    let store = RunStore::create(&base, &manifest("run-1")).unwrap();
    store.plan_tasks(["done", "pending"]).unwrap();
    let error = store.mark_completed("done").unwrap_err();
    assert_eq!(error.code(), "outcome_not_durable");

    store
        .record_outcome(benchmark_core::TaskOutcome::new(
            "done",
            TaskStatus::Completed,
            None,
            vec![],
            1,
        ))
        .unwrap();
    store.mark_completed("done").unwrap();

    let recovered = store.recover().unwrap();
    assert_eq!(recovered.completed_task_ids(), &["done"]);
    assert_eq!(recovered.runnable_task_ids(), &["pending"]);
    fs::remove_dir_all(base).unwrap();
}

#[derive(Default)]
struct MockState {
    prepared: Vec<String>,
    prepared_tool_policies: Vec<Option<String>>,
    prepared_attachment_names: Vec<String>,
    run: Vec<String>,
    closed: usize,
    active: usize,
    max_active: usize,
    cancelled: usize,
}

enum BackendBehavior {
    Completed,
    Failed,
    FailFirst,
    Pending,
    PendingPrepare,
    PendingResolveOutput,
    PendingClose,
    ResolveFailed,
    PendingCleanup,
}

struct MockBackend {
    state: Mutex<MockState>,
    behavior: BackendBehavior,
}

struct AttachmentResolver {
    fail: bool,
}

struct PendingAttachmentResolver;

#[async_trait]
impl PrivateInputResolver for PendingAttachmentResolver {
    async fn resolve(
        &self,
        _handle: &PrivateInputHandle,
    ) -> Result<ResolvedPrivateInput, AgentBackendError> {
        Ok(ResolvedPrivateInput::new(
            SecretText::new("private"),
            vec![],
        ))
    }

    async fn resolve_attachment(
        &self,
        _handle: &AttachmentHandle,
    ) -> Result<ResolvedAttachmentSource, AgentBackendError> {
        std::future::pending().await
    }
}

#[async_trait]
impl PrivateInputResolver for AttachmentResolver {
    async fn resolve(
        &self,
        _handle: &PrivateInputHandle,
    ) -> Result<ResolvedPrivateInput, AgentBackendError> {
        Ok(ResolvedPrivateInput::new(
            SecretText::new("private"),
            vec![],
        ))
    }

    async fn resolve_attachment(
        &self,
        _handle: &AttachmentHandle,
    ) -> Result<ResolvedAttachmentSource, AgentBackendError> {
        if self.fail {
            Err(AgentBackendError::Operation("PRIVATE_PATH".into()))
        } else {
            Ok(ResolvedAttachmentSource::new(
                PathBuf::from("C:/private/PRIVATE_ATTACHMENT.txt"),
                "attachment.txt",
            ))
        }
    }
}

impl Default for MockBackend {
    fn default() -> Self {
        Self {
            state: Mutex::new(MockState::default()),
            behavior: BackendBehavior::Completed,
        }
    }
}

impl MockBackend {
    fn with_behavior(behavior: BackendBehavior) -> Self {
        Self {
            state: Mutex::new(MockState::default()),
            behavior,
        }
    }
}

#[async_trait]
impl HeadlessAgentBackend for MockBackend {
    async fn prepare(
        &self,
        request: PrepareRequest,
    ) -> Result<AgentSessionHandle, AgentBackendError> {
        {
            let mut state = self.state.lock().unwrap();
            state.prepared.push(request.task_id().into());
            state.prepared_tool_policies.push(
                request
                    .tool_policy()
                    .map(|policy| policy.as_str().to_owned()),
            );
            state.prepared_attachment_names.extend(
                request
                    .resolved_attachments()
                    .iter()
                    .map(|source| source.suggested_name().to_owned()),
            );
        }
        if matches!(self.behavior, BackendBehavior::PendingPrepare) {
            return std::future::pending().await;
        }
        Ok(AgentSessionHandle::new(request.task_id()))
    }

    async fn run(
        &self,
        _session: &AgentSessionHandle,
        task: AgentTaskInput,
        _private_inputs: Arc<dyn PrivateInputResolver>,
        observer: Arc<dyn AgentRunObserver>,
    ) -> Result<AgentTaskOutcome, AgentBackendError> {
        {
            let mut state = self.state.lock().unwrap();
            state.active += 1;
            state.max_active = state.max_active.max(state.active);
            state.run.push(task.task_id().into());
            state.active -= 1;
        }
        observer.on_event(&SafeAgentEvent::tool_finished(
            task.task_id(),
            "web_search",
            true,
            Duration::from_millis(3),
        ));
        observer.on_event(&SafeAgentEvent::tool_finished(
            task.task_id(),
            "api_key=PRIVATE_TOOL",
            false,
            Duration::from_millis(4),
        ));
        match self.behavior {
            BackendBehavior::Completed
            | BackendBehavior::PendingPrepare
            | BackendBehavior::PendingResolveOutput
            | BackendBehavior::PendingClose => {
                Ok(AgentTaskOutcome::completed(Duration::from_millis(2))
                    .with_private_output(PrivateOutputHandle::new(format!(
                        "prediction-{}",
                        task.task_id()
                    )))
                    .with_usage(SafeUsageMetrics::new(100, 20, 70, 30)))
            }
            BackendBehavior::Failed => Err(AgentBackendError::Operation(
                "provider secret must not escape".into(),
            )),
            BackendBehavior::FailFirst if task.task_id() == "first" => {
                Err(AgentBackendError::Operation("private failure".into()))
            }
            BackendBehavior::FailFirst => Ok(AgentTaskOutcome::completed(Duration::from_millis(2))),
            BackendBehavior::Pending => std::future::pending().await,
            BackendBehavior::PendingCleanup => std::future::pending().await,
            BackendBehavior::ResolveFailed => {
                Ok(AgentTaskOutcome::completed(Duration::from_millis(2))
                    .with_private_output(PrivateOutputHandle::new("missing")))
            }
        }
    }

    async fn cancel(&self, _session: &AgentSessionHandle) -> Result<(), AgentBackendError> {
        self.state.lock().unwrap().cancelled += 1;
        if matches!(self.behavior, BackendBehavior::PendingCleanup) {
            return std::future::pending().await;
        }
        Ok(())
    }

    async fn resolve_output(
        &self,
        _handle: &PrivateOutputHandle,
    ) -> Result<SecretOutput, AgentBackendError> {
        if matches!(self.behavior, BackendBehavior::ResolveFailed) {
            return Err(AgentBackendError::Operation(
                "private resolver secret".into(),
            ));
        }
        if matches!(self.behavior, BackendBehavior::PendingResolveOutput) {
            return std::future::pending().await;
        }
        Ok(SecretOutput::new(SecretText::new(
            "PRIVATE_ANSWER_SENTINEL",
        )))
    }

    async fn close(&self, _session: AgentSessionHandle) -> Result<(), AgentBackendError> {
        self.state.lock().unwrap().closed += 1;
        if matches!(
            self.behavior,
            BackendBehavior::PendingCleanup | BackendBehavior::PendingClose
        ) {
            return std::future::pending().await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn native_turn_forwards_the_validated_tool_policy_to_prepare() {
    let base = temp_base("tool-policy-forwarding");
    let backend = Arc::new(MockBackend::default());
    let runner = NativeAgentRunner::new(backend.clone());

    runner
        .run_task(
            &task("policy-probe"),
            &RunContext::new("policy-probe", base.clone()),
        )
        .await
        .unwrap();

    assert_eq!(
        backend.state.lock().unwrap().prepared_tool_policies,
        [Some("smoke/v1".to_owned())]
    );
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn unsafe_native_tool_policy_is_rejected_before_backend_or_outcome_processing() {
    let base = temp_base("unsafe-tool-policy");
    let backend = Arc::new(MockBackend::default());
    let runner = NativeAgentRunner::new(backend.clone());
    let unsafe_task = BenchmarkTask::new(
        "unsafe-policy",
        None,
        None,
        ExecutionRequest::native_turn(
            PrivateInputHandle::new("private-unsafe-policy"),
            vec![],
            Duration::from_secs(5),
            ToolPolicyId::new("api_key=PRIVATE_SENTINEL"),
            OutputContract::new("text/v1"),
        ),
        None,
    );

    let error = runner
        .run_task(
            &unsafe_task,
            &RunContext::new("unsafe-policy", base.clone()),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code(), "unsupported_tool_policy");
    let state = backend.state.lock().unwrap();
    assert!(state.prepared.is_empty());
    assert!(state.run.is_empty());
    assert_eq!(state.closed, 0);
    drop(state);
    assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn attachment_resolution_happens_before_prepare_and_failure_is_safe() {
    let base = temp_base("attachment-resolution");
    let backend = Arc::new(MockBackend::default());
    let runner = NativeAgentRunner::with_private_inputs(
        backend.clone(),
        Arc::new(AttachmentResolver { fail: true }),
    );
    let error = runner
        .run_task(
            &attachment_task("probe"),
            &RunContext::new("probe", base.clone()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), "attachment_resolution_failed");
    let service = BenchmarkService::native_with_private_inputs(
        &base,
        backend.clone(),
        Arc::new(AttachmentResolver { fail: true }),
    )
    .unwrap();
    let summary = service
        .run(
            manifest("attachment-failed"),
            &BenchmarkPlan::new(vec![attachment_task("one")]),
        )
        .await
        .unwrap();

    assert_eq!(summary.outcomes()[0].status(), TaskStatus::Failed);
    assert!(backend.state.lock().unwrap().prepared.is_empty());
    let persisted =
        fs::read_to_string(base.join("eval/runs/attachment-failed/predictions.jsonl")).unwrap();
    assert!(!persisted.contains("PRIVATE_PATH"));
    assert!(!persisted.contains("PRIVATE_ATTACHMENT"));

    let success_backend = Arc::new(MockBackend::default());
    let success_runner = NativeAgentRunner::with_private_inputs(
        success_backend.clone(),
        Arc::new(AttachmentResolver { fail: false }),
    );
    success_runner
        .run_task(
            &attachment_task("success"),
            &RunContext::new("success", base.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        success_backend
            .state
            .lock()
            .unwrap()
            .prepared_attachment_names,
        ["attachment.txt"]
    );
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn attachment_resolution_consumes_the_same_task_deadline() {
    let base = temp_base("attachment-deadline");
    let backend = Arc::new(MockBackend::default());
    let runner = NativeAgentRunner::with_private_inputs(
        backend.clone(),
        Arc::new(PendingAttachmentResolver),
    );
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        runner.run_task(
            &BenchmarkTask::new(
                "attachment-timeout",
                None,
                None,
                ExecutionRequest::native_turn(
                    PrivateInputHandle::new("private"),
                    vec![AttachmentHandle::new("attachment")],
                    Duration::from_millis(5),
                    ToolPolicyId::new("smoke/v1"),
                    OutputContract::new("text/v1"),
                ),
                None,
            ),
            &RunContext::new("attachment-timeout", base.clone()),
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.code(), "task_timeout");
    assert!(backend.state.lock().unwrap().prepared.is_empty());
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn service_runs_native_tasks_serially_closes_sessions_and_resumes_without_reexecution() {
    let base = temp_base("service");
    let backend = Arc::new(MockBackend::default());
    let service = BenchmarkService::native(&base, backend.clone()).unwrap();
    let plan = BenchmarkPlan::new(vec![task("one"), task("two")]);

    let first = service.run(manifest("run-1"), &plan).await.unwrap();
    assert_eq!(first.completed(), 2);
    let resumed = service.resume("run-1", &plan).await.unwrap();
    assert_eq!(resumed.completed(), 2);

    let state = backend.state.lock().unwrap();
    assert_eq!(state.prepared, ["one", "two"]);
    assert_eq!(state.run, ["one", "two"]);
    assert_eq!(state.closed, 2);
    assert_eq!(state.max_active, 1);
    assert_eq!(
        first.outcomes()[0].tool_observations()[0].canonical_name,
        "web_search"
    );
    assert_eq!(
        first.outcomes()[0]
            .private_output()
            .unwrap()
            .text()
            .expose_to_backend(),
        "PRIVATE_ANSWER_SENTINEL"
    );
    let persisted = fs::read_to_string(base.join("eval/runs/run-1/predictions.jsonl")).unwrap();
    assert!(!persisted.contains("PRIVATE_ANSWER_SENTINEL"));
    assert!(!persisted.contains("PRIVATE_TOOL"));
    assert!(persisted.contains("[redacted-tool]"));
    assert_eq!(first.outcomes()[0].usage().unwrap().cache_hit_tokens, 70);
    assert_eq!(resumed.outcomes()[0].usage().unwrap().cache_miss_tokens, 30);
    drop(state);
    fs::remove_dir_all(base).unwrap();
}

fn short_task(id: &str) -> BenchmarkTask {
    BenchmarkTask::new(
        id,
        None,
        None,
        ExecutionRequest::native_turn(
            PrivateInputHandle::new(format!("private-{id}")),
            vec![],
            Duration::from_millis(5),
            ToolPolicyId::new("smoke/v1"),
            OutputContract::new("text/v1"),
        ),
        None,
    )
}

#[tokio::test]
async fn native_timeout_cancels_and_closes_exactly_once_with_safe_error() {
    let base = temp_base("timeout");
    let backend = Arc::new(MockBackend::with_behavior(BackendBehavior::Pending));
    let service = BenchmarkService::native(&base, backend.clone()).unwrap();
    let summary = service
        .run(
            manifest("run-timeout"),
            &BenchmarkPlan::new(vec![short_task("slow")]),
        )
        .await
        .unwrap();

    assert_eq!(summary.outcomes()[0].status(), TaskStatus::Timeout);
    assert_eq!(
        summary.outcomes()[0].failure_category(),
        Some(&benchmark_core::SafeFailureCategory::Timeout)
    );
    let state = backend.state.lock().unwrap();
    assert_eq!(state.cancelled, 1);
    assert_eq!(state.closed, 1);
    drop(state);
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn single_deadline_covers_prepare_output_resolution_and_normal_close() {
    for (name, behavior, expected_cancel, minimum_close) in [
        ("prepare", BackendBehavior::PendingPrepare, 0, 0),
        ("output", BackendBehavior::PendingResolveOutput, 1, 1),
        ("close", BackendBehavior::PendingClose, 1, 2),
    ] {
        let base = temp_base(name);
        let backend = Arc::new(MockBackend::with_behavior(behavior));
        let service = BenchmarkService::native(&base, backend.clone()).unwrap();
        let summary = tokio::time::timeout(
            Duration::from_secs(5),
            service.run(
                manifest(&format!("run-{name}")),
                &BenchmarkPlan::new(vec![short_task(name)]),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(summary.outcomes()[0].status(), TaskStatus::Timeout);
        assert_eq!(
            summary.outcomes()[0].failure_category(),
            Some(&benchmark_core::SafeFailureCategory::Timeout)
        );
        let state = backend.state.lock().unwrap();
        assert_eq!(state.cancelled, expected_cancel, "{name}");
        assert!(state.closed >= minimum_close, "{name}");
        drop(state);
        fs::remove_dir_all(base).unwrap();
    }
}

#[tokio::test]
async fn native_run_error_still_closes_once_without_leaking_backend_error() {
    let base = temp_base("run-error");
    let backend = Arc::new(MockBackend::with_behavior(BackendBehavior::Failed));
    let service = BenchmarkService::native(&base, backend.clone()).unwrap();
    let summary = service
        .run(
            manifest("run-error"),
            &BenchmarkPlan::new(vec![task("failed")]),
        )
        .await
        .unwrap();

    assert_eq!(summary.outcomes()[0].status(), TaskStatus::Failed);
    assert_eq!(
        summary.outcomes()[0].failure_category(),
        Some(&benchmark_core::SafeFailureCategory::Backend)
    );
    let state = backend.state.lock().unwrap();
    assert_eq!(state.cancelled, 0);
    assert_eq!(state.closed, 1);
    drop(state);
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn failed_case_is_terminal_and_does_not_block_the_next_case_or_resume() {
    let base = temp_base("continue");
    let backend = Arc::new(MockBackend::with_behavior(BackendBehavior::FailFirst));
    let service = BenchmarkService::native(&base, backend.clone()).unwrap();
    let plan = BenchmarkPlan::new(vec![task("first"), task("second")]);
    let summary = service.run(manifest("run-continue"), &plan).await.unwrap();
    assert_eq!(summary.outcomes().len(), 2);
    assert_eq!(summary.outcomes()[0].status(), TaskStatus::Failed);
    assert_eq!(summary.outcomes()[1].status(), TaskStatus::Completed);
    let calls = backend.state.lock().unwrap().run.len();
    let resumed = service.resume("run-continue", &plan).await.unwrap();
    assert_eq!(resumed.outcomes().len(), 2);
    assert_eq!(backend.state.lock().unwrap().run.len(), calls);
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn private_output_resolution_failure_is_invalid_output_after_close() {
    let base = temp_base("resolve-failed");
    let backend = Arc::new(MockBackend::with_behavior(BackendBehavior::ResolveFailed));
    let service = BenchmarkService::native(&base, backend.clone()).unwrap();
    let summary = service
        .run(
            manifest("run-resolve"),
            &BenchmarkPlan::new(vec![task("one")]),
        )
        .await
        .unwrap();
    assert_eq!(summary.outcomes()[0].status(), TaskStatus::Failed);
    assert_eq!(
        summary.outcomes()[0].failure_category(),
        Some(&benchmark_core::SafeFailureCategory::InvalidOutput)
    );
    assert_eq!(backend.state.lock().unwrap().closed, 1);
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn timeout_cleanup_is_bounded_and_attempts_cancel_and_close() {
    let base = temp_base("bounded-cleanup");
    let backend = Arc::new(MockBackend::with_behavior(BackendBehavior::PendingCleanup));
    let service = BenchmarkService::native(&base, backend.clone()).unwrap();
    let summary = tokio::time::timeout(
        Duration::from_secs(5),
        service.run(
            manifest("run-cleanup"),
            &BenchmarkPlan::new(vec![short_task("slow")]),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(summary.outcomes()[0].status(), TaskStatus::Timeout);
    let state = backend.state.lock().unwrap();
    assert_eq!(state.cancelled, 1);
    assert_eq!(state.closed, 1);
    drop(state);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn external_harness_is_contract_only() {
    let base = temp_base("external");
    let backend = Arc::new(MockBackend::default());
    let service = BenchmarkService::native(&base, backend).unwrap();
    let external = BenchmarkTask::new(
        "external",
        None,
        None,
        ExecutionRequest::external_harness(
            benchmark_core::VerifiedArtifact::new("workspace"),
            "sha256:0123456789abcdef",
            vec!["runner".into()],
            Duration::from_secs(5),
        ),
        None,
    );
    let summary = futures::executor::block_on(
        service.run(manifest("run-1"), &BenchmarkPlan::new(vec![external])),
    )
    .unwrap();
    assert_eq!(summary.outcomes()[0].status(), TaskStatus::Failed);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn report_is_published_without_temporary_files() {
    let base = temp_base("report");
    let store = RunStore::create(&base, &manifest("run-1")).unwrap();
    let artifact =
        benchmark_core::publish_markdown_report(&store, "# Smoke\n\ncompleted: 0\n").unwrap();
    assert_eq!(
        fs::read_to_string(artifact.path()).unwrap(),
        "# Smoke\n\ncompleted: 0\n"
    );
    assert!(!Path::new(&format!("{}.tmp", artifact.path().display())).exists());
    fs::remove_dir_all(base).unwrap();
}
