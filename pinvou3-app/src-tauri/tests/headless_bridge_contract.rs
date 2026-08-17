#![cfg(feature = "benchmark-hooks")]

use std::sync::{Arc, Mutex};

use agent_backend_api::{
    AgentRunObserver, AgentTaskInput, AgentToolPolicyId, AttachmentHandle, HeadlessAgentBackend,
    PrepareRequest, PrivateInputHandle, PrivateInputResolver, ResolvedAttachmentSource,
    ResolvedPrivateInput, SafeAgentEvent, SafeRunStatus, SecretText,
};
use async_trait::async_trait;
use pinvou3_lib::headless_bridge::{ProductHeadlessBackend, ProductRuntimePort, ProductToolPolicy};

fn prepare_request(task_id: &str, attachments: Vec<AttachmentHandle>) -> PrepareRequest {
    PrepareRequest::new(task_id, attachments).with_tool_policy(
        AgentToolPolicyId::new("pinvou-gaia-offline/v1").expect("valid test policy"),
    )
}

#[derive(Default)]
struct RecordingRuntime {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl ProductRuntimePort for RecordingRuntime {
    async fn prepare(&self, session_id: &str) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("prepare:{session_id}"));
        Ok(())
    }

    async fn run(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> anyhow::Result<pinvou3_lib::headless_bridge::ProductTurnOutcome> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("run:{session_id}:{prompt}"));
        Ok(pinvou3_lib::headless_bridge::ProductTurnOutcome {
            status: "completed".into(),
            assistant_text: "private answer".into(),
            usage: Some(agent_backend_api::SafeUsageMetrics::new(10, 4, 3, 7)),
            tools: vec![pinvou3_lib::headless_bridge::SafeToolOutcome {
                name: "weather".into(),
                failed: false,
            }],
        })
    }

    async fn run_with_policy(
        &self,
        session_id: &str,
        prompt: &str,
        _policy: ProductToolPolicy,
    ) -> anyhow::Result<pinvou3_lib::headless_bridge::ProductTurnOutcome> {
        self.run(session_id, prompt).await
    }

    async fn cancel(&self, session_id: &str) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("cancel:{session_id}"));
        Ok(())
    }

    async fn close(&self, session_id: &str) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("close:{session_id}"));
        Ok(())
    }
}

struct Resolver;

#[async_trait]
impl PrivateInputResolver for Resolver {
    async fn resolve(
        &self,
        _handle: &PrivateInputHandle,
    ) -> Result<ResolvedPrivateInput, agent_backend_api::AgentBackendError> {
        Ok(ResolvedPrivateInput::new(
            SecretText::new("secret prompt"),
            vec![],
        ))
    }
}

#[derive(Default)]
struct Observer(Mutex<Vec<SafeAgentEvent>>);

impl AgentRunObserver for Observer {
    fn on_event(&self, event: &SafeAgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

#[tokio::test]
async fn backend_runs_one_private_task_and_closes_its_session() {
    let runtime = Arc::new(RecordingRuntime::default());
    let backend = ProductHeadlessBackend::from_runtime(runtime.clone());
    let observer = Arc::new(Observer::default());
    let session = backend
        .prepare(prepare_request("case-a", vec![]))
        .await
        .unwrap();
    assert!(!backend.has_staged_attachments(&session));

    let outcome = backend
        .run(
            &session,
            AgentTaskInput::new("case-a", PrivateInputHandle::new("opaque")),
            Arc::new(Resolver),
            observer.clone(),
        )
        .await
        .unwrap();

    assert_eq!(outcome.status(), SafeRunStatus::Completed);
    assert_eq!(outcome.usage().unwrap().input_tokens(), 10);
    let output = outcome.output_handle().unwrap();
    assert_ne!(output.expose_to_backend(), "private answer");
    assert!(output.expose_to_backend().starts_with("output-"));
    let resolved = backend.resolve_output(output).await.unwrap();
    assert_eq!(resolved.text().expose_to_backend(), "private answer");
    assert!(!format!("{resolved:?}").contains("private answer"));
    backend.close(session).await.unwrap();
    let calls = runtime.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(calls[0].starts_with("prepare:eval_case-a_"));
    assert!(calls[1].contains(":secret prompt"));
    assert!(calls[2].starts_with("close:eval_case-a_"));
    let events = observer.0.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[1].tool_name(), Some("weather"));
    assert!(!format!("{events:?}").contains("secret prompt"));
    assert!(!format!("{outcome:?}").contains("private answer"));
}

#[tokio::test]
async fn output_is_not_resolvable_after_session_close() {
    let backend = ProductHeadlessBackend::from_runtime(Arc::new(RecordingRuntime::default()));
    let session = backend
        .prepare(prepare_request("case-output", vec![]))
        .await
        .unwrap();
    let outcome = backend
        .run(
            &session,
            AgentTaskInput::new("case-output", PrivateInputHandle::new("opaque")),
            Arc::new(Resolver),
            Arc::new(Observer::default()),
        )
        .await
        .unwrap();
    let handle = outcome.output_handle().unwrap().clone();
    backend.close(session).await.unwrap();

    let error = backend.resolve_output(&handle).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "agent backend operation failed: private_output_not_found"
    );
    assert!(!error.to_string().contains(handle.expose_to_backend()));
}

struct FailingRuntime;

#[async_trait]
impl ProductRuntimePort for FailingRuntime {
    async fn prepare(&self, _session_id: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn run(
        &self,
        _session_id: &str,
        _prompt: &str,
    ) -> anyhow::Result<pinvou3_lib::headless_bridge::ProductTurnOutcome> {
        anyhow::bail!("private provider failure")
    }
    async fn run_with_policy(
        &self,
        session_id: &str,
        prompt: &str,
        _policy: ProductToolPolicy,
    ) -> anyhow::Result<pinvou3_lib::headless_bridge::ProductTurnOutcome> {
        self.run(session_id, prompt).await
    }
    async fn cancel(&self, _session_id: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn close(&self, _session_id: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn runtime_failure_emits_a_safe_failed_terminal_event() {
    let backend = ProductHeadlessBackend::from_runtime(Arc::new(FailingRuntime));
    let observer = Arc::new(Observer::default());
    let session = backend
        .prepare(prepare_request("case-fail", vec![]))
        .await
        .unwrap();
    let error = backend
        .run(
            &session,
            AgentTaskInput::new("case-fail", PrivateInputHandle::new("opaque")),
            Arc::new(Resolver),
            observer.clone(),
        )
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "agent backend operation failed: run_failed"
    );
    let events = observer.0.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events.last().and_then(SafeAgentEvent::status),
        Some(SafeRunStatus::Failed)
    );
    assert!(!format!("{events:?}").contains("private provider failure"));
}

#[tokio::test]
async fn cancel_delegates_to_the_same_session() {
    let runtime = Arc::new(RecordingRuntime::default());
    let backend = ProductHeadlessBackend::from_runtime(runtime.clone());
    let session = backend
        .prepare(prepare_request("case-b", vec![]))
        .await
        .unwrap();
    backend.cancel(&session).await.unwrap();
    backend.close(session).await.unwrap();
    let calls = runtime.calls.lock().unwrap();
    assert!(calls[1].starts_with("cancel:eval_case-b_"));
    assert!(calls[2].starts_with("close:eval_case-b_"));
}

#[tokio::test]
async fn attachments_are_staged_but_runtime_access_remains_gated() {
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("private.txt");
    std::fs::write(&source, b"private attachment").unwrap();
    let backend = ProductHeadlessBackend::from_runtime(Arc::new(RecordingRuntime::default()));
    let session = backend
        .prepare(
            prepare_request("case-attachment", vec![AttachmentHandle::new("opaque")])
                .with_resolved_attachments(vec![ResolvedAttachmentSource::new(
                    &source,
                    "attachment.txt",
                )]),
        )
        .await
        .unwrap();
    assert!(backend.has_staged_attachments(&session));

    let error = backend
        .run(
            &session,
            AgentTaskInput::new("case-attachment", PrivateInputHandle::new("opaque")),
            Arc::new(AttachmentResolver),
            Arc::new(Observer::default()),
        )
        .await
        .unwrap_err();
    #[cfg(not(windows))]
    assert_eq!(
        error.to_string(),
        "agent backend operation failed: attachments_runtime_unsupported"
    );
    #[cfg(windows)]
    assert_eq!(
        error.to_string(),
        "agent backend operation failed: attachments_platform_security_unsupported"
    );
    assert!(!backend.has_staged_attachments(&session));
}

#[tokio::test]
async fn verified_attachment_capability_stages_original_handle_after_path_replacement() {
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("private.txt");
    let moved = source_dir.path().join("verified-original.txt");
    std::fs::write(&source, b"ORIGINAL_PRIVATE_BYTES").unwrap();
    let verified_file = std::fs::File::open(&source).unwrap();
    let capability =
        ResolvedAttachmentSource::from_verified_file(&source, "attachment.txt", verified_file)
            .unwrap();
    std::fs::write(&source, b"MUTATED_IN_PLACE_AND_GROWN").unwrap();
    std::fs::rename(&source, &moved).unwrap();
    std::fs::write(&source, b"REPLACEMENT_PUBLIC_BYTES").unwrap();

    let backend = ProductHeadlessBackend::from_runtime(Arc::new(RecordingRuntime::default()));
    let session = backend
        .prepare(
            prepare_request(
                "capability-replacement",
                vec![AttachmentHandle::new("opaque")],
            )
            .with_resolved_attachments(vec![capability]),
        )
        .await
        .unwrap();
    let workspace = backend.staged_attachment_workspace(&session).unwrap();
    assert_eq!(
        std::fs::read(workspace.join("attachment.txt")).unwrap(),
        b"ORIGINAL_PRIVATE_BYTES"
    );
    backend.close(session).await.unwrap();
}

#[tokio::test]
async fn verified_attachment_budget_uses_frozen_snapshot_byte_lengths() {
    const MIB: u64 = 1024 * 1024;
    let source_dir = tempfile::tempdir().unwrap();
    let mut sources = Vec::new();
    let mut handles = Vec::new();
    for index in 0..5 {
        let path = source_dir.path().join(format!("large-{index}.bin"));
        std::fs::File::create(&path)
            .unwrap()
            .set_len(20 * MIB)
            .unwrap();
        sources.push(
            ResolvedAttachmentSource::from_verified_file(
                &path,
                format!("large-{index}.bin"),
                std::fs::File::open(&path).unwrap(),
            )
            .unwrap(),
        );
        handles.push(AttachmentHandle::new(format!("large-{index}")));
    }
    let empty_path = source_dir.path().join("empty.bin");
    std::fs::File::create(&empty_path).unwrap();
    sources.push(
        ResolvedAttachmentSource::from_verified_file(
            &empty_path,
            "empty.bin",
            std::fs::File::open(&empty_path).unwrap(),
        )
        .unwrap(),
    );
    handles.push(AttachmentHandle::new("empty"));
    std::fs::write(&empty_path, b"path grew after snapshot").unwrap();

    let backend = ProductHeadlessBackend::from_runtime(Arc::new(RecordingRuntime::default()));
    let accepted = backend
        .prepare(
            prepare_request("exact-total", handles.clone())
                .with_resolved_attachments(sources.clone()),
        )
        .await
        .unwrap();
    backend.close(accepted).await.unwrap();

    let one_path = source_dir.path().join("one.bin");
    std::fs::write(&one_path, b"x").unwrap();
    sources.push(
        ResolvedAttachmentSource::from_verified_file(
            &one_path,
            "one.bin",
            std::fs::File::open(&one_path).unwrap(),
        )
        .unwrap(),
    );
    handles.push(AttachmentHandle::new("one"));
    assert_eq!(
        backend
            .prepare(prepare_request("over-total", handles).with_resolved_attachments(sources),)
            .await
            .unwrap_err()
            .to_string(),
        "agent backend operation failed: attachment_staging_failed"
    );
}

struct AttachmentResolver;

#[async_trait]
impl PrivateInputResolver for AttachmentResolver {
    async fn resolve(
        &self,
        _handle: &PrivateInputHandle,
    ) -> Result<ResolvedPrivateInput, agent_backend_api::AgentBackendError> {
        Ok(ResolvedPrivateInput::new(
            SecretText::new("secret prompt"),
            vec![AttachmentHandle::new("opaque")],
        ))
    }
}

#[tokio::test]
async fn attachment_prepare_rejects_unresolved_unsafe_and_duplicate_sources() {
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("private.txt");
    std::fs::write(&source, b"private attachment").unwrap();

    let requests = vec![
        prepare_request("missing-resolved", vec![AttachmentHandle::new("opaque")]),
        prepare_request("unsafe-name", vec![AttachmentHandle::new("opaque")])
            .with_resolved_attachments(vec![ResolvedAttachmentSource::new(&source, "../escape")]),
        prepare_request(
            "duplicate-name",
            vec![AttachmentHandle::new("one"), AttachmentHandle::new("two")],
        )
        .with_resolved_attachments(vec![
            ResolvedAttachmentSource::new(&source, "same.txt"),
            ResolvedAttachmentSource::new(&source, "same.txt"),
        ]),
        prepare_request("missing-file", vec![AttachmentHandle::new("opaque")])
            .with_resolved_attachments(vec![ResolvedAttachmentSource::new(
                source_dir.path().join("missing.txt"),
                "missing.txt",
            )]),
        prepare_request("directory", vec![AttachmentHandle::new("opaque")])
            .with_resolved_attachments(vec![ResolvedAttachmentSource::new(
                source_dir.path(),
                "directory",
            )]),
    ];

    for request in requests {
        let backend = ProductHeadlessBackend::from_runtime(Arc::new(RecordingRuntime::default()));
        assert_eq!(
            backend.prepare(request).await.unwrap_err().to_string(),
            "agent backend operation failed: attachment_staging_failed"
        );
    }
}

#[tokio::test]
async fn attachment_prepare_rejects_oversized_files() {
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("large.bin");
    let file = std::fs::File::create(&source).unwrap();
    file.set_len(25 * 1024 * 1024 + 1).unwrap();
    let backend = ProductHeadlessBackend::from_runtime(Arc::new(RecordingRuntime::default()));
    let request = prepare_request("large", vec![AttachmentHandle::new("opaque")])
        .with_resolved_attachments(vec![ResolvedAttachmentSource::new(&source, "large.bin")]);

    assert_eq!(
        backend.prepare(request).await.unwrap_err().to_string(),
        "agent backend operation failed: attachment_staging_failed"
    );
}

#[tokio::test]
async fn cancel_and_close_clear_staged_attachments() {
    for cancel_first in [true, false] {
        let source_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("private.txt");
        std::fs::write(&source, b"private attachment").unwrap();
        let backend = ProductHeadlessBackend::from_runtime(Arc::new(RecordingRuntime::default()));
        let session = backend
            .prepare(
                prepare_request("cleanup", vec![AttachmentHandle::new("opaque")])
                    .with_resolved_attachments(vec![ResolvedAttachmentSource::new(
                        &source,
                        "attachment.txt",
                    )]),
            )
            .await
            .unwrap();
        assert!(backend.has_staged_attachments(&session));
        if cancel_first {
            backend.cancel(&session).await.unwrap();
            assert!(!backend.has_staged_attachments(&session));
        }
        backend.close(session).await.unwrap();
    }
}

#[tokio::test]
async fn attachment_prepare_enforces_count_and_total_byte_budgets() {
    let source_dir = tempfile::tempdir().unwrap();
    let small = source_dir.path().join("small.txt");
    std::fs::write(&small, b"small").unwrap();
    let handles = (0..17)
        .map(|index| AttachmentHandle::new(format!("opaque-{index}")))
        .collect::<Vec<_>>();
    let sources = (0..17)
        .map(|index| ResolvedAttachmentSource::new(&small, format!("file-{index}.txt")))
        .collect::<Vec<_>>();
    let backend = ProductHeadlessBackend::from_runtime(Arc::new(RecordingRuntime::default()));
    assert_eq!(
        backend
            .prepare(prepare_request("too-many", handles).with_resolved_attachments(sources))
            .await
            .unwrap_err()
            .to_string(),
        "agent backend operation failed: attachment_staging_failed"
    );

    let mut handles = Vec::new();
    let mut sources = Vec::new();
    for index in 0..5 {
        let path = source_dir.path().join(format!("large-{index}.bin"));
        std::fs::File::create(&path)
            .unwrap()
            .set_len(21 * 1024 * 1024)
            .unwrap();
        handles.push(AttachmentHandle::new(format!("opaque-{index}")));
        sources.push(ResolvedAttachmentSource::new(
            path,
            format!("large-{index}.bin"),
        ));
    }
    assert_eq!(
        backend
            .prepare(prepare_request("too-large-total", handles).with_resolved_attachments(sources))
            .await
            .unwrap_err()
            .to_string(),
        "agent backend operation failed: attachment_staging_failed"
    );
}

#[derive(Default)]
struct BlockingCleanupRuntime {
    cancel_entered: tokio::sync::Notify,
    cancel_release: tokio::sync::Notify,
    close_entered: tokio::sync::Notify,
    close_release: tokio::sync::Notify,
}

#[async_trait]
impl ProductRuntimePort for BlockingCleanupRuntime {
    async fn prepare(&self, _session_id: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        _session_id: &str,
        _prompt: &str,
    ) -> anyhow::Result<pinvou3_lib::headless_bridge::ProductTurnOutcome> {
        Ok(pinvou3_lib::headless_bridge::ProductTurnOutcome {
            status: "completed".into(),
            assistant_text: "private answer".into(),
            usage: None,
            tools: vec![],
        })
    }

    async fn run_with_policy(
        &self,
        session_id: &str,
        prompt: &str,
        _policy: ProductToolPolicy,
    ) -> anyhow::Result<pinvou3_lib::headless_bridge::ProductTurnOutcome> {
        self.run(session_id, prompt).await
    }

    async fn cancel(&self, _session_id: &str) -> anyhow::Result<()> {
        self.cancel_entered.notify_one();
        self.cancel_release.notified().await;
        Ok(())
    }

    async fn close(&self, _session_id: &str) -> anyhow::Result<()> {
        self.close_entered.notify_one();
        self.close_release.notified().await;
        Ok(())
    }
}

#[tokio::test]
async fn cancel_drops_staged_workspace_before_awaiting_runtime() {
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("private.txt");
    std::fs::write(&source, b"private attachment").unwrap();
    let runtime = Arc::new(BlockingCleanupRuntime::default());
    let backend = ProductHeadlessBackend::from_runtime(runtime.clone());
    let session = backend
        .prepare(
            prepare_request("cancel-cleanup", vec![AttachmentHandle::new("opaque")])
                .with_resolved_attachments(vec![ResolvedAttachmentSource::new(
                    &source,
                    "attachment.txt",
                )]),
        )
        .await
        .unwrap();
    let workspace = backend.staged_attachment_workspace(&session).unwrap();
    let task = tokio::spawn({
        let backend = backend.clone();
        let session = session.clone();
        async move { backend.cancel(&session).await }
    });
    runtime.cancel_entered.notified().await;
    assert!(!backend.has_staged_attachments(&session));
    task.abort();
    let _ = task.await;
    assert!(!workspace.exists());
}

#[tokio::test]
async fn close_removes_private_output_before_awaiting_runtime() {
    let runtime = Arc::new(BlockingCleanupRuntime::default());
    let backend = ProductHeadlessBackend::from_runtime(runtime.clone());
    let session = backend
        .prepare(prepare_request("close-cleanup", vec![]))
        .await
        .unwrap();
    let outcome = backend
        .run(
            &session,
            AgentTaskInput::new("close-cleanup", PrivateInputHandle::new("opaque")),
            Arc::new(Resolver),
            Arc::new(Observer::default()),
        )
        .await
        .unwrap();
    let output = outcome.output_handle().unwrap().clone();
    let task = tokio::spawn({
        let backend = backend.clone();
        async move { backend.close(session).await }
    });
    runtime.close_entered.notified().await;
    assert_eq!(
        backend
            .resolve_output(&output)
            .await
            .unwrap_err()
            .to_string(),
        "agent backend operation failed: private_output_not_found"
    );
    task.abort();
}

#[tokio::test]
async fn cancel_removes_private_output_before_awaiting_runtime() {
    let runtime = Arc::new(BlockingCleanupRuntime::default());
    let backend = ProductHeadlessBackend::from_runtime(runtime.clone());
    let session = backend
        .prepare(prepare_request("cancel-output", vec![]))
        .await
        .unwrap();
    let outcome = backend
        .run(
            &session,
            AgentTaskInput::new("cancel-output", PrivateInputHandle::new("opaque")),
            Arc::new(Resolver),
            Arc::new(Observer::default()),
        )
        .await
        .unwrap();
    let output = outcome.output_handle().unwrap().clone();
    let task = tokio::spawn({
        let backend = backend.clone();
        let session = session.clone();
        async move { backend.cancel(&session).await }
    });
    runtime.cancel_entered.notified().await;
    assert_eq!(
        backend
            .resolve_output(&output)
            .await
            .unwrap_err()
            .to_string(),
        "agent backend operation failed: private_output_not_found"
    );
    task.abort();
}

#[cfg(not(windows))]
struct AttachmentAwareRuntime {
    saw_private_bytes: std::sync::atomic::AtomicBool,
}

#[cfg(not(windows))]
#[async_trait]
impl ProductRuntimePort for AttachmentAwareRuntime {
    async fn prepare(&self, _session_id: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        _session_id: &str,
        _prompt: &str,
    ) -> anyhow::Result<pinvou3_lib::headless_bridge::ProductTurnOutcome> {
        anyhow::bail!("ordinary run must not receive attachments")
    }

    async fn run_with_staged_attachments(
        &self,
        _session_id: &str,
        _prompt: &str,
        staged_workspace: &std::path::Path,
    ) -> anyhow::Result<pinvou3_lib::headless_bridge::ProductTurnOutcome> {
        let bytes = std::fs::read(staged_workspace.join("attachment.txt"))?;
        self.saw_private_bytes.store(
            bytes == b"private attachment",
            std::sync::atomic::Ordering::Relaxed,
        );
        Ok(pinvou3_lib::headless_bridge::ProductTurnOutcome {
            status: "completed".into(),
            assistant_text: "attachment answer".into(),
            usage: None,
            tools: vec![],
        })
    }

    async fn run_with_staged_attachments_and_policy(
        &self,
        session_id: &str,
        prompt: &str,
        staged_workspace: &std::path::Path,
        _policy: ProductToolPolicy,
    ) -> anyhow::Result<pinvou3_lib::headless_bridge::ProductTurnOutcome> {
        self.run_with_staged_attachments(session_id, prompt, staged_workspace)
            .await
    }

    async fn cancel(&self, _session_id: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn close(&self, _session_id: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(not(windows))]
#[tokio::test]
async fn staged_attachment_is_passed_to_attachment_runtime_and_then_deleted() {
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("private.txt");
    std::fs::write(&source, b"private attachment").unwrap();
    let runtime = Arc::new(AttachmentAwareRuntime {
        saw_private_bytes: std::sync::atomic::AtomicBool::new(false),
    });
    let backend = ProductHeadlessBackend::from_runtime(runtime.clone());
    let session = backend
        .prepare(
            prepare_request("attachment-runtime", vec![AttachmentHandle::new("opaque")])
                .with_resolved_attachments(vec![ResolvedAttachmentSource::new(
                    &source,
                    "attachment.txt",
                )]),
        )
        .await
        .unwrap();
    let workspace = backend.staged_attachment_workspace(&session).unwrap();

    let outcome = backend
        .run(
            &session,
            AgentTaskInput::new("attachment-runtime", PrivateInputHandle::new("opaque")),
            Arc::new(AttachmentResolver),
            Arc::new(Observer::default()),
        )
        .await
        .unwrap();

    assert_eq!(outcome.status(), SafeRunStatus::Completed);
    assert!(runtime
        .saw_private_bytes
        .load(std::sync::atomic::Ordering::Relaxed));
    assert!(!workspace.exists());
}

#[cfg(windows)]
#[tokio::test]
async fn windows_attachment_runtime_stays_security_gated() {
    let source_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("private.txt");
    std::fs::write(&source, b"private attachment").unwrap();
    let backend = ProductHeadlessBackend::from_runtime(Arc::new(RecordingRuntime::default()));
    let session = backend
        .prepare(
            prepare_request("windows-gate", vec![AttachmentHandle::new("opaque")])
                .with_resolved_attachments(vec![ResolvedAttachmentSource::new(
                    &source,
                    "attachment.txt",
                )]),
        )
        .await
        .unwrap();

    assert_eq!(
        backend
            .run(
                &session,
                AgentTaskInput::new("windows-gate", PrivateInputHandle::new("opaque")),
                Arc::new(AttachmentResolver),
                Arc::new(Observer::default()),
            )
            .await
            .unwrap_err()
            .to_string(),
        "agent backend operation failed: attachments_platform_security_unsupported"
    );
}
