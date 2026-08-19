use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

#[derive(Clone, Debug)]
pub struct RunStore {
    run_dir: PathBuf,
    lock: Arc<Mutex<()>>,
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedToolObservation {
    canonical_name: String,
    failed: bool,
    elapsed_ms: u64,
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
}

impl PersistedOutcome {
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
            });
        }
        outcome
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
            }),
            tool_observations: outcome
                .tool_observations()
                .iter()
                .map(|tool| PersistedToolObservation {
                    canonical_name: tool.canonical_name.clone(),
                    failed: tool.failed,
                    elapsed_ms: tool.elapsed_ms,
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

    pub fn mark_running(&self, task_id: &str) -> Result<()> {
        self.append_event(&RunEvent::new(task_id, RunEventKind::Running))
    }

    pub fn record_outcome(&self, outcome: TaskOutcome) -> Result<()> {
        validate_component(outcome.task_id())?;
        if let Some(prediction) = outcome.prediction() {
            validate_public_prediction(prediction)?;
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
        let bytes =
            serde_json::to_vec(value).map_err(|_| BenchmarkError::coded("serialization_failed"))?;
        if bytes.len() > 16 * 1024 {
            return Err(BenchmarkError::coded("payload_too_large"));
        }
        let _guard = self
            .lock
            .lock()
            .map_err(|_| BenchmarkError::coded("store_lock_poisoned"))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.run_dir.join(file_name))?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.flush()?;
        file.sync_data()?;
        Ok(())
    }

    fn read_json_lines<T: for<'de> Deserialize<'de>>(&self, file_name: &str) -> Result<Vec<T>> {
        let path = self.run_dir.join(file_name);
        if !path.exists() {
            return Ok(Vec::new());
        }
        BufReader::new(File::open(path)?)
            .lines()
            .map(|line| {
                let line = line?;
                serde_json::from_str(&line)
                    .map_err(|_| BenchmarkError::coded("invalid_persisted_record"))
            })
            .collect()
    }
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
}
