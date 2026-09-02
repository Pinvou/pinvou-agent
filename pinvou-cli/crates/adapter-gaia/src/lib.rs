#[cfg(test)]
mod contract_tests;
mod dataset;
mod fetch;
mod private_inputs;
mod scorer;
mod submission;

use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agent_backend_api::{AttachmentHandle, PrivateInputHandle};
use benchmark_core::{
    BenchmarkAdapter, BenchmarkDescriptor, BenchmarkError, BenchmarkId, BenchmarkPlan,
    BenchmarkTask, CompletedRun, ExecutionKind, ExecutionRequest, OfficialScoreReport,
    OutputContract, PredictionRetention, PreparedTask, PrivatePredictionContentType,
    ReferenceHandle, Result as BenchmarkResult, RunContext, Split, SubmissionArtifact,
    TaskSelection, ToolPolicyId, VerifiedDataset,
};

pub use dataset::{GAIA_REVISION_MARKER, GaiaAttachment, GaiaDataset, GaiaDatasetError, GaiaRow};
pub use fetch::{
    GAIA_READY_MARKER, GaiaAcquisition, GaiaFetchError, GaiaSnapshotManager, GaiaSource,
    HfSnapshotDownloader, SnapshotDownloadRequest, SnapshotDownloader, SnapshotFileMetadata,
    SnapshotPreflightRequest,
};
pub use private_inputs::GaiaPrivateInputs;
pub use scorer::{GAIA_SCORER_RUNTIME_PROFILE, question_scorer};

pub const GAIA_DATASET_REVISION: &str = "682dd723ee1e1697e00360edccf2366dc8418dd9";
pub const GAIA_SCORER_REVISION: &str = "1349a17979f0aca0ee9c46cd7ec26eb2fb41102e";
pub const GAIA_ADAPTER_VERSION: &str = "pinvou-gaia-adapter/v1";
pub const GAIA_SPLIT: &str = "validation";
pub const GAIA_LEVEL: u8 = 1;
pub const GAIA_PARQUET_SIZE: u64 = 39_524;
pub const GAIA_PARQUET_SHA256: &str =
    "5e574b0faeb4603b816e426cf7c7aefb1fe398d32f9c4861e1a4e3304f2b1281";

const GAIA_TOOL_POLICY: &str = "pinvou-gaia-public-web/v1";
const GAIA_OUTPUT_CONTRACT: &str = "gaia-final/v1";
const GAIA_TASK_TIMEOUT: Duration = Duration::from_secs(600);

pub struct GaiaAdapter {
    descriptor: BenchmarkDescriptor,
    scoring_dataset: Option<Arc<GaiaDataset>>,
}

impl GaiaAdapter {
    pub fn new() -> Self {
        Self {
            descriptor: BenchmarkDescriptor::new(
                BenchmarkId::new("gaia"),
                GAIA_ADAPTER_VERSION,
                GAIA_DATASET_REVISION,
                GAIA_SCORER_REVISION,
                vec![Split::new(GAIA_SPLIT)],
                ExecutionKind::NativeTurn,
            ),
            scoring_dataset: None,
        }
    }

    pub fn with_dataset(dataset: Arc<GaiaDataset>) -> Self {
        Self {
            scoring_dataset: Some(dataset),
            ..Self::new()
        }
    }

    pub fn plan(
        &self,
        dataset: &GaiaDataset,
        selection: &TaskSelection,
    ) -> BenchmarkResult<BenchmarkPlan> {
        let requested = selection
            .task_ids()
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        if requested.len() != selection.task_ids().len()
            || requested.iter().any(|task_id| {
                task_id.is_empty() || !dataset.rows().iter().any(|row| row.task_id() == *task_id)
            })
        {
            return Err(BenchmarkError::Contract(
                "gaia_task_selection_invalid".into(),
            ));
        }
        let tasks = dataset
            .rows()
            .iter()
            .filter(|row| requested.is_empty() || requested.contains(row.task_id()))
            .map(|row| {
                let task_id = row.task_id();
                let attachments = row
                    .attachment()
                    .map(|_| AttachmentHandle::new(format!("gaia:{task_id}:attachment")))
                    .into_iter()
                    .collect();
                BenchmarkTask::new(
                    task_id,
                    Some("gaia".into()),
                    Some(GAIA_LEVEL.to_string()),
                    ExecutionRequest::native_turn(
                        PrivateInputHandle::new(format!("gaia:{task_id}:prompt")),
                        attachments,
                        GAIA_TASK_TIMEOUT,
                        ToolPolicyId::new(GAIA_TOOL_POLICY),
                        OutputContract::new(GAIA_OUTPUT_CONTRACT),
                    ),
                    Some(ReferenceHandle::new(format!("gaia:{task_id}:reference"))),
                )
            })
            .collect();
        Ok(BenchmarkPlan::new(tasks))
    }
}

impl fmt::Debug for GaiaAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GaiaAdapter")
            .field("descriptor", &self.descriptor)
            .field(
                "scoring_dataset",
                &self.scoring_dataset.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

impl Default for GaiaAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl BenchmarkAdapter for GaiaAdapter {
    fn descriptor(&self) -> &BenchmarkDescriptor {
        &self.descriptor
    }

    fn private_output_retention(&self) -> PredictionRetention {
        PredictionRetention::DurableUntilPurge
    }

    fn private_prediction_content_type(&self) -> PrivatePredictionContentType {
        PrivatePredictionContentType::Utf8TextV1
    }

    fn verify_dataset(&self, dataset_root: &Path) -> BenchmarkResult<VerifiedDataset> {
        GaiaDataset::verify(dataset_root)
            .map_err(|error| BenchmarkError::Contract(error.to_string()))?;
        Ok(VerifiedDataset::new(GAIA_DATASET_REVISION, dataset_root))
    }

    fn plan(
        &self,
        dataset: &VerifiedDataset,
        selection: &TaskSelection,
    ) -> BenchmarkResult<BenchmarkPlan> {
        if dataset.id() != GAIA_DATASET_REVISION {
            return Err(BenchmarkError::Contract("gaia_revision_mismatch".into()));
        }
        if let Some(bound) = &self.scoring_dataset {
            let root = dataset
                .root()
                .canonicalize()
                .map_err(|_| BenchmarkError::Contract("gaia_revision_mismatch".into()))?;
            if root != bound.snapshot_root() {
                return Err(BenchmarkError::Contract("gaia_revision_mismatch".into()));
            }
            return GaiaAdapter::plan(self, bound, selection);
        }
        let verified = GaiaDataset::verify(dataset.root())
            .map_err(|error| BenchmarkError::Contract(error.to_string()))?;
        GaiaAdapter::plan(self, &verified, selection)
    }

    fn prepare_task(
        &self,
        task: &BenchmarkTask,
        _run: &RunContext,
    ) -> BenchmarkResult<PreparedTask> {
        Ok(PreparedTask::new(task.clone()))
    }

    fn score(&self, run: &CompletedRun) -> BenchmarkResult<OfficialScoreReport> {
        Ok(self.scoring_dataset.as_ref().map_or_else(
            || OfficialScoreReport::partial(0, 0, GAIA_SPLIT, &GAIA_LEVEL.to_string()),
            |dataset| scorer::score_dataset(dataset, run),
        ))
    }

    fn write_submission(
        &self,
        run: &CompletedRun,
        destination: &Path,
    ) -> BenchmarkResult<SubmissionArtifact> {
        let dataset = self.scoring_dataset.as_deref().ok_or_else(|| {
            BenchmarkError::Contract("gaia_submission_dataset_unavailable".into())
        })?;
        submission::write_submission(dataset, run, destination)
    }
}
