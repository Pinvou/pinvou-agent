use std::fmt;
use std::fs::File;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentBackendError {
    #[error("agent backend operation failed: {0}")]
    Operation(String),
}

macro_rules! opaque_handle {
    ($name:ident) => {
        #[derive(Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn expose_to_backend(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "([opaque])"))
            }
        }
    };
}

opaque_handle!(AgentSessionHandle);
opaque_handle!(PrivateInputHandle);
opaque_handle!(PrivateOutputHandle);
opaque_handle!(AttachmentHandle);

#[derive(Clone)]
pub struct ResolvedAttachmentSource {
    local_path: PathBuf,
    suggested_name: String,
    verified_bytes: Option<Arc<[u8]>>,
}

const MAX_VERIFIED_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;

impl ResolvedAttachmentSource {
    pub fn new(local_path: impl Into<PathBuf>, suggested_name: impl Into<String>) -> Self {
        Self {
            local_path: local_path.into(),
            suggested_name: suggested_name.into(),
            verified_bytes: None,
        }
    }

    pub fn from_verified_file(
        local_path: impl Into<PathBuf>,
        suggested_name: impl Into<String>,
        mut verified_file: File,
    ) -> Result<Self, AgentBackendError> {
        let metadata = verified_file
            .metadata()
            .map_err(|_| invalid_attachment_capability())?;
        if !metadata.is_file() || metadata.len() > MAX_VERIFIED_ATTACHMENT_BYTES {
            return Err(invalid_attachment_capability());
        }
        verified_file
            .seek(SeekFrom::Start(0))
            .map_err(|_| invalid_attachment_capability())?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        verified_file
            .take(MAX_VERIFIED_ATTACHMENT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid_attachment_capability())?;
        if bytes.len() as u64 != metadata.len()
            || bytes.len() as u64 > MAX_VERIFIED_ATTACHMENT_BYTES
        {
            return Err(invalid_attachment_capability());
        }
        Ok(Self {
            local_path: local_path.into(),
            suggested_name: suggested_name.into(),
            verified_bytes: Some(Arc::from(bytes)),
        })
    }

    pub fn local_path(&self) -> &Path {
        &self.local_path
    }

    pub fn suggested_name(&self) -> &str {
        &self.suggested_name
    }

    pub fn has_verified_file(&self) -> bool {
        self.verified_bytes.is_some()
    }

    pub fn verified_file_size(&self) -> io::Result<Option<u64>> {
        Ok(self.verified_bytes.as_ref().map(|bytes| bytes.len() as u64))
    }

    pub fn try_read_verified_file<T>(
        &self,
        read: impl FnOnce(&mut dyn Read) -> io::Result<T>,
    ) -> io::Result<Option<T>> {
        let Some(bytes) = &self.verified_bytes else {
            return Ok(None);
        };
        let mut reader = Cursor::new(bytes.as_ref());
        read(&mut reader).map(Some)
    }
}

fn invalid_attachment_capability() -> AgentBackendError {
    AgentBackendError::Operation("attachment_capability_invalid".to_owned())
}

impl fmt::Debug for ResolvedAttachmentSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResolvedAttachmentSource([redacted])")
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct SuiteModelIdentity {
    provider: String,
    model: String,
}

impl SuiteModelIdentity {
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, UnsafeSuiteModelIdentity> {
        let provider = validate_identity_component(provider.into(), 64)?;
        let model = validate_identity_component(model.into(), 128)?;
        Ok(Self { provider, model })
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

impl fmt::Debug for SuiteModelIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SuiteModelIdentity")
            .field("provider", &"[validated]")
            .field("model", &"[validated]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AgentToolPolicyId(String);

impl AgentToolPolicyId {
    pub fn new(value: impl Into<String>) -> Result<Self, UnsafeAgentToolPolicyId> {
        validate_identity_component(value.into(), 128)
            .map(Self)
            .map_err(|_| UnsafeAgentToolPolicyId)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AgentToolPolicyId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentToolPolicyId([validated])")
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("unsafe agent tool policy id")]
pub struct UnsafeAgentToolPolicyId;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AgentOutputContractId(String);

impl AgentOutputContractId {
    pub fn new(value: impl Into<String>) -> Result<Self, UnsafeAgentOutputContractId> {
        validate_identity_component(value.into(), 128)
            .map(Self)
            .map_err(|_| UnsafeAgentOutputContractId)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AgentOutputContractId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentOutputContractId([validated])")
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("unsafe agent output contract id")]
pub struct UnsafeAgentOutputContractId;

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("unsafe suite model identity")]
pub struct UnsafeSuiteModelIdentity;

fn validate_identity_component(
    value: String,
    max_chars: usize,
) -> Result<String, UnsafeSuiteModelIdentity> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > max_chars
        || value.chars().any(char::is_control)
        || !value.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | ':' | '/' | '@' | '+' | '-')
        })
    {
        return Err(UnsafeSuiteModelIdentity);
    }
    let normalized = value.to_ascii_lowercase();
    let compact = normalized
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>();
    const CREDENTIAL_FAMILIES: [&str; 11] = [
        "apikey",
        "accesstoken",
        "authtoken",
        "authorization",
        "bearer",
        "basic",
        "clientsecret",
        "cookie",
        "credential",
        "password",
        "privatekey",
    ];
    if CREDENTIAL_FAMILIES
        .iter()
        .any(|marker| compact.contains(marker))
        || ["sk-", "ghp_", "github_pat", "glpat-"]
            .iter()
            .any(|marker| normalized.contains(marker))
        || normalized.starts_with("xox")
        || normalized.starts_with("akia")
        || contains_sensitive_identity_field(&normalized)
        || value
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(looks_like_random_credential)
    {
        return Err(UnsafeSuiteModelIdentity);
    }
    Ok(value.to_owned())
}

fn contains_sensitive_identity_field(value: &str) -> bool {
    let segments = value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    segments.iter().enumerate().any(|(index, segment)| {
        *segment == "secret"
            || (*segment == "token" && segments.get(index + 1).copied() != Some("count"))
    })
}

fn looks_like_random_credential(segment: &str) -> bool {
    if segment.len() < 32
        || !segment.bytes().all(|byte| byte.is_ascii_alphanumeric())
        || segment.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return false;
    }

    let has_lower = segment.bytes().any(|byte| byte.is_ascii_lowercase());
    let has_upper = segment.bytes().any(|byte| byte.is_ascii_uppercase());
    let has_digit = segment.bytes().any(|byte| byte.is_ascii_digit());
    let mut seen = [false; 128];
    for byte in segment.bytes() {
        seen[usize::from(byte)] = true;
    }
    let distinct_chars = seen.into_iter().filter(|seen| *seen).count();

    has_lower && has_upper && has_digit && distinct_chars >= 12
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SafeUsageMetrics {
    input_tokens: u64,
    output_tokens: u64,
    cache_hit_tokens: u64,
    cache_miss_tokens: u64,
}

impl SafeUsageMetrics {
    pub fn new(
        input_tokens: u64,
        output_tokens: u64,
        cache_hit_tokens: u64,
        cache_miss_tokens: u64,
    ) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cache_hit_tokens,
            cache_miss_tokens,
        }
    }

    pub fn input_tokens(self) -> u64 {
        self.input_tokens
    }
    pub fn output_tokens(self) -> u64 {
        self.output_tokens
    }
    pub fn cache_hit_tokens(self) -> u64 {
        self.cache_hit_tokens
    }
    pub fn cache_miss_tokens(self) -> u64 {
        self.cache_miss_tokens
    }
}

#[derive(Clone)]
pub struct SecretText(String);

impl SecretText {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose_to_backend(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretText([redacted])")
    }
}

#[derive(Clone)]
pub struct SecretOutput(SecretText);

impl SecretOutput {
    pub fn new(text: SecretText) -> Self {
        Self(text)
    }

    pub fn text(&self) -> &SecretText {
        &self.0
    }
}

impl fmt::Debug for SecretOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretOutput([redacted])")
    }
}

#[async_trait]
pub trait PrivateOutputResolver: Send + Sync {
    async fn resolve(
        &self,
        handle: &PrivateOutputHandle,
    ) -> Result<SecretOutput, AgentBackendError>;
}

#[derive(Clone)]
pub struct ResolvedPrivateInput {
    prompt: SecretText,
    attachments: Vec<AttachmentHandle>,
}

impl ResolvedPrivateInput {
    pub fn new(prompt: SecretText, attachments: Vec<AttachmentHandle>) -> Self {
        Self {
            prompt,
            attachments,
        }
    }

    pub fn prompt(&self) -> &SecretText {
        &self.prompt
    }

    pub fn attachments(&self) -> &[AttachmentHandle] {
        &self.attachments
    }
}

impl fmt::Debug for ResolvedPrivateInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedPrivateInput")
            .field("prompt", &"[redacted]")
            .field("attachment_count", &self.attachments.len())
            .finish()
    }
}

#[async_trait]
pub trait PrivateInputResolver: Send + Sync {
    async fn resolve(
        &self,
        handle: &PrivateInputHandle,
    ) -> Result<ResolvedPrivateInput, AgentBackendError>;

    async fn resolve_attachment(
        &self,
        _handle: &AttachmentHandle,
    ) -> Result<ResolvedAttachmentSource, AgentBackendError> {
        Err(AgentBackendError::Operation(
            "attachment_resolution_unsupported".to_owned(),
        ))
    }
}

#[derive(Clone)]
pub struct PrepareRequest {
    task_id: String,
    attachments: Vec<AttachmentHandle>,
    resolved_attachments: Vec<ResolvedAttachmentSource>,
    tool_policy: Option<AgentToolPolicyId>,
}

impl PrepareRequest {
    pub fn new(task_id: impl Into<String>, attachments: Vec<AttachmentHandle>) -> Self {
        Self {
            task_id: task_id.into(),
            attachments,
            resolved_attachments: Vec::new(),
            tool_policy: None,
        }
    }

    pub fn with_resolved_attachments(
        mut self,
        resolved_attachments: Vec<ResolvedAttachmentSource>,
    ) -> Self {
        self.resolved_attachments = resolved_attachments;
        self
    }

    pub fn with_tool_policy(mut self, tool_policy: AgentToolPolicyId) -> Self {
        self.tool_policy = Some(tool_policy);
        self
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub fn attachments(&self) -> &[AttachmentHandle] {
        &self.attachments
    }

    pub fn resolved_attachments(&self) -> &[ResolvedAttachmentSource] {
        &self.resolved_attachments
    }

    pub fn tool_policy(&self) -> Option<&AgentToolPolicyId> {
        self.tool_policy.as_ref()
    }
}

impl fmt::Debug for PrepareRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PrepareRequest")
            .field("task_id", &self.task_id)
            .field("attachment_count", &self.attachments.len())
            .field(
                "resolved_attachment_count",
                &self.resolved_attachments.len(),
            )
            .field("tool_policy", &self.tool_policy)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct AgentTaskInput {
    task_id: String,
    prompt: PrivateInputHandle,
    output_contract: Option<AgentOutputContractId>,
}

impl AgentTaskInput {
    pub fn new(task_id: impl Into<String>, prompt: PrivateInputHandle) -> Self {
        Self {
            task_id: task_id.into(),
            prompt,
            output_contract: None,
        }
    }

    pub fn with_output_contract(mut self, output_contract: AgentOutputContractId) -> Self {
        self.output_contract = Some(output_contract);
        self
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub fn prompt_handle(&self) -> &PrivateInputHandle {
        &self.prompt
    }

    pub fn output_contract(&self) -> Option<&AgentOutputContractId> {
        self.output_contract.as_ref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SafeRunStatus {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct AgentTaskOutcome {
    status: SafeRunStatus,
    output: Option<PrivateOutputHandle>,
    elapsed: Duration,
    usage: Option<SafeUsageMetrics>,
}

impl AgentTaskOutcome {
    pub fn completed(elapsed: Duration) -> Self {
        Self {
            status: SafeRunStatus::Completed,
            output: None,
            elapsed,
            usage: None,
        }
    }

    pub fn with_private_output(mut self, output: PrivateOutputHandle) -> Self {
        self.output = Some(output);
        self
    }

    pub fn status(&self) -> SafeRunStatus {
        self.status
    }

    pub fn output_handle(&self) -> Option<&PrivateOutputHandle> {
        self.output.as_ref()
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    pub fn with_usage(mut self, usage: SafeUsageMetrics) -> Self {
        self.usage = Some(usage);
        self
    }

    pub fn usage(&self) -> Option<SafeUsageMetrics> {
        self.usage
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SafeAgentEvent {
    RunStarted {
        task_id: String,
    },
    RunFinished {
        task_id: String,
        status: SafeRunStatus,
        elapsed: Duration,
    },
    ToolFinished {
        task_id: String,
        tool_name: String,
        status: SafeRunStatus,
        elapsed: Duration,
    },
}

impl SafeAgentEvent {
    pub fn run_started(task_id: impl Into<String>) -> Self {
        Self::RunStarted {
            task_id: task_id.into(),
        }
    }

    pub fn tool_finished(
        task_id: impl Into<String>,
        tool_name: impl Into<String>,
        succeeded: bool,
        elapsed: Duration,
    ) -> Self {
        Self::ToolFinished {
            task_id: task_id.into(),
            tool_name: tool_name.into(),
            status: if succeeded {
                SafeRunStatus::Completed
            } else {
                SafeRunStatus::Failed
            },
            elapsed,
        }
    }

    pub fn task_id(&self) -> &str {
        match self {
            Self::RunStarted { task_id }
            | Self::RunFinished { task_id, .. }
            | Self::ToolFinished { task_id, .. } => task_id,
        }
    }

    pub fn tool_name(&self) -> Option<&str> {
        match self {
            Self::ToolFinished { tool_name, .. } => Some(tool_name),
            _ => None,
        }
    }

    pub fn status(&self) -> Option<SafeRunStatus> {
        match self {
            Self::RunStarted { .. } => None,
            Self::RunFinished { status, .. } | Self::ToolFinished { status, .. } => Some(*status),
        }
    }
}

pub trait AgentRunObserver: Send + Sync {
    fn on_event(&self, event: &SafeAgentEvent);
}

#[derive(Debug, Default)]
pub struct NoopAgentRunObserver;

impl AgentRunObserver for NoopAgentRunObserver {
    fn on_event(&self, _event: &SafeAgentEvent) {}
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("agent observer panicked")]
pub struct ObserverPanic;

pub fn notify_observer(
    observer: &dyn AgentRunObserver,
    event: &SafeAgentEvent,
) -> Result<(), ObserverPanic> {
    catch_unwind(AssertUnwindSafe(|| observer.on_event(event))).map_err(|_| ObserverPanic)
}

#[async_trait]
pub trait HeadlessAgentBackend: Send + Sync {
    fn suite_model_identity(&self) -> Option<SuiteModelIdentity> {
        None
    }

    async fn prepare(
        &self,
        request: PrepareRequest,
    ) -> Result<AgentSessionHandle, AgentBackendError>;
    async fn run(
        &self,
        session: &AgentSessionHandle,
        task: AgentTaskInput,
        private_inputs: Arc<dyn PrivateInputResolver>,
        observer: Arc<dyn AgentRunObserver>,
    ) -> Result<AgentTaskOutcome, AgentBackendError>;
    async fn cancel(&self, session: &AgentSessionHandle) -> Result<(), AgentBackendError>;
    async fn resolve_output(
        &self,
        _handle: &PrivateOutputHandle,
    ) -> Result<SecretOutput, AgentBackendError> {
        Err(AgentBackendError::Operation(
            "private_output_resolution_unsupported".to_owned(),
        ))
    }
    async fn close(&self, session: AgentSessionHandle) -> Result<(), AgentBackendError>;
}
