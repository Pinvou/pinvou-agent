use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_backend_api::{HeadlessAgentBackend, PrivateInputResolver};

use crate::Result;
use crate::{
    BenchmarkAdapter, BenchmarkError, BenchmarkPlan, CompletedRun, NativeAgentRunner,
    OfficialScoreReport, PredictionRetention, PrivatePredictionContentType, RunContext,
    RunManifest, RunStore, SafeFailureCategory, SubmissionArtifact, TaskOutcome, TaskRunner,
    TaskSelection, TaskStatus, VerifiedDataset,
};

pub struct BenchmarkService<R> {
    base: PathBuf,
    runner: Arc<R>,
}

impl<B> BenchmarkService<NativeAgentRunner<B>>
where
    B: HeadlessAgentBackend + 'static,
{
    pub fn native(base: &Path, backend: Arc<B>) -> Result<Self> {
        if !base.is_absolute() {
            return Err(BenchmarkError::coded("unsafe_base_directory"));
        }
        Ok(Self {
            base: base.into(),
            runner: Arc::new(NativeAgentRunner::new(backend)),
        })
    }

    pub fn native_with_private_inputs(
        base: &Path,
        backend: Arc<B>,
        private_inputs: Arc<dyn PrivateInputResolver>,
    ) -> Result<Self> {
        if !base.is_absolute() {
            return Err(BenchmarkError::coded("unsafe_base_directory"));
        }
        Ok(Self {
            base: base.into(),
            runner: Arc::new(NativeAgentRunner::with_private_inputs(
                backend,
                private_inputs,
            )),
        })
    }
}

impl<R> BenchmarkService<R>
where
    R: TaskRunner + 'static,
{
    pub fn with_runner(base: &Path, runner: Arc<R>) -> Result<Self> {
        if !base.is_absolute() {
            return Err(BenchmarkError::coded("unsafe_base_directory"));
        }
        Ok(Self {
            base: base.into(),
            runner,
        })
    }

    pub async fn run(&self, manifest: RunManifest, plan: &BenchmarkPlan) -> Result<RunSummary> {
        let store = RunStore::create(&self.base, &manifest)?;
        let _execution = store.claim_execution()?;
        store.plan_tasks(plan.tasks().iter().map(|task| task.task_id()))?;
        self.execute(
            &store,
            &manifest,
            plan,
            PredictionRetention::Ephemeral,
            PrivatePredictionContentType::Utf8TextV1,
        )
        .await
    }

    pub async fn run_adapter(
        &self,
        manifest: RunManifest,
        adapter: &dyn BenchmarkAdapter,
        dataset: &VerifiedDataset,
        selection: &TaskSelection,
    ) -> Result<RunSummary> {
        if manifest.benchmark() != adapter.descriptor().id().as_str() {
            return Err(BenchmarkError::coded("adapter_manifest_mismatch"));
        }
        if dataset.id() != adapter.descriptor().dataset_revision() {
            return Err(BenchmarkError::coded("adapter_dataset_mismatch"));
        }
        let plan = adapter.plan(dataset, selection)?;
        let store = RunStore::create(&self.base, &manifest)?;
        let _execution = store.claim_execution()?;
        let prepared = self.prepare_plan(adapter, &store, &manifest, &plan)?;
        store.plan_tasks(prepared.tasks().iter().map(|task| task.task_id()))?;
        self.execute(
            &store,
            &manifest,
            &prepared,
            adapter.private_output_retention(),
            adapter.private_prediction_content_type(),
        )
        .await
    }

    pub fn score_adapter(
        &self,
        adapter: &dyn BenchmarkAdapter,
        run: &CompletedRun,
    ) -> Result<OfficialScoreReport> {
        adapter.score(run)
    }

    pub fn write_adapter_submission(
        &self,
        adapter: &dyn BenchmarkAdapter,
        run: &CompletedRun,
        destination: &Path,
    ) -> Result<SubmissionArtifact> {
        adapter.write_submission(run, destination)
    }

    pub async fn resume(&self, run_id: &str, plan: &BenchmarkPlan) -> Result<RunSummary> {
        let store = RunStore::open(&self.base, run_id)?;
        let _execution = store.claim_execution()?;
        let manifest = store.read_manifest()?;
        if manifest.run_id() != run_id {
            return Err(BenchmarkError::coded("resume_manifest_mismatch"));
        }
        self.execute(
            &store,
            &manifest,
            plan,
            PredictionRetention::Ephemeral,
            PrivatePredictionContentType::Utf8TextV1,
        )
        .await
    }

    pub async fn resume_adapter(
        &self,
        run_id: &str,
        expected_manifest: &RunManifest,
        adapter: &dyn BenchmarkAdapter,
        dataset: &VerifiedDataset,
        selection: &TaskSelection,
    ) -> Result<RunSummary> {
        let store = RunStore::open(&self.base, run_id)?;
        let _execution = store.claim_execution()?;
        let manifest = store.read_manifest()?;
        if expected_manifest.validate().is_err()
            || manifest.validate().is_err()
            || expected_manifest.run_id() != run_id
            || !manifest.matches_expected(expected_manifest)
            || !expected_manifest.matches_descriptor(adapter.descriptor())
        {
            return Err(BenchmarkError::coded("resume_manifest_mismatch"));
        }
        if dataset.id() != adapter.descriptor().dataset_revision() {
            return Err(BenchmarkError::coded("adapter_dataset_mismatch"));
        }
        let plan = adapter.plan(dataset, selection)?;
        let prepared = self.prepare_plan(adapter, &store, &manifest, &plan)?;
        store.reconcile_planned_tasks(prepared.tasks().iter().map(|task| task.task_id()))?;
        self.execute(
            &store,
            &manifest,
            &prepared,
            adapter.private_output_retention(),
            adapter.private_prediction_content_type(),
        )
        .await
    }

    fn prepare_plan(
        &self,
        adapter: &dyn BenchmarkAdapter,
        store: &RunStore,
        manifest: &RunManifest,
        plan: &BenchmarkPlan,
    ) -> Result<BenchmarkPlan> {
        let context = RunContext::new(manifest.run_id(), store.run_dir().to_owned());
        let tasks = plan
            .tasks()
            .iter()
            .map(|task| {
                adapter
                    .prepare_task(task, &context)
                    .map(|prepared| prepared.task().clone())
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(BenchmarkPlan::new(tasks))
    }

    async fn execute(
        &self,
        store: &RunStore,
        manifest: &RunManifest,
        plan: &BenchmarkPlan,
        retention: PredictionRetention,
        content_type: PrivatePredictionContentType,
    ) -> Result<RunSummary> {
        let recovered = store.recover()?;
        let runnable: BTreeSet<&str> = recovered
            .runnable_task_ids()
            .iter()
            .map(String::as_str)
            .collect();
        let context = RunContext::new(manifest.run_id(), store.run_dir().to_owned());
        let mut outcomes = Vec::new();
        for task in plan
            .tasks()
            .iter()
            .filter(|task| runnable.contains(task.task_id()))
        {
            store.mark_running(task.task_id())?;
            let outcome = match self.runner.run_task(task, &context).await {
                Ok(outcome) => outcome,
                Err(error) => {
                    let timeout = error.code() == "task_timeout";
                    let invalid_output = matches!(
                        error.code(),
                        "private_output_resolution_failed" | "missing_final_answer"
                    );
                    let outcome = TaskOutcome::new(
                        task.task_id(),
                        if timeout {
                            TaskStatus::Timeout
                        } else {
                            TaskStatus::Failed
                        },
                        None,
                        vec![],
                        0,
                    )
                    .with_failure_category(if timeout {
                        SafeFailureCategory::Timeout
                    } else if invalid_output {
                        SafeFailureCategory::InvalidOutput
                    } else {
                        SafeFailureCategory::Backend
                    });
                    if error.code() == "missing_final_answer" {
                        outcome.with_failure_reason(crate::SafeFailureReason::MissingFinalAnswer)
                    } else {
                        outcome
                    }
                }
            };
            let outcome = match retention {
                PredictionRetention::Ephemeral => outcome.with_prediction(None),
                PredictionRetention::DurableUntilPurge
                    if outcome.status() == TaskStatus::Completed =>
                {
                    let prediction = store.persist_private_prediction(&outcome, content_type)?;
                    outcome.with_prediction(Some(prediction))
                }
                PredictionRetention::DurableUntilPurge => outcome.with_prediction(None),
            };
            let status = outcome.status();
            outcomes.push(outcome.clone());
            store.record_outcome(outcome)?;
            match status {
                TaskStatus::Completed => store.mark_completed(task.task_id())?,
                TaskStatus::Timeout => store.mark_timeout(task.task_id())?,
                _ => store.mark_failed(task.task_id())?,
            }
        }
        let recovered = store.recover()?;
        let mut cumulative = store.read_outcomes()?;
        for outcome in outcomes {
            if let Some(existing) = cumulative
                .iter_mut()
                .find(|value| value.task_id() == outcome.task_id())
            {
                *existing = outcome;
            } else {
                cumulative.push(outcome);
            }
        }
        Ok(RunSummary {
            run_id: manifest.run_id().into(),
            completed: recovered.completed_task_ids().len(),
            remaining: recovered.runnable_task_ids().len(),
            outcomes: cumulative,
        })
    }
}

#[derive(Clone, Debug)]
pub struct RunSummary {
    run_id: String,
    completed: usize,
    remaining: usize,
    outcomes: Vec<TaskOutcome>,
}

impl RunSummary {
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn completed(&self) -> usize {
        self.completed
    }

    pub fn remaining(&self) -> usize {
        self.remaining
    }
    pub fn outcomes(&self) -> &[TaskOutcome] {
        &self.outcomes
    }
}
