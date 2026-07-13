use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use deepseek_tui::core::events::TurnOutcomeStatus;
use deepseek_tui::task_manager::{
    ExecutionTask, TaskExecutionEvent, TaskExecutionReporter, TaskExecutionResult, TaskExecutor,
    TaskStatus,
};
use tokio_util::sync::CancellationToken;

use crate::bridge::prefs::{SavedModel, UserPrefs};
use crate::bridge::sessions::{ScheduledRunMode, ScheduledRunProfile, SessionStore};
use crate::engine_pool::{EnginePool, ScheduledTurnCompletion};

type StartedCallback =
    Box<dyn FnMut(String) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send>;

/// The narrow boundary between base-owned task execution and Pinvou's scheduled
/// conversation storage/engine runtime. Keeping this injectable lets executor
/// behavior be tested without starting a model or a WebView.
#[async_trait]
pub(crate) trait ScheduledConversationRuntime: Send + Sync {
    fn create_session(&self, profile: ScheduledRunProfile) -> Result<String>;

    async fn run_turn(
        &self,
        session_id: &str,
        prompt: String,
        cancel: CancellationToken,
        on_started: StartedCallback,
    ) -> Result<ScheduledTurnCompletion>;
}

/// Production implementation backed by the existing session store and engine
/// pool. The pool owns terminal persistence and always evicts the engine while
/// this runtime deliberately retains the durable scheduled session.
#[derive(Clone)]
pub(crate) struct EngineScheduledRuntime {
    store: SessionStore,
    pool: EnginePool,
}

impl EngineScheduledRuntime {
    pub(crate) fn new(store: SessionStore, pool: EnginePool) -> Self {
        Self { store, pool }
    }
}

#[async_trait]
impl ScheduledConversationRuntime for EngineScheduledRuntime {
    fn create_session(&self, mut profile: ScheduledRunProfile) -> Result<String> {
        let prefs = UserPrefs::load();
        bind_profile_model_id(&mut profile, &prefs.advanced.saved_models)?;
        Ok(self.store.create_scheduled_run(profile)?.metadata.id)
    }

    async fn run_turn(
        &self,
        session_id: &str,
        prompt: String,
        cancel: CancellationToken,
        mut on_started: StartedCallback,
    ) -> Result<ScheduledTurnCompletion> {
        self.pool
            .run_scheduled_turn(session_id, prompt, cancel, move |turn_id| {
                on_started(turn_id.to_string())
            })
            .await
    }
}

/// Host executor installed into DeepSeek-TUI's `TaskManager` for scheduled
/// automations. Scheduling and durable task/run state remain base-owned.
pub(crate) struct ScheduledChatExecutor {
    runtime: Arc<dyn ScheduledConversationRuntime>,
}

impl ScheduledChatExecutor {
    pub(crate) fn new(runtime: Arc<dyn ScheduledConversationRuntime>) -> Self {
        Self { runtime }
    }

    pub(crate) fn from_services(store: SessionStore, pool: EnginePool) -> Self {
        Self::new(Arc::new(EngineScheduledRuntime::new(store, pool)))
    }
}

#[async_trait]
impl TaskExecutor for ScheduledChatExecutor {
    async fn execute(
        &self,
        task: ExecutionTask,
        reporter: TaskExecutionReporter,
        cancel: CancellationToken,
    ) -> TaskExecutionResult {
        let profile = ScheduledRunProfile {
            task_id: task.id().to_string(),
            model: task.model().to_string(),
            model_id: None,
            workspace: task.workspace().to_path_buf(),
            // Scheduled tasks have no interactive mode selector. Yolo is safe
            // only when the persisted task explicitly enables auto approval.
            mode: ScheduledRunMode::for_scheduled_auto_approve(task.auto_approve()),
            allow_shell: task.allow_shell(),
            trust_mode: task.trust_mode(),
            auto_approve: task.auto_approve(),
        };
        let session_id = match self.runtime.create_session(profile) {
            Ok(session_id) => session_id,
            Err(error) => return failed(error),
        };

        // The durable conversation identity exists before any engine send. The
        // base manager consumes this first event before the later turn link.
        if let Err(error) = reporter
            .report(TaskExecutionEvent::ThreadCreated {
                thread_id: session_id.clone(),
            })
            .await
        {
            return failed(format!(
                "Failed to persist scheduled conversation identity: {error}"
            ));
        }

        let link_reporter = reporter.clone();
        let linked_session_id = session_id.clone();
        let result_cancel = cancel.clone();
        let completion = self
            .runtime
            .run_turn(
                &session_id,
                task.prompt().to_string(),
                cancel,
                Box::new(move |turn_id| {
                    let reporter = link_reporter.clone();
                    let thread_id = linked_session_id.clone();
                    Box::pin(async move {
                        reporter
                            .report(TaskExecutionEvent::ThreadLinked { thread_id, turn_id })
                            .await?;
                        Ok(())
                    })
                }),
            )
            .await;

        if result_cancel.is_cancelled() {
            return canceled();
        }
        match completion {
            Ok(completion) => map_completion(completion),
            Err(error) => failed(error),
        }
    }
}

fn bind_profile_model_id(profile: &mut ScheduledRunProfile, models: &[SavedModel]) -> Result<()> {
    let mut matches = models.iter().filter(|model| model.model == profile.model);
    let selected = matches.next();
    profile.model_id = match (selected, matches.next()) {
        (Some(selected), None) => Some(selected.id.clone()),
        // Preserve the durable conversation even when model resolution will
        // fail. EnginePool performs the strict zero/duplicate check before send.
        _ => None,
    };
    Ok(())
}

fn map_completion(completion: ScheduledTurnCompletion) -> TaskExecutionResult {
    if completion.cancel_requested || completion.status == TurnOutcomeStatus::Interrupted {
        return TaskExecutionResult {
            status: TaskStatus::Canceled,
            result_text: None,
            error: completion.error,
        };
    }

    match completion.status {
        TurnOutcomeStatus::Completed => TaskExecutionResult {
            status: TaskStatus::Completed,
            result_text: Some("Scheduled conversation completed".to_string()),
            error: None,
        },
        TurnOutcomeStatus::Failed => TaskExecutionResult {
            status: TaskStatus::Failed,
            result_text: None,
            error: Some(
                completion
                    .error
                    .unwrap_or_else(|| "Scheduled conversation failed".to_string()),
            ),
        },
        TurnOutcomeStatus::Interrupted => unreachable!("handled above"),
    }
}

fn failed(error: impl std::fmt::Display) -> TaskExecutionResult {
    TaskExecutionResult {
        status: TaskStatus::Failed,
        result_text: None,
        error: Some(error.to_string()),
    }
}

fn canceled() -> TaskExecutionResult {
    TaskExecutionResult {
        status: TaskStatus::Canceled,
        result_text: None,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use anyhow::{bail, Result};
    use async_trait::async_trait;
    use deepseek_tui::core::events::TurnOutcomeStatus;
    use deepseek_tui::task_manager::{
        NewTaskRequest, SharedTaskManager, TaskManager, TaskManagerConfig, TaskRecord, TaskStatus,
    };
    use tokio::sync::Notify;
    use tokio_util::sync::CancellationToken;

    use crate::bridge::prefs::{ModelPreset, SavedModel};
    use crate::bridge::sessions::{ScheduledRunMode, ScheduledRunProfile};
    use crate::credential_store::{CredentialEditAction, CredentialState};
    use crate::engine_pool::ScheduledTurnCompletion;

    #[derive(Debug)]
    enum Script {
        Complete { turn_id: String },
        Fail { turn_id: String, error: String },
        Interrupted { turn_id: String },
        WaitForCancelError { turn_id: String },
        SendError { error: String },
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RunCall {
        session_id: String,
        prompt: String,
    }

    struct ScriptedRuntime {
        scripts: Mutex<VecDeque<Script>>,
        profiles: Mutex<Vec<(String, ScheduledRunProfile)>>,
        calls: Mutex<Vec<RunCall>>,
        next_session: AtomicUsize,
        started: Notify,
    }

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Result<Self> {
            static NEXT_ROOT: AtomicUsize = AtomicUsize::new(1);
            let number = NEXT_ROOT.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "pinvou-scheduled-executor-{}-{number}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl ScriptedRuntime {
        fn new(scripts: impl IntoIterator<Item = Script>) -> Self {
            Self {
                scripts: Mutex::new(scripts.into_iter().collect()),
                profiles: Mutex::new(Vec::new()),
                calls: Mutex::new(Vec::new()),
                next_session: AtomicUsize::new(1),
                started: Notify::new(),
            }
        }

        fn profiles(&self) -> Vec<(String, ScheduledRunProfile)> {
            self.profiles.lock().unwrap().clone()
        }

        fn calls(&self) -> Vec<RunCall> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ScheduledConversationRuntime for ScriptedRuntime {
        fn create_session(&self, profile: ScheduledRunProfile) -> Result<String> {
            let number = self.next_session.fetch_add(1, Ordering::SeqCst);
            let session_id = format!("sched-fake-{number}");
            self.profiles
                .lock()
                .unwrap()
                .push((session_id.clone(), profile));
            Ok(session_id)
        }

        async fn run_turn(
            &self,
            session_id: &str,
            prompt: String,
            cancel: CancellationToken,
            mut on_started: StartedCallback,
        ) -> Result<ScheduledTurnCompletion> {
            self.calls.lock().unwrap().push(RunCall {
                session_id: session_id.to_string(),
                prompt,
            });
            let script = self
                .scripts
                .lock()
                .unwrap()
                .pop_front()
                .expect("one script per run");
            match script {
                Script::Complete { turn_id } => {
                    on_started(turn_id.clone()).await?;
                    self.started.notify_one();
                    Ok(completion(
                        turn_id,
                        TurnOutcomeStatus::Completed,
                        None,
                        false,
                    ))
                }
                Script::Fail { turn_id, error } => {
                    on_started(turn_id.clone()).await?;
                    self.started.notify_one();
                    Ok(completion(
                        turn_id,
                        TurnOutcomeStatus::Failed,
                        Some(error),
                        false,
                    ))
                }
                Script::Interrupted { turn_id } => {
                    on_started(turn_id.clone()).await?;
                    self.started.notify_one();
                    Ok(completion(
                        turn_id,
                        TurnOutcomeStatus::Interrupted,
                        None,
                        false,
                    ))
                }
                Script::WaitForCancelError { turn_id } => {
                    on_started(turn_id.clone()).await?;
                    self.started.notify_one();
                    cancel.cancelled().await;
                    bail!("engine stop timed out")
                }
                Script::SendError { error } => bail!(error),
            }
        }
    }

    fn completion(
        turn_id: String,
        status: TurnOutcomeStatus,
        error: Option<String>,
        cancel_requested: bool,
    ) -> ScheduledTurnCompletion {
        ScheduledTurnCompletion {
            turn_id,
            status,
            error,
            cancel_requested,
        }
    }

    async fn manager_with_runtime(
        runtime: Arc<ScriptedRuntime>,
    ) -> Result<(TestRoot, SharedTaskManager)> {
        let root = TestRoot::new()?;
        let config = TaskManagerConfig {
            data_dir: root.0.join("tasks"),
            worker_count: 1,
            default_workspace: PathBuf::from("D:/default-workspace"),
            default_model: "default-model".to_string(),
            default_mode: "agent".to_string(),
            allow_shell: false,
            trust_mode: false,
            max_subagents: 1,
        };
        let executor = Arc::new(ScheduledChatExecutor::new(runtime));
        let manager = TaskManager::start_with_executor(config, executor).await?;
        Ok((root, manager))
    }

    async fn wait_for_terminal_state(
        manager: &SharedTaskManager,
        task_id: &str,
        timeout: std::time::Duration,
    ) -> Result<TaskRecord> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let task = manager.get_task(task_id).await?;
            if matches!(
                task.status,
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Canceled
            ) {
                return Ok(task);
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("timed out waiting for task {task_id}");
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    fn request(prompt: &str) -> NewTaskRequest {
        NewTaskRequest {
            prompt: prompt.to_string(),
            model: Some("scheduled-model".to_string()),
            workspace: Some(PathBuf::from("D:/scheduled-workspace")),
            mode: Some("plan".to_string()),
            allow_shell: Some(true),
            trust_mode: Some(true),
            auto_approve: Some(false),
        }
    }

    fn saved_model(id: &str, wire_name: &str) -> SavedModel {
        SavedModel {
            id: id.to_string(),
            name: id.to_string(),
            preset: ModelPreset::OpenaiCompatible,
            model: wire_name.to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            api_key: String::new(),
            credential_ref: None,
            credential_state: CredentialState::Missing,
            has_secret: false,
            credential_action: None::<CredentialEditAction>,
        }
    }

    #[test]
    fn model_binding_captures_the_one_matching_saved_model_id() {
        let mut profile = ScheduledRunProfile {
            task_id: "task-model".to_string(),
            model: "wire-model".to_string(),
            model_id: None,
            workspace: PathBuf::from("D:/scheduled-workspace"),
            mode: ScheduledRunMode::Agent,
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
        };

        bind_profile_model_id(
            &mut profile,
            &[
                saved_model("other", "other-model"),
                saved_model("wanted", "wire-model"),
            ],
        )
        .expect("one exact model match");

        assert_eq!(profile.model_id.as_deref(), Some("wanted"));
    }

    #[test]
    fn model_binding_leaves_missing_or_ambiguous_wire_names_for_runtime_failure() {
        let profile = ScheduledRunProfile {
            task_id: "task-model".to_string(),
            model: "wire-model".to_string(),
            model_id: None,
            workspace: PathBuf::from("D:/scheduled-workspace"),
            mode: ScheduledRunMode::Agent,
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
        };

        let mut missing = profile.clone();
        bind_profile_model_id(&mut missing, &[]).expect("session creation must continue");
        assert_eq!(missing.model_id, None);

        let mut ambiguous = profile.clone();
        assert!(bind_profile_model_id(
            &mut ambiguous,
            &[
                saved_model("first", "wire-model"),
                saved_model("second", "wire-model"),
            ],
        )
        .is_ok());
        assert_eq!(ambiguous.model_id, None);
    }

    #[test]
    fn scheduled_auto_approve_false_never_builds_yolo_context() {
        assert_eq!(
            ScheduledRunMode::for_scheduled_auto_approve(false),
            ScheduledRunMode::Agent
        );
        assert_eq!(
            ScheduledRunMode::for_scheduled_auto_approve(true),
            ScheduledRunMode::Yolo
        );
    }

    #[tokio::test]
    async fn success_creates_and_links_a_durable_independent_session_before_completion(
    ) -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([Script::Complete {
            turn_id: "real-turn-42".to_string(),
        }]));
        let (_root, manager) = manager_with_runtime(runtime.clone()).await?;

        let queued = manager.add_task(request("prepare the daily brief")).await?;
        let finished =
            wait_for_terminal_state(&manager, &queued.id, std::time::Duration::from_secs(5))
                .await?;

        assert_eq!(finished.status, TaskStatus::Completed);
        assert_eq!(finished.thread_id.as_deref(), Some("sched-fake-1"));
        assert_eq!(finished.turn_id.as_deref(), Some("real-turn-42"));
        let created_index = finished
            .timeline
            .iter()
            .position(|entry| entry.kind == "runtime_thread")
            .expect("ThreadCreated must be durable");
        let linked_index = finished
            .timeline
            .iter()
            .position(|entry| entry.kind == "runtime_link")
            .expect("ThreadLinked must be durable");
        assert!(created_index < linked_index);

        let profiles = runtime.profiles();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].0, "sched-fake-1");
        assert_eq!(profiles[0].1.task_id, queued.id);
        assert_eq!(profiles[0].1.model, "scheduled-model");
        assert_eq!(profiles[0].1.model_id, None);
        assert_eq!(
            profiles[0].1.workspace,
            PathBuf::from("D:/scheduled-workspace")
        );
        assert_eq!(
            profiles[0].1.mode,
            ScheduledRunMode::Agent,
            "autoApprove=false must never build a Yolo scheduled context"
        );
        assert!(profiles[0].1.allow_shell);
        assert!(profiles[0].1.trust_mode);
        assert!(!profiles[0].1.auto_approve);
        assert_eq!(
            runtime.calls(),
            vec![RunCall {
                session_id: "sched-fake-1".to_string(),
                prompt: "prepare the daily brief".to_string(),
            }]
        );
        manager.shutdown();
        Ok(())
    }

    #[tokio::test]
    async fn each_task_gets_a_distinct_scheduled_session() -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([
            Script::Complete {
                turn_id: "turn-one".to_string(),
            },
            Script::Complete {
                turn_id: "turn-two".to_string(),
            },
        ]));
        let (_root, manager) = manager_with_runtime(runtime.clone()).await?;

        let first = manager.add_task(request("first")).await?;
        let second = manager.add_task(request("second")).await?;
        let first =
            wait_for_terminal_state(&manager, &first.id, std::time::Duration::from_secs(5)).await?;
        let second =
            wait_for_terminal_state(&manager, &second.id, std::time::Duration::from_secs(5))
                .await?;

        assert_ne!(first.thread_id, second.thread_id);
        assert_eq!(runtime.profiles().len(), 2);
        manager.shutdown();
        Ok(())
    }

    #[tokio::test]
    async fn failed_and_interrupted_turns_map_to_task_terminal_states() -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([
            Script::Fail {
                turn_id: "failed-turn".to_string(),
                error: "model failed".to_string(),
            },
            Script::Interrupted {
                turn_id: "interrupted-turn".to_string(),
            },
        ]));
        let (_root, manager) = manager_with_runtime(runtime.clone()).await?;

        let failed = manager.add_task(request("fail")).await?;
        let interrupted = manager.add_task(request("interrupt")).await?;
        let failed =
            wait_for_terminal_state(&manager, &failed.id, std::time::Duration::from_secs(5))
                .await?;
        let interrupted =
            wait_for_terminal_state(&manager, &interrupted.id, std::time::Duration::from_secs(5))
                .await?;

        assert_eq!(failed.status, TaskStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("model failed"));
        assert_eq!(failed.turn_id.as_deref(), Some("failed-turn"));
        assert_eq!(interrupted.status, TaskStatus::Canceled);
        assert_eq!(interrupted.turn_id.as_deref(), Some("interrupted-turn"));
        assert_eq!(
            runtime.profiles().len(),
            2,
            "terminal failures keep sessions"
        );
        manager.shutdown();
        Ok(())
    }

    #[tokio::test]
    async fn explicit_cancel_wins_even_if_runtime_stop_returns_an_error() -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([Script::WaitForCancelError {
            turn_id: "cancelled-turn".to_string(),
        }]));
        let (_root, manager) = manager_with_runtime(runtime.clone()).await?;
        let queued = manager.add_task(request("wait")).await?;
        runtime.started.notified().await;

        manager.cancel_task(&queued.id).await?;
        let finished =
            wait_for_terminal_state(&manager, &queued.id, std::time::Duration::from_secs(5))
                .await?;

        assert_eq!(finished.status, TaskStatus::Canceled);
        assert_eq!(finished.thread_id.as_deref(), Some("sched-fake-1"));
        assert_eq!(finished.turn_id.as_deref(), Some("cancelled-turn"));
        assert_eq!(runtime.profiles().len(), 1, "cancel keeps the session");
        manager.shutdown();
        Ok(())
    }

    #[tokio::test]
    async fn missing_model_send_failure_keeps_the_precreated_session() -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([Script::SendError {
            error: "send failed".to_string(),
        }]));
        let (_root, manager) = manager_with_runtime(runtime.clone()).await?;
        let mut missing_model_request = request("cannot send");
        missing_model_request.model = Some("deleted-model".to_string());
        let queued = manager.add_task(missing_model_request).await?;

        let finished =
            wait_for_terminal_state(&manager, &queued.id, std::time::Duration::from_secs(5))
                .await?;

        assert_eq!(finished.status, TaskStatus::Failed);
        assert_eq!(finished.error.as_deref(), Some("send failed"));
        assert_eq!(finished.thread_id.as_deref(), Some("sched-fake-1"));
        assert_eq!(finished.turn_id, None);
        let profiles = runtime.profiles();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].1.model, "deleted-model");
        manager.shutdown();
        Ok(())
    }
}
