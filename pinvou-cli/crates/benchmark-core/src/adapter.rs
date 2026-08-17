use std::path::Path;

use crate::{
    BenchmarkDescriptor, BenchmarkPlan, BenchmarkTask, CompletedRun, OfficialScoreReport,
    PredictionRetention, PreparedTask, PrivatePredictionContentType, Result, RunContext,
    SubmissionArtifact, TaskSelection, VerifiedDataset,
};

pub trait BenchmarkAdapter: Send + Sync {
    fn descriptor(&self) -> &BenchmarkDescriptor;
    fn private_output_retention(&self) -> PredictionRetention {
        PredictionRetention::Ephemeral
    }
    fn private_prediction_content_type(&self) -> PrivatePredictionContentType {
        PrivatePredictionContentType::Utf8TextV1
    }
    fn verify_dataset(&self, dataset_root: &Path) -> Result<VerifiedDataset>;
    fn plan(&self, dataset: &VerifiedDataset, selection: &TaskSelection) -> Result<BenchmarkPlan>;
    fn prepare_task(&self, task: &BenchmarkTask, run: &RunContext) -> Result<PreparedTask>;
    fn score(&self, run: &CompletedRun) -> Result<OfficialScoreReport>;
    fn write_submission(
        &self,
        run: &CompletedRun,
        destination: &Path,
    ) -> Result<SubmissionArtifact>;
}
