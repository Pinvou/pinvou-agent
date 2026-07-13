use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::Weekday;
use deepseek_tui::automation_manager::{
    spawn_scheduler, AutomationManager, AutomationManagerOptions, AutomationRecord,
    AutomationRunRecord, AutomationRunRetentionGuard, AutomationRunStatus, AutomationSchedule,
    AutomationSchedulerConfig, AutomationStatus, CreateAutomationRequest, SharedAutomationManager,
    UpdateAutomationRequest,
};
use deepseek_tui::task_manager::{SharedTaskManager, TaskManager, TaskManagerConfig, TaskStatus};
use parking_lot::{Mutex as ParkingMutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::bridge::prefs::UserPrefs;
use crate::bridge::sessions::{SessionKind, SessionStore};
use crate::bridge::Pinvou3Bridge;
use crate::engine_pool::EnginePool;
use crate::scheduled_executor::ScheduledChatExecutor;

const DELETE_CANCEL_TIMEOUT: Duration = Duration::from_secs(15);
const SCHEDULED_TASK_PRUNE_INTERVAL: Duration = Duration::from_secs(60 * 60);
const SCHEDULED_RUN_READ_STATE_SCHEMA_VERSION: u32 = 2;
const SCHEDULED_EXECUTION_MODE: &str = "yolo";

fn scheduled_run_read_state_schema_version() -> u32 {
    SCHEDULED_RUN_READ_STATE_SCHEMA_VERSION
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ScheduledRunReadRegistry {
    #[serde(default = "scheduled_run_read_state_schema_version")]
    schema_version: u32,
    #[serde(default)]
    viewed_runs: HashMap<String, HashSet<String>>,
}

impl Default for ScheduledRunReadRegistry {
    fn default() -> Self {
        Self {
            schema_version: SCHEDULED_RUN_READ_STATE_SCHEMA_VERSION,
            viewed_runs: HashMap::new(),
        }
    }
}

#[derive(Clone)]
struct ScheduledRunReadStore {
    path: Arc<PathBuf>,
    registry: Arc<RwLock<ScheduledRunReadRegistry>>,
}

impl ScheduledRunReadStore {
    fn open(path: PathBuf) -> Result<Self> {
        let mut migrated = false;
        let registry = match std::fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<ScheduledRunReadRegistry>(&raw) {
                Ok(registry)
                    if registry.schema_version == SCHEDULED_RUN_READ_STATE_SCHEMA_VERSION =>
                {
                    registry
                }
                Ok(registry)
                    if registry.schema_version < SCHEDULED_RUN_READ_STATE_SCHEMA_VERSION =>
                {
                    migrated = true;
                    ScheduledRunReadRegistry::default()
                }
                Ok(registry) => {
                    quarantine_invalid_read_state(
                        &path,
                        &format!(
                            "schema v{} is newer than supported v{}",
                            registry.schema_version, SCHEDULED_RUN_READ_STATE_SCHEMA_VERSION
                        ),
                    );
                    ScheduledRunReadRegistry::default()
                }
                Err(error) => {
                    quarantine_invalid_read_state(&path, &format!("invalid JSON: {error}"));
                    ScheduledRunReadRegistry::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ScheduledRunReadRegistry::default()
            }
            Err(error) => {
                log::warn!(
                    "Unable to read scheduled run state {}: {error}; treating all runs as unread",
                    path.display()
                );
                ScheduledRunReadRegistry::default()
            }
        };
        let store = Self {
            path: Arc::new(path),
            registry: Arc::new(RwLock::new(registry)),
        };
        if migrated {
            if let Err(error) = store.persist(&store.registry.read()) {
                log::warn!(
                    "Unable to persist migrated scheduled run state {}: {error:#}",
                    store.path.display()
                );
            }
        }
        Ok(store)
    }

    fn is_viewed(&self, automation_id: &str, run_id: &str) -> bool {
        self.registry
            .read()
            .viewed_runs
            .get(automation_id)
            .is_some_and(|runs| runs.contains(run_id))
    }

    fn mark_viewed(&self, automation_id: &str, run_id: &str) -> Result<()> {
        if automation_id.trim().is_empty() || run_id.trim().is_empty() {
            bail!("scheduled automation and run ids cannot be empty");
        }
        let mut registry = self.registry.write();
        let inserted = registry
            .viewed_runs
            .entry(automation_id.to_string())
            .or_default()
            .insert(run_id.to_string());
        if !inserted {
            return Ok(());
        }
        if let Err(error) = self.persist(&registry) {
            if let Some(runs) = registry.viewed_runs.get_mut(automation_id) {
                runs.remove(run_id);
                if runs.is_empty() {
                    registry.viewed_runs.remove(automation_id);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    fn remove_automation(&self, automation_id: &str) -> Result<()> {
        let mut registry = self.registry.write();
        let Some(removed) = registry.viewed_runs.remove(automation_id) else {
            return Ok(());
        };
        if let Err(error) = self.persist(&registry) {
            registry
                .viewed_runs
                .insert(automation_id.to_string(), removed);
            return Err(error);
        }
        Ok(())
    }

    fn compact(&self, automation_id: &str, current_run_ids: &HashSet<String>) -> Result<()> {
        let mut registry = self.registry.write();
        let Some(existing) = registry.viewed_runs.get(automation_id).cloned() else {
            return Ok(());
        };
        let retained = existing
            .iter()
            .filter(|run_id| current_run_ids.contains(*run_id))
            .cloned()
            .collect::<HashSet<_>>();
        if retained == existing {
            return Ok(());
        }
        if retained.is_empty() {
            registry.viewed_runs.remove(automation_id);
        } else {
            registry
                .viewed_runs
                .insert(automation_id.to_string(), retained);
        }
        if let Err(error) = self.persist(&registry) {
            registry
                .viewed_runs
                .insert(automation_id.to_string(), existing);
            return Err(error);
        }
        Ok(())
    }

    fn persist(&self, registry: &ScheduledRunReadRegistry) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("create scheduled run read-state dir {}", parent.display())
            })?;
        }
        let payload =
            serde_json::to_vec_pretty(registry).context("serialize scheduled run read state")?;
        deepseek_tui::utils::write_atomic(self.path.as_ref(), &payload)
            .with_context(|| format!("write scheduled run read state {}", self.path.display()))
    }
}

fn quarantine_invalid_read_state(path: &std::path::Path, reason: &str) {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("scheduled-run-read-state.json");
    let quarantine_path = path.with_file_name(format!("{file_name}.invalid-{timestamp}"));
    match std::fs::rename(path, &quarantine_path) {
        Ok(()) => log::warn!(
            "Quarantined scheduled run state {} to {} ({reason}); treating all runs as unread",
            path.display(),
            quarantine_path.display()
        ),
        Err(error) => log::warn!(
            "Invalid scheduled run state {} ({reason}) could not be quarantined: {error}; treating all runs as unread",
            path.display()
        ),
    }
}

pub struct ScheduledTaskState {
    automations: SharedAutomationManager,
    #[allow(dead_code)]
    task_manager: Option<SharedTaskManager>,
    sessions: SessionStore,
    read_state: ScheduledRunReadStore,
    operation_locks: ParkingMutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
    last_task_prune: ParkingMutex<Option<Instant>>,
    pool: Option<EnginePool>,
    fallback_model: String,
    #[allow(dead_code)]
    scheduler_cancel: Option<CancellationToken>,
    #[allow(dead_code)]
    scheduler_handle: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTaskDto {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub rrule: String,
    pub schedule_label: String,
    pub status: String,
    pub next_run_at: Option<String>,
    pub last_run_at: Option<String>,
    pub cwds: Vec<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub allow_shell: bool,
    pub trust_mode: bool,
    pub auto_approve: bool,
    pub has_unread_runs: bool,
    pub is_running: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedScheduledTaskDto {
    #[serde(flatten)]
    pub task: ScheduledTaskDto,
    pub deleted_session_ids: Vec<String>,
}

pub type ScheduledTaskDetailDto = ScheduledTaskDto;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateScheduledTaskInput {
    pub name: String,
    pub prompt: String,
    pub rrule: String,
    #[serde(default)]
    pub cwds: Vec<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub allow_shell: Option<bool>,
    #[serde(default)]
    pub trust_mode: Option<bool>,
    #[serde(default)]
    pub auto_approve: Option<bool>,
    #[serde(default)]
    pub paused: Option<bool>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateScheduledTaskInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub rrule: Option<String>,
    #[serde(default)]
    pub cwds: Option<Vec<String>>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub allow_shell: Option<bool>,
    #[serde(default)]
    pub trust_mode: Option<bool>,
    #[serde(default)]
    pub auto_approve: Option<bool>,
    #[serde(default)]
    pub paused: Option<bool>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledRunDto {
    pub id: String,
    pub automation_id: String,
    pub session_id: Option<String>,
    pub scheduled_for: String,
    pub status: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub task_id: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub error: Option<String>,
    pub unread: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledRunViewedDto {
    pub automation_id: String,
    pub run_id: String,
    pub has_unread_runs: bool,
}

const SCHEDULED_TASK_CHAT_PROMPT: &str = r#"我想创建一个 Pinvou 定时任务。请通过提问帮我确定方案，回复保持简短，不要长篇解释。

这是一个纯对话收集流程。不要调用任何工具，不要写文件，不要读写 ~/.pinvou3，也不要手动创建 automations JSON。信息完整后只输出给前端解析的任务参数，前端会立即创建并打开任务详情，不再要求用户二次确认。

请一次只问我一个问题，并依次确认这些信息：
1. 任务要做什么。
2. 什么时候运行。支持每 N 分钟、每 N 小时、每天指定时间、每周指定星期和时间。
3. 工作目录。没有明确要求时可以留空，但必须把任务设为暂停，等用户选择目录后再启用。
4. 是否允许 shell 或文件操作。
5. 权限选项，包括 trustMode 和 autoApprove。

整理草稿时，请把时间转换成 rrule：
- 每 10 分钟一次：FREQ=MINUTELY;INTERVAL=10
- 每 6 小时一次：FREQ=HOURLY;INTERVAL=6
- 每天 08:30：FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30
- 每周一、三 09:30：FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=9;BYMINUTE=30

当信息足够时，请直接给出最终任务参数，并使用下面这种完整代码块格式：
```scheduled-task-draft
{
  "name": "AI 招聘情报晨报",
  "prompt": "检索并汇总...",
  "rrule": "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30",
  "cwds": [],
  "allowShell": false,
  "trustMode": false,
  "autoApprove": false,
  "paused": true
}
```
输出代码块后不要继续提问，也不要假装自己调用了创建命令；前端会负责创建任务。"#;

pub fn scheduled_automation_root() -> std::path::PathBuf {
    crate::bridge::paths::pinvou3_home().join("automations")
}

#[derive(Clone)]
struct ScheduledConversationRetentionGuard {
    sessions: SessionStore,
}

impl ScheduledConversationRetentionGuard {
    fn ensure_live_owner(&self, session_id: &str, task_id: &str) -> Result<bool> {
        let Some(profile) = self.sessions.scheduled_profile(session_id) else {
            return Ok(false);
        };
        if profile.task_id != task_id {
            bail!(
                "scheduled retention ownership mismatch: session {session_id} belongs to {}, not {task_id}",
                profile.task_id
            );
        }
        self.sessions
            .load(session_id)
            .with_context(|| format!("load retained scheduled session {session_id}"))?;
        Ok(true)
    }
}

impl AutomationRunRetentionGuard for ScheduledConversationRetentionGuard {
    fn retain_terminal_run(&self, run: &AutomationRunRecord) -> Result<bool> {
        if let Some(thread_id) = run.thread_id.as_deref() {
            if let Some(task_id) = run.task_id.as_deref() {
                let task_sessions = self.sessions.scheduled_session_ids_for_task(task_id);
                if task_sessions.len() > 1 {
                    bail!(
                        "scheduled retention ownership is ambiguous for task {task_id}: {task_sessions:?}"
                    );
                }
                if let Some(expected_thread_id) = task_sessions.first() {
                    if expected_thread_id != thread_id {
                        bail!(
                            "scheduled retention thread mismatch for task {task_id}: run links {thread_id}, profile links {expected_thread_id}"
                        );
                    }
                }
                if self.ensure_live_owner(thread_id, task_id)? {
                    return Ok(true);
                }
                if !task_sessions.is_empty() {
                    bail!(
                        "scheduled retention profile for task {task_id} disappeared while inspecting {thread_id}"
                    );
                }
            } else if self.sessions.scheduled_profile(thread_id).is_some() {
                bail!(
                    "scheduled retention run '{}' links session {thread_id} without a task id",
                    run.id
                );
            }

            return match self.sessions.session_kind(thread_id) {
                Ok(SessionKind::ScheduledRun) => bail!(
                    "scheduled retention session {thread_id} has payload ownership but no readable profile"
                ),
                Ok(SessionKind::Chat) => Ok(false),
                Err(error) => Err(error)
                    .with_context(|| format!("inspect retention thread {thread_id}")),
            };
        }

        let Some(task_id) = run.task_id.as_deref() else {
            return Ok(false);
        };
        let task_sessions = self.sessions.scheduled_session_ids_for_task(task_id);
        match task_sessions.as_slice() {
            [] => Ok(false),
            [session_id] => self.ensure_live_owner(session_id, task_id),
            _ => bail!(
                "scheduled retention ownership is ambiguous for unlinked task {task_id}: {task_sessions:?}"
            ),
        }
    }
}

fn open_scheduled_automation_manager(
    root: PathBuf,
    sessions: &SessionStore,
) -> Result<AutomationManager> {
    AutomationManager::open_with_options(
        root,
        AutomationManagerOptions {
            retention_guard: Some(Arc::new(ScheduledConversationRetentionGuard {
                sessions: sessions.clone(),
            })),
            ..AutomationManagerOptions::default()
        },
    )
}

#[allow(dead_code)]
pub fn scheduled_task_data_root() -> std::path::PathBuf {
    crate::bridge::paths::pinvou3_home().join("tasks")
}

impl ScheduledTaskState {
    #[allow(dead_code)]
    pub fn boot_read_only() -> Result<Self> {
        let sessions = SessionStore::boot()?;
        sessions.reconcile_scheduled_profiles()?;
        let read_state =
            ScheduledRunReadStore::open(crate::bridge::paths::scheduled_run_read_state_path())?;
        let manager = open_scheduled_automation_manager(scheduled_automation_root(), &sessions)?;
        let fallback_model = default_automation_model(None);
        normalize_legacy_automations(&manager, &fallback_model)?;
        Ok(Self {
            automations: Arc::new(tokio::sync::Mutex::new(manager)),
            task_manager: None,
            sessions,
            read_state,
            operation_locks: ParkingMutex::new(HashMap::new()),
            last_task_prune: ParkingMutex::new(None),
            pool: None,
            fallback_model,
            scheduler_cancel: None,
            scheduler_handle: None,
        })
    }

    pub async fn boot_runtime(
        bridge: &Pinvou3Bridge,
        pool: EnginePool,
        sessions: SessionStore,
    ) -> Result<Self> {
        sessions.reconcile_scheduled_profiles()?;
        let read_state =
            ScheduledRunReadStore::open(crate::bridge::paths::scheduled_run_read_state_path())?;
        let fallback_model = default_automation_model(Some(bridge));
        let manager = open_scheduled_automation_manager(scheduled_automation_root(), &sessions)?;
        normalize_legacy_automations(&manager, &fallback_model)?;
        let automations = Arc::new(tokio::sync::Mutex::new(manager));
        let task_cfg = TaskManagerConfig {
            data_dir: scheduled_task_data_root(),
            worker_count: 1,
            default_workspace: crate::bridge::paths::user_home_dir(),
            default_model: fallback_model.clone(),
            default_mode: SCHEDULED_EXECUTION_MODE.to_string(),
            allow_shell: bridge.allow_shell(),
            trust_mode: false,
            max_subagents: 2,
        };
        let executor = Arc::new(ScheduledChatExecutor::from_services(
            sessions.clone(),
            pool.clone(),
        ));
        let task_manager = TaskManager::start_with_executor(task_cfg, executor).await?;
        let initial_task_prune = {
            let manager = automations.lock().await;
            manager.reconcile_run_statuses(&task_manager).await?;
            manager.protected_task_ids()
        };
        let initial_task_prune_at = match initial_task_prune {
            Ok(protected_task_ids) => {
                match task_manager.prune_terminal_tasks(&protected_task_ids).await {
                    Ok(pruned) => {
                        if !pruned.is_empty() {
                            log::info!(
                                "Pruned {} unreferenced scheduled task records during startup",
                                pruned.len()
                            );
                        }
                        Some(Instant::now())
                    }
                    Err(error) => {
                        log::warn!(
                        "Unable to prune unreferenced scheduled task records during startup: {error:#}"
                    );
                        None
                    }
                }
            }
            Err(error) => {
                log::warn!(
                    "Scheduled task pruning skipped during startup because ownership is uncertain: {error:#}"
                );
                None
            }
        };
        let cancel = CancellationToken::new();
        let scheduler_handle = spawn_scheduler(
            automations.clone(),
            task_manager.clone(),
            cancel.clone(),
            AutomationSchedulerConfig::default(),
        );
        Ok(Self {
            automations,
            task_manager: Some(task_manager),
            sessions,
            read_state,
            operation_locks: ParkingMutex::new(HashMap::new()),
            last_task_prune: ParkingMutex::new(initial_task_prune_at),
            pool: Some(pool),
            fallback_model,
            scheduler_cancel: Some(cancel),
            scheduler_handle: Some(scheduler_handle),
        })
    }

    #[allow(dead_code)]
    pub fn automations(&self) -> SharedAutomationManager {
        self.automations.clone()
    }

    fn operation_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.operation_locks.lock();
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(id).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(id.to_string(), Arc::downgrade(&lock));
        lock
    }

    async fn lock_operation(&self, id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        self.operation_lock(id).lock_owned().await
    }

    async fn create_task(
        &self,
        input: CreateScheduledTaskInput,
    ) -> Result<ScheduledTaskDto, String> {
        let manager = self.automations.lock().await;
        let created = manager
            .create_automation(build_create_request(
                input,
                current_automation_model(&self.fallback_model),
            )?)
            .map_err(|err| format!("Failed to create scheduled task: {err}"))?;
        Ok(map_scheduled_task(created))
    }

    async fn update_task(
        &self,
        id: String,
        input: UpdateScheduledTaskInput,
    ) -> Result<ScheduledTaskDto, String> {
        let _operation = self.lock_operation(&id).await;
        let manager = self.automations.lock().await;
        let current = manager
            .get_automation(&id)
            .map_err(|err| format!("Failed to update scheduled task '{id}': {err}"))?;
        let updated = manager
            .update_automation(&id, build_update_request(input, &current)?)
            .map_err(|err| format!("Failed to update scheduled task '{id}': {err}"))?;
        map_scheduled_task_from_manager(&manager, updated, &self.sessions, &self.read_state)
            .map_err(|err| format!("Failed to read scheduled task runs for '{id}': {err}"))
    }

    async fn pause_task(&self, id: String) -> Result<ScheduledTaskDto, String> {
        let _operation = self.lock_operation(&id).await;
        let manager = self.automations.lock().await;
        let updated = manager
            .pause_automation(&id)
            .map_err(|err| format!("Failed to pause scheduled task '{id}': {err}"))?;
        map_scheduled_task_from_manager(&manager, updated, &self.sessions, &self.read_state)
            .map_err(|err| format!("Failed to read scheduled task runs for '{id}': {err}"))
    }

    async fn resume_task(&self, id: String) -> Result<ScheduledTaskDto, String> {
        let _operation = self.lock_operation(&id).await;
        let manager = self.automations.lock().await;
        let current = manager
            .get_automation(&id)
            .map_err(|err| format!("Failed to resume scheduled task '{id}': {err}"))?;
        require_scheduled_workspace(&current.cwds)?;
        let updated = manager
            .resume_automation(&id)
            .map_err(|err| format!("Failed to resume scheduled task '{id}': {err}"))?;
        map_scheduled_task_from_manager(&manager, updated, &self.sessions, &self.read_state)
            .map_err(|err| format!("Failed to read scheduled task runs for '{id}': {err}"))
    }

    async fn delete_task(&self, id: String) -> Result<DeletedScheduledTaskDto, String> {
        // Keep the per-automation gate for the complete destructive workflow. A
        // cancellation-style outer timeout could release the gate after only
        // some sessions were removed, allowing update/run-now into partial state.
        let _operation = self.lock_operation(&id).await;
        self.delete_task_inner(&id)
            .await
            .map_err(|err| format!("Failed to delete scheduled task '{id}': {err:#}"))
    }

    async fn run_task_now(&self, id: String) -> Result<ScheduledRunDto, String> {
        let _operation = self.lock_operation(&id).await;
        let manager = self.automations.lock().await;
        let current = manager
            .get_automation(&id)
            .map_err(|err| format!("Failed to run scheduled task '{id}': {err}"))?;
        require_scheduled_workspace(&current.cwds)?;
        let task_manager = self
            .task_manager
            .as_ref()
            .ok_or_else(|| "Scheduled task runtime is unavailable".to_string())?;
        let run = manager
            .run_now(&id, task_manager)
            .await
            .map_err(|err| format!("Failed to run scheduled task '{id}': {err}"))?;
        Ok(map_scheduled_run(run, &self.sessions, &self.read_state))
    }

    async fn mark_run_viewed(
        &self,
        automation_id: String,
        run_id: String,
    ) -> Result<ScheduledRunViewedDto, String> {
        let manager = self.automations.lock().await;
        manager
            .get_automation(&automation_id)
            .map_err(|err| format!("Failed to read scheduled task '{automation_id}': {err}"))?;
        let runs = manager.list_runs(&automation_id, None).map_err(|err| {
            format!("Failed to list scheduled task runs for '{automation_id}': {err}")
        })?;
        let run = runs.iter().find(|run| run.id == run_id).ok_or_else(|| {
            format!("Scheduled run '{run_id}' does not belong to task '{automation_id}'")
        })?;
        ensure_scheduled_run_can_be_marked_viewed(run, &self.sessions)
            .map_err(|err| err.to_string())?;
        drop(manager);
        compact_viewed_runs(&self.read_state, &automation_id, &runs);
        self.read_state
            .mark_viewed(&automation_id, &run_id)
            .map_err(|err| format!("Failed to mark scheduled run '{run_id}' as viewed: {err}"))?;
        Ok(ScheduledRunViewedDto {
            automation_id,
            run_id,
            has_unread_runs: has_unread_scheduled_runs(&runs, &self.sessions, &self.read_state),
        })
    }

    async fn delete_task_inner(&self, id: &str) -> Result<DeletedScheduledTaskDto> {
        {
            let manager = self.automations.lock().await;
            manager
                .pause_automation(id)
                .with_context(|| format!("pause automation {id}"))?;
        }

        self.reconcile_runs().await?;
        let runs = {
            let manager = self.automations.lock().await;
            manager.list_runs(id, None)?
        };
        self.cancel_active_run_tasks(&runs).await?;
        self.reconcile_runs().await?;

        let runs = {
            let manager = self.automations.lock().await;
            manager.list_runs(id, None)?
        };
        let owned_sessions = owned_scheduled_sessions(&runs, &self.sessions)?;
        if !owned_sessions.is_empty() && self.pool.is_none() {
            bail!("engine pool is unavailable while scheduled sessions still exist");
        }
        let mut deleted_session_ids = Vec::with_capacity(owned_sessions.len());
        for (session_id, task_id) in owned_sessions {
            if let Some(pool) = &self.pool {
                pool.delete_scheduled_run(&session_id, &task_id)
                    .await
                    .with_context(|| format!("delete scheduled session {session_id}"))?;
                deleted_session_ids.push(session_id);
            }
        }

        let deleted = {
            let manager = self.automations.lock().await;
            manager.delete_automation(id)?
        };
        if let Err(error) = self.read_state.remove_automation(id) {
            log::warn!(
                "Deleted scheduled task {id}, but failed to remove its viewed-run state: {error:#}"
            );
        }
        Ok(DeletedScheduledTaskDto {
            task: map_scheduled_task(deleted),
            deleted_session_ids,
        })
    }

    async fn reconcile_runs(&self) -> Result<()> {
        let Some(task_manager) = &self.task_manager else {
            return Ok(());
        };
        {
            let manager = self.automations.lock().await;
            manager.reconcile_run_statuses(task_manager).await?;
        }
        self.maybe_prune_terminal_tasks().await;
        Ok(())
    }

    async fn maybe_prune_terminal_tasks(&self) {
        let Some(task_manager) = &self.task_manager else {
            return;
        };
        let now = Instant::now();
        {
            let mut last_prune = self.last_task_prune.lock();
            if last_prune
                .as_ref()
                .is_some_and(|last| now.duration_since(*last) < SCHEDULED_TASK_PRUNE_INTERVAL)
            {
                return;
            }
            // Claim this maintenance window before doing any disk work. On
            // failure it is cleared below so the next reconciliation retries.
            *last_prune = Some(now);
        }

        let protected_task_ids = {
            let manager = self.automations.lock().await;
            manager.protected_task_ids()
        };
        let result = match protected_task_ids {
            Ok(protected_task_ids) => task_manager
                .prune_terminal_tasks(&protected_task_ids)
                .await
                .map(|pruned| {
                    if !pruned.is_empty() {
                        log::info!(
                            "Pruned {} unreferenced scheduled task records",
                            pruned.len()
                        );
                    }
                }),
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            *self.last_task_prune.lock() = None;
            log::warn!(
                "Scheduled task pruning skipped because retained ownership could not be proven: {error:#}"
            );
        }
    }

    async fn cancel_active_run_tasks(&self, runs: &[AutomationRunRecord]) -> Result<()> {
        let active = runs.iter().filter(|run| {
            matches!(
                run.status,
                AutomationRunStatus::Queued | AutomationRunStatus::Running
            )
        });
        let Some(task_manager) = &self.task_manager else {
            if active.count() == 0 {
                return Ok(());
            }
            bail!("task manager is unavailable while automation runs are active");
        };

        for run in active {
            let task_id = run
                .task_id
                .as_deref()
                .with_context(|| format!("active automation run {} has no task id", run.id))?;
            task_manager
                .cancel_task(task_id)
                .await
                .with_context(|| format!("cancel task {task_id}"))?;
            wait_for_task_terminal(task_manager, task_id, DELETE_CANCEL_TIMEOUT).await?;
        }
        Ok(())
    }
}

fn default_automation_model(bridge: Option<&Pinvou3Bridge>) -> String {
    UserPrefs::load()
        .active_model()
        .map(|model| model.model.clone())
        .or_else(|| bridge.map(Pinvou3Bridge::model))
        .unwrap_or_else(|| "default-model".to_string())
}

fn current_automation_model(fallback: &str) -> String {
    UserPrefs::load()
        .active_model()
        .map(|model| model.model.clone())
        .unwrap_or_else(|| fallback.to_string())
}

fn normalize_legacy_automations(manager: &AutomationManager, fallback_model: &str) -> Result<()> {
    for record in manager.list_automations()? {
        let missing_model = record
            .model
            .as_deref()
            .is_none_or(|model| model.trim().is_empty());
        let mode_needs_normalization = record.mode.as_deref() != Some(SCHEDULED_EXECUTION_MODE);
        let active_without_workspace = matches!(record.status, AutomationStatus::Active)
            && !has_scheduled_workspace(&record.cwds);
        let needs_update = missing_model
            || mode_needs_normalization
            || record.allow_shell.is_none()
            || record.trust_mode.is_none()
            || record.auto_approve.is_none()
            || active_without_workspace;
        if !needs_update {
            continue;
        }
        manager
            .update_automation(
                &record.id,
                UpdateAutomationRequest {
                    name: None,
                    prompt: None,
                    rrule: None,
                    cwds: None,
                    model: missing_model.then(|| fallback_model.to_string()),
                    mode: mode_needs_normalization.then(|| SCHEDULED_EXECUTION_MODE.to_string()),
                    allow_shell: record.allow_shell.is_none().then_some(false),
                    trust_mode: record.trust_mode.is_none().then_some(false),
                    auto_approve: record.auto_approve.is_none().then_some(false),
                    status: active_without_workspace.then_some(AutomationStatus::Paused),
                },
            )
            .with_context(|| format!("normalize legacy automation {}", record.id))?;
    }
    Ok(())
}

fn owned_session_id(record: &AutomationRunRecord, sessions: &SessionStore) -> Option<String> {
    let task_id = record.task_id.as_deref()?;
    let thread_id = record.thread_id.as_deref()?;
    sessions
        .scheduled_profile(thread_id)
        .filter(|profile| {
            profile.task_id == task_id && sessions.scheduled_session_exists(thread_id)
        })
        .map(|_| thread_id.to_string())
}

fn ensure_scheduled_run_is_viewable(
    record: &AutomationRunRecord,
    sessions: &SessionStore,
) -> Result<()> {
    if owned_session_id(record, sessions).is_none() {
        bail!(
            "Scheduled run '{}' has no valid conversation to mark as viewed",
            record.id
        );
    }
    Ok(())
}

fn ensure_scheduled_run_can_be_marked_viewed(
    record: &AutomationRunRecord,
    sessions: &SessionStore,
) -> Result<()> {
    if !matches!(record.status, AutomationRunStatus::Completed) {
        bail!(
            "Scheduled run '{}' is not completed and cannot be marked viewed",
            record.id
        );
    }
    ensure_scheduled_run_is_viewable(record, sessions)
}

fn owned_scheduled_sessions(
    runs: &[AutomationRunRecord],
    sessions: &SessionStore,
) -> Result<Vec<(String, String)>> {
    let mut seen = HashSet::new();
    let mut task_ids = HashSet::new();
    let mut owned = Vec::new();
    for run in runs {
        if let Some(task_id) = run.task_id.as_deref() {
            task_ids.insert(task_id.to_string());
        }
        let (Some(task_id), Some(thread_id)) = (run.task_id.as_deref(), run.thread_id.as_deref())
        else {
            continue;
        };
        let Some(profile) = sessions.scheduled_profile(thread_id) else {
            continue;
        };
        if profile.task_id != task_id {
            bail!(
                "scheduled session {thread_id} belongs to task {}, not {task_id}",
                profile.task_id
            );
        }
        if seen.insert(thread_id.to_string()) {
            owned.push((thread_id.to_string(), task_id.to_string()));
        }
    }
    // A crash can create the scheduled session and persist its profile before
    // the base run record receives ThreadCreated. Recover those sessions via
    // the durable execution-task ownership rather than relying only on run links.
    for task_id in task_ids {
        for session_id in sessions.scheduled_session_ids_for_task(&task_id) {
            if seen.insert(session_id.clone()) {
                owned.push((session_id, task_id.clone()));
            }
        }
    }
    owned.sort();
    Ok(owned)
}

#[cfg(test)]
fn delete_owned_scheduled_session(
    sessions: &SessionStore,
    session_id: &str,
    task_id: &str,
) -> Result<String> {
    sessions
        .delete_scheduled_run(session_id, task_id)
        .with_context(|| format!("delete scheduled session {session_id}"))?;
    Ok(session_id.to_string())
}

async fn wait_for_task_terminal(
    task_manager: &SharedTaskManager,
    task_id: &str,
    timeout: Duration,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let task = task_manager.get_task(task_id).await?;
        if matches!(
            task.status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Canceled
        ) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("timed out waiting for task {task_id} to stop");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

impl Drop for ScheduledTaskState {
    fn drop(&mut self) {
        if let Some(cancel) = &self.scheduler_cancel {
            cancel.cancel();
        }
        if let Some(handle) = self.scheduler_handle.take() {
            handle.abort();
        }
    }
}

fn map_scheduled_task(record: AutomationRecord) -> ScheduledTaskDto {
    map_scheduled_task_with_run_state(record, false, false)
}

fn map_scheduled_task_with_run_state(
    record: AutomationRecord,
    has_unread_runs: bool,
    is_running: bool,
) -> ScheduledTaskDto {
    ScheduledTaskDto {
        id: record.id,
        name: record.name,
        prompt: record.prompt,
        rrule: record.rrule.clone(),
        schedule_label: humanize_rrule(&record.rrule),
        status: automation_status_label(&record.status),
        next_run_at: record.next_run_at.map(|value| value.to_rfc3339()),
        last_run_at: record.last_run_at.map(|value| value.to_rfc3339()),
        cwds: record
            .cwds
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        model: record.model,
        mode: record.mode,
        allow_shell: record.allow_shell.unwrap_or(false),
        trust_mode: record.trust_mode.unwrap_or(false),
        auto_approve: record.auto_approve.unwrap_or(false),
        has_unread_runs,
        is_running,
    }
}

fn scheduled_run_is_unread(
    record: &AutomationRunRecord,
    sessions: &SessionStore,
    read_state: &ScheduledRunReadStore,
) -> bool {
    matches!(record.status, AutomationRunStatus::Completed)
        && owned_session_id(record, sessions).is_some()
        && !read_state.is_viewed(&record.automation_id, &record.id)
}

fn has_unread_scheduled_runs(
    records: &[AutomationRunRecord],
    sessions: &SessionStore,
    read_state: &ScheduledRunReadStore,
) -> bool {
    records
        .iter()
        .any(|record| scheduled_run_is_unread(record, sessions, read_state))
}

fn has_running_scheduled_runs(records: &[AutomationRunRecord]) -> bool {
    records.iter().any(|record| {
        matches!(
            record.status,
            AutomationRunStatus::Queued | AutomationRunStatus::Running
        )
    })
}

fn compact_viewed_runs(
    read_state: &ScheduledRunReadStore,
    automation_id: &str,
    records: &[AutomationRunRecord],
) {
    let current_run_ids = records
        .iter()
        .map(|record| record.id.clone())
        .collect::<HashSet<_>>();
    if let Err(error) = read_state.compact(automation_id, &current_run_ids) {
        log::warn!(
            "Unable to compact viewed-run state for scheduled task {automation_id}: {error:#}"
        );
    }
}

fn map_scheduled_task_from_manager(
    manager: &AutomationManager,
    record: AutomationRecord,
    sessions: &SessionStore,
    read_state: &ScheduledRunReadStore,
) -> Result<ScheduledTaskDto> {
    let runs = manager.list_runs(&record.id, None)?;
    compact_viewed_runs(read_state, &record.id, &runs);
    let has_unread_runs = has_unread_scheduled_runs(&runs, sessions, read_state);
    let is_running = has_running_scheduled_runs(&runs);
    Ok(map_scheduled_task_with_run_state(
        record,
        has_unread_runs,
        is_running,
    ))
}

fn map_scheduled_run(
    record: AutomationRunRecord,
    sessions: &SessionStore,
    read_state: &ScheduledRunReadStore,
) -> ScheduledRunDto {
    let session_id = owned_session_id(&record, sessions);
    let unread = matches!(record.status, AutomationRunStatus::Completed)
        && session_id.is_some()
        && !read_state.is_viewed(&record.automation_id, &record.id);
    ScheduledRunDto {
        id: record.id.clone(),
        automation_id: record.automation_id.clone(),
        session_id,
        scheduled_for: record.scheduled_for.to_rfc3339(),
        status: automation_run_status_label(&record.status),
        created_at: record.created_at.to_rfc3339(),
        started_at: record.started_at.map(|value| value.to_rfc3339()),
        ended_at: record.ended_at.map(|value| value.to_rfc3339()),
        task_id: record.task_id,
        thread_id: record.thread_id,
        turn_id: record.turn_id,
        error: record.error,
        unread,
    }
}

fn paused_to_status(paused: bool) -> AutomationStatus {
    if paused {
        AutomationStatus::Paused
    } else {
        AutomationStatus::Active
    }
}

fn normalize_scheduled_workspaces(cwds: Vec<String>) -> Vec<PathBuf> {
    cwds.into_iter()
        .filter_map(|value| {
            let value = value.trim();
            (!value.is_empty()).then(|| PathBuf::from(value))
        })
        .collect()
}

fn has_scheduled_workspace(cwds: &[PathBuf]) -> bool {
    cwds.iter().any(|path| !path.as_os_str().is_empty())
}

fn require_scheduled_workspace(cwds: &[PathBuf]) -> Result<(), String> {
    if has_scheduled_workspace(cwds) {
        Ok(())
    } else {
        Err("Scheduled task requires a workspace before it can run".to_string())
    }
}

fn build_create_request(
    input: CreateScheduledTaskInput,
    default_model: String,
) -> Result<CreateAutomationRequest, String> {
    let model = input
        .model
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(default_model);
    canonical_scheduled_mode(input.mode, None)?;
    let cwds = normalize_scheduled_workspaces(input.cwds);
    let requested_status = paused_to_status(input.paused.unwrap_or(false));
    let status = if matches!(requested_status, AutomationStatus::Active)
        && !has_scheduled_workspace(&cwds)
    {
        AutomationStatus::Paused
    } else {
        requested_status
    };
    Ok(CreateAutomationRequest {
        name: input.name,
        prompt: input.prompt,
        rrule: input.rrule,
        cwds,
        model: Some(model),
        mode: Some(SCHEDULED_EXECUTION_MODE.to_string()),
        allow_shell: Some(input.allow_shell.unwrap_or(false)),
        trust_mode: Some(input.trust_mode.unwrap_or(false)),
        auto_approve: Some(input.auto_approve.unwrap_or(false)),
        status: Some(status),
    })
}

fn build_update_request(
    input: UpdateScheduledTaskInput,
    current: &AutomationRecord,
) -> Result<UpdateAutomationRequest, String> {
    canonical_scheduled_mode(input.mode, None)?;
    let cwds = input.cwds.map(normalize_scheduled_workspaces);
    let requested_status = input.paused.map(paused_to_status);
    let effective_cwds = cwds.as_deref().unwrap_or(&current.cwds);
    let effective_status = requested_status.as_ref().unwrap_or(&current.status);
    let status = if matches!(effective_status, AutomationStatus::Active)
        && !has_scheduled_workspace(effective_cwds)
    {
        Some(AutomationStatus::Paused)
    } else {
        requested_status
    };
    Ok(UpdateAutomationRequest {
        name: input.name,
        prompt: input.prompt,
        rrule: input.rrule,
        cwds,
        model: input.model,
        mode: Some(SCHEDULED_EXECUTION_MODE.to_string()),
        allow_shell: input.allow_shell,
        trust_mode: input.trust_mode,
        auto_approve: input.auto_approve,
        status,
    })
}

fn canonical_scheduled_mode(
    mode: Option<String>,
    default: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(mode) = mode.or_else(|| default.map(str::to_string)) else {
        return Ok(None);
    };
    let mode = mode.trim();
    match mode {
        "agent" | "plan" | "yolo" => Ok(Some(mode.to_string())),
        _ => Err(format!(
            "Scheduled task mode must be exactly one of agent|plan|yolo, got '{mode}'"
        )),
    }
}

fn automation_status_label(status: &deepseek_tui::automation_manager::AutomationStatus) -> String {
    match status {
        deepseek_tui::automation_manager::AutomationStatus::Active => "active",
        deepseek_tui::automation_manager::AutomationStatus::Paused => "paused",
    }
    .to_string()
}

fn automation_run_status_label(
    status: &deepseek_tui::automation_manager::AutomationRunStatus,
) -> String {
    match status {
        deepseek_tui::automation_manager::AutomationRunStatus::Queued => "queued",
        deepseek_tui::automation_manager::AutomationRunStatus::Running => "running",
        deepseek_tui::automation_manager::AutomationRunStatus::Completed => "completed",
        deepseek_tui::automation_manager::AutomationRunStatus::Failed => "failed",
        deepseek_tui::automation_manager::AutomationRunStatus::Canceled => "canceled",
    }
    .to_string()
}

fn is_every_day(days: &[Weekday]) -> bool {
    let all_days = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ];
    days.len() == all_days.len() && all_days.iter().all(|day| days.contains(day))
}

fn weekday_label(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "周一",
        Weekday::Tue => "周二",
        Weekday::Wed => "周三",
        Weekday::Thu => "周四",
        Weekday::Fri => "周五",
        Weekday::Sat => "周六",
        Weekday::Sun => "周日",
    }
}

pub fn humanize_rrule(rrule: &str) -> String {
    match AutomationSchedule::parse_rrule(rrule) {
        Ok(AutomationSchedule::Minutely { interval_minutes }) => {
            if interval_minutes == 1 {
                "每分钟".to_string()
            } else {
                format!("每 {interval_minutes} 分钟")
            }
        }
        Ok(AutomationSchedule::Hourly {
            interval_hours,
            byday,
        }) => {
            let hourly = if interval_hours == 1 {
                "每小时".to_string()
            } else {
                format!("每 {interval_hours} 小时")
            };
            match byday {
                Some(days) if !days.is_empty() => {
                    let labels = days
                        .into_iter()
                        .map(weekday_label)
                        .collect::<Vec<_>>()
                        .join("、");
                    format!("{labels} {hourly}")
                }
                _ => hourly,
            }
        }
        Ok(AutomationSchedule::Weekly {
            byday,
            byhour,
            byminute,
        }) if is_every_day(&byday) => format!("每天 {byhour:02}:{byminute:02}"),
        Ok(AutomationSchedule::Weekly {
            byday,
            byhour,
            byminute,
        }) => {
            let days = byday
                .into_iter()
                .map(weekday_label)
                .collect::<Vec<_>>()
                .join("、");
            format!("{days} {byhour:02}:{byminute:02}")
        }
        Err(_) => rrule.to_string(),
    }
}

#[tauri::command]
pub async fn list_scheduled_tasks(
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<Vec<ScheduledTaskDto>, String> {
    state
        .reconcile_runs()
        .await
        .map_err(|err| format!("Failed to reconcile scheduled task runs: {err}"))?;
    let manager = state.automations.lock().await;
    let records = manager
        .list_automations()
        .map_err(|err| format!("Failed to list scheduled tasks: {err}"))?;
    records
        .into_iter()
        .map(|record| {
            map_scheduled_task_from_manager(&manager, record, &state.sessions, &state.read_state)
        })
        .collect::<Result<Vec<_>>>()
        .map_err(|err| format!("Failed to read scheduled task runs: {err}"))
}

#[tauri::command]
pub async fn read_scheduled_task(
    id: String,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<ScheduledTaskDetailDto, String> {
    let manager = state.automations.lock().await;
    let record = manager
        .get_automation(&id)
        .map_err(|err| format!("Failed to read scheduled task '{id}': {err}"))?;
    map_scheduled_task_from_manager(&manager, record, &state.sessions, &state.read_state)
        .map_err(|err| format!("Failed to read scheduled task runs for '{id}': {err}"))
}

#[tauri::command]
pub async fn list_scheduled_task_runs(
    id: String,
    limit: Option<usize>,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<Vec<ScheduledRunDto>, String> {
    state
        .reconcile_runs()
        .await
        .map_err(|err| format!("Failed to reconcile scheduled task runs: {err}"))?;
    let manager = state.automations.lock().await;
    manager
        .get_automation(&id)
        .map_err(|err| format!("Failed to read scheduled task '{id}': {err}"))?;
    let records = manager
        .list_runs(&id, limit)
        .map_err(|err| format!("Failed to list scheduled task runs for '{id}': {err}"))?;
    if limit.is_none() {
        compact_viewed_runs(&state.read_state, &id, &records);
    }
    Ok(records
        .into_iter()
        .map(|record| map_scheduled_run(record, &state.sessions, &state.read_state))
        .collect())
}

#[tauri::command]
pub async fn create_scheduled_task(
    input: CreateScheduledTaskInput,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<ScheduledTaskDto, String> {
    state.create_task(input).await
}

#[tauri::command]
pub async fn update_scheduled_task(
    id: String,
    input: UpdateScheduledTaskInput,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<ScheduledTaskDto, String> {
    state.update_task(id, input).await
}

#[tauri::command]
pub async fn pause_scheduled_task(
    id: String,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<ScheduledTaskDto, String> {
    state.pause_task(id).await
}

#[tauri::command]
pub async fn resume_scheduled_task(
    id: String,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<ScheduledTaskDto, String> {
    state.resume_task(id).await
}

#[tauri::command]
pub async fn delete_scheduled_task(
    id: String,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<DeletedScheduledTaskDto, String> {
    state.delete_task(id).await
}

#[tauri::command]
pub async fn run_scheduled_task_now(
    id: String,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<ScheduledRunDto, String> {
    state.run_task_now(id).await
}

#[tauri::command]
pub async fn mark_scheduled_run_viewed(
    automation_id: String,
    run_id: String,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<ScheduledRunViewedDto, String> {
    state.mark_run_viewed(automation_id, run_id).await
}

#[tauri::command]
pub fn scheduled_task_chat_prompt() -> Result<String, String> {
    Ok(SCHEDULED_TASK_CHAT_PROMPT.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    impl ScheduledTaskState {
        async fn create_for_test(
            &self,
            input: CreateScheduledTaskInput,
        ) -> Result<ScheduledTaskDto, String> {
            self.create_task(input).await
        }

        async fn update_for_test(
            &self,
            id: String,
            input: UpdateScheduledTaskInput,
        ) -> Result<ScheduledTaskDto, String> {
            self.update_task(id, input).await
        }

        async fn pause_for_test(&self, id: String) -> Result<ScheduledTaskDto, String> {
            self.pause_task(id).await
        }

        async fn resume_for_test(&self, id: String) -> Result<ScheduledTaskDto, String> {
            self.resume_task(id).await
        }

        async fn delete_for_test(&self, id: String) -> Result<DeletedScheduledTaskDto, String> {
            self.delete_task(id).await
        }
    }

    fn temp_home() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-scheduled-tasks-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp home");
        dir
    }

    #[test]
    fn scheduled_task_root_uses_pinvou_home() {
        let _guard = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);
        assert_eq!(scheduled_automation_root(), dir.join("automations"));
        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn humanize_daily_weekly_rrule() {
        let label = humanize_rrule("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30");
        assert_eq!(label, "每天 08:30");
    }

    #[test]
    fn humanize_hourly_rrule() {
        let label = humanize_rrule("FREQ=HOURLY;INTERVAL=6");
        assert_eq!(label, "每 6 小时");
    }

    #[test]
    fn humanize_minutely_rrule() {
        let label = humanize_rrule("FREQ=MINUTELY;INTERVAL=10");
        assert_eq!(label, "每 10 分钟");
    }

    #[test]
    fn humanize_hourly_rrule_with_byday() {
        let label = humanize_rrule("FREQ=HOURLY;INTERVAL=2;BYDAY=MO,TU");
        assert_eq!(label, "周一、周二 每 2 小时");
    }

    #[test]
    fn scheduled_task_chat_prompt_includes_immediate_creation_guidance() {
        let prompt = scheduled_task_chat_prompt().expect("prompt");
        assert!(prompt.contains("请一次只问我一个问题，并依次确认这些信息："));
        assert!(prompt.contains("1. 任务要做什么。"));
        assert!(prompt.contains(
            "2. 什么时候运行。支持每 N 分钟、每 N 小时、每天指定时间、每周指定星期和时间。"
        ));
        assert!(prompt.contains("5. 权限选项，包括 trustMode 和 autoApprove。"));
        assert!(prompt.contains("必须把任务设为暂停"));
        assert!(prompt.contains("整理草稿时，请把时间转换成 rrule："));
        assert!(prompt.contains("FREQ=MINUTELY;INTERVAL=10"));
        assert!(prompt.contains("FREQ=HOURLY;INTERVAL=6"));
        assert!(prompt.contains("FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=9;BYMINUTE=30"));
        assert!(!prompt.contains("不支持分钟级"));
        assert!(prompt.contains("```scheduled-task-draft"));
        assert!(prompt.contains("前端会立即创建并打开任务详情，不再要求用户二次确认"));
        assert!(prompt.contains("前端会负责创建任务"));
        assert!(!prompt.contains("由用户点击确认后系统创建"));
    }

    #[tokio::test]
    async fn create_pause_delete_round_trip() {
        let _guard = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);
        let state = ScheduledTaskState::boot_read_only().expect("state");
        let created = state
            .create_for_test(CreateScheduledTaskInput {
                name: "测试计划".to_string(),
                prompt: "检查项目状态".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=6".to_string(),
                cwds: vec![dir.join("workspace").to_string_lossy().into_owned()],
                model: None,
                mode: Some("agent".to_string()),
                allow_shell: Some(false),
                trust_mode: Some(false),
                auto_approve: Some(false),
                paused: Some(false),
            })
            .await
            .expect("create");
        assert_eq!(created.status, "active");

        let paused = state
            .pause_for_test(created.id.clone())
            .await
            .expect("pause");
        assert_eq!(paused.status, "paused");

        let deleted = state.delete_for_test(created.id).await.expect("delete");
        assert_eq!(deleted.task.name, "测试计划");
        assert!(deleted.deleted_session_ids.is_empty());
        let serialized = serde_json::to_value(&deleted).expect("delete response json");
        assert_eq!(
            serialized.get("name").and_then(serde_json::Value::as_str),
            Some("测试计划")
        );
        assert_eq!(
            serialized
                .get("deletedSessionIds")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(0)
        );
        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn empty_workspace_tasks_fail_closed_until_a_project_is_selected() {
        let _guard = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);
        let state = ScheduledTaskState::boot_read_only().expect("state");

        let created = state
            .create_for_test(CreateScheduledTaskInput {
                name: "待选项目".to_string(),
                prompt: "检查项目状态".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: vec!["   ".to_string()],
                model: None,
                mode: Some("yolo".to_string()),
                allow_shell: Some(false),
                trust_mode: Some(false),
                auto_approve: Some(false),
                paused: Some(false),
            })
            .await
            .expect("empty-workspace task is retained for editing");
        assert_eq!(created.status, "paused");
        assert!(created.cwds.is_empty());

        let updated = state
            .update_for_test(
                created.id.clone(),
                UpdateScheduledTaskInput {
                    name: None,
                    prompt: None,
                    rrule: None,
                    cwds: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: None,
                    paused: Some(false),
                },
            )
            .await
            .expect("an activation request without a workspace stays editable");
        assert_eq!(updated.status, "paused");

        let resume_error = state
            .resume_for_test(created.id.clone())
            .await
            .expect_err("resume must reject an empty workspace");
        assert!(
            resume_error.contains("requires a workspace"),
            "{resume_error}"
        );
        let run_error = state
            .run_task_now(created.id)
            .await
            .expect_err("manual run must reject an empty workspace");
        assert!(run_error.contains("requires a workspace"), "{run_error}");

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn owned_session_delete_reports_id_only_after_successful_removal() {
        let _guard = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);
        let sessions = SessionStore::boot().expect("sessions");
        let create_session = |task_id: &str| {
            sessions
                .create_scheduled_run(crate::bridge::sessions::ScheduledRunProfile {
                    task_id: task_id.to_string(),
                    model: "model-1".to_string(),
                    model_id: None,
                    workspace: dir.join("workspace"),
                    mode: crate::bridge::sessions::ScheduledRunMode::Agent,
                    allow_shell: false,
                    trust_mode: false,
                    auto_approve: false,
                })
                .expect("scheduled session")
                .metadata
                .id
        };
        let deleted_id = create_session("task-delete-success");

        let reported =
            delete_owned_scheduled_session(&sessions, &deleted_id, "task-delete-success")
                .expect("successful deletion");

        assert_eq!(reported, deleted_id);
        assert!(!sessions.scheduled_session_exists(&reported));

        let retained_id = create_session("task-delete-retained");
        assert!(
            delete_owned_scheduled_session(&sessions, &retained_id, "wrong-task-owner").is_err()
        );
        assert!(sessions.scheduled_session_exists(&retained_id));

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn update_and_resume_round_trip() {
        let _guard = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);
        let state = ScheduledTaskState::boot_read_only().expect("state");

        let created = state
            .create_for_test(CreateScheduledTaskInput {
                name: "晨检".to_string(),
                prompt: "检查运行状态".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=2".to_string(),
                cwds: vec!["/tmp/workspace-a".to_string()],
                model: None,
                mode: Some("agent".to_string()),
                allow_shell: Some(false),
                trust_mode: Some(false),
                auto_approve: Some(false),
                paused: Some(true),
            })
            .await
            .expect("create");
        assert_eq!(created.status, "paused");

        let updated = state
            .update_for_test(
                created.id.clone(),
                UpdateScheduledTaskInput {
                    name: Some("晚检".to_string()),
                    prompt: Some("检查夜间任务".to_string()),
                    rrule: Some("FREQ=HOURLY;INTERVAL=4".to_string()),
                    cwds: Some(vec!["/tmp/workspace-b".to_string()]),
                    model: None,
                    mode: Some("plan".to_string()),
                    allow_shell: Some(true),
                    trust_mode: Some(true),
                    auto_approve: Some(true),
                    paused: None,
                },
            )
            .await
            .expect("update");
        assert_eq!(updated.name, "晚检");
        assert_eq!(updated.prompt, "检查夜间任务");
        assert_eq!(updated.rrule, "FREQ=HOURLY;INTERVAL=4");
        assert_eq!(updated.cwds, vec!["/tmp/workspace-b".to_string()]);
        assert_eq!(updated.mode.as_deref(), Some("yolo"));
        assert!(updated.allow_shell);
        assert!(updated.trust_mode);
        assert!(updated.auto_approve);
        assert_eq!(updated.status, "paused");

        let resumed = state
            .resume_for_test(created.id.clone())
            .await
            .expect("resume");
        assert_eq!(resumed.status, "active");
        assert!(resumed.next_run_at.is_some());
        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn task_dto_does_not_expose_legacy_source_session_binding() {
        let now = chrono::Utc::now();
        let dto = map_scheduled_task(AutomationRecord {
            schema_version: 1,
            id: "automation-1".to_string(),
            name: "daily brief".to_string(),
            prompt: "prepare it".to_string(),
            rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
            cwds: Vec::new(),
            model: Some("model-1".to_string()),
            mode: Some("agent".to_string()),
            allow_shell: Some(false),
            trust_mode: Some(false),
            auto_approve: Some(false),
            status: AutomationStatus::Active,
            created_at: now,
            updated_at: now,
            next_run_at: None,
            last_run_at: None,
        });

        let value = serde_json::to_value(dto).expect("serialize task dto");
        assert!(
            value.get("sourceSessionId").is_none(),
            "base-owned automations must not expose the removed chat-session binding"
        );
        assert_eq!(
            value.get("isRunning"),
            Some(&serde_json::Value::Bool(false))
        );
    }

    #[test]
    fn task_running_state_is_aggregated_from_queued_or_running_runs() {
        let now = chrono::Utc::now();
        let run = |id: &str, status| AutomationRunRecord {
            schema_version: 1,
            id: id.to_string(),
            automation_id: "automation-1".to_string(),
            scheduled_for: now,
            status,
            created_at: now,
            started_at: None,
            ended_at: None,
            task_id: None,
            thread_id: None,
            turn_id: None,
            error: None,
        };

        assert!(has_running_scheduled_runs(&[run(
            "queued",
            AutomationRunStatus::Queued
        )]));
        assert!(has_running_scheduled_runs(&[run(
            "running",
            AutomationRunStatus::Running
        )]));
        assert!(!has_running_scheduled_runs(&[
            run("completed", AutomationRunStatus::Completed),
            run("failed", AutomationRunStatus::Failed),
        ]));
    }

    #[test]
    fn create_request_persists_an_explicit_default_model() {
        let request = build_create_request(
            CreateScheduledTaskInput {
                name: "daily brief".to_string(),
                prompt: "prepare it".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                paused: Some(false),
            },
            "active-user-model".to_string(),
        )
        .expect("valid create request");

        assert_eq!(request.model.as_deref(), Some("active-user-model"));
        assert_eq!(request.mode.as_deref(), Some("yolo"));
        assert_eq!(request.allow_shell, Some(false));
        assert_eq!(request.trust_mode, Some(false));
        assert_eq!(request.auto_approve, Some(false));
        assert_eq!(request.status, Some(AutomationStatus::Paused));
    }

    #[test]
    fn legacy_automation_defaults_are_persisted_before_runtime_start() {
        let dir = temp_home();
        let manager = AutomationManager::open(dir.join("automations")).expect("open manager");
        let created = manager
            .create_automation(CreateAutomationRequest {
                name: "legacy task".to_string(),
                prompt: "legacy prompt".to_string(),
                rrule: "FREQ=MINUTELY;INTERVAL=10".to_string(),
                cwds: Vec::new(),
                model: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                status: Some(AutomationStatus::Paused),
            })
            .expect("create legacy automation");
        let active_without_workspace = manager
            .create_automation(CreateAutomationRequest {
                name: "unsafe legacy task".to_string(),
                prompt: "must be paused during startup".to_string(),
                rrule: "FREQ=MINUTELY;INTERVAL=10".to_string(),
                cwds: Vec::new(),
                model: Some("legacy-model".to_string()),
                mode: Some("yolo".to_string()),
                allow_shell: Some(false),
                trust_mode: Some(false),
                auto_approve: Some(false),
                status: Some(AutomationStatus::Active),
            })
            .expect("create unsafe legacy automation");

        normalize_legacy_automations(&manager, "fallback-model")
            .expect("normalize legacy automation");
        let normalized = manager
            .get_automation(&created.id)
            .expect("read normalized automation");
        assert_eq!(normalized.model.as_deref(), Some("fallback-model"));
        assert_eq!(normalized.mode.as_deref(), Some("yolo"));
        assert_eq!(normalized.allow_shell, Some(false));
        assert_eq!(normalized.trust_mode, Some(false));
        assert_eq!(normalized.auto_approve, Some(false));
        assert_eq!(
            manager
                .get_automation(&active_without_workspace.id)
                .expect("read fail-closed legacy automation")
                .status,
            AutomationStatus::Paused
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn boot_read_only_retention_keeps_owned_old_runs_and_prunes_unowned_history() {
        let _guard = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);
        let state = ScheduledTaskState::boot_read_only().expect("read-only scheduled state");
        let automation = state
            .automations
            .lock()
            .await
            .create_automation(CreateAutomationRequest {
                name: "retention owner".to_string(),
                prompt: "retain linked conversations".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                model: Some("model-1".to_string()),
                mode: Some("yolo".to_string()),
                allow_shell: Some(false),
                trust_mode: Some(false),
                auto_approve: Some(false),
                status: Some(AutomationStatus::Paused),
            })
            .expect("automation");
        let create_session = |task_id: &str| {
            state
                .sessions
                .create_scheduled_run(crate::bridge::sessions::ScheduledRunProfile {
                    task_id: task_id.to_string(),
                    model: "model-1".to_string(),
                    model_id: None,
                    workspace: dir.join("workspace"),
                    mode: crate::bridge::sessions::ScheduledRunMode::Agent,
                    allow_shell: false,
                    trust_mode: false,
                    auto_approve: false,
                })
                .expect("scheduled session")
                .metadata
                .id
        };
        let linked_session = create_session("task-linked");
        let prelink_session = create_session("task-before-link");
        let run_dir = scheduled_automation_root()
            .join("runs")
            .join(&automation.id);
        std::fs::create_dir_all(&run_dir).expect("run authority directory");
        let base = chrono::Utc::now() - chrono::Duration::days(2);
        for index in 0..=1_002 {
            let (task_id, thread_id) = match index {
                0 => (
                    Some("task-linked".to_string()),
                    Some(linked_session.clone()),
                ),
                1 => (Some("task-before-link".to_string()), None),
                _ => (None, None),
            };
            let timestamp = base + chrono::Duration::seconds(index);
            let run = AutomationRunRecord {
                schema_version: 1,
                id: format!("terminal-{index:04}"),
                automation_id: automation.id.clone(),
                scheduled_for: timestamp,
                status: AutomationRunStatus::Completed,
                created_at: timestamp,
                started_at: Some(timestamp),
                ended_at: Some(timestamp),
                task_id,
                thread_id,
                turn_id: Some(format!("turn-{index:04}")),
                error: None,
            };
            std::fs::write(
                run_dir.join(format!("{}.json", run.id)),
                serde_json::to_vec_pretty(&run).expect("run json"),
            )
            .expect("run authority");
        }

        let retained = state
            .automations
            .lock()
            .await
            .list_runs(&automation.id, None)
            .expect("retained runs");

        assert!(retained.iter().any(|run| run.id == "terminal-0000"));
        assert!(retained.iter().any(|run| run.id == "terminal-0001"));
        assert!(
            !run_dir.join("terminal-0002.json").exists(),
            "the oldest run without a durable session owner remains prunable"
        );
        assert_eq!(
            state
                .sessions
                .scheduled_session_ids_for_task("task-before-link"),
            vec![prelink_session]
        );

        drop(state);
        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn create_rejects_noncanonical_mode_before_persisting() {
        let _guard = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);
        let state = ScheduledTaskState::boot_read_only().expect("state");

        let error = state
            .create_for_test(CreateScheduledTaskInput {
                name: "invalid mode".to_string(),
                prompt: "must not persist".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                model: None,
                mode: Some("planner".to_string()),
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                paused: Some(false),
            })
            .await
            .expect_err("planner is not a canonical scheduled mode");
        assert!(error.contains("agent|plan|yolo"), "{error}");
        assert!(state
            .automations
            .lock()
            .await
            .list_automations()
            .expect("list")
            .is_empty());

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn update_rejects_noncanonical_mode_before_persisting() {
        let _guard = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);
        let state = ScheduledTaskState::boot_read_only().expect("state");
        let created = state
            .create_for_test(CreateScheduledTaskInput {
                name: "valid mode".to_string(),
                prompt: "keep valid".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                model: None,
                mode: Some("agent".to_string()),
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                paused: Some(false),
            })
            .await
            .expect("create valid task");

        let error = state
            .update_for_test(
                created.id.clone(),
                UpdateScheduledTaskInput {
                    name: None,
                    prompt: None,
                    rrule: None,
                    cwds: None,
                    model: None,
                    mode: Some("planner".to_string()),
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: None,
                    paused: None,
                },
            )
            .await
            .expect_err("planner is not a canonical scheduled mode");
        assert!(error.contains("agent|plan|yolo"), "{error}");
        assert_eq!(
            state
                .automations
                .lock()
                .await
                .get_automation(&created.id)
                .expect("persisted task")
                .mode
                .as_deref(),
            Some("yolo")
        );

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn run_dto_exposes_session_only_for_the_owning_task() {
        let _guard = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);

        let store = SessionStore::boot().expect("open test sessions");
        let saved = store
            .create_scheduled_run(crate::bridge::sessions::ScheduledRunProfile {
                task_id: "execution-task-1".to_string(),
                model: "model-1".to_string(),
                model_id: None,
                workspace: dir.join("workspace"),
                mode: crate::bridge::sessions::ScheduledRunMode::Agent,
                allow_shell: false,
                trust_mode: false,
                auto_approve: false,
            })
            .expect("create scheduled session");
        let now = chrono::Utc::now();
        let read_state =
            ScheduledRunReadStore::open(crate::bridge::paths::scheduled_run_read_state_path())
                .expect("open read state");
        let owned_run = AutomationRunRecord {
            schema_version: 1,
            id: "run-1".to_string(),
            automation_id: "automation-1".to_string(),
            scheduled_for: now,
            status: AutomationRunStatus::Completed,
            created_at: now,
            started_at: Some(now),
            ended_at: Some(now),
            task_id: Some("execution-task-1".to_string()),
            thread_id: Some(saved.metadata.id.clone()),
            turn_id: Some("turn-1".to_string()),
            error: None,
        };

        let owned = serde_json::to_value(map_scheduled_run(owned_run.clone(), &store, &read_state))
            .expect("serialize owned run");
        assert_eq!(
            owned.get("sessionId").and_then(serde_json::Value::as_str),
            Some(saved.metadata.id.as_str())
        );
        assert!(owned.get("completedAt").is_none());
        assert!(owned.get("outputPaths").is_none());
        assert!(owned.get("messageId").is_none());

        let mismatched = serde_json::to_value(map_scheduled_run(
            AutomationRunRecord {
                task_id: Some("execution-task-2".to_string()),
                ..owned_run.clone()
            },
            &store,
            &read_state,
        ))
        .expect("serialize mismatched run");
        assert!(mismatched
            .get("sessionId")
            .is_some_and(serde_json::Value::is_null));

        let unlinked_run = AutomationRunRecord {
            thread_id: None,
            ..owned_run.clone()
        };
        let recovered = owned_scheduled_sessions(&[unlinked_run], &store)
            .expect("recover session from durable task ownership");
        assert_eq!(
            recovered,
            vec![(saved.metadata.id.clone(), "execution-task-1".to_string())]
        );

        std::fs::remove_file(
            crate::bridge::paths::scheduled_run_sessions_root()
                .join(format!("{}.json", saved.metadata.id)),
        )
        .expect("remove scheduled session payload");
        let missing_payload =
            serde_json::to_value(map_scheduled_run(owned_run, &store, &read_state))
                .expect("serialize run with missing session payload");
        assert!(missing_payload
            .get("sessionId")
            .is_some_and(serde_json::Value::is_null));

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scheduled_run_conversations_are_viewable_once_their_owned_session_exists() {
        let _guard = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);

        let sessions = SessionStore::boot().expect("open test sessions");
        let make_session = |task_id: &str| {
            sessions
                .create_scheduled_run(crate::bridge::sessions::ScheduledRunProfile {
                    task_id: task_id.to_string(),
                    model: "model-1".to_string(),
                    model_id: None,
                    workspace: dir.join(task_id),
                    mode: crate::bridge::sessions::ScheduledRunMode::Agent,
                    allow_shell: false,
                    trust_mode: false,
                    auto_approve: false,
                })
                .expect("create scheduled session")
                .metadata
                .id
        };
        let session_1 = make_session("execution-task-1");
        let session_2 = make_session("execution-task-2");
        let now = chrono::Utc::now();
        let make_run = |run_id: &str, task_id: &str, session_id: &str| AutomationRunRecord {
            schema_version: 1,
            id: run_id.to_string(),
            automation_id: "automation-1".to_string(),
            scheduled_for: now,
            status: AutomationRunStatus::Completed,
            created_at: now,
            started_at: Some(now),
            ended_at: Some(now),
            task_id: Some(task_id.to_string()),
            thread_id: Some(session_id.to_string()),
            turn_id: Some(format!("turn-{run_id}")),
            error: None,
        };
        let run_1 = make_run("run-1", "execution-task-1", &session_1);
        let run_2 = make_run("run-2", "execution-task-2", &session_2);
        let read_state =
            ScheduledRunReadStore::open(crate::bridge::paths::scheduled_run_read_state_path())
                .expect("open read state");

        assert!(scheduled_run_is_unread(&run_1, &sessions, &read_state));
        assert!(scheduled_run_is_unread(&run_2, &sessions, &read_state));
        assert!(ensure_scheduled_run_is_viewable(&run_1, &sessions).is_ok());
        assert!(has_unread_scheduled_runs(
            &[run_1.clone(), run_2.clone()],
            &sessions,
            &read_state
        ));

        read_state
            .mark_viewed("automation-1", "run-1")
            .expect("mark first viewed");
        assert!(!scheduled_run_is_unread(&run_1, &sessions, &read_state));
        assert!(scheduled_run_is_unread(&run_2, &sessions, &read_state));
        assert!(has_unread_scheduled_runs(
            &[run_1.clone(), run_2.clone()],
            &sessions,
            &read_state
        ));

        let reopened =
            ScheduledRunReadStore::open(crate::bridge::paths::scheduled_run_read_state_path())
                .expect("reopen read state");
        assert!(!scheduled_run_is_unread(&run_1, &sessions, &reopened));
        assert!(scheduled_run_is_unread(&run_2, &sessions, &reopened));
        reopened
            .mark_viewed("automation-1", "run-2")
            .expect("mark second viewed");
        assert!(!has_unread_scheduled_runs(
            &[run_1.clone(), run_2.clone()],
            &sessions,
            &reopened
        ));

        let failed_run = AutomationRunRecord {
            status: AutomationRunStatus::Failed,
            ..run_1.clone()
        };
        assert!(!scheduled_run_is_unread(&failed_run, &sessions, &reopened));
        assert!(ensure_scheduled_run_is_viewable(&failed_run, &sessions).is_ok());

        let running_run = AutomationRunRecord {
            status: AutomationRunStatus::Running,
            ..run_2.clone()
        };
        assert!(ensure_scheduled_run_is_viewable(&running_run, &sessions).is_ok());

        let queued_run = AutomationRunRecord {
            status: AutomationRunStatus::Queued,
            ..run_2.clone()
        };
        assert!(ensure_scheduled_run_is_viewable(&queued_run, &sessions).is_ok());

        let missing_session_run = AutomationRunRecord {
            thread_id: Some("missing-scheduled-session".to_string()),
            ..run_2
        };
        assert!(ensure_scheduled_run_is_viewable(&missing_session_run, &sessions).is_err());

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scheduled_read_state_fail_opens_and_quarantines_invalid_payloads() {
        let dir = temp_home();

        for (name, payload) in [
            ("broken.json", "{ definitely-not-json".to_string()),
            (
                "future.json",
                serde_json::json!({
                    "schema_version": SCHEDULED_RUN_READ_STATE_SCHEMA_VERSION + 1,
                    "viewed_runs": { "automation-1": ["run-1"] }
                })
                .to_string(),
            ),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, &payload).expect("write invalid read state");
            let store = ScheduledRunReadStore::open(path.clone())
                .expect("invalid read state must not block startup");
            assert!(store.registry.read().viewed_runs.is_empty());
            assert!(!path.exists(), "invalid state should be quarantined");
            let quarantine_prefix = format!("{name}.invalid-");
            let quarantined = std::fs::read_dir(&dir)
                .expect("read quarantine dir")
                .filter_map(Result::ok)
                .find(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(&quarantine_prefix)
                })
                .expect("quarantined read-state file");
            assert_eq!(
                std::fs::read_to_string(quarantined.path()).expect("read quarantined payload"),
                payload
            );
        }

        let unreadable_path = dir.join("read-error");
        std::fs::create_dir(&unreadable_path).expect("create directory at state path");
        let store = ScheduledRunReadStore::open(unreadable_path.clone())
            .expect("read errors must not block startup");
        assert!(store.registry.read().viewed_runs.is_empty());
        assert!(
            unreadable_path.is_dir(),
            "read-error source must be preserved"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scheduled_read_state_compacts_removed_run_ids() {
        let dir = temp_home();
        let path = dir.join("read-state.json");
        let store = ScheduledRunReadStore::open(path.clone()).expect("open read state");
        store
            .mark_viewed("automation-1", "run-retained")
            .expect("mark retained run");
        store
            .mark_viewed("automation-1", "run-pruned")
            .expect("mark pruned run");

        store
            .compact("automation-1", &HashSet::from(["run-retained".to_string()]))
            .expect("compact viewed runs");
        assert!(store.is_viewed("automation-1", "run-retained"));
        assert!(!store.is_viewed("automation-1", "run-pruned"));

        let reopened = ScheduledRunReadStore::open(path).expect("reopen compacted state");
        assert!(reopened.is_viewed("automation-1", "run-retained"));
        assert!(!reopened.is_viewed("automation-1", "run-pruned"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn delete_serializes_followup_operations_and_waiters_observe_not_found() {
        let _guard = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);
        let state = ScheduledTaskState::boot_read_only().expect("state");
        let created = state
            .create_for_test(CreateScheduledTaskInput {
                name: "serialized delete".to_string(),
                prompt: "test operation lock".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                model: None,
                mode: Some("yolo".to_string()),
                allow_shell: Some(false),
                trust_mode: Some(false),
                auto_approve: Some(false),
                paused: Some(false),
            })
            .await
            .expect("create task");
        let initial_guard = state.lock_operation(&created.id).await;
        let state = Arc::new(state);

        let delete_state = Arc::clone(&state);
        let delete_id = created.id.clone();
        let delete = tokio::spawn(async move { delete_state.delete_task(delete_id).await });
        tokio::task::yield_now().await;
        let resume_state = Arc::clone(&state);
        let resume_id = created.id.clone();
        let resume = tokio::spawn(async move { resume_state.resume_task(resume_id).await });
        tokio::task::yield_now().await;
        assert!(!delete.is_finished());
        assert!(!resume.is_finished());

        drop(initial_guard);
        delete.await.expect("join delete").expect("delete task");
        let resume_error = resume
            .await
            .expect("join resume")
            .expect_err("resume after delete must observe not found");
        assert!(resume_error.contains("Failed to resume scheduled task"));

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn delete_success_is_not_reverted_by_read_state_cleanup_failure() {
        let _guard = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        std::env::set_var("PINVOU3_HOME", &dir);
        let mut state = ScheduledTaskState::boot_read_only().expect("state");
        let created = state
            .create_for_test(CreateScheduledTaskInput {
                name: "cleanup failure".to_string(),
                prompt: "delete remains successful".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                model: None,
                mode: Some("yolo".to_string()),
                allow_shell: Some(false),
                trust_mode: Some(false),
                auto_approve: Some(false),
                paused: Some(false),
            })
            .await
            .expect("create task");

        let blocking_parent = dir.join("read-state-parent-is-a-file");
        std::fs::write(&blocking_parent, b"not a directory").expect("write blocking parent");
        let mut registry = ScheduledRunReadRegistry::default();
        registry.viewed_runs.insert(
            created.id.clone(),
            HashSet::from(["viewed-run".to_string()]),
        );
        state.read_state = ScheduledRunReadStore {
            path: Arc::new(blocking_parent.join("read-state.json")),
            registry: Arc::new(RwLock::new(registry)),
        };

        let deleted = state
            .delete_for_test(created.id.clone())
            .await
            .expect("automation deletion must remain successful");
        assert_eq!(deleted.task.id, created.id);
        assert!(state
            .automations
            .lock()
            .await
            .get_automation(&created.id)
            .is_err());

        match previous {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
