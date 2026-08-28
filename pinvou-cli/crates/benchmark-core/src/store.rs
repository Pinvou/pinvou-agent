use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fs2::FileExt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::private_prediction::PrivatePredictionStore;
use crate::security::{ensure_explicit_base, validate_component};
use crate::{
    ArtifactReference, CompletedRun, Prediction, PredictionOrigin, PrivatePredictionContentType,
    PrivatePredictionPayload, Result, ToolObservation, UsageMetrics,
};
use crate::{
    BenchmarkError, RunEvent, RunEventKind, RunManifest, SafeFailureCategory, TaskOutcome,
    TaskStatus,
};

const MANIFEST_FILE: &str = "manifest.json";
const EVENT_FILE: &str = "events.jsonl";
const OUTCOME_FILE: &str = "predictions.jsonl";
const EXECUTION_LOCK_FILE: &str = ".execution.lock";

#[derive(Clone, Debug)]
pub struct RunStore {
    run_dir: PathBuf,
    lock: Arc<Mutex<()>>,
}

#[derive(Debug)]
pub(crate) struct RunExecutionGuard(File);

impl Drop for RunExecutionGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

trait PersistedRecord: DeserializeOwned {
    fn has_supported_schema(&self) -> bool;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedOutcome {
    schema_version: u16,
    task_id: String,
    status: PersistedStatus,
    prediction_type: Option<String>,
    prediction_handle: Option<String>,
    artifacts: Vec<String>,
    usage: Option<UsageMetricsDto>,
    elapsed_ms: u64,
    failure_category: Option<PersistedFailure>,
    #[serde(default)]
    failure_reason: Option<PersistedFailureReason>,
    tool_observations: Vec<PersistedToolObservation>,
    #[serde(default)]
    model_request_observations: Vec<PersistedModelRequestObservation>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedToolObservation {
    canonical_name: String,
    failed: bool,
    elapsed_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure_code: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedModelRequestObservation {
    request_duration_ms: u64,
    ttft_ms: Option<u64>,
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PersistedStatus {
    Planned,
    Running,
    Completed,
    Failed,
    Timeout,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct UsageMetricsDto {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    cache_hit_tokens: u64,
    #[serde(default)]
    cache_miss_tokens: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PersistedFailure {
    Backend,
    Timeout,
    InvalidOutput,
    Infrastructure,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PersistedFailureReason {
    MissingFinalAnswer,
    AgentTurnFailed,
    AgentToolFailed,
    ModelContextLimit,
    ModelRateLimited,
    ModelRequestTimeout,
    ModelTransportFailed,
    ModelProtocolFailed,
    AttachmentResolutionFailed,
    AttachmentStagingFailed,
    BackendPrepareFailed,
    BackendCloseFailed,
    PrivateOutputResolutionFailed,
}

impl PersistedOutcome {
    fn schema_supported(&self) -> bool {
        self.schema_version == 1
    }

    fn into_outcome(self) -> TaskOutcome {
        let status = match self.status {
            PersistedStatus::Planned => TaskStatus::Planned,
            PersistedStatus::Running => TaskStatus::Running,
            PersistedStatus::Completed => TaskStatus::Completed,
            PersistedStatus::Failed => TaskStatus::Failed,
            PersistedStatus::Timeout => TaskStatus::Timeout,
            PersistedStatus::Cancelled => TaskStatus::Cancelled,
        };
        let prediction = self
            .prediction_type
            .zip(self.prediction_handle)
            .map(|(kind, handle)| Prediction::durable(kind, handle));
        let mut outcome = TaskOutcome::new(
            self.task_id,
            status,
            prediction,
            self.artifacts
                .into_iter()
                .map(ArtifactReference::new)
                .collect(),
            self.elapsed_ms,
        )
        .with_tool_observations(
            self.tool_observations
                .into_iter()
                .map(|tool| ToolObservation {
                    canonical_name: tool.canonical_name,
                    failed: tool.failed,
                    elapsed_ms: tool.elapsed_ms,
                    failure_code: tool.failure_code,
                })
                .collect(),
        )
        .with_model_request_observations(
            self.model_request_observations
                .into_iter()
                .map(|metric| crate::ModelRequestObservation {
                    request_duration_ms: metric.request_duration_ms,
                    ttft_ms: metric.ttft_ms,
                    input_tokens: metric.input_tokens,
                    output_tokens: metric.output_tokens,
                })
                .collect(),
        );
        if let Some(usage) = self.usage {
            outcome = outcome.with_usage(UsageMetrics {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_hit_tokens: usage.cache_hit_tokens,
                cache_miss_tokens: usage.cache_miss_tokens,
            });
        }
        if let Some(category) = self.failure_category {
            outcome = outcome.with_failure_category(match category {
                PersistedFailure::Backend => SafeFailureCategory::Backend,
                PersistedFailure::Timeout => SafeFailureCategory::Timeout,
                PersistedFailure::InvalidOutput => SafeFailureCategory::InvalidOutput,
                PersistedFailure::Infrastructure => SafeFailureCategory::Infrastructure,
                PersistedFailure::Cancelled => SafeFailureCategory::Cancelled,
            });
        }
        if let Some(reason) = self.failure_reason {
            outcome = outcome.with_failure_reason(match reason {
                PersistedFailureReason::MissingFinalAnswer => {
                    crate::SafeFailureReason::MissingFinalAnswer
                }
                PersistedFailureReason::AgentTurnFailed => {
                    crate::SafeFailureReason::AgentTurnFailed
                }
                PersistedFailureReason::AgentToolFailed => {
                    crate::SafeFailureReason::AgentToolFailed
                }
                PersistedFailureReason::ModelContextLimit => {
                    crate::SafeFailureReason::ModelContextLimit
                }
                PersistedFailureReason::ModelRateLimited => {
                    crate::SafeFailureReason::ModelRateLimited
                }
                PersistedFailureReason::ModelRequestTimeout => {
                    crate::SafeFailureReason::ModelRequestTimeout
                }
                PersistedFailureReason::ModelTransportFailed => {
                    crate::SafeFailureReason::ModelTransportFailed
                }
                PersistedFailureReason::ModelProtocolFailed => {
                    crate::SafeFailureReason::ModelProtocolFailed
                }
                PersistedFailureReason::AttachmentResolutionFailed => {
                    crate::SafeFailureReason::AttachmentResolutionFailed
                }
                PersistedFailureReason::AttachmentStagingFailed => {
                    crate::SafeFailureReason::AttachmentStagingFailed
                }
                PersistedFailureReason::BackendPrepareFailed => {
                    crate::SafeFailureReason::BackendPrepareFailed
                }
                PersistedFailureReason::BackendCloseFailed => {
                    crate::SafeFailureReason::BackendCloseFailed
                }
                PersistedFailureReason::PrivateOutputResolutionFailed => {
                    crate::SafeFailureReason::PrivateOutputResolutionFailed
                }
            });
        }
        outcome
    }
}

impl PersistedRecord for PersistedOutcome {
    fn has_supported_schema(&self) -> bool {
        self.schema_supported()
    }
}

impl PersistedRecord for RunEvent {
    fn has_supported_schema(&self) -> bool {
        self.schema_supported()
    }
}

impl From<&TaskOutcome> for PersistedOutcome {
    fn from(outcome: &TaskOutcome) -> Self {
        Self {
            schema_version: 1,
            task_id: outcome.task_id().into(),
            status: match outcome.status() {
                TaskStatus::Planned => PersistedStatus::Planned,
                TaskStatus::Running => PersistedStatus::Running,
                TaskStatus::Completed => PersistedStatus::Completed,
                TaskStatus::Failed => PersistedStatus::Failed,
                TaskStatus::Timeout => PersistedStatus::Timeout,
                TaskStatus::Cancelled => PersistedStatus::Cancelled,
            },
            prediction_type: outcome.prediction().map(|value| value.type_tag().into()),
            prediction_handle: outcome
                .prediction()
                .map(|value| value.payload_handle().expose_to_adapter().into()),
            artifacts: outcome
                .artifacts()
                .iter()
                .map(|value| value.as_str().into())
                .collect(),
            usage: outcome.usage().map(|usage: &UsageMetrics| UsageMetricsDto {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_hit_tokens: usage.cache_hit_tokens,
                cache_miss_tokens: usage.cache_miss_tokens,
            }),
            elapsed_ms: outcome.elapsed_ms(),
            failure_category: outcome.failure_category().map(|category| match category {
                SafeFailureCategory::Backend => PersistedFailure::Backend,
                SafeFailureCategory::Timeout => PersistedFailure::Timeout,
                SafeFailureCategory::InvalidOutput => PersistedFailure::InvalidOutput,
                SafeFailureCategory::Infrastructure => PersistedFailure::Infrastructure,
                SafeFailureCategory::Cancelled => PersistedFailure::Cancelled,
            }),
            failure_reason: outcome.failure_reason().map(|reason| match reason {
                crate::SafeFailureReason::MissingFinalAnswer => {
                    PersistedFailureReason::MissingFinalAnswer
                }
                crate::SafeFailureReason::AgentTurnFailed => {
                    PersistedFailureReason::AgentTurnFailed
                }
                crate::SafeFailureReason::AgentToolFailed => {
                    PersistedFailureReason::AgentToolFailed
                }
                crate::SafeFailureReason::ModelContextLimit => {
                    PersistedFailureReason::ModelContextLimit
                }
                crate::SafeFailureReason::ModelRateLimited => {
                    PersistedFailureReason::ModelRateLimited
                }
                crate::SafeFailureReason::ModelRequestTimeout => {
                    PersistedFailureReason::ModelRequestTimeout
                }
                crate::SafeFailureReason::ModelTransportFailed => {
                    PersistedFailureReason::ModelTransportFailed
                }
                crate::SafeFailureReason::ModelProtocolFailed => {
                    PersistedFailureReason::ModelProtocolFailed
                }
                crate::SafeFailureReason::AttachmentResolutionFailed => {
                    PersistedFailureReason::AttachmentResolutionFailed
                }
                crate::SafeFailureReason::AttachmentStagingFailed => {
                    PersistedFailureReason::AttachmentStagingFailed
                }
                crate::SafeFailureReason::BackendPrepareFailed => {
                    PersistedFailureReason::BackendPrepareFailed
                }
                crate::SafeFailureReason::BackendCloseFailed => {
                    PersistedFailureReason::BackendCloseFailed
                }
                crate::SafeFailureReason::PrivateOutputResolutionFailed => {
                    PersistedFailureReason::PrivateOutputResolutionFailed
                }
            }),
            tool_observations: outcome
                .tool_observations()
                .iter()
                .map(|tool| PersistedToolObservation {
                    canonical_name: tool.canonical_name.clone(),
                    failed: tool.failed,
                    elapsed_ms: tool.elapsed_ms,
                    failure_code: tool.failure_code.clone(),
                })
                .collect(),
            model_request_observations: outcome
                .model_request_observations()
                .iter()
                .map(|metric| PersistedModelRequestObservation {
                    request_duration_ms: metric.request_duration_ms,
                    ttft_ms: metric.ttft_ms,
                    input_tokens: metric.input_tokens,
                    output_tokens: metric.output_tokens,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveredRun {
    completed: Vec<String>,
    runnable: Vec<String>,
}

impl RecoveredRun {
    pub fn completed_task_ids(&self) -> &[String] {
        &self.completed
    }

    pub fn runnable_task_ids(&self) -> &[String] {
        &self.runnable
    }
}

impl RunStore {
    pub fn create(base: &Path, manifest: &RunManifest) -> Result<Self> {
        ensure_explicit_base(base)?;
        manifest.validate()?;
        fs::create_dir_all(base)?;
        let base = base.canonicalize()?;
        let runs_dir = base.join("eval").join("runs");
        fs::create_dir_all(&runs_dir)?;
        let run_dir = runs_dir.join(manifest.run_id());
        match fs::create_dir(&run_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(BenchmarkError::coded("run_exists"));
            }
            Err(error) => return Err(error.into()),
        }
        let store = Self {
            run_dir,
            lock: Arc::new(Mutex::new(())),
        };
        store.publish_new_json(MANIFEST_FILE, manifest)?;
        Ok(store)
    }

    pub fn open(base: &Path, run_id: &str) -> Result<Self> {
        ensure_explicit_base(base)?;
        validate_component(run_id)?;
        let base = base.canonicalize()?;
        let run_dir = base.join("eval").join("runs").join(run_id);
        if !run_dir.is_dir() || !run_dir.join(MANIFEST_FILE).is_file() {
            return Err(BenchmarkError::coded("run_not_found"));
        }
        Ok(Self {
            run_dir,
            lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.run_dir.join(MANIFEST_FILE)
    }

    pub fn run_dir(&self) -> &Path {
        &self.run_dir
    }

    pub(crate) fn claim_execution(&self) -> Result<RunExecutionGuard> {
        let lock = open_execution_lock(&self.run_dir.join(EXECUTION_LOCK_FILE))?;
        match FileExt::try_lock_exclusive(&lock) {
            Ok(()) => {}
            Err(error) if error.kind() == fs2::lock_contended_error().kind() => {
                return Err(BenchmarkError::coded("run_in_progress"));
            }
            Err(error) => return Err(error.into()),
        }
        let guard = RunExecutionGuard(lock);
        self.repair_torn_jsonl_tail::<RunEvent>(EVENT_FILE)?;
        self.repair_torn_jsonl_tail::<PersistedOutcome>(OUTCOME_FILE)?;
        Ok(guard)
    }

    pub fn read_manifest(&self) -> Result<RunManifest> {
        let manifest = serde_json::from_reader(File::open(self.manifest_path())?)
            .map_err(|_| BenchmarkError::coded("invalid_manifest"))?;
        Ok(manifest)
    }

    pub fn plan_tasks<'a>(&self, task_ids: impl IntoIterator<Item = &'a str>) -> Result<()> {
        for task_id in task_ids {
            validate_component(task_id)?;
            self.append_event(&RunEvent::new(task_id, RunEventKind::Planned))?;
        }
        Ok(())
    }

    /// 将重新生成的固定计划与已持久化计划对齐。规划阶段中断可能只留下前缀；
    /// resume 会补写缺失任务，但拒绝磁盘上不属于当前固定计划的任务。
    pub fn reconcile_planned_tasks<'a>(
        &self,
        task_ids: impl IntoIterator<Item = &'a str>,
    ) -> Result<()> {
        let expected = task_ids
            .into_iter()
            .map(|task_id| {
                validate_component(task_id)?;
                Ok(task_id.to_owned())
            })
            .collect::<Result<Vec<_>>>()?;
        let expected_set: BTreeSet<&str> = expected.iter().map(String::as_str).collect();
        if expected_set.len() != expected.len() {
            return Err(BenchmarkError::coded("duplicate_planned_task"));
        }

        let persisted: BTreeSet<String> = self
            .read_json_lines::<RunEvent>(EVENT_FILE)?
            .into_iter()
            .filter(|event| event.kind() == RunEventKind::Planned)
            .map(|event| event.task_id().to_owned())
            .collect();
        if persisted
            .iter()
            .any(|task_id| !expected_set.contains(task_id.as_str()))
        {
            return Err(BenchmarkError::coded("resume_plan_mismatch"));
        }
        self.plan_tasks(
            expected
                .iter()
                .filter(|task_id| !persisted.contains(task_id.as_str()))
                .map(String::as_str),
        )
    }

    pub fn mark_running(&self, task_id: &str) -> Result<()> {
        self.append_event(&RunEvent::new(task_id, RunEventKind::Running))
    }

    pub fn record_outcome(&self, outcome: TaskOutcome) -> Result<()> {
        validate_component(outcome.task_id())?;
        if let Some(prediction) = outcome.prediction() {
            validate_public_prediction(prediction)?;
        }
        if self
            .read_json_lines::<PersistedOutcome>(OUTCOME_FILE)?
            .iter()
            .any(|persisted| persisted.task_id == outcome.task_id())
        {
            return Err(BenchmarkError::coded("duplicate_outcome"));
        }
        self.append_json_line(OUTCOME_FILE, &PersistedOutcome::from(&outcome))
    }

    pub(crate) fn persist_private_prediction(
        &self,
        outcome: &TaskOutcome,
        content_type: PrivatePredictionContentType,
    ) -> Result<Prediction> {
        let private_output = outcome
            .private_output()
            .ok_or_else(|| BenchmarkError::coded("private_prediction_unavailable"))?;
        let prediction_type = content_type.type_tag();
        let manifest = self.read_manifest()?;
        let store = PrivatePredictionStore::create(&self.run_dir, manifest.run_id())?;
        let payload = match content_type {
            PrivatePredictionContentType::Utf8TextV1 => {
                PrivatePredictionPayload::utf8(private_output.text().expose_to_backend())?
            }
            PrivatePredictionContentType::CanonicalJsonV1 => {
                PrivatePredictionPayload::canonical_json(
                    private_output
                        .text()
                        .expose_to_backend()
                        .as_bytes()
                        .to_vec(),
                )?
            }
        };
        let handle = store.put(outcome.task_id(), prediction_type, payload)?;
        Ok(Prediction::durable(
            prediction_type,
            handle.expose_to_adapter(),
        ))
    }

    pub fn read_outcomes(&self) -> Result<Vec<TaskOutcome>> {
        Ok(self
            .read_json_lines::<PersistedOutcome>(OUTCOME_FILE)?
            .into_iter()
            .map(PersistedOutcome::into_outcome)
            .collect())
    }

    pub fn completed_run(&self) -> Result<CompletedRun> {
        let manifest = self.read_manifest()?;
        let outcomes = self.read_outcomes()?;
        if !outcomes
            .iter()
            .any(|outcome| outcome.prediction().is_some())
        {
            return Ok(CompletedRun::new(manifest.run_id(), outcomes));
        }
        let store = PrivatePredictionStore::create(&self.run_dir, manifest.run_id())?;
        Ok(CompletedRun::with_scorer_view(
            manifest.run_id(),
            outcomes,
            store.scorer_view(),
        ))
    }

    pub fn mark_completed(&self, task_id: &str) -> Result<()> {
        self.mark_terminal(task_id, RunEventKind::Completed)
    }
    pub fn mark_failed(&self, task_id: &str) -> Result<()> {
        self.mark_terminal(task_id, RunEventKind::Failed)
    }
    pub fn mark_timeout(&self, task_id: &str) -> Result<()> {
        self.mark_terminal(task_id, RunEventKind::Timeout)
    }
    fn mark_terminal(&self, task_id: &str, kind: RunEventKind) -> Result<()> {
        let outcomes = self.read_json_lines::<PersistedOutcome>(OUTCOME_FILE)?;
        if !outcomes.iter().any(|value| value.task_id == task_id) {
            return Err(BenchmarkError::coded("outcome_not_durable"));
        }
        self.append_event(&RunEvent::new(task_id, kind))
    }

    pub fn recover(&self) -> Result<RecoveredRun> {
        let outcome_records = self.read_json_lines::<PersistedOutcome>(OUTCOME_FILE)?;
        let outcomes: BTreeSet<String> = outcome_records
            .iter()
            .map(|outcome| outcome.task_id.clone())
            .collect();
        let successful: BTreeSet<String> = outcome_records
            .into_iter()
            .filter(|outcome| matches!(outcome.status, PersistedStatus::Completed))
            .map(|outcome| outcome.task_id)
            .collect();
        let events = self.read_json_lines::<RunEvent>(EVENT_FILE)?;
        let mut planned = Vec::new();
        let mut latest = BTreeMap::new();
        for event in events {
            validate_component(event.task_id())?;
            if event.kind() == RunEventKind::Planned && !latest.contains_key(event.task_id()) {
                planned.push(event.task_id().to_owned());
            }
            if event.kind().is_terminal() && !outcomes.contains(event.task_id()) {
                return Err(BenchmarkError::coded("terminal_without_outcome"));
            }
            latest.insert(event.task_id().to_owned(), event.kind());
        }
        let completed: Vec<String> = planned
            .iter()
            .filter(|task_id| successful.contains(task_id.as_str()))
            .cloned()
            .collect();
        let runnable = planned
            .into_iter()
            .filter(|task_id| !outcomes.contains(task_id))
            .collect();
        Ok(RecoveredRun {
            completed,
            runnable,
        })
    }

    pub(crate) fn append_event(&self, event: &RunEvent) -> Result<()> {
        self.append_json_line(EVENT_FILE, event)
    }

    pub(crate) fn publish_new_bytes(&self, file_name: &str, bytes: &[u8]) -> Result<PathBuf> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| BenchmarkError::coded("store_lock_poisoned"))?;
        let destination = self.run_dir.join(file_name);
        let temporary = self.run_dir.join(format!("{file_name}.tmp"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        match fs::hard_link(&temporary, &destination) {
            Ok(()) => {
                fs::remove_file(&temporary)?;
                Ok(destination)
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    Err(BenchmarkError::coded("artifact_exists"))
                } else {
                    Err(error.into())
                }
            }
        }
    }

    fn publish_new_json<T: Serialize>(&self, file_name: &str, value: &T) -> Result<PathBuf> {
        let bytes = serde_json::to_vec_pretty(value)
            .map_err(|_| BenchmarkError::coded("serialization_failed"))?;
        self.publish_new_bytes(file_name, &bytes)
    }

    fn append_json_line<T: Serialize>(&self, file_name: &str, value: &T) -> Result<()> {
        let mut bytes =
            serde_json::to_vec(value).map_err(|_| BenchmarkError::coded("serialization_failed"))?;
        if bytes.len() > 16 * 1024 {
            return Err(BenchmarkError::coded("payload_too_large"));
        }
        bytes.push(b'\n');
        let _guard = self
            .lock
            .lock()
            .map_err(|_| BenchmarkError::coded("store_lock_poisoned"))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.run_dir.join(file_name))?;
        file.write_all(&bytes)?;
        file.flush()?;
        file.sync_data()?;
        Ok(())
    }

    fn read_json_lines<T: PersistedRecord>(&self, file_name: &str) -> Result<Vec<T>> {
        let path = self.run_dir.join(file_name);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let mut reader = BufReader::new(File::open(path)?);
        let mut records = Vec::new();
        loop {
            let mut line = Vec::new();
            let read = reader.read_until(b'\n', &mut line)?;
            if read == 0 {
                break;
            }
            let terminated = line.last() == Some(&b'\n');
            trim_jsonl_terminator(&mut line);
            match serde_json::from_slice::<T>(&line) {
                Ok(record) if record.has_supported_schema() => records.push(record),
                Ok(_) => return Err(BenchmarkError::coded("invalid_persisted_record")),
                Err(_) if !terminated => break,
                Err(_) => return Err(BenchmarkError::coded("invalid_persisted_record")),
            }
        }
        Ok(records)
    }

    fn repair_torn_jsonl_tail<T: PersistedRecord>(&self, file_name: &str) -> Result<()> {
        let path = self.run_dir.join(file_name);
        if !path.exists() {
            return Ok(());
        }
        let file = OpenOptions::new().read(true).write(true).open(&path)?;
        let mut reader = BufReader::new(file.try_clone()?);
        let mut record_start = 0_u64;
        loop {
            let mut line = Vec::new();
            let read = reader.read_until(b'\n', &mut line)?;
            if read == 0 {
                return Ok(());
            }
            let terminated = line.last() == Some(&b'\n');
            trim_jsonl_terminator(&mut line);
            match serde_json::from_slice::<T>(&line) {
                Ok(record) if !record.has_supported_schema() => {
                    return Err(BenchmarkError::coded("invalid_persisted_record"));
                }
                Ok(_) if !terminated => {
                    let mut append = OpenOptions::new().append(true).open(&path)?;
                    append.write_all(b"\n")?;
                    append.sync_data()?;
                    return Ok(());
                }
                Ok(_) => {}
                Err(_) if !terminated => {
                    file.set_len(record_start)?;
                    file.sync_data()?;
                    return Ok(());
                }
                Err(_) => return Err(BenchmarkError::coded("invalid_persisted_record")),
            }
            record_start = record_start
                .checked_add(read as u64)
                .ok_or_else(|| BenchmarkError::coded("invalid_persisted_record"))?;
        }
    }
}

fn trim_jsonl_terminator(line: &mut Vec<u8>) {
    if line.last() == Some(&b'\n') {
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
    }
}

#[cfg(unix)]
fn open_execution_lock(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(Into::into)
}

#[cfg(not(unix))]
fn open_execution_lock(path: &Path) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(Into::into)
}

fn valid_core_prediction_handle(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn validate_public_prediction(prediction: &Prediction) -> Result<()> {
    if prediction.origin() != PredictionOrigin::DurablePrivateStore
        || !matches!(prediction.type_tag(), "utf8-text/v1" | "canonical-json/v1")
        || !valid_core_prediction_handle(prediction.payload_handle().expose_to_adapter())
    {
        return Err(BenchmarkError::coded("unsafe_public_prediction"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store(name: &str) -> (PathBuf, RunStore) {
        let base = std::env::temp_dir().join(format!(
            "pinvou-run-store-{name}-{:016x}",
            rand::random::<u64>()
        ));
        fs::create_dir(&base).expect("create test base");
        let descriptor = crate::BenchmarkDescriptor::new(
            crate::BenchmarkId::new("smoke"),
            "smoke/v1",
            "dataset/v1",
            "scorer/v1",
            vec![crate::Split::new("smoke")],
            crate::ExecutionKind::NativeTurn,
        );
        let manifest = crate::RunManifest::new(
            format!("run-{name}"),
            &descriptor,
            crate::Split::new("smoke"),
            crate::ModelIdentity::new("fixture", "model").unwrap(),
            crate::ToolPolicyId::new("smoke/v1"),
            1,
        )
        .unwrap();
        let store = RunStore::create(&base, &manifest).expect("create run store");
        (base, store)
    }

    #[test]
    fn public_persistence_accepts_only_registered_core_predictions() {
        let backend = Prediction::backend("BACKEND_HANDLE_SENTINEL");
        assert_eq!(
            validate_public_prediction(&backend).unwrap_err().code(),
            "unsafe_public_prediction"
        );
        let forged_type = Prediction::durable("answer/v1", "a".repeat(64));
        assert_eq!(
            validate_public_prediction(&forged_type).unwrap_err().code(),
            "unsafe_public_prediction"
        );
        let forged_handle = Prediction::durable("utf8-text/v1", "PRIVATE_ANSWER_SENTINEL");
        assert_eq!(
            validate_public_prediction(&forged_handle)
                .unwrap_err()
                .code(),
            "unsafe_public_prediction"
        );
        validate_public_prediction(&Prediction::durable("utf8-text/v1", "a".repeat(64)))
            .expect("registered core prediction");
        validate_public_prediction(&Prediction::durable("canonical-json/v1", "b".repeat(64)))
            .expect("registered canonical json prediction");
    }

    #[test]
    fn execution_lock_is_process_wide_and_released_with_the_guard() {
        let (base, store) = test_store("execution-lock");
        let guard = store.claim_execution().expect("claim first execution");
        let reopened = RunStore::open(&base, "run-execution-lock").unwrap();
        assert_eq!(
            reopened.claim_execution().unwrap_err().code(),
            "run_in_progress"
        );
        drop(guard);
        reopened
            .claim_execution()
            .expect("execution lock released after guard drop");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn resume_reconciles_a_partially_persisted_plan() {
        let (base, store) = test_store("reconcile-plan");
        store.plan_tasks(["first"]).unwrap();

        store
            .reconcile_planned_tasks(["first", "second", "third"])
            .unwrap();

        assert_eq!(
            store.recover().unwrap().runnable_task_ids(),
            &["first", "second", "third"]
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn resume_rejects_tasks_outside_the_regenerated_plan() {
        let (base, store) = test_store("reconcile-plan-mismatch");
        store.plan_tasks(["unexpected"]).unwrap();

        assert_eq!(
            store
                .reconcile_planned_tasks(["expected"])
                .unwrap_err()
                .code(),
            "resume_plan_mismatch"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn torn_unterminated_tail_is_ignored_then_truncated_before_resume() {
        let (base, store) = test_store("torn-tail");
        store.plan_tasks(["planned"]).unwrap();
        let path = store.run_dir().join(EVENT_FILE);
        let intact = fs::read(&path).unwrap();
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(br#"{"schema_version":1,"task_id":"torn""#)
            .unwrap();

        assert_eq!(store.recover().unwrap().runnable_task_ids(), &["planned"]);
        let guard = store.claim_execution().expect("repair torn tail");
        assert_eq!(fs::read(&path).unwrap(), intact);
        drop(guard);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn complete_unterminated_record_is_preserved_and_terminated_before_resume() {
        let (base, store) = test_store("complete-tail");
        let path = store.run_dir().join(EVENT_FILE);
        let event = serde_json::to_vec(&RunEvent::new("planned", RunEventKind::Planned)).unwrap();
        fs::write(&path, &event).unwrap();

        let guard = store.claim_execution().expect("normalize complete tail");
        let repaired = fs::read(&path).unwrap();
        assert_eq!(repaired.strip_suffix(b"\n"), Some(event.as_slice()));
        drop(guard);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn unsupported_persisted_schema_is_never_treated_as_a_torn_tail() {
        let (base, store) = test_store("schema");
        fs::write(
            store.run_dir().join(EVENT_FILE),
            b"{\"schema_version\":2,\"task_id\":\"planned\",\"kind\":\"planned\"}\n",
        )
        .unwrap();

        assert_eq!(
            store.recover().unwrap_err().code(),
            "invalid_persisted_record"
        );
        assert_eq!(
            store.claim_execution().unwrap_err().code(),
            "invalid_persisted_record"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn duplicate_task_outcomes_are_rejected_at_the_store_boundary() {
        let (base, store) = test_store("duplicate-outcome");
        let outcome = TaskOutcome::new("task", TaskStatus::Completed, None, vec![], 1);
        store.record_outcome(outcome.clone()).unwrap();
        assert_eq!(
            store.record_outcome(outcome).unwrap_err().code(),
            "duplicate_outcome"
        );
        fs::remove_dir_all(base).unwrap();
    }
}
