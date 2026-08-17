use std::fs;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use agent_backend_api::{
    AgentBackendError, AgentRunObserver, AgentSessionHandle, AgentTaskInput, AgentTaskOutcome,
    AgentToolPolicyId, AttachmentHandle, HeadlessAgentBackend, NoopAgentRunObserver,
    PrepareRequest, PrivateInputHandle, PrivateInputResolver, PrivateOutputHandle,
    ResolvedAttachmentSource, ResolvedPrivateInput, SafeAgentEvent, SafeRunStatus,
    SafeUsageMetrics, SecretOutput, SecretText, SuiteModelIdentity, notify_observer,
};
use async_trait::async_trait;

struct StubBackend;

#[async_trait]
impl HeadlessAgentBackend for StubBackend {
    fn suite_model_identity(&self) -> Option<SuiteModelIdentity> {
        Some(SuiteModelIdentity::new("openai", "gpt-safe").unwrap())
    }

    async fn prepare(
        &self,
        request: PrepareRequest,
    ) -> Result<AgentSessionHandle, AgentBackendError> {
        Ok(AgentSessionHandle::new(format!(
            "session-{}",
            request.task_id()
        )))
    }

    async fn run(
        &self,
        _session: &AgentSessionHandle,
        task: AgentTaskInput,
        private_inputs: Arc<dyn PrivateInputResolver>,
        observer: Arc<dyn AgentRunObserver>,
    ) -> Result<AgentTaskOutcome, AgentBackendError> {
        let resolved = private_inputs.resolve(task.prompt_handle()).await?;
        assert_eq!(resolved.prompt().expose_to_backend(), "private prompt");
        let _ = notify_observer(
            observer.as_ref(),
            &SafeAgentEvent::run_started(task.task_id().to_owned()),
        );
        Ok(AgentTaskOutcome::completed(Duration::from_millis(7)))
    }

    async fn cancel(&self, _session: &AgentSessionHandle) -> Result<(), AgentBackendError> {
        Ok(())
    }

    async fn resolve_output(
        &self,
        handle: &PrivateOutputHandle,
    ) -> Result<SecretOutput, AgentBackendError> {
        if handle.expose_to_backend() != "output-1" {
            return Err(AgentBackendError::Operation(
                "private_output_not_found".into(),
            ));
        }
        Ok(SecretOutput::new(SecretText::new(
            "PRIVATE_OUTPUT_SENTINEL",
        )))
    }

    async fn close(&self, _session: AgentSessionHandle) -> Result<(), AgentBackendError> {
        Ok(())
    }
}

#[test]
fn suite_model_identity_rejects_unsafe_values() {
    for (provider, model) in [
        ("", "model"),
        ("openai", " "),
        ("open\nai", "model"),
        ("openai", "bearer-private"),
        ("api_key=private", "model"),
        ("api-key-private", "model"),
        ("openai", "access-token-private"),
        ("client_secret_private", "model"),
        ("openai", "authorization-private"),
        ("openai", "cookie-private"),
        ("openai", "basic-private"),
        ("openai", "sk-private"),
        ("openai", "ghp_private"),
        ("openai", "github_pat_private"),
        ("openai", "glpat-private"),
        ("openai", "xoxb-private"),
        ("openai", "AKIAIOSFODNN7EXAMPLE"),
        ("my-secret-key", "model"),
        ("openai", "token:private-value"),
        ("openai", "org/token/private"),
        ("openai", "Ab3dEf5hIj7kLm9nPq2rSt4vWx6yZa8b"),
        ("open ai", "model"),
        ("openai", "model?key=value"),
        ("openai", &"m".repeat(129)),
    ] {
        let error = SuiteModelIdentity::new(provider, model).unwrap_err();
        assert_eq!(error.to_string(), "unsafe suite model identity");
        assert!(!error.to_string().contains("private"));
    }
}

#[test]
fn suite_model_identity_accepts_the_documented_safe_grammar() {
    let identity = SuiteModelIdentity::new(" azure.openai ", " org/model:v1@prod+fast ").unwrap();
    assert_eq!(identity.provider(), "azure.openai");
    assert_eq!(identity.model(), "org/model:v1@prod+fast");

    let identity = SuiteModelIdentity::new("openai", "sky-model").unwrap();
    assert_eq!(identity.model(), "sky-model");

    for model in [
        "tokenizer-v2",
        "org/token-count-v1",
        "0123456789abcdef0123456789abcdef01234567",
    ] {
        assert_eq!(
            SuiteModelIdentity::new("openai", model).unwrap().model(),
            model
        );
    }
}

#[test]
fn agent_tool_policy_id_accepts_the_gaia_offline_policy() {
    let policy = AgentToolPolicyId::new("pinvou-gaia-offline/v1").unwrap();

    assert_eq!(policy.as_str(), "pinvou-gaia-offline/v1");
}

#[test]
fn agent_tool_policy_id_rejects_credentials_controls_and_oversized_values() {
    for value in [
        "token=secret",
        "policy\nname",
        "api-key-private",
        "bearer-private",
        "client_secret_private",
        "credential-private",
        "password-private",
        "cookie-private",
        "authorization-private",
        &"p".repeat(129),
    ] {
        let error = AgentToolPolicyId::new(value).unwrap_err();
        assert_eq!(error.to_string(), "unsafe agent tool policy id");
        assert!(!format!("{error:?}").contains(value));
    }
}

#[test]
fn agent_tool_policy_debug_does_not_expose_credential_sentinel() {
    let credential_sentinel = "PRIVATE_CREDENTIAL_SENTINEL";
    let error = AgentToolPolicyId::new(credential_sentinel).unwrap_err();

    assert!(!format!("{error:?}").contains(credential_sentinel));
}

#[test]
fn prepare_request_defaults_to_no_tool_policy_and_can_opt_in() {
    let legacy = PrepareRequest::new("legacy-task", Vec::new());
    assert_eq!(legacy.tool_policy(), None);

    let policy = AgentToolPolicyId::new("pinvou-gaia-offline/v1").unwrap();
    let request = PrepareRequest::new("gaia-task", Vec::new()).with_tool_policy(policy.clone());
    assert_eq!(request.tool_policy(), Some(&policy));
    assert!(!format!("{request:?}").contains(policy.as_str()));
}

#[test]
fn outcome_exposes_numeric_usage_without_private_text() {
    let usage = SafeUsageMetrics::new(10, 4, 3, 7);
    let outcome = AgentTaskOutcome::completed(Duration::from_millis(1)).with_usage(usage);

    assert_eq!(outcome.usage(), Some(usage));
    assert_eq!(usage.input_tokens(), 10);
    assert_eq!(usage.output_tokens(), 4);
    assert_eq!(usage.cache_hit_tokens(), 3);
    assert_eq!(usage.cache_miss_tokens(), 7);
}

#[test]
fn suite_model_identity_serializes_only_non_sensitive_fields() {
    let backend: Arc<dyn HeadlessAgentBackend> = Arc::new(StubBackend);
    let identity = backend.suite_model_identity().unwrap();
    let json = serde_json::to_string(&identity).unwrap();
    let debug = format!("{identity:?}");

    assert_eq!(identity.provider(), "openai");
    assert_eq!(identity.model(), "gpt-safe");
    assert_eq!(json, r#"{"provider":"openai","model":"gpt-safe"}"#);
    for forbidden in ["api_key", "token", "authorization", "endpoint", "headers"] {
        assert!(!json.to_ascii_lowercase().contains(forbidden));
        assert!(!debug.to_ascii_lowercase().contains(forbidden));
    }
}

struct StubPrivateInputs;

#[async_trait]
impl PrivateInputResolver for StubPrivateInputs {
    async fn resolve(
        &self,
        _handle: &PrivateInputHandle,
    ) -> Result<ResolvedPrivateInput, AgentBackendError> {
        Ok(ResolvedPrivateInput::new(
            SecretText::new("private prompt"),
            Vec::new(),
        ))
    }
}

#[test]
fn backend_contract_supports_full_session_lifecycle() {
    let backend: Arc<dyn HeadlessAgentBackend> = Arc::new(StubBackend);
    let session = futures::executor::block_on(backend.prepare(PrepareRequest::new(
        "task-1",
        vec![AttachmentHandle::new("attachment-1")],
    )))
    .unwrap();
    let outcome = futures::executor::block_on(backend.run(
        &session,
        AgentTaskInput::new("task-1", PrivateInputHandle::new("prompt-1")),
        Arc::new(StubPrivateInputs),
        Arc::new(NoopAgentRunObserver),
    ))
    .unwrap();

    assert_eq!(outcome.status(), SafeRunStatus::Completed);
    futures::executor::block_on(backend.cancel(&session)).unwrap();
    futures::executor::block_on(backend.close(session)).unwrap();
}

#[test]
fn resolved_private_input_debug_is_redacted() {
    let input = ResolvedPrivateInput::new(
        SecretText::new("PRIVATE_PROMPT_SENTINEL"),
        vec![AttachmentHandle::new("PRIVATE_ATTACHMENT_SENTINEL")],
    );
    let debug = format!("{input:?}");
    assert!(!debug.contains("PRIVATE_PROMPT_SENTINEL"));
    assert!(!debug.contains("PRIVATE_ATTACHMENT_SENTINEL"));
}

#[test]
fn resolved_attachment_and_prepare_debug_are_redacted() {
    let source = ResolvedAttachmentSource::new(
        "C:/PRIVATE_ATTACHMENT_PATH/sentinel.txt",
        "PRIVATE_ATTACHMENT_NAME.txt",
    );
    let request = PrepareRequest::new("case-attachment", vec![AttachmentHandle::new("opaque")])
        .with_resolved_attachments(vec![source.clone()]);

    let debug = format!("{source:?} {request:?}");
    assert!(!debug.contains("PRIVATE_ATTACHMENT_PATH"));
    assert!(!debug.contains("PRIVATE_ATTACHMENT_NAME"));
    assert_eq!(request.resolved_attachments().len(), 1);
}

#[test]
fn verified_attachment_capability_reads_open_file_after_path_replacement() {
    let root = std::env::temp_dir().join(format!(
        "pinvou-agent-backend-capability-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let source_path = root.join("source.txt");
    let original_path = root.join("original.txt");
    fs::write(&source_path, b"ORIGINAL_PRIVATE_BYTES").unwrap();
    let verified_file = fs::File::open(&source_path).unwrap();
    let source =
        ResolvedAttachmentSource::from_verified_file(&source_path, "attachment.txt", verified_file)
            .unwrap();

    fs::rename(&source_path, &original_path).unwrap();
    fs::write(&source_path, b"REPLACEMENT_PUBLIC_BYTES").unwrap();
    for _ in 0..2 {
        let mut bytes = Vec::new();
        let read = source
            .try_read_verified_file(|file| file.read_to_end(&mut bytes))
            .unwrap();
        assert_eq!(read, Some("ORIGINAL_PRIVATE_BYTES".len()));
        assert_eq!(bytes, b"ORIGINAL_PRIVATE_BYTES");
    }
    assert!(source.has_verified_file());
    assert!(!format!("{source:?}").contains("ORIGINAL_PRIVATE_BYTES"));
    drop(source);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verified_attachment_capability_freezes_bytes_and_rejects_oversized_files() {
    let root = std::env::temp_dir().join(format!(
        "pinvou-agent-backend-frozen-capability-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let source_path = root.join("source.bin");
    fs::write(&source_path, b"FROZEN_PRIVATE_BYTES").unwrap();
    let source = ResolvedAttachmentSource::from_verified_file(
        &source_path,
        "attachment.bin",
        fs::File::open(&source_path).unwrap(),
    )
    .unwrap();

    fs::write(&source_path, b"MUTATED_IN_PLACE_AND_GROWN").unwrap();
    let mut bytes = Vec::new();
    source
        .try_read_verified_file(|reader| reader.read_to_end(&mut bytes))
        .unwrap()
        .unwrap();
    assert_eq!(bytes, b"FROZEN_PRIVATE_BYTES");
    assert_eq!(
        source.verified_file_size().unwrap(),
        Some(bytes.len() as u64)
    );

    let oversized_path = root.join("oversized.bin");
    fs::File::create(&oversized_path)
        .unwrap()
        .set_len(20 * 1024 * 1024 + 1)
        .unwrap();
    let error = ResolvedAttachmentSource::from_verified_file(
        &oversized_path,
        "oversized.bin",
        fs::File::open(&oversized_path).unwrap(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        AgentBackendError::Operation(ref code) if code == "attachment_capability_invalid"
    ));

    drop(source);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn attachment_resolution_default_is_fixed_and_handle_safe() {
    let resolver = StubPrivateInputs;
    let handle = AttachmentHandle::new("PRIVATE_ATTACHMENT_HANDLE");
    let error = futures::executor::block_on(resolver.resolve_attachment(&handle)).unwrap_err();

    assert_eq!(
        error.to_string(),
        "agent backend operation failed: attachment_resolution_unsupported"
    );
    assert!(!error.to_string().contains(handle.expose_to_backend()));
}

#[test]
fn secret_output_debug_is_redacted_and_resolves_through_backend() {
    let backend: Arc<dyn HeadlessAgentBackend> = Arc::new(StubBackend);
    let output =
        futures::executor::block_on(backend.resolve_output(&PrivateOutputHandle::new("output-1")))
            .unwrap();

    assert_eq!(output.text().expose_to_backend(), "PRIVATE_OUTPUT_SENTINEL");
    assert!(!format!("{output:?}").contains("PRIVATE_OUTPUT_SENTINEL"));
}

#[test]
fn unknown_private_output_has_a_fixed_safe_error() {
    let backend: Arc<dyn HeadlessAgentBackend> = Arc::new(StubBackend);
    let error = futures::executor::block_on(
        backend.resolve_output(&PrivateOutputHandle::new("unknown-private-value")),
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "agent backend operation failed: private_output_not_found"
    );
    assert!(!error.to_string().contains("unknown-private-value"));
}

struct PanickingObserver {
    calls: Arc<AtomicUsize>,
}

impl AgentRunObserver for PanickingObserver {
    fn on_event(&self, _event: &SafeAgentEvent) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        panic!("observer failures must not enter the agent control flow");
    }
}

#[test]
fn observer_panic_is_contained() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observer = PanickingObserver {
        calls: calls.clone(),
    };

    assert!(notify_observer(&observer, &SafeAgentEvent::run_started("task-1")).is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn safe_events_expose_lifecycle_metadata_only() {
    let event =
        SafeAgentEvent::tool_finished("task-1", "web_search", true, Duration::from_millis(12));

    assert_eq!(event.task_id(), "task-1");
    assert_eq!(event.tool_name(), Some("web_search"));
    assert_eq!(event.status(), Some(SafeRunStatus::Completed));
}
