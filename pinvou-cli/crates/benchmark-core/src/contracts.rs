use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_backend_api::SecretOutput;
use agent_backend_api::{AttachmentHandle, PrivateInputHandle};

macro_rules! public_id {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub struct $name(String);
        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

macro_rules! opaque_handle {
    ($name:ident) => {
        #[derive(Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);
        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
            pub fn expose_to_adapter(&self) -> &str {
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

public_id!(BenchmarkId);
public_id!(Split);
public_id!(ToolPolicyId);
public_id!(OutputContract);
public_id!(VerifiedArtifact);
public_id!(ArtifactReference);
opaque_handle!(ReferenceHandle);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionKind {
    NativeTurn,
    ExternalHarness,
}

#[derive(Clone, Debug)]
pub struct BenchmarkDescriptor {
    id: BenchmarkId,
    adapter_version: String,
    dataset_revision: String,
    scorer_revision: String,
    supported_splits: Vec<Split>,
    execution_kind: ExecutionKind,
}

impl BenchmarkDescriptor {
    pub fn new(
        id: BenchmarkId,
        adapter_version: impl Into<String>,
        dataset_revision: impl Into<String>,
        scorer_revision: impl Into<String>,
        supported_splits: Vec<Split>,
        execution_kind: ExecutionKind,
    ) -> Self {
        Self {
            id,
            adapter_version: adapter_version.into(),
            dataset_revision: dataset_revision.into(),
            scorer_revision: scorer_revision.into(),
            supported_splits,
            execution_kind,
        }
    }
    pub fn id(&self) -> &BenchmarkId {
        &self.id
    }
    pub fn adapter_version(&self) -> &str {
        &self.adapter_version
    }
    pub fn dataset_revision(&self) -> &str {
        &self.dataset_revision
    }
    pub fn scorer_revision(&self) -> &str {
        &self.scorer_revision
    }
    pub fn supported_splits(&self) -> &[Split] {
        &self.supported_splits
    }
    pub fn execution_kind(&self) -> ExecutionKind {
        self.execution_kind
    }
}

#[derive(Clone, Debug)]
pub struct BenchmarkTask {
    task_id: String,
    category: Option<String>,
    level: Option<String>,
    execution: ExecutionRequest,
    reference_handle: Option<ReferenceHandle>,
}

impl BenchmarkTask {
    pub fn new(
        task_id: impl Into<String>,
        category: Option<String>,
        level: Option<String>,
        execution: ExecutionRequest,
        reference_handle: Option<ReferenceHandle>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            category,
            level,
            execution,
            reference_handle,
        }
    }
    pub fn task_id(&self) -> &str {
        &self.task_id
    }
    pub fn category(&self) -> Option<&str> {
        self.category.as_deref()
    }
    pub fn level(&self) -> Option<&str> {
        self.level.as_deref()
    }
    pub fn execution(&self) -> &ExecutionRequest {
        &self.execution
    }
    pub fn reference_handle(&self) -> Option<&ReferenceHandle> {
        self.reference_handle.as_ref()
    }
}

#[derive(Clone, Debug)]
pub enum ExecutionRequest {
    NativeTurn {
        prompt_handle: PrivateInputHandle,
        attachments: Vec<AttachmentHandle>,
        timeout: Duration,
        tool_policy: ToolPolicyId,
        output_contract: OutputContract,
    },
    ExternalHarness {
        workspace_archive: VerifiedArtifact,
        container_image_digest: String,
        harness_command: Vec<String>,
        timeout: Duration,
    },
}

impl ExecutionRequest {
    pub fn native_turn(
        prompt_handle: PrivateInputHandle,
        attachments: Vec<AttachmentHandle>,
        timeout: Duration,
        tool_policy: ToolPolicyId,
        output_contract: OutputContract,
    ) -> Self {
        Self::NativeTurn {
            prompt_handle,
            attachments,
            timeout,
            tool_policy,
            output_contract,
        }
    }
    pub fn external_harness(
        workspace_archive: VerifiedArtifact,
        container_image_digest: impl Into<String>,
        harness_command: Vec<String>,
        timeout: Duration,
    ) -> Self {
        Self::ExternalHarness {
            workspace_archive,
            container_image_digest: container_image_digest.into(),
            harness_command,
            timeout,
        }
    }
    pub fn kind(&self) -> ExecutionKind {
        match self {
            Self::NativeTurn { .. } => ExecutionKind::NativeTurn,
            Self::ExternalHarness { .. } => ExecutionKind::ExternalHarness,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Planned,
    Running,
    Completed,
    Failed,
    Timeout,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PredictionRetention {
    Ephemeral,
    DurableUntilPurge,
}

opaque_handle!(PredictionHandle);

#[derive(Clone, Debug)]
pub struct Prediction {
    type_tag: String,
    payload: PredictionHandle,
    origin: PredictionOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PredictionOrigin {
    BackendCapability,
    DurablePrivateStore,
}

impl Prediction {
    pub(crate) fn backend(handle: impl Into<String>) -> Self {
        Self {
            type_tag: "agent-output-handle/v1".into(),
            payload: PredictionHandle::new(handle),
            origin: PredictionOrigin::BackendCapability,
        }
    }
    pub(crate) fn durable(type_tag: impl Into<String>, handle: impl Into<String>) -> Self {
        Self {
            type_tag: type_tag.into(),
            payload: PredictionHandle::new(handle),
            origin: PredictionOrigin::DurablePrivateStore,
        }
    }
    pub fn type_tag(&self) -> &str {
        &self.type_tag
    }
    pub fn payload_handle(&self) -> &PredictionHandle {
        &self.payload
    }
    pub(crate) fn origin(&self) -> PredictionOrigin {
        self.origin
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsageMetrics {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hit_tokens: u64,
    pub cache_miss_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolObservation {
    pub canonical_name: String,
    pub failed: bool,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SafeFailureCategory {
    Backend,
    Timeout,
    InvalidOutput,
    Infrastructure,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SafeFailureReason {
    MissingFinalAnswer,
}

#[derive(Clone, Debug)]
pub struct TaskOutcome {
    task_id: String,
    status: TaskStatus,
    prediction: Option<Prediction>,
    artifacts: Vec<ArtifactReference>,
    usage: Option<UsageMetrics>,
    elapsed_ms: u64,
    trajectory_ref: Option<PathBuf>,
    failure_category: Option<SafeFailureCategory>,
    failure_reason: Option<SafeFailureReason>,
    tool_observations: Vec<ToolObservation>,
    private_output: Option<SecretOutput>,
}
impl TaskOutcome {
    pub fn new(
        task_id: impl Into<String>,
        status: TaskStatus,
        prediction: Option<Prediction>,
        artifacts: Vec<ArtifactReference>,
        elapsed_ms: u64,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            status,
            prediction,
            artifacts,
            usage: None,
            elapsed_ms,
            trajectory_ref: None,
            failure_category: None,
            failure_reason: None,
            tool_observations: Vec::new(),
            private_output: None,
        }
    }
    pub fn task_id(&self) -> &str {
        &self.task_id
    }
    pub fn status(&self) -> TaskStatus {
        self.status
    }
    pub fn prediction(&self) -> Option<&Prediction> {
        self.prediction.as_ref()
    }
    pub fn artifacts(&self) -> &[ArtifactReference] {
        &self.artifacts
    }
    pub fn usage(&self) -> Option<&UsageMetrics> {
        self.usage.as_ref()
    }
    pub fn with_usage(mut self, usage: UsageMetrics) -> Self {
        self.usage = Some(usage);
        self
    }
    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms
    }
    pub fn trajectory_ref(&self) -> Option<&Path> {
        self.trajectory_ref.as_deref()
    }
    pub fn failure_category(&self) -> Option<&SafeFailureCategory> {
        self.failure_category.as_ref()
    }
    pub fn with_failure_category(mut self, category: SafeFailureCategory) -> Self {
        self.failure_category = Some(category);
        self
    }
    pub fn failure_reason(&self) -> Option<SafeFailureReason> {
        self.failure_reason
    }
    pub fn with_failure_reason(mut self, reason: SafeFailureReason) -> Self {
        self.failure_reason = Some(reason);
        self
    }
    pub fn with_tool_observations(mut self, observations: Vec<ToolObservation>) -> Self {
        self.tool_observations = observations;
        self
    }
    pub fn tool_observations(&self) -> &[ToolObservation] {
        &self.tool_observations
    }
    pub fn with_private_output(mut self, output: SecretOutput) -> Self {
        self.private_output = Some(output);
        self
    }
    pub fn private_output(&self) -> Option<&SecretOutput> {
        self.private_output.as_ref()
    }
    pub(crate) fn with_prediction(mut self, prediction: Option<Prediction>) -> Self {
        self.prediction = prediction;
        self
    }
}

#[derive(Clone, Debug)]
pub struct VerifiedDataset {
    id: String,
    root: PathBuf,
}
impl VerifiedDataset {
    pub fn new(id: impl Into<String>, root: impl Into<PathBuf>) -> Self {
        Self {
            id: id.into(),
            root: root.into(),
        }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[derive(Clone, Debug, Default)]
pub struct TaskSelection {
    task_ids: Vec<String>,
}
impl TaskSelection {
    pub fn all() -> Self {
        Self::default()
    }
    pub fn from_task_ids(task_ids: Vec<String>) -> Self {
        Self { task_ids }
    }
    pub fn task_ids(&self) -> &[String] {
        &self.task_ids
    }
}

#[derive(Clone, Debug)]
pub struct BenchmarkPlan {
    tasks: Vec<BenchmarkTask>,
}
impl BenchmarkPlan {
    pub fn new(tasks: Vec<BenchmarkTask>) -> Self {
        Self { tasks }
    }
    pub fn tasks(&self) -> &[BenchmarkTask] {
        &self.tasks
    }
}

#[derive(Clone, Debug)]
pub struct RunContext {
    run_id: String,
    run_root: PathBuf,
}
impl RunContext {
    pub fn new(run_id: impl Into<String>, run_root: PathBuf) -> Self {
        Self {
            run_id: run_id.into(),
            run_root,
        }
    }
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub fn run_root(&self) -> &Path {
        &self.run_root
    }
}

#[derive(Clone, Debug)]
pub struct PreparedTask {
    task: BenchmarkTask,
}
impl PreparedTask {
    pub fn new(task: BenchmarkTask) -> Self {
        Self { task }
    }
    pub fn task(&self) -> &BenchmarkTask {
        &self.task
    }
}

#[derive(Clone, Debug)]
pub struct CompletedRun {
    run_id: String,
    outcomes: Vec<TaskOutcome>,
    scorer_view: Option<crate::ScorerView>,
}
impl CompletedRun {
    pub fn new(run_id: impl Into<String>, outcomes: Vec<TaskOutcome>) -> Self {
        Self {
            run_id: run_id.into(),
            outcomes,
            scorer_view: None,
        }
    }
    pub(crate) fn with_scorer_view(
        run_id: impl Into<String>,
        outcomes: Vec<TaskOutcome>,
        scorer_view: crate::ScorerView,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            outcomes,
            scorer_view: Some(scorer_view),
        }
    }
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub fn outcomes(&self) -> &[TaskOutcome] {
        &self.outcomes
    }
    pub fn resolve_private_prediction(
        &self,
        outcome: &TaskOutcome,
    ) -> crate::Result<crate::PrivatePredictionPayload> {
        if outcome.task_id().is_empty()
            || !self
                .outcomes
                .iter()
                .any(|candidate| std::ptr::eq(candidate, outcome))
        {
            return Err(crate::BenchmarkError::coded(
                "private_prediction_unavailable",
            ));
        }
        self.scorer_view
            .as_ref()
            .ok_or_else(|| crate::BenchmarkError::coded("private_prediction_unavailable"))?
            .resolve(outcome)
            .map_err(|_| crate::BenchmarkError::coded("private_prediction_unavailable"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OfficialScoreReport {
    evaluated: u64,
    correct: u64,
    complete: bool,
    official_dataset_compatible: bool,
    split: String,
    level: String,
}
impl OfficialScoreReport {
    pub fn compatible(evaluated: u64, correct: u64, split: &str, level: &str) -> Self {
        if evaluated == 0 || correct > evaluated {
            return Self::partial(evaluated, correct, split, level);
        }
        Self {
            evaluated,
            correct,
            complete: true,
            official_dataset_compatible: true,
            split: split.into(),
            level: level.into(),
        }
    }
    pub fn partial(evaluated: u64, correct: u64, split: &str, level: &str) -> Self {
        Self {
            evaluated,
            correct: correct.min(evaluated),
            complete: false,
            official_dataset_compatible: false,
            split: split.into(),
            level: level.into(),
        }
    }
    pub fn new(evaluated: u64, correct: u64) -> Self {
        Self::partial(evaluated, correct, "unspecified", "unspecified")
    }
    pub fn evaluated(&self) -> u64 {
        self.evaluated
    }
    pub fn correct(&self) -> u64 {
        self.correct
    }
    pub fn accuracy(&self) -> f64 {
        if self.evaluated == 0 {
            0.0
        } else {
            self.correct as f64 / self.evaluated as f64
        }
    }
    pub fn comparable_accuracy(&self) -> Option<f64> {
        (self.complete && self.official_dataset_compatible && self.evaluated > 0)
            .then(|| self.accuracy())
    }
    pub fn is_complete(&self) -> bool {
        self.complete
    }
    pub fn is_official_dataset_compatible(&self) -> bool {
        self.official_dataset_compatible
    }
    pub fn split(&self) -> &str {
        &self.split
    }
    pub fn level(&self) -> &str {
        &self.level
    }
}

#[derive(Clone)]
pub struct SubmissionArtifact {
    path: PathBuf,
}

impl fmt::Debug for SubmissionArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SubmissionArtifact([redacted])")
    }
}
impl SubmissionArtifact {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}
