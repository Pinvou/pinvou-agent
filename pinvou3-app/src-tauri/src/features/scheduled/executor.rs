use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use deepseek_tui::core::events::TurnOutcomeStatus;
use deepseek_tui::task_manager::{
    ExecutionTask, TaskExecutionEvent, TaskExecutionResult, TaskExecutor, TaskStatus,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::features::assistant::engine_pool::{EnginePool, ScheduledTurnCompletion};
use crate::features::assistant::platform::bridge::Pinvou3Bridge;
use crate::features::memory::MemoryOrganizeReport;
use crate::features::scheduled::tasks::SCHEDULED_TASK_KIND_MEMORY_ORGANIZE;
use crate::features::sessions::{ScheduledRunMode, ScheduledRunProfile, SessionStore};
use crate::platform::prefs::{SavedModel, UserPrefs};

type StartedCallback =
    Box<dyn FnMut(String) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send>;
type ModelIdResolver = Arc<dyn Fn(&str, &str) -> Option<String> + Send + Sync>;
/// automation_id → 任务种类（None = 普通聊天任务）。
type KindResolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The narrow boundary between base-owned task execution and Pinvou's scheduled
/// conversation storage/engine runtime. Keeping this injectable lets executor
/// behavior be tested without starting a model or a WebView.
#[async_trait]
pub(crate) trait ScheduledConversationRuntime: Send + Sync {
    /// Resolve the same Shell switch used by an ordinary Yolo conversation.
    /// This is deliberately evaluated for every run so settings changes do
    /// not leave long-lived scheduled tasks with stale permissions.
    fn yolo_allow_shell(&self) -> bool;

    fn model_id_for_automation(&self, automation_id: &str, model: &str) -> Option<String>;

    fn create_session(&self, profile: ScheduledRunProfile) -> Result<String>;

    async fn run_turn(
        &self,
        session_id: &str,
        prompt: String,
        cancel: CancellationToken,
        on_started: StartedCallback,
    ) -> Result<ScheduledTurnCompletion>;

    /// 执行一次 app 侧记忆整理（`memory_organize` 种类任务的运行体）。
    /// 与对话轮不同：不创建会话、不经过引擎。取消语义见
    /// [`ScheduledChatExecutor::execute_memory_organize`]。
    async fn organize_memory(&self, cancel: CancellationToken) -> Result<MemoryOrganizeReport>;
}

/// Production implementation backed by the existing session store and engine
/// pool. The pool owns terminal persistence and always evicts the engine while
/// this runtime deliberately retains the durable scheduled session.
#[derive(Clone)]
pub(crate) struct EngineScheduledRuntime {
    store: SessionStore,
    pool: EnginePool,
    model_id_resolver: Option<ModelIdResolver>,
}

impl EngineScheduledRuntime {
    pub(crate) fn new(
        store: SessionStore,
        pool: EnginePool,
        model_id_resolver: Option<ModelIdResolver>,
    ) -> Self {
        Self {
            store,
            pool,
            model_id_resolver,
        }
    }
}

#[async_trait]
impl ScheduledConversationRuntime for EngineScheduledRuntime {
    fn yolo_allow_shell(&self) -> bool {
        Pinvou3Bridge::allow_shell_for_prefs(&UserPrefs::load())
    }

    fn model_id_for_automation(&self, automation_id: &str, model: &str) -> Option<String> {
        self.model_id_resolver
            .as_ref()
            .and_then(|resolver| resolver(automation_id, model))
    }

    fn create_session(&self, mut profile: ScheduledRunProfile) -> Result<String> {
        let prefs = UserPrefs::load();
        if profile.model_id.is_none() {
            bind_profile_model_id(&mut profile, &prefs.advanced.saved_models)?;
        }
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

    async fn organize_memory(&self, cancel: CancellationToken) -> Result<MemoryOrganizeReport> {
        // 与 chat 命令层的 organize_memory 同口径：memory 关闭时先拒绝。
        if !crate::features::memory::memory_enabled() {
            anyhow::bail!("memory disabled");
        }
        // 与 commands::memory 的 organize_memory 同一套 bridge 解析：有活跃会话
        // 时按该会话绑定模型（fresh bridge，含运行时凭据准备）；否则退化为共享
        // bridge + 全局 prefs 直读。
        let bridge = match self.store.active_id() {
            Some(session_id) => self.pool.fresh_bridge_for(&session_id).await?,
            None => {
                let mut bridge = self.pool.bridge.clone();
                bridge.prefs = UserPrefs::load();
                bridge.session_model = None;
                bridge
            }
        };
        crate::features::memory::organize_memory_with_llm(&bridge, Some(&cancel)).await
    }
}

/// Host executor installed into CodeWhale's `TaskManager` for scheduled
/// automations. Scheduling and durable task/run state remain base-owned.
pub(crate) struct ScheduledChatExecutor {
    runtime: Arc<dyn ScheduledConversationRuntime>,
    kind_resolver: Option<KindResolver>,
}

impl ScheduledChatExecutor {
    pub(crate) fn new(runtime: Arc<dyn ScheduledConversationRuntime>) -> Self {
        Self {
            runtime,
            kind_resolver: None,
        }
    }

    /// 注入 automation_id → kind 解析（生产实现读 task-kinds sidecar）。
    pub(crate) fn with_kind_resolver(mut self, kind_resolver: KindResolver) -> Self {
        self.kind_resolver = Some(kind_resolver);
        self
    }

    pub(crate) fn from_services(
        store: SessionStore,
        pool: EnginePool,
        model_id_resolver: ModelIdResolver,
        kind_resolver: Option<KindResolver>,
    ) -> Self {
        let executor = Self::new(Arc::new(EngineScheduledRuntime::new(
            store,
            pool,
            Some(model_id_resolver),
        )));
        match kind_resolver {
            Some(resolver) => executor.with_kind_resolver(resolver),
            None => executor,
        }
    }

    fn kind_for(&self, automation_id: &str) -> Option<String> {
        self.kind_resolver
            .as_ref()
            .and_then(|resolver| resolver(automation_id))
    }

    /// `memory_organize` 种类的一次运行：不建会话、不发引擎轮，也不发
    /// ThreadCreated / ThreadLinked 事件——run 生命周期只需要底座 Task 的终态
    /// （reconcile 会把 thread_id/turn_id 缺省的 Task 状态回写到 run 记录），
    /// 整理报告本身经记忆历史命令查看，运行记录保持干净。
    ///
    /// 取消语义：select 让取消即时生效，不必等最长 75s 的整理调用返回（否则
    /// 删除运行中任务的 15s 终态等待几乎必然超时）；取消分支丢弃在途 future，
    /// 连同未返回的 LLM 请求一起中止。LLM 已返回、应用动作前的取消由
    /// organize 内部的应用前检查兜住；仅剩同步应用段（毫秒级）无法打断，
    /// 该情形按取消记录 run，报告已落 organize_history.json 可审计。
    async fn execute_memory_organize(&self, cancel: CancellationToken) -> TaskExecutionResult {
        let organized = tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            organized = self.runtime.organize_memory(cancel.clone()) => Some(organized),
        };
        let organized = match organized {
            Some(organized) => organized,
            None => return canceled(),
        };
        if cancel.is_cancelled() {
            return canceled();
        }
        match organized {
            Ok(report) => TaskExecutionResult {
                status: TaskStatus::Completed,
                result_text: Some(memory_organize_summary(&report)),
                error: None,
            },
            Err(error) => TaskExecutionResult {
                status: TaskStatus::Failed,
                result_text: None,
                error: Some(format!("memory organize: {error:#}")),
            },
        }
    }
}

/// 运行完成时的一段人读摘要（仅进入底座 Task 的 result_summary，不落 run 记录）。
fn memory_organize_summary(report: &MemoryOrganizeReport) -> String {
    let total =
        |counts: &std::collections::BTreeMap<String, u32>| counts.values().copied().sum::<u32>();
    let scanned = total(&report.scanned);
    if report.no_change {
        return format!("Memory organize completed: scanned {scanned}, no changes needed");
    }
    format!(
        "Memory organize completed: scanned {scanned}, deleted {}, updated {}, merged {}, skipped sensitive {}",
        total(&report.deleted),
        total(&report.updated),
        total(&report.merged),
        report.skipped_sensitive,
    )
}

#[async_trait]
impl TaskExecutor for ScheduledChatExecutor {
    async fn execute(
        &self,
        task: ExecutionTask,
        events: mpsc::UnboundedSender<TaskExecutionEvent>,
        cancel: CancellationToken,
    ) -> TaskExecutionResult {
        let allow_shell = self.runtime.yolo_allow_shell();
        let automation_id = task
            .conversation_key()
            .unwrap_or_else(|| task.id())
            .to_string();
        // 记忆整理任务在这里分流：不创建会话、不解析模型绑定。
        if self.kind_for(&automation_id).as_deref() == Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE) {
            return self.execute_memory_organize(cancel).await;
        }
        let model = task.model().to_string();
        let model_id = self.runtime.model_id_for_automation(&automation_id, &model);
        let profile = ScheduledRunProfile {
            // The stable automation id owns the shared task workspace. Each
            // execution still keeps its own task id for cancellation/status.
            // Direct executor tests have no AutomationManager and therefore
            // use the task id as their isolated owner.
            task_id: automation_id,
            model,
            model_id,
            workspace: task.workspace().to_path_buf(),
            // Scheduled execution is the unattended form of an ordinary Yolo
            // conversation. Never trust task-level permission fields.
            mode: ScheduledRunMode::Yolo,
            allow_shell,
            trust_mode: true,
            auto_approve: true,
        };
        let session_id = match self.runtime.create_session(profile) {
            Ok(session_id) => session_id,
            Err(error) => return failed(error),
        };

        // Report the conversation identity ahead of ThreadLinked so a run
        // that fails or is interrupted before its first turn still links to
        // its session. The channel is fire-and-forget: persistence happens
        // when the manager drains events, with no ack before the send below.
        let _ = events.send(TaskExecutionEvent::ThreadCreated {
            thread_id: session_id.clone(),
        });

        let link_events = events.clone();
        let linked_session_id = session_id.clone();
        let result_cancel = cancel.clone();
        let completion = self
            .runtime
            .run_turn(
                &session_id,
                task.prompt().to_string(),
                cancel,
                Box::new(move |turn_id| {
                    let events = link_events.clone();
                    let thread_id = linked_session_id.clone();
                    Box::pin(async move {
                        events
                            .send(TaskExecutionEvent::ThreadLinked { thread_id, turn_id })
                            .map_err(|_| anyhow!("scheduled task event channel closed"))
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
    use std::collections::{BTreeMap, HashMap, VecDeque};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use anyhow::{Result, bail};
    use async_trait::async_trait;
    use deepseek_tui::automation_manager::{
        AutomationManager, AutomationStatus, CreateAutomationRequest, run_now_shared,
    };
    use deepseek_tui::core::events::TurnOutcomeStatus;
    use deepseek_tui::task_manager::{
        NewTaskRequest, SharedTaskManager, TaskManager, TaskManagerConfig, TaskRecord, TaskStatus,
    };
    use tokio::sync::Notify;
    use tokio_util::sync::CancellationToken;

    use crate::features::assistant::engine_pool::ScheduledTurnCompletion;
    use crate::features::sessions::{ScheduledRunMode, ScheduledRunProfile};
    use crate::platform::credential_store::{CredentialEditAction, CredentialState};
    use crate::platform::prefs::{ModelPreset, SavedModel};

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
        model_bindings: Mutex<HashMap<(String, String), String>>,
        next_session: AtomicUsize,
        yolo_allow_shell: AtomicBool,
        organize_calls: AtomicUsize,
        organize_error: Mutex<Option<String>>,
        organize_hang_after_cancel: AtomicBool,
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
                model_bindings: Mutex::new(HashMap::new()),
                next_session: AtomicUsize::new(1),
                yolo_allow_shell: AtomicBool::new(true),
                organize_calls: AtomicUsize::new(0),
                organize_error: Mutex::new(None),
                organize_hang_after_cancel: AtomicBool::new(false),
                started: Notify::new(),
            }
        }

        fn set_yolo_allow_shell(&self, allow_shell: bool) {
            self.yolo_allow_shell.store(allow_shell, Ordering::SeqCst);
        }

        fn set_organize_hang_after_cancel(&self, hang: bool) {
            self.organize_hang_after_cancel
                .store(hang, Ordering::SeqCst);
        }

        fn set_organize_error(&self, error: Option<String>) {
            *self.organize_error.lock().unwrap() = error;
        }

        fn organize_call_count(&self) -> usize {
            self.organize_calls.load(Ordering::SeqCst)
        }

        fn profiles(&self) -> Vec<(String, ScheduledRunProfile)> {
            self.profiles.lock().unwrap().clone()
        }

        fn calls(&self) -> Vec<RunCall> {
            self.calls.lock().unwrap().clone()
        }

        fn bind_model_id(&self, automation_id: &str, model: &str, model_id: &str) {
            self.model_bindings.lock().unwrap().insert(
                (automation_id.to_string(), model.to_string()),
                model_id.to_string(),
            );
        }
    }

    #[async_trait]
    impl ScheduledConversationRuntime for ScriptedRuntime {
        fn yolo_allow_shell(&self) -> bool {
            self.yolo_allow_shell.load(Ordering::SeqCst)
        }

        fn model_id_for_automation(&self, automation_id: &str, model: &str) -> Option<String> {
            self.model_bindings
                .lock()
                .unwrap()
                .get(&(automation_id.to_string(), model.to_string()))
                .cloned()
        }

        fn create_session(&self, profile: ScheduledRunProfile) -> Result<String> {
            let mut profiles = self.profiles.lock().unwrap();
            let number = self.next_session.fetch_add(1, Ordering::SeqCst);
            let session_id = format!("sched-fake-{number}");
            profiles.push((session_id.clone(), profile));
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

        async fn organize_memory(&self, cancel: CancellationToken) -> Result<MemoryOrganizeReport> {
            self.organize_calls.fetch_add(1, Ordering::SeqCst);
            if self.organize_hang_after_cancel.load(Ordering::SeqCst) {
                // 模拟长时间无返回的 LLM 调用：通知测试已进入整理，再触发取消
                // 并永久挂起。修复前 executor 会等在这里，删除运行中任务的
                // 15s 终态等待几乎必然超时。
                self.started.notify_one();
                cancel.cancel();
                std::future::pending::<()>().await;
            }
            if let Some(error) = self.organize_error.lock().unwrap().clone() {
                bail!(error);
            }
            Ok(organize_report())
        }
    }

    fn organize_report() -> MemoryOrganizeReport {
        MemoryOrganizeReport {
            started_at: "2026-01-01T00:00:00Z".to_string(),
            finished_at: "2026-01-01T00:01:00Z".to_string(),
            model: "test-model".to_string(),
            scanned: BTreeMap::from([("preferences".to_string(), 3)]),
            deleted: BTreeMap::new(),
            updated: BTreeMap::new(),
            merged: BTreeMap::new(),
            skipped_sensitive: 0,
            no_change: true,
            warnings: Vec::new(),
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

    async fn manager_with_executor(
        executor: Arc<ScheduledChatExecutor>,
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
        };
        let manager = TaskManager::start_with_executor(config, executor).await?;
        Ok((root, manager))
    }

    async fn manager_with_runtime(
        runtime: Arc<ScriptedRuntime>,
    ) -> Result<(TestRoot, SharedTaskManager)> {
        manager_with_executor(Arc::new(ScheduledChatExecutor::new(runtime))).await
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
            allow_shell: Some(false),
            trust_mode: Some(false),
            auto_approve: Some(false),
            owner_session_id: None,
        }
    }

    fn saved_model(id: &str, wire_name: &str) -> SavedModel {
        SavedModel {
            id: id.to_string(),
            name: id.to_string(),
            alias: None,
            preset: ModelPreset::OpenaiCompatible,
            context_window_tokens: None,
            max_output_tokens: None,
            reasoning_effort: None,
            model: wire_name.to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            provider_kind: None,
            vendor: None,
            endpoint_mode: None,
            image_capability_override: Default::default(),
            vision_model_id: None,
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
        assert!(
            bind_profile_model_id(
                &mut ambiguous,
                &[
                    saved_model("first", "wire-model"),
                    saved_model("second", "wire-model"),
                ],
            )
            .is_ok()
        );
        assert_eq!(ambiguous.model_id, None);
    }

    #[tokio::test]
    async fn success_creates_and_links_a_durable_independent_session_before_completion()
    -> Result<()> {
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
        let linked = finished
            .timeline
            .iter()
            .find(|entry| entry.kind == "runtime_link")
            .expect("ThreadLinked must be recorded");
        assert!(linked.summary.contains("sched-fake-1"));

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
            ScheduledRunMode::Yolo,
            "task-level mode and autoApprove must not weaken scheduled Yolo"
        );
        assert!(profiles[0].1.allow_shell);
        assert!(profiles[0].1.trust_mode);
        assert!(profiles[0].1.auto_approve);
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
    async fn executor_injects_stable_model_id_from_automation_binding() -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([Script::Complete {
            turn_id: "bound-turn".to_string(),
        }]));
        let (root, manager) = manager_with_runtime(runtime.clone()).await?;
        let automation_manager = Arc::new(tokio::sync::Mutex::new(AutomationManager::open(
            root.0.join("automations"),
        )?));
        let automation =
            automation_manager
                .lock()
                .await
                .create_automation(CreateAutomationRequest {
                    name: "bound automation".to_string(),
                    prompt: "run with bound model".to_string(),
                    rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                    cwds: Vec::new(),
                    model: Some("scheduled-model".to_string()),
                    mode: Some("yolo".to_string()),
                    allow_shell: Some(false),
                    trust_mode: Some(false),
                    auto_approve: Some(false),
                    delivery_mode: None,
                    status: Some(AutomationStatus::Paused),
                })?;
        runtime.bind_model_id(&automation.id, "scheduled-model", "deepseek-b");

        let run = run_now_shared(&automation_manager, &automation.id, &manager).await?;
        let task_id = run.task_id.as_deref().expect("task id");
        wait_for_terminal_state(&manager, task_id, std::time::Duration::from_secs(5)).await?;

        let profiles = runtime.profiles();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].1.task_id, automation.id);
        assert_eq!(profiles[0].1.model, "scheduled-model");
        assert_eq!(profiles[0].1.model_id.as_deref(), Some("deepseek-b"));
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
    async fn two_runs_of_one_automation_get_independent_conversations() -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([
            Script::Complete {
                turn_id: "turn-first-run".to_string(),
            },
            Script::Complete {
                turn_id: "turn-second-run".to_string(),
            },
        ]));
        let (root, task_manager) = manager_with_runtime(runtime.clone()).await?;
        let automation_manager = Arc::new(tokio::sync::Mutex::new(AutomationManager::open(
            root.0.join("automations"),
        )?));
        let automation =
            automation_manager
                .lock()
                .await
                .create_automation(CreateAutomationRequest {
                    name: "daily brief".to_string(),
                    prompt: "prepare the brief".to_string(),
                    rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                    cwds: Vec::new(),
                    model: Some("scheduled-model".to_string()),
                    mode: Some("yolo".to_string()),
                    allow_shell: Some(false),
                    trust_mode: Some(false),
                    auto_approve: Some(false),
                    delivery_mode: None,
                    status: Some(AutomationStatus::Paused),
                })?;

        let first_run = run_now_shared(&automation_manager, &automation.id, &task_manager).await?;
        let first_task_id = first_run.task_id.as_deref().expect("first task id");
        wait_for_terminal_state(
            &task_manager,
            first_task_id,
            std::time::Duration::from_secs(5),
        )
        .await?;

        let second_run = run_now_shared(&automation_manager, &automation.id, &task_manager).await?;
        let second_task_id = second_run.task_id.as_deref().expect("second task id");
        wait_for_terminal_state(
            &task_manager,
            second_task_id,
            std::time::Duration::from_secs(5),
        )
        .await?;

        assert_ne!(first_task_id, second_task_id, "run task ids stay distinct");
        let profiles = runtime.profiles();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].1.task_id, automation.id);
        assert_eq!(profiles[1].1.task_id, automation.id);
        assert_ne!(
            profiles[0].0, profiles[1].0,
            "every scheduled run must create an independent conversation"
        );
        task_manager.shutdown();
        Ok(())
    }

    #[tokio::test]
    async fn shell_permission_is_resolved_again_for_each_scheduled_run() -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([
            Script::Complete {
                turn_id: "turn-shell-on".to_string(),
            },
            Script::Complete {
                turn_id: "turn-shell-off".to_string(),
            },
        ]));
        let (_root, manager) = manager_with_runtime(runtime.clone()).await?;

        let first = manager.add_task(request("first")).await?;
        wait_for_terminal_state(&manager, &first.id, std::time::Duration::from_secs(5)).await?;

        runtime.set_yolo_allow_shell(false);
        let second = manager.add_task(request("second")).await?;
        wait_for_terminal_state(&manager, &second.id, std::time::Duration::from_secs(5)).await?;

        let profiles = runtime.profiles();
        assert_eq!(profiles.len(), 2);
        assert!(profiles[0].1.allow_shell);
        assert!(!profiles[1].1.allow_shell);
        assert!(profiles.iter().all(|(_, profile)| {
            profile.mode == ScheduledRunMode::Yolo && profile.trust_mode && profile.auto_approve
        }));
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

    #[tokio::test]
    async fn memory_organize_run_completes_without_a_session_or_engine_turn() -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([]));
        let executor = Arc::new(
            ScheduledChatExecutor::new(runtime.clone()).with_kind_resolver(Arc::new(|_| {
                Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE.to_string())
            })),
        );
        let (_root, manager) = manager_with_executor(executor).await?;

        let queued = manager.add_task(request("organize memory")).await?;
        let finished =
            wait_for_terminal_state(&manager, &queued.id, std::time::Duration::from_secs(5))
                .await?;

        assert_eq!(finished.status, TaskStatus::Completed);
        assert!(finished.error.is_none());
        assert_eq!(
            finished.result_summary.as_deref(),
            Some("Memory organize completed: scanned 3, no changes needed"),
        );
        // 整理类运行不建会话也不跑引擎轮：thread/turn 保持缺省。
        assert_eq!(finished.thread_id, None, "must not create a session");
        assert_eq!(finished.turn_id, None);
        assert!(
            runtime.profiles().is_empty(),
            "create_session must not be called"
        );
        assert!(runtime.calls().is_empty(), "run_turn must not be called");
        assert_eq!(runtime.organize_call_count(), 1);
        manager.shutdown();
        Ok(())
    }

    #[tokio::test]
    async fn memory_organize_failure_maps_to_a_failed_run() -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([]));
        runtime.set_organize_error(Some("model unavailable".to_string()));
        let executor = Arc::new(
            ScheduledChatExecutor::new(runtime.clone()).with_kind_resolver(Arc::new(|_| {
                Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE.to_string())
            })),
        );
        let (_root, manager) = manager_with_executor(executor).await?;

        let queued = manager.add_task(request("organize memory")).await?;
        let finished =
            wait_for_terminal_state(&manager, &queued.id, std::time::Duration::from_secs(5))
                .await?;

        assert_eq!(finished.status, TaskStatus::Failed);
        assert_eq!(
            finished.error.as_deref(),
            Some("memory organize: model unavailable")
        );
        assert_eq!(finished.thread_id, None);
        assert!(
            runtime.profiles().is_empty(),
            "failed organize must not leave a session either"
        );
        manager.shutdown();
        Ok(())
    }

    #[tokio::test]
    async fn memory_organize_run_cancels_without_waiting_for_the_llm_call() -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([]));
        runtime.set_organize_hang_after_cancel(true);
        let executor = Arc::new(
            ScheduledChatExecutor::new(runtime.clone()).with_kind_resolver(Arc::new(|_| {
                Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE.to_string())
            })),
        );
        let (_root, manager) = manager_with_executor(executor).await?;

        let queued = manager.add_task(request("organize memory")).await?;
        // 等 runtime 进入永不返回的整理调用，再取消：修复前 executor 会一直
        // 等 75s 的整理调用本身，删除运行中任务的 15s 终态等待必然超时。
        runtime.started.notified().await;
        manager.cancel_task(&queued.id).await?;
        let finished =
            wait_for_terminal_state(&manager, &queued.id, std::time::Duration::from_secs(5))
                .await?;

        assert_eq!(finished.status, TaskStatus::Canceled);
        assert_eq!(finished.thread_id, None);
        assert_eq!(
            runtime.organize_call_count(),
            1,
            "cancel must not wait for the in-flight organize call"
        );
        manager.shutdown();
        Ok(())
    }
}
