use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::Weekday;
use deepseek_tui::automation_manager::{
    AutomationManager, AutomationRecord, AutomationRunRecord, AutomationRunStatus,
    AutomationSchedule, AutomationSchedulerConfig, AutomationStatus, CreateAutomationRequest,
    SharedAutomationManager, UpdateAutomationRequest, reconcile_run_statuses_shared,
    run_now_shared, spawn_scheduler,
};
use deepseek_tui::task_manager::{SharedTaskManager, TaskManager, TaskManagerConfig, TaskStatus};
use parking_lot::Mutex as ParkingMutex;
use tokio_util::sync::CancellationToken;

use crate::features::assistant::engine_pool::EnginePool;
use crate::features::assistant::platform::bridge::Pinvou3Bridge;
use crate::features::scheduled::executor::ScheduledChatExecutor;
use crate::features::sessions::SessionStore;
use crate::platform::prefs::UserPrefs;

const DELETE_CANCEL_TIMEOUT: Duration = Duration::from_secs(15);
const SCHEDULED_RETENTION_INTERVAL: Duration = Duration::from_secs(15);
const MAX_TERMINAL_RUNS_PER_AUTOMATION: usize = 50;
const SCHEDULED_RUN_READ_STATE_SCHEMA_VERSION: u32 = 2;
const SCHEDULED_MODEL_BINDING_SCHEMA_VERSION: u32 = 1;
const SCHEDULED_TASK_KIND_SCHEMA_VERSION: u32 = 1;
const SCHEDULED_TASK_UI_METADATA_SCHEMA_VERSION: u32 = 1;
// Starts at 2: v1 was a full-task snapshot format that only existed during
// development of this feature and was never released; the v1→v2 migration
// strips the legacy overbroad snapshot fields.
const SCHEDULED_HISTORY_ARCHIVE_SCHEMA_VERSION: u32 = 2;
const SCHEDULED_EXECUTION_MODE: &str = "yolo";
/// Only supported task kind for now: runs app-side memory organization instead of an
/// engine conversation turn.
pub(crate) const SCHEDULED_TASK_KIND_MEMORY_ORGANIZE: &str = "memory_organize";

#[path = "stores.rs"]
mod stores;
use stores::*;

/// 删除一次定时运行对话的那一步。抽成 trait 只为可注入：EnginePool 需要活的
/// WebView AppHandle，无法在单元测试里构造，而删除级联的正确性必须被测到。
#[async_trait::async_trait]
pub(crate) trait ScheduledConversationDeleter: Send + Sync {
    async fn delete_scheduled_conversation(
        &self,
        session_id: &str,
        expected_task_id: &str,
    ) -> Result<()>;
}

#[async_trait::async_trait]
impl ScheduledConversationDeleter for EnginePool {
    async fn delete_scheduled_conversation(
        &self,
        session_id: &str,
        expected_task_id: &str,
    ) -> Result<()> {
        self.delete_scheduled_run(session_id, expected_task_id)
            .await
    }
}

pub struct ScheduledTaskState {
    automations: SharedAutomationManager,
    task_manager: Option<SharedTaskManager>,
    sessions: SessionStore,
    read_state: ScheduledRunReadStore,
    model_bindings: ScheduledTaskModelBindingStore,
    task_kinds: ScheduledTaskKindStore,
    ui_metadata: ScheduledTaskUiMetadataStore,
    history_archive: ScheduledHistoryArchiveStore,
    operation_locks: ParkingMutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
    pool: Option<EnginePool>,
    fallback_model: String,
    #[allow(dead_code)]
    scheduler_cancel: Option<CancellationToken>,
    #[allow(dead_code)]
    scheduler_handle: Option<tokio::task::JoinHandle<()>>,
    #[allow(dead_code)]
    retention_handle: Option<tokio::task::JoinHandle<()>>,
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
    pub model_id: Option<String>,
    /// Task kind; None = ordinary chat task, `memory_organize` = app-side memory
    /// organize run.
    pub kind: Option<String>,
    pub mode: Option<String>,
    pub allow_shell: bool,
    pub trust_mode: bool,
    pub auto_approve: bool,
    pub has_unread_runs: bool,
    pub is_running: bool,
    pub pinned: bool,
    pub pinned_at: Option<String>,
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
    pub model_id: Option<String>,
    /// Task kind; only None (ordinary chat task) or "memory_organize" is accepted.
    #[serde(default)]
    pub kind: Option<String>,
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
    pub model_id: Option<String>,
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
    pub session_title: Option<String>,
    pub pinned: bool,
    pub pinned_at: Option<String>,
    pub archived: bool,
    pub task_name: Option<String>,
    pub task_model: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledRunViewedDto {
    pub automation_id: String,
    pub run_id: String,
    pub has_unread_runs: bool,
}

const SCHEDULED_TASK_CHAT_PROMPT: &str = r#"我想创建一个 Pinvou 定时任务。请通过提问帮我确定方案，回复保持简短，不要长篇解释。

这是一个纯对话收集流程。不要调用任何工具，不要写文件，不要读写 ~/.pinvou3，也不要手动创建 automations JSON。信息完整后只输出给前端解析的任务参数，前端会通过 create_scheduled_task 创建并打开任务详情，不再要求用户二次确认。

严禁使用 schtasks、Windows Task Scheduler、任务计划程序、cron、crontab、systemd timer 或任何系统级计划任务。错误做法：使用 schtasks 创建 Windows 任务。正确做法：返回 scheduled-task-draft JSON，由 Pinvou 前端调用 create_scheduled_task。

请一次只问我一个问题，并依次确认这些信息：
1. 任务要做什么。
2. 什么时候运行。支持每 N 小时（可指定起始时间）、每天指定时间、每周指定星期和时间。不支持分钟级规则；如果用户要求“每 5 分钟”等分钟级频率，必须询问用户改成每 N 小时、每天指定时间或每周指定时间，不要输出草稿。

每次运行创建独立对话；同一个定时任务的所有运行对话共享该任务的专属工作间，不同任务互不共享。产物仍归属各次运行对话。不需要询问工作目录或权限设置。

整理草稿时，请把时间转换成 rrule：
- 每 6 小时一次，从 08:30 起算：FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30
- 每天 08:30：FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30
- 每周一、三 09:30：FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=9;BYMINUTE=30

当信息足够时，请直接给出最终任务参数，并使用下面这种完整代码块格式：
```scheduled-task-draft
{
  "name": "AI 招聘情报晨报",
  "prompt": "检索并汇总...",
  "rrule": "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30",
  "paused": false
}
```
输出代码块后不要继续提问，也不要假装自己调用了创建命令；前端会负责创建任务。"#;

pub fn scheduled_automation_root() -> std::path::PathBuf {
    crate::platform::paths::pinvou3_home().join("automations")
}

fn scheduled_model_bindings_path() -> std::path::PathBuf {
    scheduled_automation_root().join("model-bindings.json")
}

fn scheduled_task_kinds_path() -> std::path::PathBuf {
    scheduled_automation_root().join("task-kinds.json")
}

fn scheduled_task_ui_metadata_path() -> std::path::PathBuf {
    scheduled_automation_root().join("task-ui-metadata.json")
}

fn scheduled_history_archive_path() -> std::path::PathBuf {
    scheduled_automation_root().join("history-archive.json")
}

fn open_scheduled_automation_manager(root: PathBuf) -> Result<AutomationManager> {
    AutomationManager::open(root)
}

#[allow(dead_code)]
pub fn scheduled_task_data_root() -> std::path::PathBuf {
    crate::platform::paths::pinvou3_home().join("tasks")
}

impl ScheduledTaskState {
    pub async fn boot_runtime(
        bridge: &Pinvou3Bridge,
        pool: EnginePool,
        sessions: SessionStore,
    ) -> Result<Self> {
        sessions.reconcile_scheduled_profiles()?;
        let read_state =
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())?;
        let model_bindings = ScheduledTaskModelBindingStore::open(scheduled_model_bindings_path())?;
        let task_kinds = ScheduledTaskKindStore::open(scheduled_task_kinds_path())?;
        let ui_metadata = ScheduledTaskUiMetadataStore::open(scheduled_task_ui_metadata_path())?;
        let history_archive = ScheduledHistoryArchiveStore::open(scheduled_history_archive_path())?;
        let fallback_model = default_automation_model(Some(bridge));
        let manager = open_scheduled_automation_manager(scheduled_automation_root())?;
        let allow_shell = bridge.allow_shell();
        let automations = Arc::new(tokio::sync::Mutex::new(manager));
        let task_cfg = TaskManagerConfig {
            data_dir: scheduled_task_data_root(),
            worker_count: 1,
            default_workspace: crate::platform::paths::scheduled_tasks_root(),
            default_model: fallback_model.clone(),
            default_mode: SCHEDULED_EXECUTION_MODE.to_string(),
            allow_shell,
            trust_mode: true,
        };
        let executor = Arc::new(ScheduledChatExecutor::from_services(
            sessions.clone(),
            pool.clone(),
            {
                let model_bindings = model_bindings.clone();
                Arc::new(move |automation_id: &str, model: &str| {
                    model_bindings.model_id_for(automation_id, model)
                })
            },
            {
                let task_kinds = task_kinds.clone();
                Some(Arc::new(move |automation_id: &str| {
                    task_kinds.kind_for(automation_id)
                }))
            },
        ));
        let task_manager = TaskManager::start_with_executor(task_cfg, executor).await?;
        {
            let manager = automations.lock().await;
            ensure_all_automation_workspaces(&manager)?;
        }
        reconcile_run_statuses_shared(&automations, &task_manager).await?;
        let cancel = CancellationToken::new();
        let scheduler_handle = spawn_scheduler(
            automations.clone(),
            task_manager.clone(),
            cancel.clone(),
            AutomationSchedulerConfig::default(),
        );
        let retention_handle = spawn_scheduled_retention(
            automations.clone(),
            task_manager.clone(),
            pool.clone(),
            read_state.clone(),
            cancel.clone(),
        );
        Ok(Self {
            automations,
            task_manager: Some(task_manager),
            sessions,
            read_state,
            model_bindings,
            task_kinds,
            ui_metadata,
            history_archive,
            operation_locks: ParkingMutex::new(HashMap::new()),
            pool: Some(pool),
            fallback_model,
            scheduler_cancel: Some(cancel),
            scheduler_handle: Some(scheduler_handle),
            retention_handle: Some(retention_handle),
        })
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
        let requested_model_id = input.model_id.clone();
        let requires_model_binding = requested_model_id.is_some();
        // The kind is validated before any persistence/normalization; junk values are
        // rejected outright (same style as the mode check).
        let requested_kind = canonical_scheduled_kind(input.kind.clone())?;
        // Memory organize tasks follow the same rule as the settings-page manual entry:
        // refuse creation while memory is disabled (off by default, force-off for en-ja),
        // or the task is created fine but every run logs a "memory disabled" failure.
        if requested_kind.as_deref() == Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE)
            && !crate::features::memory::memory_enabled()
        {
            return Err("memory organize tasks require memory to be enabled".to_string());
        }
        let created = manager
            .create_automation(build_create_request(
                input,
                current_automation_model(&self.fallback_model),
                current_yolo_allow_shell(),
            )?)
            .map_err(|err| format!("Failed to create scheduled task: {err}"))?;
        let created = ensure_automation_workspace(&manager, created)
            .map_err(|err| format!("Failed to create scheduled task workspace: {err:#}"))?;
        if let Err(error) =
            self.model_bindings
                .set(&created.id, requested_model_id, created.model.clone())
        {
            if requires_model_binding {
                let _ = manager.delete_automation(&created.id);
                return Err(format!(
                    "Failed to save scheduled task model binding: {error:#}"
                ));
            }
            log::warn!(
                "Created scheduled task {}, but failed to save its model binding: {error:#}",
                created.id
            );
        }
        if let Some(kind) = requested_kind {
            if let Err(error) = self.task_kinds.set_kind(&created.id, Some(kind)) {
                let _ = manager.delete_automation(&created.id);
                return Err(format!("Failed to save scheduled task kind: {error:#}"));
            }
        }
        Ok(map_scheduled_task_with_bindings(
            created,
            Some(&self.model_bindings),
            Some(&self.task_kinds),
            Some(&self.ui_metadata),
        ))
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
        let requested_model_update = input.model.clone();
        let requested_model_id = input.model_id.clone();
        let requires_model_binding = requested_model_id.is_some();
        let updated = manager
            .update_automation(&id, build_update_request(input, &current)?)
            .map_err(|err| format!("Failed to update scheduled task '{id}': {err}"))?;
        let updated = ensure_automation_workspace(&manager, updated)
            .map_err(|err| format!("Failed to update scheduled task workspace '{id}': {err:#}"))?;
        if requested_model_update.is_some() || requested_model_id.is_some() {
            if let Err(error) =
                self.model_bindings
                    .set(&id, requested_model_id, updated.model.clone())
            {
                if requires_model_binding {
                    return Err(format!(
                        "Failed to save scheduled task model binding: {error:#}"
                    ));
                }
                log::warn!(
                    "Updated scheduled task {id}, but failed to save its model binding: {error:#}"
                );
            }
        }
        map_scheduled_task_from_manager(
            &manager,
            updated,
            &self.sessions,
            &self.read_state,
            &self.model_bindings,
            &self.task_kinds,
            &self.ui_metadata,
            None,
        )
        .map_err(|err| format!("Failed to read scheduled task runs for '{id}': {err}"))
    }

    async fn pause_task(&self, id: String) -> Result<ScheduledTaskDto, String> {
        let _operation = self.lock_operation(&id).await;
        let manager = self.automations.lock().await;
        let updated = manager
            .pause_automation(&id)
            .map_err(|err| format!("Failed to pause scheduled task '{id}': {err}"))?;
        map_scheduled_task_from_manager(
            &manager,
            updated,
            &self.sessions,
            &self.read_state,
            &self.model_bindings,
            &self.task_kinds,
            &self.ui_metadata,
            None,
        )
        .map_err(|err| format!("Failed to read scheduled task runs for '{id}': {err}"))
    }

    async fn resume_task(&self, id: String) -> Result<ScheduledTaskDto, String> {
        let _operation = self.lock_operation(&id).await;
        let manager = self.automations.lock().await;
        let updated = manager
            .resume_automation(&id)
            .map_err(|err| format!("Failed to resume scheduled task '{id}': {err}"))?;
        let updated = ensure_automation_workspace(&manager, updated)
            .map_err(|err| format!("Failed to resume scheduled task workspace '{id}': {err:#}"))?;
        map_scheduled_task_from_manager(
            &manager,
            updated,
            &self.sessions,
            &self.read_state,
            &self.model_bindings,
            &self.task_kinds,
            &self.ui_metadata,
            None,
        )
        .map_err(|err| format!("Failed to read scheduled task runs for '{id}': {err}"))
    }

    async fn set_task_pinned(&self, id: String, pinned: bool) -> Result<ScheduledTaskDto, String> {
        let _operation = self.lock_operation(&id).await;
        let manager = self.automations.lock().await;
        let current = manager
            .get_automation(&id)
            .map_err(|_| "定时任务不存在或已被删除".to_string())?;
        self.ui_metadata
            .set_pinned(&id, pinned)
            .map_err(|err| format!("Failed to update scheduled task pin state '{id}': {err:#}"))?;
        map_scheduled_task_from_manager(
            &manager,
            current,
            &self.sessions,
            &self.read_state,
            &self.model_bindings,
            &self.task_kinds,
            &self.ui_metadata,
            None,
        )
        .map_err(|err| format!("Failed to read scheduled task runs for '{id}': {err}"))
    }

    async fn delete_task(&self, id: String) -> Result<DeletedScheduledTaskDto, String> {
        // Keep the per-automation gate for the complete destructive workflow. A
        // cancellation-style outer timeout could release the gate midway through
        // the archive-then-delete sequence, allowing update/run-now into partial state.
        let _operation = self.lock_operation(&id).await;
        self.delete_task_inner(&id)
            .await
            .map_err(|err| format!("Failed to delete scheduled task '{id}': {err:#}"))
    }

    async fn run_task_now(&self, id: String) -> Result<ScheduledRunDto, String> {
        let _operation = self.lock_operation(&id).await;
        let task_manager = self
            .task_manager
            .as_ref()
            .ok_or_else(|| "Scheduled task runtime is unavailable".to_string())?;
        {
            let manager = self.automations.lock().await;
            let current = manager
                .get_automation(&id)
                .map_err(|err| format!("Failed to run scheduled task '{id}': {err}"))?;
            ensure_automation_workspace(&manager, current).map_err(|err| {
                format!("Failed to prepare scheduled task workspace '{id}': {err:#}")
            })?;
        }
        let run = run_now_shared(&self.automations, &id, task_manager)
            .await
            .map_err(|err| format!("Failed to run scheduled task '{id}': {err}"))?;
        let session_titles = scheduled_session_titles(&self.sessions)
            .map_err(|err| format!("Failed to list scheduled conversations: {err:#}"))?;
        Ok(map_scheduled_run(
            run,
            &self.sessions,
            &self.read_state,
            &session_titles,
        ))
    }

    async fn mark_run_viewed(
        &self,
        automation_id: String,
        run_id: String,
    ) -> Result<ScheduledRunViewedDto, String> {
        let runs =
            {
                let manager = self.automations.lock().await;
                match manager.get_automation(&automation_id) {
                    Ok(_) => manager.list_runs(&automation_id, None).map_err(|err| {
                        format!("Failed to list scheduled task runs for '{automation_id}': {err}")
                    })?,
                    Err(active_error) => self.history_archive.runs_for(&automation_id).ok_or_else(
                        || {
                            format!(
                                "Failed to read scheduled task '{automation_id}': {active_error}"
                            )
                        },
                    )?,
                }
            };
        let run = runs.iter().find(|run| run.id == run_id).ok_or_else(|| {
            format!("Scheduled run '{run_id}' does not belong to task '{automation_id}'")
        })?;
        ensure_scheduled_run_can_be_marked_viewed(run, &self.sessions)
            .map_err(|err| err.to_string())?;
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

    /// 删除一条定时运行记录（commands::delete_session 对 ScheduledRun 分发到这里）。
    /// 联动删除该次 Session、Run 与底座 Task；定时任务定义、共享工作间和其他运行保留。
    pub(crate) async fn delete_run_for_session(&self, session_id: &str) -> Result<(), String> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| "Scheduled task runtime is unavailable".to_string())?;
        self.delete_run_for_session_with(session_id, pool).await
    }

    /// 删除级联的本体，会话删除步骤由调用方注入。EnginePool 需要活的 WebView
    /// AppHandle 才能构造，注入后级联本身（删哪条 run、什么该留下）才能被测试覆盖；
    /// 真实实现里那一步的 turn 门 + evict 语义由 engine_pool 自己的测试保证。
    async fn delete_run_for_session_with(
        &self,
        session_id: &str,
        deleter: &dyn ScheduledConversationDeleter,
    ) -> Result<(), String> {
        let profile = self
            .sessions
            .scheduled_profile(session_id)
            .ok_or_else(|| format!("Scheduled run session '{session_id}' does not exist"))?;
        let automation_id = profile.task_id;
        let _operation = self.lock_operation(&automation_id).await;
        self.reconcile_runs()
            .await
            .map_err(|err| format!("Failed to reconcile scheduled runs: {err:#}"))?;

        // 崩溃可能留下 profile 已存在但 run 尚未 ThreadLinked 的会话，此时仍要能删会话——
        // 底座对「没有 run」已用 Ok(空) 表达，所以读失败必须上抛：把读失败当成没有 run 会
        // 删掉 Session 却留下 Run/Task，而 owned_session_id 从此返回 None,残留将不可见。
        let archived_run = self.history_archive.find_run(&automation_id, session_id);
        let active_run = {
            let manager = self.automations.lock().await;
            match manager.get_automation(&automation_id) {
                Ok(_) => manager
                    .list_runs(&automation_id, None)
                    .map_err(|err| {
                        format!("Failed to list runs for scheduled task '{automation_id}': {err}")
                    })?
                    .into_iter()
                    .find(|run| run.thread_id.as_deref() == Some(session_id)),
                Err(_) if archived_run.is_some() => None,
                Err(error) => {
                    return Err(format!(
                        "Failed to read scheduled task '{automation_id}': {error}"
                    ));
                }
            }
        };
        if active_run
            .as_ref()
            .or(archived_run.as_ref())
            .is_some_and(|run| {
                matches!(
                    run.status,
                    AutomationRunStatus::Queued | AutomationRunStatus::Running
                )
            })
        {
            return Err("正在运行的定时任务记录不能删除".to_string());
        }

        if let Some(run) = active_run {
            // Keep the foundation's established active-run cascade order. Its
            // task/run deletion API owns the active record and remains unchanged.
            deleter
                .delete_scheduled_conversation(session_id, &automation_id)
                .await
                .map_err(|err| format!("Failed to delete scheduled conversation: {err:#}"))?;
            let task_manager = self
                .task_manager
                .as_ref()
                .ok_or_else(|| "Scheduled task runtime is unavailable".to_string())?;
            let remaining = {
                let manager = self.automations.lock().await;
                manager
                    .delete_terminal_run(&run, task_manager)
                    .await
                    .map_err(|err| {
                        format!("Failed to delete scheduled run '{}': {err:#}", run.id)
                    })?;
                manager.list_runs(&automation_id, None).map_err(|err| {
                    format!("Failed to reload scheduled task runs after deletion: {err}")
                })?
            };
            compact_viewed_runs(&self.read_state, &automation_id, &remaining);
        } else if let Some(run) = archived_run {
            // Archived history is app-owned. Commit its removal before deleting
            // the conversation so an archive write failure cannot strand a
            // visible run whose session has already disappeared.
            let removal = self
                .history_archive
                .remove_run(&automation_id, &run.id)
                .map_err(|err| {
                    format!(
                        "Failed to delete archived scheduled run '{}': {err:#}",
                        run.id
                    )
                })?
                .ok_or_else(|| {
                    format!(
                        "Archived scheduled run '{}' changed before deletion",
                        run.id
                    )
                })?;
            if let Err(delete_error) = deleter
                .delete_scheduled_conversation(session_id, &automation_id)
                .await
            {
                if let Err(rollback_error) =
                    self.history_archive.restore_task(removal.archived_task)
                {
                    return Err(format!(
                        "Failed to delete scheduled conversation: {delete_error:#}; additionally failed to restore archived scheduled run '{}': {rollback_error:#}",
                        run.id
                    ));
                }
                return Err(format!(
                    "Failed to delete scheduled conversation: {delete_error:#}"
                ));
            }
            compact_viewed_runs(&self.read_state, &automation_id, &removal.remaining_runs);
        } else {
            // A crash may leave a scheduled profile before its run is linked.
            deleter
                .delete_scheduled_conversation(session_id, &automation_id)
                .await
                .map_err(|err| format!("Failed to delete scheduled conversation: {err:#}"))?;
        }
        Ok(())
    }

    async fn delete_task_inner(&self, id: &str) -> Result<DeletedScheduledTaskDto> {
        let original_status = {
            let manager = self.automations.lock().await;
            let original = manager
                .get_automation(id)
                .with_context(|| format!("read automation {id} before deletion"))?;
            manager
                .pause_automation(id)
                .with_context(|| format!("pause automation {id}"))?;
            original.status
        };

        let result = async {
            self.reconcile_runs().await?;
            let runs = {
                let manager = self.automations.lock().await;
                manager.list_runs(id, None)?
            };
            self.cancel_active_run_tasks(&runs).await?;
            self.reconcile_runs().await?;

            let (task_snapshot, runs) = {
                let manager = self.automations.lock().await;
                (manager.get_automation(id)?, manager.list_runs(id, None)?)
            };
            self.history_archive
                .archive_task(ArchivedScheduledTaskSnapshot::from(&task_snapshot), runs)
                .with_context(|| format!("archive scheduled task history {id}"))?;

            let deleted = {
                let manager = self.automations.lock().await;
                match manager.delete_automation(id) {
                    Ok(deleted) => deleted,
                    Err(error) if manager.get_automation(id).is_err() => {
                        log::warn!(
                            "Scheduled task {id} definition was removed but run-directory cleanup failed; using the durable history archive: {error:#}"
                        );
                        task_snapshot
                    }
                    Err(error) => {
                        drop(manager);
                        self.history_archive
                            .remove_task(id)
                            .with_context(|| format!("roll back scheduled history archive {id}"))?;
                        return Err(error).with_context(|| format!("delete automation {id}"));
                    }
                }
            };
            if let Err(error) = self.model_bindings.remove(id) {
                log::warn!(
                    "Deleted scheduled task {id}, but failed to remove its model binding: {error:#}"
                );
            }
            if let Err(error) = self.ui_metadata.remove(id) {
                log::warn!(
                    "Deleted scheduled task {id}, but failed to remove its UI metadata: {error:#}"
                );
            }
            // Build the DTO before clearing the kind: the delete receipt keeps the kind so
            // the frontend can render the template marker.
            let deleted = DeletedScheduledTaskDto {
                task: map_scheduled_task_with_bindings(
                    deleted,
                    Some(&self.model_bindings),
                    Some(&self.task_kinds),
                    Some(&self.ui_metadata),
                ),
                deleted_session_ids: Vec::new(),
            };
            if let Err(error) = self.task_kinds.remove(id) {
                log::warn!(
                    "Deleted scheduled task {id}, but failed to remove its kind: {error:#}"
                );
            }
            Ok(deleted)
        }
        .await;

        match result {
            Ok(deleted) => Ok(deleted),
            Err(error) => {
                if let Err(restore_error) = self
                    .restore_task_status_if_present(id, original_status)
                    .await
                {
                    return Err(error.context(format!(
                        "additionally failed to restore automation {id} after failed deletion: {restore_error:#}"
                    )));
                }
                Err(error)
            }
        }
    }

    async fn restore_task_status_if_present(
        &self,
        id: &str,
        original_status: AutomationStatus,
    ) -> Result<()> {
        let manager = self.automations.lock().await;
        let Ok(current) = manager.get_automation(id) else {
            // delete_automation removes the definition before cleaning its run
            // directory. A missing definition is therefore a committed delete,
            // and must never be recreated by compensation.
            return Ok(());
        };
        if current.status == original_status {
            return Ok(());
        }
        match original_status {
            AutomationStatus::Active => manager.resume_automation(id)?,
            AutomationStatus::Paused => manager.pause_automation(id)?,
        };
        Ok(())
    }

    async fn reconcile_runs(&self) -> Result<()> {
        let Some(task_manager) = &self.task_manager else {
            return Ok(());
        };
        reconcile_run_statuses_shared(&self.automations, task_manager).await?;
        Ok(())
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

fn current_yolo_allow_shell() -> bool {
    Pinvou3Bridge::allow_shell_for_prefs(&UserPrefs::load())
}

fn owned_session_id(record: &AutomationRunRecord, sessions: &SessionStore) -> Option<String> {
    let thread_id = record.thread_id.as_deref()?;
    sessions
        .scheduled_profile(thread_id)
        .filter(|profile| {
            profile.task_id == record.automation_id && sessions.scheduled_session_exists(thread_id)
        })
        .map(|_| thread_id.to_string())
}

fn owned_session_id_from_snapshot(
    record: &AutomationRunRecord,
    sessions: &SessionStore,
    session_titles: &HashMap<String, String>,
) -> Option<String> {
    let thread_id = record.thread_id.as_deref()?;
    sessions
        .scheduled_profile(thread_id)
        .filter(|profile| {
            profile.task_id == record.automation_id && session_titles.contains_key(thread_id)
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
    automation_id: &str,
    runs: &[AutomationRunRecord],
    sessions: &SessionStore,
) -> Result<Vec<(String, String)>> {
    let mut seen = HashSet::new();
    let mut owned = Vec::new();
    for run in runs {
        if run.automation_id != automation_id {
            bail!(
                "scheduled run {} belongs to automation {}, not {automation_id}",
                run.id,
                run.automation_id
            );
        }
        let Some(thread_id) = run.thread_id.as_deref() else {
            continue;
        };
        let Some(profile) = sessions.scheduled_profile(thread_id) else {
            continue;
        };
        if profile.task_id != automation_id {
            bail!(
                "scheduled session {thread_id} belongs to automation {}, not {automation_id}",
                profile.task_id
            );
        }
        if seen.insert(thread_id.to_string()) {
            owned.push((thread_id.to_string(), profile.task_id));
        }
    }
    // A crash can create the scheduled session and persist its profile before
    // the base run record receives ThreadLinked. Recover those sessions via
    // stable automation ownership rather than relying only on run links.
    for session_id in sessions.scheduled_session_ids_for_task(automation_id) {
        if seen.insert(session_id.clone()) {
            owned.push((session_id, automation_id.to_string()));
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
        if let Some(handle) = self.retention_handle.take() {
            handle.abort();
        }
    }
}

fn spawn_scheduled_retention(
    automations: SharedAutomationManager,
    task_manager: SharedTaskManager,
    pool: EnginePool,
    read_state: ScheduledRunReadStore,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if cancel.is_cancelled() {
                break;
            }
            if let Err(error) =
                prune_scheduled_history(&automations, &task_manager, &pool, &read_state).await
            {
                log::warn!("Scheduled history retention failed: {error:#}");
            }
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(SCHEDULED_RETENTION_INTERVAL) => {}
            }
        }
    })
}

async fn prune_scheduled_history(
    automations: &SharedAutomationManager,
    task_manager: &SharedTaskManager,
    pool: &EnginePool,
    read_state: &ScheduledRunReadStore,
) -> Result<()> {
    let candidates = {
        let manager = automations.lock().await;
        manager.terminal_run_prune_candidates(MAX_TERMINAL_RUNS_PER_AUTOMATION)?
    };

    let mut changed_automations = HashSet::new();
    for run in candidates {
        if let Some(session_id) = run.thread_id.as_deref() {
            // The pool waits on the same per-session turn gate as interactive
            // follow-ups, so retention cannot tear down a streaming response.
            if let Err(error) = pool
                .delete_scheduled_run(session_id, &run.automation_id)
                .await
            {
                log::warn!(
                    "Unable to delete retained scheduled session {session_id} for run {}: {error:#}",
                    run.id
                );
                continue;
            }
        }
        let manager = automations.lock().await;
        match manager.delete_terminal_run(&run, task_manager).await {
            Ok(true) => {
                changed_automations.insert(run.automation_id.clone());
            }
            Ok(false) => {}
            Err(error) => log::warn!(
                "Unable to delete retained scheduled run {}: {error:#}",
                run.id
            ),
        }
    }
    for automation_id in changed_automations {
        let remaining = {
            let manager = automations.lock().await;
            manager.list_runs(&automation_id, None)?
        };
        compact_viewed_runs(read_state, &automation_id, &remaining);
    }
    Ok(())
}

fn map_scheduled_task_with_bindings(
    record: AutomationRecord,
    model_bindings: Option<&ScheduledTaskModelBindingStore>,
    task_kinds: Option<&ScheduledTaskKindStore>,
    ui_metadata: Option<&ScheduledTaskUiMetadataStore>,
) -> ScheduledTaskDto {
    map_scheduled_task_with_run_state(
        record,
        false,
        false,
        model_bindings,
        task_kinds,
        ui_metadata,
    )
}

fn map_scheduled_task_with_run_state(
    record: AutomationRecord,
    has_unread_runs: bool,
    is_running: bool,
    model_bindings: Option<&ScheduledTaskModelBindingStore>,
    task_kinds: Option<&ScheduledTaskKindStore>,
    ui_metadata: Option<&ScheduledTaskUiMetadataStore>,
) -> ScheduledTaskDto {
    let model_id = record
        .model
        .as_deref()
        .and_then(|model| model_bindings.and_then(|store| store.model_id_for(&record.id, model)));
    let kind = task_kinds.and_then(|store| store.kind_for(&record.id));
    let (pinned, pinned_at) = ui_metadata
        .map(|store| store.metadata_for(&record.id))
        .unwrap_or((false, None));
    ScheduledTaskDto {
        id: record.id,
        name: record.name,
        prompt: record.prompt,
        rrule: record.rrule.clone(),
        schedule_label: humanize_rrule(&record.rrule),
        status: automation_status_label(&record.status),
        next_run_at: record.next_run_at.map(|value| value.to_rfc3339()),
        last_run_at: record.last_run_at.map(|value| value.to_rfc3339()),
        // The automation manager still needs a durable execution workspace,
        // but scheduled-task workspace selection is no longer a user-facing
        // setting. Keep the DTO contract empty so old UI affordances stay gone.
        cwds: Vec::new(),
        model: record.model,
        model_id,
        kind,
        mode: record.mode,
        allow_shell: record.allow_shell.unwrap_or(false),
        trust_mode: record.trust_mode.unwrap_or(false),
        auto_approve: record.auto_approve.unwrap_or(true),
        has_unread_runs,
        is_running,
        pinned,
        pinned_at,
    }
}

fn scheduled_task_internal_workspace(id: &str) -> PathBuf {
    crate::platform::paths::scheduled_task_workspace_dir(id)
}

fn ensure_automation_workspace(
    manager: &AutomationManager,
    record: AutomationRecord,
) -> Result<AutomationRecord> {
    let workspace = scheduled_task_internal_workspace(&record.id);
    std::fs::create_dir_all(&workspace)
        .with_context(|| format!("create scheduled task workspace {}", workspace.display()))?;
    if record.cwds.len() == 1 && record.cwds.first() == Some(&workspace) {
        return Ok(record);
    }
    manager
        .update_automation(
            &record.id,
            UpdateAutomationRequest {
                cwds: Some(vec![workspace]),
                ..UpdateAutomationRequest::default()
            },
        )
        .with_context(|| format!("persist scheduled task workspace for {}", record.id))
}

fn ensure_all_automation_workspaces(manager: &AutomationManager) -> Result<()> {
    for record in manager.list_automations()? {
        ensure_automation_workspace(manager, record)?;
    }
    Ok(())
}

fn scheduled_run_is_unread(
    record: &AutomationRunRecord,
    sessions: &SessionStore,
    read_state: &ScheduledRunReadStore,
) -> bool {
    let Some(session_id) = owned_session_id(record, sessions) else {
        return false;
    };
    matches!(record.status, AutomationRunStatus::Completed)
        && !sessions.is_hidden(&session_id)
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

fn has_unread_scheduled_runs_from_snapshot(
    records: &[AutomationRunRecord],
    sessions: &SessionStore,
    read_state: &ScheduledRunReadStore,
    session_titles: &HashMap<String, String>,
) -> bool {
    records.iter().any(|record| {
        let Some(session_id) = owned_session_id_from_snapshot(record, sessions, session_titles)
        else {
            return false;
        };
        matches!(record.status, AutomationRunStatus::Completed)
            && !sessions.is_hidden(&session_id)
            && !read_state.is_viewed(&record.automation_id, &record.id)
    })
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
    model_bindings: &ScheduledTaskModelBindingStore,
    task_kinds: &ScheduledTaskKindStore,
    ui_metadata: &ScheduledTaskUiMetadataStore,
    session_titles: Option<&HashMap<String, String>>,
) -> Result<ScheduledTaskDto> {
    let runs = manager.list_runs(&record.id, None)?;
    compact_viewed_runs(read_state, &record.id, &runs);
    let owned_session_titles;
    let session_titles = match session_titles {
        Some(snapshot) => snapshot,
        None => {
            owned_session_titles = scheduled_session_titles(sessions)?;
            &owned_session_titles
        }
    };
    let has_unread_runs =
        has_unread_scheduled_runs_from_snapshot(&runs, sessions, read_state, session_titles);
    let is_running = has_running_scheduled_runs(&runs);
    Ok(map_scheduled_task_with_run_state(
        record,
        has_unread_runs,
        is_running,
        Some(model_bindings),
        Some(task_kinds),
        Some(ui_metadata),
    ))
}

fn map_scheduled_run(
    record: AutomationRunRecord,
    sessions: &SessionStore,
    read_state: &ScheduledRunReadStore,
    session_titles: &HashMap<String, String>,
) -> ScheduledRunDto {
    map_scheduled_run_with_task(record, sessions, read_state, session_titles, None, None)
}

fn map_scheduled_run_with_task(
    record: AutomationRunRecord,
    sessions: &SessionStore,
    read_state: &ScheduledRunReadStore,
    session_titles: &HashMap<String, String>,
    task_name: Option<String>,
    task_model: Option<String>,
) -> ScheduledRunDto {
    let session_id = owned_session_id_from_snapshot(&record, sessions, session_titles);
    let session_title = session_id
        .as_deref()
        .and_then(|id| session_titles.get(id).cloned())
        .filter(|title| title != "Scheduled run");
    let pinned = session_id
        .as_deref()
        .is_some_and(|id| sessions.is_pinned(id));
    let pinned_at = session_id.as_deref().and_then(|id| sessions.pinned_at(id));
    let archived = session_id
        .as_deref()
        .is_some_and(|id| sessions.is_hidden(id));
    let unread = matches!(record.status, AutomationRunStatus::Completed)
        && session_id.is_some()
        && !archived
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
        session_title,
        pinned,
        pinned_at,
        archived,
        task_name,
        task_model,
    }
}

fn scheduled_session_titles(sessions: &SessionStore) -> Result<HashMap<String, String>> {
    Ok(sessions
        .list_scheduled()?
        .into_iter()
        .map(|metadata| (metadata.id, metadata.title))
        .collect())
}

fn paused_to_status(paused: bool) -> AutomationStatus {
    if paused {
        AutomationStatus::Paused
    } else {
        AutomationStatus::Active
    }
}

fn build_create_request(
    input: CreateScheduledTaskInput,
    default_model: String,
    allow_shell: bool,
) -> Result<CreateAutomationRequest, String> {
    // Legacy clients may still send these fields. Read and intentionally
    // discard them so they cannot become task-level execution policy.
    let _ = (
        &input.cwds,
        input.allow_shell,
        input.trust_mode,
        input.auto_approve,
    );
    let model = input
        .model
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(default_model);
    canonical_scheduled_mode(input.mode, None)?;
    // Kind is allow-listed at creation time (create_task owns the normalized result and
    // persists it to the sidecar).
    canonical_scheduled_kind(input.kind)?;
    // 工作间由 automation_id 自动分配；客户端不能提供或覆盖路径。
    let status = paused_to_status(input.paused.unwrap_or(false));
    Ok(CreateAutomationRequest {
        name: input.name,
        prompt: input.prompt,
        rrule: input.rrule,
        cwds: Vec::new(),
        model: Some(model),
        mode: Some(SCHEDULED_EXECUTION_MODE.to_string()),
        // 权限不属于定时任务的用户设置；保留输入字段仅用于旧调用兼容。
        // 与普通聊天 Yolo 一致：Shell 跟随全局开关，信任和自动批准开启。
        allow_shell: Some(allow_shell),
        trust_mode: Some(true),
        // 不可绕过的审批（rlm_eval/hook ask）仍由 force_prompt 拦截。
        auto_approve: Some(true),
        delivery_mode: None,
        status: Some(status),
    })
}

fn build_update_request(
    input: UpdateScheduledTaskInput,
    _current: &AutomationRecord,
) -> Result<UpdateAutomationRequest, String> {
    let _ = (
        &input.cwds,
        input.allow_shell,
        input.trust_mode,
        input.auto_approve,
    );
    // The kind is a one-time property decided at creation: the update contract has no
    // kind (UpdateScheduledTaskInput lacks the field, and serde silently drops an extra
    // "kind" in the JSON), so the sidecar is never rewritten.
    canonical_scheduled_mode(input.mode, None)?;
    let status = input.paused.map(paused_to_status);
    Ok(UpdateAutomationRequest {
        name: input.name,
        prompt: input.prompt,
        rrule: input.rrule,
        // 目录概念已移除,cwds 不再接受更新(见 build_create_request)。
        cwds: None,
        model: input.model,
        mode: Some(SCHEDULED_EXECUTION_MODE.to_string()),
        // 权限不再接受任务级更新。
        allow_shell: None,
        trust_mode: None,
        // 恒定 YOLO,更新请求不允许改动(见 build_create_request)。
        auto_approve: None,
        delivery_mode: None,
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

/// Kind allow-list: missing means an ordinary chat task; only `memory_organize` is
/// accepted for now. Same style as [`canonical_scheduled_mode`]: exact match after
/// trimming, everything else is rejected.
fn canonical_scheduled_kind(kind: Option<String>) -> Result<Option<String>, String> {
    let Some(kind) = kind else {
        return Ok(None);
    };
    let kind = kind.trim();
    match kind {
        SCHEDULED_TASK_KIND_MEMORY_ORGANIZE => Ok(Some(kind.to_string())),
        _ => Err(format!(
            "Scheduled task kind must be exactly one of memory_organize, got '{kind}'"
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

fn is_every_workday(days: &[Weekday]) -> bool {
    let workdays = [
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
    ];
    days.len() == workdays.len() && workdays.iter().all(|day| days.contains(day))
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
        Ok(AutomationSchedule::Hourly {
            interval_hours,
            byday,
            anchor_hour,
            anchor_minute,
        }) => {
            let mut hourly = if interval_hours == 1 {
                "每小时".to_string()
            } else {
                format!("每 {interval_hours} 小时")
            };
            if let Some(hour) = anchor_hour {
                hourly.push_str(&format!(
                    " · {hour:02}:{:02} 起",
                    anchor_minute.unwrap_or(0)
                ));
            } else if let Some(minute) = anchor_minute {
                hourly.push_str(&format!(" · 第 {minute:02} 分"));
            }
            match byday {
                Some(days) if !days.is_empty() => {
                    let labels = if is_every_workday(&days) {
                        "工作日".to_string()
                    } else {
                        days.into_iter()
                            .map(weekday_label)
                            .collect::<Vec<_>>()
                            .join("、")
                    };
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
        }) if is_every_workday(&byday) => format!("工作日 {byhour:02}:{byminute:02}"),
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
        Ok(AutomationSchedule::Once { .. } | AutomationSchedule::Cron { .. }) => rrule.to_string(),
        Err(_) => rrule.to_string(),
    }
}
pub async fn list_scheduled_tasks(
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<Vec<ScheduledTaskDto>, String> {
    state
        .reconcile_runs()
        .await
        .map_err(|err| format!("Failed to reconcile scheduled task runs: {err}"))?;
    let manager = state.automations.lock().await;
    ensure_all_automation_workspaces(&manager)
        .map_err(|err| format!("Failed to prepare scheduled task workspaces: {err:#}"))?;
    let records = manager
        .list_automations()
        .map_err(|err| format!("Failed to list scheduled tasks: {err}"))?;
    let current_ids = records
        .iter()
        .map(|record| record.id.clone())
        .collect::<HashSet<_>>();
    if let Err(error) = state.model_bindings.compact(&current_ids) {
        log::warn!("Unable to compact scheduled model bindings: {error:#}");
    }
    if let Err(error) = state.task_kinds.compact(&current_ids) {
        log::warn!("Unable to compact scheduled task kinds: {error:#}");
    }
    if let Err(error) = state.ui_metadata.compact(&current_ids) {
        log::warn!("Unable to compact scheduled task UI metadata: {error:#}");
    }
    let session_titles = scheduled_session_titles(&state.sessions)
        .map_err(|err| format!("Failed to list scheduled conversations: {err:#}"))?;
    records
        .into_iter()
        .map(|record| {
            map_scheduled_task_from_manager(
                &manager,
                record,
                &state.sessions,
                &state.read_state,
                &state.model_bindings,
                &state.task_kinds,
                &state.ui_metadata,
                Some(&session_titles),
            )
        })
        .collect::<Result<Vec<_>>>()
        .map_err(|err| format!("Failed to read scheduled task runs: {err}"))
}
pub async fn read_scheduled_task(
    id: String,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<ScheduledTaskDetailDto, String> {
    let manager = state.automations.lock().await;
    let record = manager
        .get_automation(&id)
        .map_err(|err| format!("Failed to read scheduled task '{id}': {err}"))?;
    let record = ensure_automation_workspace(&manager, record)
        .map_err(|err| format!("Failed to prepare scheduled task workspace '{id}': {err:#}"))?;
    let session_titles = scheduled_session_titles(&state.sessions)
        .map_err(|err| format!("Failed to list scheduled conversations: {err:#}"))?;
    map_scheduled_task_from_manager(
        &manager,
        record,
        &state.sessions,
        &state.read_state,
        &state.model_bindings,
        &state.task_kinds,
        &state.ui_metadata,
        Some(&session_titles),
    )
    .map_err(|err| format!("Failed to read scheduled task runs for '{id}': {err}"))
}
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
    let record = manager
        .get_automation(&id)
        .map_err(|err| format!("Failed to read scheduled task '{id}': {err}"))?;
    let record = ensure_automation_workspace(&manager, record)
        .map_err(|err| format!("Failed to prepare scheduled task workspace '{id}': {err:#}"))?;
    let records = manager
        .list_runs(&id, limit)
        .map_err(|err| format!("Failed to list scheduled task runs for '{id}': {err}"))?;
    if limit.is_none() {
        compact_viewed_runs(&state.read_state, &id, &records);
    }
    let session_titles = scheduled_session_titles(&state.sessions)
        .map_err(|err| format!("Failed to list scheduled conversations: {err:#}"))?;
    let task_name = record.name;
    let task_model = record.model;
    Ok(records
        .into_iter()
        .map(|record| {
            map_scheduled_run_with_task(
                record,
                &state.sessions,
                &state.read_state,
                &session_titles,
                Some(task_name.clone()),
                task_model.clone(),
            )
        })
        .collect())
}

/// Return every retained scheduled run with a single reconciliation and a single
/// metadata scan. The frontend uses this for the global Scheduled Runs sidebar;
/// task-detail reads keep using `list_scheduled_task_runs`.
pub async fn list_scheduled_runs(
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<Vec<ScheduledRunDto>, String> {
    state
        .reconcile_runs()
        .await
        .map_err(|err| format!("Failed to reconcile scheduled task runs: {err}"))?;
    let mut records = Vec::new();
    let mut active_run_keys = HashSet::new();
    {
        let manager = state.automations.lock().await;
        let automations = manager
            .list_automations()
            .map_err(|err| format!("Failed to list scheduled tasks: {err}"))?;
        for automation in automations {
            let runs = manager.list_runs(&automation.id, None).map_err(|err| {
                format!(
                    "Failed to list scheduled task runs for '{}': {err}",
                    automation.id
                )
            })?;
            active_run_keys.extend(
                runs.iter()
                    .map(|run| (run.automation_id.clone(), run.id.clone())),
            );
            records.extend(
                runs.into_iter()
                    .map(|run| (run, Some(automation.name.clone()), automation.model.clone())),
            );
        }
    }
    for archived in state.history_archive.archived_tasks() {
        records.extend(
            archived
                .runs
                .into_iter()
                .filter(|run| {
                    !active_run_keys.contains(&(run.automation_id.clone(), run.id.clone()))
                })
                .map(|run| {
                    (
                        run,
                        Some(archived.task.name.clone()),
                        archived.task.model.clone(),
                    )
                }),
        );
    }
    let mut runs_by_automation = HashMap::<String, Vec<AutomationRunRecord>>::new();
    for (run, _, _) in &records {
        runs_by_automation
            .entry(run.automation_id.clone())
            .or_default()
            .push(run.clone());
    }
    for (automation_id, runs) in runs_by_automation {
        compact_viewed_runs(&state.read_state, &automation_id, &runs);
    }
    records.sort_by_key(|(run, _, _)| std::cmp::Reverse(run.scheduled_for));
    let session_titles = scheduled_session_titles(&state.sessions)
        .map_err(|err| format!("Failed to list scheduled conversations: {err:#}"))?;
    Ok(records
        .into_iter()
        .map(|(record, task_name, task_model)| {
            map_scheduled_run_with_task(
                record,
                &state.sessions,
                &state.read_state,
                &session_titles,
                task_name,
                task_model,
            )
        })
        .collect())
}
pub async fn create_scheduled_task(
    input: CreateScheduledTaskInput,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<ScheduledTaskDto, String> {
    state.create_task(input).await
}
pub async fn update_scheduled_task(
    id: String,
    input: UpdateScheduledTaskInput,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<ScheduledTaskDto, String> {
    state.update_task(id, input).await
}
pub async fn pause_scheduled_task(
    id: String,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<ScheduledTaskDto, String> {
    state.pause_task(id).await
}
pub async fn resume_scheduled_task(
    id: String,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<ScheduledTaskDto, String> {
    state.resume_task(id).await
}
pub async fn set_scheduled_task_pinned(
    id: String,
    pinned: bool,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<ScheduledTaskDto, String> {
    state.set_task_pinned(id, pinned).await
}
pub async fn delete_scheduled_task(
    id: String,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<DeletedScheduledTaskDto, String> {
    state.delete_task(id).await
}
pub async fn run_scheduled_task_now(
    id: String,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<ScheduledRunDto, String> {
    state.run_task_now(id).await
}
pub async fn mark_scheduled_run_viewed(
    automation_id: String,
    run_id: String,
    state: tauri::State<'_, ScheduledTaskState>,
) -> Result<ScheduledRunViewedDto, String> {
    state.mark_run_viewed(automation_id, run_id).await
}
pub fn scheduled_task_chat_prompt() -> Result<String, String> {
    Ok(SCHEDULED_TASK_CHAT_PROMPT.to_string())
}

#[cfg(test)]
// Tests borrow platform::paths::tests::ENV_LOCK (std Mutex) to serialize global env access;
// cargo test runs test threads in parallel, but env-writing tests are mutually serialized, and
// the lock is held across await only inside a current_thread runtime with no reentrant path,
// so it cannot deadlock.
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use parking_lot::RwLock;

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

        async fn set_pinned_for_test(
            &self,
            id: String,
            pinned: bool,
        ) -> Result<ScheduledTaskDto, String> {
            self.set_task_pinned(id, pinned).await
        }

        async fn delete_for_test(&self, id: String) -> Result<DeletedScheduledTaskDto, String> {
            self.delete_task(id).await
        }
    }

    fn temp_home() -> std::path::PathBuf {
        // 叠加进程内原子计数器,避免纯纳秒命名在高并发下碰撞
        // (本 helper 被 18 个测试共用同一前缀,部分不持 ENV_LOCK,并发时同名目录会互相删文件)。
        let dir = std::env::temp_dir().join(format!(
            "pinvou3-scheduled-tasks-test-{}-{}",
            std::process::id(),
            crate::bridge::paths::tests::unique_suffix()
        ));
        std::fs::create_dir_all(&dir).expect("create temp home");
        dir
    }

    // ── VersionedJsonStore<T> 通用核心(合并三 store) ──────────────────
    // 这组测试覆盖合并前的行为契约:open 空 / 当前版本直读 / 旧版本迁移 /
    // 损坏 JSON quarantine / 原子 persist;并区分 Rename 与 LogInPlace 两种策略。
    #[test]
    fn versioned_json_store_open_empty_uses_default_registry() {
        let dir = temp_home();
        let path = dir.join("empty-read-state.json");
        let store = VersionedJsonStore::<ScheduledRunReadRegistry>::open(path.clone())
            .expect("open missing file");
        assert!(store.registry.read().viewed_runs.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn versioned_json_store_open_current_version_passes_through() {
        let dir = temp_home();
        let path = dir.join("current.json");
        let mut registry = ScheduledRunReadRegistry::default();
        registry.viewed_runs.insert(
            "automation-1".to_string(),
            HashSet::from(["run-1".to_string()]),
        );
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&registry).expect("serialize"),
        )
        .expect("write registry");

        let store = VersionedJsonStore::<ScheduledRunReadRegistry>::open(path)
            .expect("open current-version payload");
        let guard = store.registry.read();
        assert_eq!(
            guard.viewed_runs.get("automation-1"),
            Some(&HashSet::from(["run-1".to_string()]))
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn versioned_json_store_open_old_version_migrates_and_repersists() {
        let dir = temp_home();
        let path = dir.join("old-ui-metadata.json");
        // 旧版本(0 < 当前 1)且保留 tasks,触发 migrate。
        let legacy = serde_json::json!({
            "schema_version": 0,
            "tasks": {
                "automation-1": {
                    "pinned": true,
                    "pinned_at": "2024-01-01T00:00:00Z",
                    "updated_at": "2024-01-01T00:00:00Z"
                }
            }
        });
        std::fs::write(&path, legacy.to_string()).expect("write legacy payload");

        let store = VersionedJsonStore::<ScheduledTaskUiMetadataRegistry>::open(path.clone())
            .expect("open old-version payload");
        let guard = store.registry.read();
        assert_eq!(
            guard.schema_version,
            SCHEDULED_TASK_UI_METADATA_SCHEMA_VERSION
        );
        assert!(guard.tasks.contains_key("automation-1"));
        drop(guard);

        // 迁移后已重新落盘为当前版本。
        let repersisted: ScheduledTaskUiMetadataRegistry =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read repersisted"))
                .expect("parse repersisted");
        assert_eq!(
            repersisted.schema_version,
            SCHEDULED_TASK_UI_METADATA_SCHEMA_VERSION
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn versioned_json_store_open_corrupt_json_keeps_log_in_place_store() {
        let dir = temp_home();
        let path = dir.join("corrupt-ui-metadata.json");
        std::fs::write(&path, "{ definitely-not-json").expect("write corrupt payload");

        let store = VersionedJsonStore::<ScheduledTaskUiMetadataRegistry>::open(path.clone())
            .expect("corrupt payload must not block startup");
        assert!(store.registry.read().tasks.is_empty());
        // LogInPlace 策略:不搬走原文件。
        assert!(
            path.exists(),
            "log-in-place store must leave the offending file untouched"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn versioned_json_store_persist_writes_atomically_and_roundtrips() {
        let dir = temp_home();
        let path = dir.join("persist-model-bindings.json");
        let store = VersionedJsonStore::<ScheduledTaskModelBindingRegistry>::open(path.clone())
            .expect("open fresh store");

        let mut registry = store.registry.read().clone();
        registry.tasks.insert(
            "automation-1".to_string(),
            ScheduledTaskModelBinding {
                model_id: "model-id-1".to_string(),
                model: "model-1".to_string(),
                updated_at: "2024-01-01T00:00:00Z".to_string(),
            },
        );
        store.persist(&registry).expect("persist registry");

        let reloaded: ScheduledTaskModelBindingRegistry =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read persisted"))
                .expect("parse persisted");
        assert_eq!(
            reloaded
                .tasks
                .get("automation-1")
                .map(|binding| binding.model_id.clone()),
            Some("model-id-1".to_string())
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scheduled_task_kind_store_round_trips_and_normalizes_unrecognized_kinds() {
        let dir = temp_home();
        let path = dir.join("task-kinds.json");
        let store = ScheduledTaskKindStore::open(path.clone()).expect("open kind store");
        assert_eq!(store.kind_for("automation-1"), None, "empty registry");

        store
            .set_kind(
                "automation-1",
                Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE.to_string()),
            )
            .expect("set kind");
        assert_eq!(
            store.kind_for("automation-1").as_deref(),
            Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE)
        );

        let reopened = ScheduledTaskKindStore::open(path.clone()).expect("reopen kind store");
        assert_eq!(
            reopened.kind_for("automation-1").as_deref(),
            Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE),
            "kind must persist across restart"
        );

        // Unsupported kinds (legacy leftovers or hand-written) read back as an ordinary
        // chat task.
        let mut registry = reopened.registry.read().clone();
        registry.tasks.insert(
            "automation-legacy".to_string(),
            ScheduledTaskKindEntry {
                kind: "legacy_kind".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            },
        );
        reopened.persist(&registry).expect("persist legacy kind");
        let reloaded =
            ScheduledTaskKindStore::open(path.clone()).expect("reload legacy kind store");
        assert_eq!(reloaded.kind_for("automation-legacy"), None);
        assert_eq!(
            reloaded.kind_for("automation-1").as_deref(),
            Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE)
        );

        // set_kind(None) removes the record, back to an ordinary chat task.
        reloaded.set_kind("automation-1", None).expect("clear kind");
        assert_eq!(reloaded.kind_for("automation-1"), None);
        let cleared = ScheduledTaskKindStore::open(path).expect("reopen cleared store");
        assert_eq!(cleared.kind_for("automation-1"), None);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scheduled_history_archive_migrates_legacy_full_task_to_minimal_snapshot() {
        let dir = temp_home();
        let path = dir.join("legacy-history-archive.json");
        let legacy = serde_json::json!({
            "schema_version": 1,
            "tasks": {
                "automation-1": {
                    "task": {
                        "id": "automation-1",
                        "name": "Legacy task",
                        "model": "legacy-model",
                        "prompt": "private prompt that must not survive migration",
                        "cwds": ["C:/private/workspace"],
                        "allow_shell": true,
                        "trust_mode": true
                    },
                    "runs": [],
                    "deleted_at": "2026-01-01T00:00:00Z"
                }
            }
        });
        std::fs::write(&path, legacy.to_string()).expect("write legacy archive");

        let store = ScheduledHistoryArchiveStore::open(path.clone()).expect("migrate archive");
        let archived = store.archived_tasks();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].task.id, "automation-1");
        assert_eq!(archived[0].task.name, "Legacy task");
        assert_eq!(archived[0].task.model.as_deref(), Some("legacy-model"));

        let persisted: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&path).expect("read migrated history archive"),
        )
        .expect("parse migrated history archive");
        assert_eq!(
            persisted["schema_version"],
            SCHEDULED_HISTORY_ARCHIVE_SCHEMA_VERSION
        );
        let snapshot = &persisted["tasks"]["automation-1"]["task"];
        assert!(snapshot.get("prompt").is_none());
        assert!(snapshot.get("cwds").is_none());
        assert!(snapshot.get("allow_shell").is_none());
        assert!(snapshot.get("trust_mode").is_none());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scheduled_history_archive_skips_only_corrupt_entries_and_runs() {
        let dir = temp_home();
        let path = dir.join("partially-corrupt-history-archive.json");
        let payload = serde_json::json!({
            "schema_version": SCHEDULED_HISTORY_ARCHIVE_SCHEMA_VERSION,
            "tasks": {
                "automation-good": {
                    "task": {
                        "id": "automation-good",
                        "name": "Retained task",
                        "model": "retained-model"
                    },
                    "runs": [
                        {
                            "id": "run-good",
                            "automation_id": "automation-good",
                            "scheduled_for": "2026-01-01T00:00:00Z",
                            "status": "completed",
                            "created_at": "2026-01-01T00:00:00Z"
                        },
                        { "id": 42, "not": "a valid run" }
                    ],
                    "deleted_at": "2026-01-01T00:00:00Z"
                },
                "automation-bad": {
                    "task": { "id": 42 },
                    "runs": [],
                    "deleted_at": "2026-01-01T00:00:00Z"
                }
            }
        });
        std::fs::write(&path, payload.to_string()).expect("write partial archive");

        let store = ScheduledHistoryArchiveStore::open(path.clone())
            .expect("one corrupt entry must not quarantine the whole archive");
        let archived = store.archived_tasks();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].task.id, "automation-good");
        assert_eq!(archived[0].runs.len(), 1);
        assert_eq!(archived[0].runs[0].id, "run-good");
        assert!(path.exists(), "the valid archive file must remain in place");
        assert!(
            std::fs::read_dir(&dir)
                .expect("read archive directory")
                .filter_map(|entry| entry.ok())
                .all(|entry| !entry.file_name().to_string_lossy().contains(".invalid-")),
            "partial item damage must not quarantine valid history"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scheduled_task_root_uses_pinvou_home() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        assert_eq!(scheduled_automation_root(), dir.join("automations"));
        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn humanize_rrule_labels() {
        let cases = [
            (
                "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30",
                "每天 08:30",
            ),
            (
                "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9;BYMINUTE=0",
                "工作日 09:00",
            ),
            ("FREQ=HOURLY;INTERVAL=6", "每 6 小时"),
            (
                "FREQ=HOURLY;INTERVAL=2;BYHOUR=8;BYMINUTE=30",
                "每 2 小时 · 08:30 起",
            ),
            ("FREQ=HOURLY;INTERVAL=2;BYDAY=MO,TU", "周一、周二 每 2 小时"),
            (
                "FREQ=HOURLY;INTERVAL=2;BYDAY=MO,TU,WE,TH,FR",
                "工作日 每 2 小时",
            ),
        ];
        for (rrule, expected) in cases {
            assert_eq!(humanize_rrule(rrule), expected, "rrule: {rrule}");
        }
    }

    // ── 删除一次定时运行的级联 ────────────────────────────────────────
    //
    // 造真实 fixture:automation + 两次 run_now，每次经 executor 建出真实的
    // sched-* 会话并 ThreadCreated 链上，底座为每次 run 生成真实 Task。
    // 然后删掉第一条运行的会话，验证「删这一次」和「别的都留着」。

    /// 直接实现底座 TaskExecutor：建一个真实的定时会话并报告 ThreadCreated。
    /// 不碰 engine，从而不需要 EnginePool。`hold` 置位时会话建好后挂住不返回，
    /// 用来造出**真的**停在 Running 的运行——手改磁盘上的状态会被
    /// delete 路径里的 reconcile_runs 按 Task 状态回写掉，测不出拒删。
    struct SessionCreatingExecutor {
        sessions: SessionStore,
        hold: Option<Arc<tokio::sync::Notify>>,
    }

    #[async_trait::async_trait]
    impl deepseek_tui::task_manager::TaskExecutor for SessionCreatingExecutor {
        async fn execute(
            &self,
            task: deepseek_tui::task_manager::ExecutionTask,
            events: tokio::sync::mpsc::UnboundedSender<
                deepseek_tui::task_manager::TaskExecutionEvent,
            >,
            _cancel: CancellationToken,
        ) -> deepseek_tui::task_manager::TaskExecutionResult {
            use crate::features::sessions::{ScheduledRunMode, ScheduledRunProfile};
            let automation_id = task
                .conversation_key()
                .unwrap_or_else(|| task.id())
                .to_string();
            let session = self
                .sessions
                .create_scheduled_run(ScheduledRunProfile {
                    task_id: automation_id,
                    model: "cascade-model".to_string(),
                    model_id: None,
                    workspace: task.workspace().to_path_buf(),
                    mode: ScheduledRunMode::Yolo,
                    allow_shell: false,
                    trust_mode: true,
                    auto_approve: true,
                })
                .expect("create scheduled session");
            let _ = events.send(
                deepseek_tui::task_manager::TaskExecutionEvent::ThreadCreated {
                    thread_id: session.metadata.id.clone(),
                },
            );
            if let Some(hold) = &self.hold {
                hold.notified().await;
            }
            deepseek_tui::task_manager::TaskExecutionResult {
                status: TaskStatus::Completed,
                result_text: Some("cascade fixture run".to_string()),
                error: None,
            }
        }
    }

    /// 注入给 delete_run_for_session_with 的会话删除步骤。生产实现是
    /// EnginePool（turn 门 + evict + 同一个 store 调用），这里只保留 store 调用。
    struct StoreConversationDeleter(SessionStore);

    #[async_trait::async_trait]
    impl ScheduledConversationDeleter for StoreConversationDeleter {
        async fn delete_scheduled_conversation(
            &self,
            session_id: &str,
            expected_task_id: &str,
        ) -> Result<()> {
            self.0.delete_scheduled_run(session_id, expected_task_id)
        }
    }

    struct FailingConversationDeleter;

    #[async_trait::async_trait]
    impl ScheduledConversationDeleter for FailingConversationDeleter {
        async fn delete_scheduled_conversation(
            &self,
            _session_id: &str,
            _expected_task_id: &str,
        ) -> Result<()> {
            bail!("injected conversation deletion failure")
        }
    }

    struct UnexpectedConversationDeleter;

    #[async_trait::async_trait]
    impl ScheduledConversationDeleter for UnexpectedConversationDeleter {
        async fn delete_scheduled_conversation(
            &self,
            _session_id: &str,
            _expected_task_id: &str,
        ) -> Result<()> {
            panic!("conversation deletion must not run before the archive commit")
        }
    }

    /// 等到目标 run 既挂上了会话、状态又到位。`want_terminal=false` 用于挂住的
    /// executor：那时 run 会停在 Running。
    ///
    /// 每轮必须先 reconcile：Run 记录的 thread_id / 状态不会自己变，只有
    /// reconcile_run_statuses 会把它们从底座 Task 同步过来（生产路径上由
    /// ScheduledTaskState::reconcile_runs 触发）。
    async fn wait_for_linked_run(
        automations: &SharedAutomationManager,
        task_manager: &SharedTaskManager,
        automation_id: &str,
        run_id: &str,
        want_terminal: bool,
    ) -> AutomationRunRecord {
        for _ in 0..400 {
            reconcile_run_statuses_shared(automations, task_manager)
                .await
                .expect("reconcile run statuses");
            let run = {
                let manager = automations.lock().await;
                manager
                    .list_runs(automation_id, None)
                    .expect("list runs")
                    .into_iter()
                    .find(|run| run.id == run_id)
            };
            if let Some(run) = run {
                let status_ready = if want_terminal {
                    matches!(run.status, AutomationRunStatus::Completed)
                } else {
                    matches!(run.status, AutomationRunStatus::Running)
                };
                if run.thread_id.is_some() && status_ready {
                    return run;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("timed out waiting for run {run_id} (want_terminal={want_terminal})");
    }

    struct CascadeFixture {
        state: ScheduledTaskState,
        deleter: StoreConversationDeleter,
        task_manager: SharedTaskManager,
        automation_id: String,
        hold: Option<Arc<tokio::sync::Notify>>,
    }

    async fn cascade_fixture(hold: Option<Arc<tokio::sync::Notify>>) -> CascadeFixture {
        let sessions = SessionStore::boot().expect("session store");
        let read_state =
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
                .expect("read state");
        let model_bindings = ScheduledTaskModelBindingStore::open(scheduled_model_bindings_path())
            .expect("model bindings");
        let task_kinds =
            ScheduledTaskKindStore::open(scheduled_task_kinds_path()).expect("task kinds");
        let ui_metadata = ScheduledTaskUiMetadataStore::open(scheduled_task_ui_metadata_path())
            .expect("ui metadata");
        let manager =
            open_scheduled_automation_manager(scheduled_automation_root()).expect("automations");
        let automations: SharedAutomationManager = Arc::new(tokio::sync::Mutex::new(manager));
        let task_manager = TaskManager::start_with_executor(
            TaskManagerConfig {
                data_dir: scheduled_task_data_root(),
                worker_count: 1,
                default_workspace: crate::platform::paths::scheduled_tasks_root(),
                default_model: "cascade-model".to_string(),
                default_mode: SCHEDULED_EXECUTION_MODE.to_string(),
                allow_shell: false,
                trust_mode: true,
            },
            Arc::new(SessionCreatingExecutor {
                sessions: sessions.clone(),
                hold: hold.clone(),
            }),
        )
        .await
        .expect("task manager");

        let automation = {
            let manager = automations.lock().await;
            let created = manager
                .create_automation(CreateAutomationRequest {
                    name: "级联删除任务".to_string(),
                    prompt: "run".to_string(),
                    rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                    cwds: Vec::new(),
                    model: Some("cascade-model".to_string()),
                    mode: Some(SCHEDULED_EXECUTION_MODE.to_string()),
                    allow_shell: Some(false),
                    trust_mode: Some(true),
                    auto_approve: Some(true),
                    delivery_mode: None,
                    status: Some(AutomationStatus::Paused),
                })
                .expect("create automation");
            ensure_automation_workspace(&manager, created).expect("workspace")
        };

        CascadeFixture {
            deleter: StoreConversationDeleter(sessions.clone()),
            state: ScheduledTaskState {
                automations,
                task_manager: Some(task_manager.clone()),
                sessions,
                read_state,
                model_bindings,
                task_kinds,
                ui_metadata,
                history_archive: ScheduledHistoryArchiveStore::open(
                    scheduled_history_archive_path(),
                )
                .expect("history archive"),
                operation_locks: ParkingMutex::new(HashMap::new()),
                pool: None,
                fallback_model: "cascade-model".to_string(),
                scheduler_cancel: None,
                scheduler_handle: None,
                retention_handle: None,
            },
            task_manager,
            automation_id: automation.id,
            hold,
        }
    }

    async fn run_once(fixture: &CascadeFixture) -> AutomationRunRecord {
        let queued = run_now_shared(
            &fixture.state.automations,
            &fixture.automation_id,
            &fixture.task_manager,
        )
        .await
        .expect("run now");
        wait_for_linked_run(
            &fixture.state.automations,
            &fixture.task_manager,
            &fixture.automation_id,
            &queued.id,
            fixture.hold.is_none(),
        )
        .await
    }

    /// 删一次运行 = 删掉该次 Session + Run + 底座 Task；任务定义、共享工作间
    /// 和其它运行必须原样保留。这是本次改动里唯一的破坏性路径。
    #[tokio::test]
    async fn deleting_one_run_removes_its_session_run_and_task_only() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let fixture = cascade_fixture(None).await;
        let doomed = run_once(&fixture).await;
        let survivor = run_once(&fixture).await;
        let doomed_session = doomed.thread_id.clone().expect("doomed session");
        let survivor_session = survivor.thread_id.clone().expect("survivor session");
        assert_ne!(doomed_session, survivor_session, "每次运行是独立对话");
        let doomed_task = doomed.task_id.clone().expect("doomed task");
        let survivor_task = survivor.task_id.clone().expect("survivor task");
        let workspace =
            crate::platform::paths::scheduled_task_workspace_dir(&fixture.automation_id);
        assert!(workspace.is_dir(), "fixture 应已建出共享工作间");

        fixture
            .state
            .delete_run_for_session_with(&doomed_session, &fixture.deleter)
            .await
            .expect("delete the doomed run");

        // 被删那次:Session / Run / Task 三者都消失。
        assert!(
            !fixture
                .state
                .sessions
                .scheduled_session_exists(&doomed_session),
            "被删运行的会话必须消失"
        );
        let remaining = {
            let manager = fixture.state.automations.lock().await;
            manager
                .list_runs(&fixture.automation_id, None)
                .expect("list runs")
        };
        assert!(
            !remaining.iter().any(|run| run.id == doomed.id),
            "被删运行的 Run 记录必须消失"
        );
        assert!(
            fixture.task_manager.get_task(&doomed_task).await.is_err(),
            "被删运行的底座 Task 必须消失"
        );

        // 任务定义 / 共享工作间 / 另一次运行:全部保留。
        {
            let manager = fixture.state.automations.lock().await;
            manager
                .get_automation(&fixture.automation_id)
                .expect("任务定义必须保留");
        }
        assert!(workspace.is_dir(), "共享工作间必须保留");
        assert!(
            remaining.iter().any(|run| run.id == survivor.id),
            "其它运行的 Run 记录必须保留"
        );
        assert!(
            fixture
                .state
                .sessions
                .scheduled_session_exists(&survivor_session),
            "其它运行的会话必须保留"
        );
        assert!(
            fixture.task_manager.get_task(&survivor_task).await.is_ok(),
            "其它运行的底座 Task 必须保留"
        );

        fixture.task_manager.shutdown();
        drop(fixture);
        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 正在排队/运行的记录不允许删除——拒绝发生在任何破坏性动作之前。
    #[tokio::test]
    async fn deleting_task_preserves_completed_run_history_and_sessions() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let fixture = cascade_fixture(None).await;
        let first = run_once(&fixture).await;
        let second = run_once(&fixture).await;
        let first_session = first.thread_id.clone().expect("first session");
        let second_session = second.thread_id.clone().expect("second session");

        let deleted = fixture
            .state
            .delete_for_test(fixture.automation_id.clone())
            .await
            .expect("delete task while retaining history");
        assert!(deleted.deleted_session_ids.is_empty());
        {
            let manager = fixture.state.automations.lock().await;
            assert!(
                manager.get_automation(&fixture.automation_id).is_err(),
                "the schedule definition must be removed"
            );
        }
        assert!(
            fixture
                .state
                .sessions
                .scheduled_session_exists(&first_session),
            "the first historical conversation must be retained"
        );
        assert!(
            fixture
                .state
                .sessions
                .scheduled_session_exists(&second_session),
            "the second historical conversation must be retained"
        );
        let archived = fixture
            .state
            .history_archive
            .runs_for(&fixture.automation_id)
            .expect("archived runs");
        assert_eq!(archived.len(), 2);
        assert!(archived.iter().any(|run| run.id == first.id));
        assert!(archived.iter().any(|run| run.id == second.id));

        let reopened = ScheduledHistoryArchiveStore::open(scheduled_history_archive_path())
            .expect("reopen history archive");
        let reopened_task = reopened
            .archived_tasks()
            .into_iter()
            .find(|archived| archived.task.id == fixture.automation_id)
            .expect("persisted archived task");
        assert_eq!(reopened_task.task.name, deleted.task.name);
        assert_eq!(reopened_task.runs.len(), 2);

        fixture
            .state
            .mark_run_viewed(fixture.automation_id.clone(), second.id.clone())
            .await
            .expect("mark archived run viewed");
        fixture
            .state
            .delete_run_for_session_with(&first_session, &fixture.deleter)
            .await
            .expect("delete one archived run");
        assert!(
            !fixture
                .state
                .sessions
                .scheduled_session_exists(&first_session)
        );
        assert!(
            fixture
                .state
                .sessions
                .scheduled_session_exists(&second_session)
        );
        let remaining = fixture
            .state
            .history_archive
            .runs_for(&fixture.automation_id)
            .expect("remaining archived runs");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, second.id);

        fixture.task_manager.shutdown();
        drop(fixture);
        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn failed_task_history_archive_restores_original_active_status() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let fixture = cascade_fixture(None).await;
        {
            let manager = fixture.state.automations.lock().await;
            let active = manager
                .resume_automation(&fixture.automation_id)
                .expect("activate fixture task");
            assert_eq!(active.status, AutomationStatus::Active);
        }
        let archive_path = scheduled_history_archive_path();
        std::fs::create_dir_all(&archive_path).expect("block archive file with directory");

        let error = fixture
            .state
            .delete_for_test(fixture.automation_id.clone())
            .await
            .expect_err("archive persistence must fail");
        assert!(error.contains("archive scheduled task history"), "{error}");
        {
            let manager = fixture.state.automations.lock().await;
            let retained = manager
                .get_automation(&fixture.automation_id)
                .expect("failed deletion must retain task definition");
            assert_eq!(
                retained.status,
                AutomationStatus::Active,
                "compensation must restore the pre-delete scheduling status"
            );
        }
        assert!(
            fixture
                .state
                .history_archive
                .runs_for(&fixture.automation_id)
                .is_none(),
            "failed archive writes must also roll back in-memory archive state"
        );

        fixture.task_manager.shutdown();
        drop(fixture);
        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn archived_run_archive_write_failure_keeps_session_and_retryable_history() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let fixture = cascade_fixture(None).await;
        let run = run_once(&fixture).await;
        let session_id = run.thread_id.clone().expect("archived run session");
        fixture
            .state
            .delete_for_test(fixture.automation_id.clone())
            .await
            .expect("archive task history");
        let archive_path = scheduled_history_archive_path();
        std::fs::remove_file(&archive_path).expect("remove writable archive file");
        std::fs::create_dir(&archive_path).expect("block archive file with directory");

        let error = fixture
            .state
            .delete_run_for_session_with(&session_id, &UnexpectedConversationDeleter)
            .await
            .expect_err("archive commit must fail before session deletion");
        assert!(
            error.contains("Failed to delete archived scheduled run"),
            "{error}"
        );
        assert!(
            fixture.state.sessions.scheduled_session_exists(&session_id),
            "archive persistence failure must leave the conversation untouched"
        );
        let retained = fixture
            .state
            .history_archive
            .runs_for(&fixture.automation_id)
            .expect("retryable archived history");
        assert!(retained.iter().any(|item| item.id == run.id));

        fixture.task_manager.shutdown();
        drop(fixture);
        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn archived_run_conversation_failure_rolls_back_persisted_history() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let fixture = cascade_fixture(None).await;
        let run = run_once(&fixture).await;
        let session_id = run.thread_id.clone().expect("archived run session");
        fixture
            .state
            .delete_for_test(fixture.automation_id.clone())
            .await
            .expect("archive task history");

        let error = fixture
            .state
            .delete_run_for_session_with(&session_id, &FailingConversationDeleter)
            .await
            .expect_err("injected conversation deletion must fail");
        assert!(
            error.contains("injected conversation deletion failure"),
            "{error}"
        );
        assert!(
            fixture.state.sessions.scheduled_session_exists(&session_id),
            "failed conversation deletion must retain the session"
        );
        let retained = fixture
            .state
            .history_archive
            .runs_for(&fixture.automation_id)
            .expect("rolled-back archived history");
        assert!(retained.iter().any(|item| item.id == run.id));
        let reopened = ScheduledHistoryArchiveStore::open(scheduled_history_archive_path())
            .expect("reopen rolled-back history archive");
        assert!(
            reopened
                .runs_for(&fixture.automation_id)
                .expect("persisted rolled-back history")
                .iter()
                .any(|item| item.id == run.id),
            "rollback must be durable across restart"
        );

        fixture.task_manager.shutdown();
        drop(fixture);
        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn deleting_an_active_run_is_refused_before_anything_is_removed() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        // executor 挂住 → 这条 run 真的停在 Running，reconcile 也不会把它改成终态。
        let hold = Arc::new(tokio::sync::Notify::new());
        let fixture = cascade_fixture(Some(hold.clone())).await;
        let run = run_once(&fixture).await;
        let session_id = run.thread_id.clone().expect("session");
        assert!(matches!(run.status, AutomationRunStatus::Running));

        let error = fixture
            .state
            .delete_run_for_session_with(&session_id, &fixture.deleter)
            .await
            .expect_err("running 记录不能删");
        assert!(
            error.contains("正在运行的定时任务记录不能删除"),
            "错误应说明原因，实际: {error}"
        );
        assert!(
            fixture.state.sessions.scheduled_session_exists(&session_id),
            "拒绝删除时会话必须原样保留"
        );
        {
            let manager = fixture.state.automations.lock().await;
            assert!(
                manager
                    .list_runs(&fixture.automation_id, None)
                    .expect("list runs")
                    .iter()
                    .any(|item| item.id == run.id),
                "拒绝删除时 Run 记录必须原样保留"
            );
        }

        hold.notify_waiters();
        fixture.task_manager.shutdown();
        drop(fixture);
        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scheduled_task_chat_prompt_includes_immediate_creation_guidance() {
        let prompt = scheduled_task_chat_prompt().expect("prompt");
        assert!(prompt.contains("请一次只问我一个问题，并依次确认这些信息："));
        assert!(prompt.contains("1. 任务要做什么。"));
        assert!(prompt.contains(
            "2. 什么时候运行。支持每 N 小时（可指定起始时间）、每天指定时间、每周指定星期和时间。"
        ));
        assert!(!prompt.contains("3."));
        assert!(prompt.contains("不需要询问工作目录或权限设置"));
        assert!(!prompt.contains("allowShell"));
        assert!(!prompt.contains("trustMode"));
        assert!(!prompt.contains("cwds"));
        assert!(prompt.contains("整理草稿时，请把时间转换成 rrule："));
        assert!(prompt.contains("FREQ=HOURLY;INTERVAL=6;BYHOUR=8;BYMINUTE=30"));
        assert!(prompt.contains("FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=9;BYMINUTE=30"));
        assert!(prompt.contains("create_scheduled_task"));
        assert!(prompt.contains("schtasks"));
        assert!(prompt.contains("Windows Task Scheduler"));
        assert!(prompt.contains("cron"));
        assert!(prompt.contains("systemd timer"));
        assert!(prompt.contains("不支持分钟级"));
        assert!(prompt.contains("```scheduled-task-draft"));
        assert!(
            prompt.contains(
                "前端会通过 create_scheduled_task 创建并打开任务详情，不再要求用户二次确认"
            )
        );
        assert!(prompt.contains("前端会负责创建任务"));
        assert!(!prompt.contains("由用户点击确认后系统创建"));
    }

    #[tokio::test]
    async fn create_pause_delete_round_trip() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        let sessions = SessionStore::boot().expect("session store");
        sessions
            .reconcile_scheduled_profiles()
            .expect("reconcile scheduled profiles");
        let read_state =
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
                .expect("read state");
        let model_bindings = ScheduledTaskModelBindingStore::open(scheduled_model_bindings_path())
            .expect("model bindings");
        let task_kinds =
            ScheduledTaskKindStore::open(scheduled_task_kinds_path()).expect("task kinds");
        let ui_metadata = ScheduledTaskUiMetadataStore::open(scheduled_task_ui_metadata_path())
            .expect("ui metadata");
        let manager =
            open_scheduled_automation_manager(scheduled_automation_root()).expect("automations");
        let state = ScheduledTaskState {
            automations: Arc::new(tokio::sync::Mutex::new(manager)),
            task_manager: None,
            sessions,
            read_state,
            model_bindings,
            task_kinds,
            ui_metadata,
            history_archive: ScheduledHistoryArchiveStore::open(scheduled_history_archive_path())
                .expect("history archive"),
            operation_locks: ParkingMutex::new(HashMap::new()),
            pool: None,
            fallback_model: default_automation_model(None),
            scheduler_cancel: None,
            scheduler_handle: None,
            retention_handle: None,
        };
        let created = state
            .create_for_test(CreateScheduledTaskInput {
                name: "测试计划".to_string(),
                prompt: "检查项目状态".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=6".to_string(),
                cwds: vec![dir.join("workspace").to_string_lossy().into_owned()],
                kind: None,
                model: None,
                model_id: None,
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
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn tasks_run_without_a_workspace_like_ordinary_chats() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        let sessions = SessionStore::boot().expect("session store");
        sessions
            .reconcile_scheduled_profiles()
            .expect("reconcile scheduled profiles");
        let read_state =
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
                .expect("read state");
        let model_bindings = ScheduledTaskModelBindingStore::open(scheduled_model_bindings_path())
            .expect("model bindings");
        let task_kinds =
            ScheduledTaskKindStore::open(scheduled_task_kinds_path()).expect("task kinds");
        let ui_metadata = ScheduledTaskUiMetadataStore::open(scheduled_task_ui_metadata_path())
            .expect("ui metadata");
        let manager =
            open_scheduled_automation_manager(scheduled_automation_root()).expect("automations");
        let state = ScheduledTaskState {
            automations: Arc::new(tokio::sync::Mutex::new(manager)),
            task_manager: None,
            sessions,
            read_state,
            model_bindings,
            task_kinds,
            ui_metadata,
            history_archive: ScheduledHistoryArchiveStore::open(scheduled_history_archive_path())
                .expect("history archive"),
            operation_locks: ParkingMutex::new(HashMap::new()),
            pool: None,
            fallback_model: default_automation_model(None),
            scheduler_cancel: None,
            scheduler_handle: None,
            retention_handle: None,
        };

        // 无 cwds 的任务是一等公民并直接激活；工作间由 automation_id 自动分配。
        let created = state
            .create_for_test(CreateScheduledTaskInput {
                name: "开箱即用".to_string(),
                prompt: "汇总近期工作".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: vec!["D:/should-be-ignored".to_string()],
                kind: None,
                model: None,
                model_id: None,
                mode: Some("yolo".to_string()),
                allow_shell: Some(false),
                trust_mode: Some(false),
                auto_approve: Some(false),
                paused: Some(false),
            })
            .await
            .expect("create without workspace");
        assert_eq!(created.status, "active");
        assert!(created.cwds.is_empty(), "configured cwds must be ignored");
        let persisted = state
            .automations
            .lock()
            .await
            .get_automation(&created.id)
            .expect("persisted automation");
        assert_eq!(
            persisted.cwds,
            vec![crate::platform::paths::scheduled_task_workspace_dir(
                &created.id
            )],
            "backend must persist the internally assigned task workspace"
        );
        assert!(
            persisted.cwds[0].is_dir(),
            "internal scheduled task workspace should exist on disk"
        );

        let paused = state
            .pause_for_test(created.id.clone())
            .await
            .expect("pause");
        assert_eq!(paused.status, "paused");
        let resumed = state
            .resume_for_test(created.id.clone())
            .await
            .expect("resume must not require a workspace");
        assert_eq!(resumed.status, "active");

        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn create_update_delete_scheduled_task_model_binding() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        let sessions = SessionStore::boot().expect("session store");
        sessions
            .reconcile_scheduled_profiles()
            .expect("reconcile scheduled profiles");
        let read_state =
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
                .expect("read state");
        let model_bindings = ScheduledTaskModelBindingStore::open(scheduled_model_bindings_path())
            .expect("model bindings");
        let task_kinds =
            ScheduledTaskKindStore::open(scheduled_task_kinds_path()).expect("task kinds");
        let ui_metadata = ScheduledTaskUiMetadataStore::open(scheduled_task_ui_metadata_path())
            .expect("ui metadata");
        let manager =
            open_scheduled_automation_manager(scheduled_automation_root()).expect("automations");
        let state = ScheduledTaskState {
            automations: Arc::new(tokio::sync::Mutex::new(manager)),
            task_manager: None,
            sessions,
            read_state,
            model_bindings,
            task_kinds,
            ui_metadata,
            history_archive: ScheduledHistoryArchiveStore::open(scheduled_history_archive_path())
                .expect("history archive"),
            operation_locks: ParkingMutex::new(HashMap::new()),
            pool: None,
            fallback_model: default_automation_model(None),
            scheduler_cancel: None,
            scheduler_handle: None,
            retention_handle: None,
        };

        let created = state
            .create_for_test(CreateScheduledTaskInput {
                name: "模型绑定".to_string(),
                prompt: "检查模型绑定".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                kind: None,
                model: Some("deepseek-v4-flash".to_string()),
                model_id: Some("deepseek-b".to_string()),
                mode: Some("yolo".to_string()),
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                paused: Some(false),
            })
            .await
            .expect("create model-bound task");
        assert_eq!(created.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(created.model_id.as_deref(), Some("deepseek-b"));
        assert_eq!(
            state
                .model_bindings
                .model_id_for(&created.id, "deepseek-v4-flash")
                .as_deref(),
            Some("deepseek-b")
        );

        let updated = state
            .update_for_test(
                created.id.clone(),
                UpdateScheduledTaskInput {
                    name: None,
                    prompt: None,
                    rrule: None,
                    cwds: None,
                    model: Some("qwen-max".to_string()),
                    model_id: Some("qwen-prod".to_string()),
                    mode: Some("yolo".to_string()),
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: None,
                    paused: None,
                },
            )
            .await
            .expect("update model binding");
        assert_eq!(updated.model.as_deref(), Some("qwen-max"));
        assert_eq!(updated.model_id.as_deref(), Some("qwen-prod"));
        assert!(
            state
                .model_bindings
                .model_id_for(&created.id, "deepseek-v4-flash")
                .is_none()
        );
        assert_eq!(
            state
                .model_bindings
                .model_id_for(&created.id, "qwen-max")
                .as_deref(),
            Some("qwen-prod")
        );

        state
            .delete_for_test(created.id.clone())
            .await
            .expect("delete model-bound task");
        assert!(
            state
                .model_bindings
                .model_id_for(&created.id, "qwen-max")
                .is_none()
        );

        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Full lifecycle of the `memory_organize` kind: junk kinds are rejected at creation
    /// and leave nothing persisted; a valid kind lands in the sidecar and comes back on
    /// the DTO; the update contract has no kind; the delete receipt still carries the
    /// kind and the sidecar is cleaned up.
    #[tokio::test]
    async fn memory_organize_kind_create_update_and_delete_lifecycle() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        // The lifecycle targets a user with memory enabled: turn memory on first (for the
        // creation gate, see memory_organize_task_creation_requires_memory_enabled).
        let settings = crate::platform::paths::settings_path();
        std::fs::create_dir_all(settings.parent().unwrap()).expect("settings dir");
        std::fs::write(
            &settings,
            "{\"language\":\"zh-Hans\",\"memory_enabled\":true}",
        )
        .expect("write settings");
        let sessions = SessionStore::boot().expect("session store");
        sessions
            .reconcile_scheduled_profiles()
            .expect("reconcile scheduled profiles");
        let read_state =
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
                .expect("read state");
        let model_bindings = ScheduledTaskModelBindingStore::open(scheduled_model_bindings_path())
            .expect("model bindings");
        let task_kinds =
            ScheduledTaskKindStore::open(scheduled_task_kinds_path()).expect("task kinds");
        let ui_metadata = ScheduledTaskUiMetadataStore::open(scheduled_task_ui_metadata_path())
            .expect("ui metadata");
        let manager =
            open_scheduled_automation_manager(scheduled_automation_root()).expect("automations");
        let state = ScheduledTaskState {
            automations: Arc::new(tokio::sync::Mutex::new(manager)),
            task_manager: None,
            sessions,
            read_state,
            model_bindings,
            task_kinds,
            ui_metadata,
            history_archive: ScheduledHistoryArchiveStore::open(scheduled_history_archive_path())
                .expect("history archive"),
            operation_locks: ParkingMutex::new(HashMap::new()),
            pool: None,
            fallback_model: default_automation_model(None),
            scheduler_cancel: None,
            scheduler_handle: None,
            retention_handle: None,
        };

        // Junk kind: the rejection error matches the mode-check style, and nothing is
        // persisted at all.
        let error = state
            .create_for_test(CreateScheduledTaskInput {
                name: "bad kind".to_string(),
                prompt: "must not persist".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                kind: Some("summarize".to_string()),
                model: None,
                model_id: None,
                mode: Some("yolo".to_string()),
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                paused: Some(false),
            })
            .await
            .expect_err("unknown kind must be rejected");
        assert!(error.contains("memory_organize"), "{error}");
        assert!(
            state
                .automations
                .lock()
                .await
                .list_automations()
                .expect("list")
                .is_empty(),
            "rejected kind must not leave an automation"
        );
        assert!(
            state.task_kinds.registry.read().tasks.is_empty(),
            "rejected kind must not leave a kind entry"
        );

        // Valid kind: persisted to the sidecar, DTO carries the kind.
        let created = state
            .create_for_test(CreateScheduledTaskInput {
                name: "整理记忆".to_string(),
                prompt: "ignored".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=24".to_string(),
                cwds: Vec::new(),
                kind: Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE.to_string()),
                model: None,
                model_id: None,
                mode: Some("yolo".to_string()),
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                paused: Some(false),
            })
            .await
            .expect("create memory organize task");
        assert_eq!(
            created.kind.as_deref(),
            Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE)
        );
        assert_eq!(
            state.task_kinds.kind_for(&created.id).as_deref(),
            Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE)
        );
        let reloaded =
            ScheduledTaskKindStore::open(scheduled_task_kinds_path()).expect("reload kind store");
        assert_eq!(
            reloaded.kind_for(&created.id).as_deref(),
            Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE),
            "kind must persist across restart"
        );

        // Update contract has no kind: an extra kind in the JSON is silently dropped by
        // serde, sidecar unchanged.
        let update: UpdateScheduledTaskInput = serde_json::from_value(serde_json::json!({
            "name": "整理记忆（改名）",
            "kind": "summarize"
        }))
        .expect("update payload with a stale kind field parses");
        let updated = state
            .update_for_test(created.id.clone(), update)
            .await
            .expect("update memory organize task");
        assert_eq!(updated.name, "整理记忆（改名）");
        assert_eq!(
            updated.kind.as_deref(),
            Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE),
            "kind is immutable after creation"
        );
        assert_eq!(
            state.task_kinds.kind_for(&created.id).as_deref(),
            Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE)
        );

        // Delete receipt still carries the kind for frontend rendering; sidecar cleaned up.
        let deleted = state
            .delete_for_test(created.id.clone())
            .await
            .expect("delete memory organize task");
        assert_eq!(
            deleted.task.kind.as_deref(),
            Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE)
        );
        assert_eq!(state.task_kinds.kind_for(&created.id), None);

        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn scheduled_task_ui_metadata_pin_unpin_and_delete_cleanup() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        let sessions = SessionStore::boot().expect("session store");
        sessions
            .reconcile_scheduled_profiles()
            .expect("reconcile scheduled profiles");
        let read_state =
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
                .expect("read state");
        let model_bindings = ScheduledTaskModelBindingStore::open(scheduled_model_bindings_path())
            .expect("model bindings");
        let task_kinds =
            ScheduledTaskKindStore::open(scheduled_task_kinds_path()).expect("task kinds");
        let ui_metadata = ScheduledTaskUiMetadataStore::open(scheduled_task_ui_metadata_path())
            .expect("ui metadata");
        let manager =
            open_scheduled_automation_manager(scheduled_automation_root()).expect("automations");
        let state = ScheduledTaskState {
            automations: Arc::new(tokio::sync::Mutex::new(manager)),
            task_manager: None,
            sessions,
            read_state,
            model_bindings,
            task_kinds,
            ui_metadata,
            history_archive: ScheduledHistoryArchiveStore::open(scheduled_history_archive_path())
                .expect("history archive"),
            operation_locks: ParkingMutex::new(HashMap::new()),
            pool: None,
            fallback_model: default_automation_model(None),
            scheduler_cancel: None,
            scheduler_handle: None,
            retention_handle: None,
        };

        let created = state
            .create_for_test(CreateScheduledTaskInput {
                name: "置顶任务".to_string(),
                prompt: "检查置顶".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                kind: None,
                model: Some("deepseek-v4-flash".to_string()),
                model_id: None,
                mode: Some("yolo".to_string()),
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                paused: Some(false),
            })
            .await
            .expect("create task");
        assert!(!created.pinned);
        assert!(created.pinned_at.is_none());

        let pinned = state
            .set_pinned_for_test(created.id.clone(), true)
            .await
            .expect("pin task");
        assert!(pinned.pinned);
        assert!(pinned.pinned_at.is_some());
        assert_eq!(
            state.ui_metadata.metadata_for(&created.id),
            (true, pinned.pinned_at.clone())
        );

        let reloaded =
            ScheduledTaskUiMetadataStore::open(scheduled_task_ui_metadata_path()).expect("reload");
        assert_eq!(
            reloaded.metadata_for(&created.id),
            (true, pinned.pinned_at.clone())
        );

        let unpinned = state
            .set_pinned_for_test(created.id.clone(), false)
            .await
            .expect("unpin task");
        assert!(!unpinned.pinned);
        assert!(unpinned.pinned_at.is_none());
        assert_eq!(state.ui_metadata.metadata_for(&created.id), (false, None));

        state
            .set_pinned_for_test(created.id.clone(), true)
            .await
            .expect("pin before delete");
        state
            .delete_for_test(created.id.clone())
            .await
            .expect("delete pinned task");
        assert_eq!(state.ui_metadata.metadata_for(&created.id), (false, None));

        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn owned_session_delete_reports_id_only_after_successful_removal() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        let sessions = SessionStore::boot().expect("sessions");
        let create_session = |task_id: &str| {
            sessions
                .create_scheduled_run(crate::features::sessions::ScheduledRunProfile {
                    task_id: task_id.to_string(),
                    model: "model-1".to_string(),
                    model_id: None,
                    workspace: dir.join("workspace"),
                    mode: crate::features::sessions::ScheduledRunMode::Agent,
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
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn update_and_resume_round_trip() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        let sessions = SessionStore::boot().expect("session store");
        sessions
            .reconcile_scheduled_profiles()
            .expect("reconcile scheduled profiles");
        let read_state =
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
                .expect("read state");
        let model_bindings = ScheduledTaskModelBindingStore::open(scheduled_model_bindings_path())
            .expect("model bindings");
        let task_kinds =
            ScheduledTaskKindStore::open(scheduled_task_kinds_path()).expect("task kinds");
        let ui_metadata = ScheduledTaskUiMetadataStore::open(scheduled_task_ui_metadata_path())
            .expect("ui metadata");
        let manager =
            open_scheduled_automation_manager(scheduled_automation_root()).expect("automations");
        let state = ScheduledTaskState {
            automations: Arc::new(tokio::sync::Mutex::new(manager)),
            task_manager: None,
            sessions,
            read_state,
            model_bindings,
            task_kinds,
            ui_metadata,
            history_archive: ScheduledHistoryArchiveStore::open(scheduled_history_archive_path())
                .expect("history archive"),
            operation_locks: ParkingMutex::new(HashMap::new()),
            pool: None,
            fallback_model: default_automation_model(None),
            scheduler_cancel: None,
            scheduler_handle: None,
            retention_handle: None,
        };
        let expected_allow_shell = current_yolo_allow_shell();

        let created = state
            .create_for_test(CreateScheduledTaskInput {
                name: "晨检".to_string(),
                prompt: "检查运行状态".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=2".to_string(),
                cwds: vec!["/tmp/workspace-a".to_string()],
                kind: None,
                model: None,
                model_id: None,
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
                    model_id: None,
                    mode: Some("plan".to_string()),
                    allow_shell: Some(!expected_allow_shell),
                    trust_mode: Some(false),
                    auto_approve: Some(false),
                    paused: None,
                },
            )
            .await
            .expect("update");
        assert_eq!(updated.name, "晚检");
        assert_eq!(updated.prompt, "检查夜间任务");
        assert_eq!(updated.rrule, "FREQ=HOURLY;INTERVAL=4");
        assert!(updated.cwds.is_empty(), "cwds updates must be ignored");
        assert_eq!(updated.mode.as_deref(), Some("yolo"));
        assert_eq!(
            updated.allow_shell, expected_allow_shell,
            "Shell must follow the global Yolo setting"
        );
        assert!(updated.trust_mode, "scheduled Yolo must stay trusted");
        assert!(updated.auto_approve);
        assert_eq!(updated.status, "paused");

        let resumed = state
            .resume_for_test(created.id.clone())
            .await
            .expect("resume");
        assert_eq!(resumed.status, "active");
        assert!(resumed.next_run_at.is_some());
        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn task_dto_does_not_expose_legacy_source_session_binding() {
        let now = chrono::Utc::now();
        let dto = map_scheduled_task_with_bindings(
            AutomationRecord {
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
                delivery_mode: None,
                status: AutomationStatus::Active,
                created_at: now,
                updated_at: now,
                next_run_at: None,
                last_run_at: None,
            },
            None,
            None,
            None,
        );

        let value = serde_json::to_value(dto).expect("serialize task dto");
        assert!(
            value.get("sourceSessionId").is_none(),
            "base-owned automations must not expose the removed chat-session binding"
        );
        assert_eq!(
            value.get("kind"),
            Some(&serde_json::Value::Null),
            "chat tasks serialize kind as null when no kind store is consulted"
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
    fn forkguard_create_request_persists_model_and_global_yolo_permissions() {
        let request = build_create_request(
            CreateScheduledTaskInput {
                name: "daily brief".to_string(),
                prompt: "prepare it".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                kind: None,
                model: None,
                model_id: None,
                mode: Some("plan".to_string()),
                allow_shell: Some(false),
                trust_mode: Some(false),
                auto_approve: Some(false),
                paused: Some(false),
            },
            "active-user-model".to_string(),
            true,
        )
        .expect("valid create request");

        assert_eq!(request.model.as_deref(), Some("active-user-model"));
        assert_eq!(request.mode.as_deref(), Some("yolo"));
        assert_eq!(request.allow_shell, Some(true));
        assert_eq!(request.trust_mode, Some(true));
        assert_eq!(request.auto_approve, Some(true));
        // 目录概念已移除:无 cwds 也直接激活,不再强制暂停。
        assert_eq!(request.status, Some(AutomationStatus::Active));
        assert!(request.cwds.is_empty());
    }

    #[tokio::test]
    async fn create_rejects_noncanonical_mode_before_persisting() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        let sessions = SessionStore::boot().expect("session store");
        sessions
            .reconcile_scheduled_profiles()
            .expect("reconcile scheduled profiles");
        let read_state =
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
                .expect("read state");
        let model_bindings = ScheduledTaskModelBindingStore::open(scheduled_model_bindings_path())
            .expect("model bindings");
        let task_kinds =
            ScheduledTaskKindStore::open(scheduled_task_kinds_path()).expect("task kinds");
        let ui_metadata = ScheduledTaskUiMetadataStore::open(scheduled_task_ui_metadata_path())
            .expect("ui metadata");
        let manager =
            open_scheduled_automation_manager(scheduled_automation_root()).expect("automations");
        let state = ScheduledTaskState {
            automations: Arc::new(tokio::sync::Mutex::new(manager)),
            task_manager: None,
            sessions,
            read_state,
            model_bindings,
            task_kinds,
            ui_metadata,
            history_archive: ScheduledHistoryArchiveStore::open(scheduled_history_archive_path())
                .expect("history archive"),
            operation_locks: ParkingMutex::new(HashMap::new()),
            pool: None,
            fallback_model: default_automation_model(None),
            scheduler_cancel: None,
            scheduler_handle: None,
            retention_handle: None,
        };

        let error = state
            .create_for_test(CreateScheduledTaskInput {
                name: "invalid mode".to_string(),
                prompt: "must not persist".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                kind: None,
                model: None,
                model_id: None,
                mode: Some("planner".to_string()),
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                paused: Some(false),
            })
            .await
            .expect_err("planner is not a canonical scheduled mode");
        assert!(error.contains("agent|plan|yolo"), "{error}");
        assert!(
            state
                .automations
                .lock()
                .await
                .list_automations()
                .expect("list")
                .is_empty()
        );

        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn update_rejects_noncanonical_mode_before_persisting() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        let sessions = SessionStore::boot().expect("session store");
        sessions
            .reconcile_scheduled_profiles()
            .expect("reconcile scheduled profiles");
        let read_state =
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
                .expect("read state");
        let model_bindings = ScheduledTaskModelBindingStore::open(scheduled_model_bindings_path())
            .expect("model bindings");
        let task_kinds =
            ScheduledTaskKindStore::open(scheduled_task_kinds_path()).expect("task kinds");
        let ui_metadata = ScheduledTaskUiMetadataStore::open(scheduled_task_ui_metadata_path())
            .expect("ui metadata");
        let manager =
            open_scheduled_automation_manager(scheduled_automation_root()).expect("automations");
        let state = ScheduledTaskState {
            automations: Arc::new(tokio::sync::Mutex::new(manager)),
            task_manager: None,
            sessions,
            read_state,
            model_bindings,
            task_kinds,
            ui_metadata,
            history_archive: ScheduledHistoryArchiveStore::open(scheduled_history_archive_path())
                .expect("history archive"),
            operation_locks: ParkingMutex::new(HashMap::new()),
            pool: None,
            fallback_model: default_automation_model(None),
            scheduler_cancel: None,
            scheduler_handle: None,
            retention_handle: None,
        };
        let created = state
            .create_for_test(CreateScheduledTaskInput {
                name: "valid mode".to_string(),
                prompt: "keep valid".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                kind: None,
                model: None,
                model_id: None,
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
                    model_id: None,
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
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn run_dto_exposes_session_only_for_the_owning_task() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let store = SessionStore::boot().expect("open test sessions");
        let saved = store
            .create_scheduled_run(crate::features::sessions::ScheduledRunProfile {
                task_id: "automation-1".to_string(),
                model: "model-1".to_string(),
                model_id: None,
                workspace: dir.join("workspace"),
                mode: crate::features::sessions::ScheduledRunMode::Agent,
                allow_shell: false,
                trust_mode: false,
                auto_approve: false,
            })
            .expect("create scheduled session");
        let now = chrono::Utc::now();
        let read_state =
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
                .expect("open read state");
        let mut session_titles = scheduled_session_titles(&store).expect("list scheduled titles");
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

        let owned = serde_json::to_value(map_scheduled_run(
            owned_run.clone(),
            &store,
            &read_state,
            &session_titles,
        ))
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
                automation_id: "automation-2".to_string(),
                ..owned_run.clone()
            },
            &store,
            &read_state,
            &session_titles,
        ))
        .expect("serialize mismatched run");
        assert!(
            mismatched
                .get("sessionId")
                .is_some_and(serde_json::Value::is_null)
        );

        let unlinked_run = AutomationRunRecord {
            thread_id: None,
            ..owned_run.clone()
        };
        let recovered = owned_scheduled_sessions("automation-1", &[unlinked_run], &store)
            .expect("recover session from durable task ownership");
        assert_eq!(
            recovered,
            vec![(saved.metadata.id.clone(), "automation-1".to_string())]
        );

        std::fs::remove_file(
            crate::platform::paths::sessions_root().join(format!("{}.json", saved.metadata.id)),
        )
        .expect("remove scheduled session payload");
        session_titles.remove(&saved.metadata.id);
        let missing_payload = serde_json::to_value(map_scheduled_run(
            owned_run,
            &store,
            &read_state,
            &session_titles,
        ))
        .expect("serialize run with missing session payload");
        assert!(
            missing_payload
                .get("sessionId")
                .is_some_and(serde_json::Value::is_null)
        );

        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scheduled_run_conversations_are_viewable_once_their_owned_session_exists() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };

        let sessions = SessionStore::boot().expect("open test sessions");
        let make_session = || {
            sessions
                .create_scheduled_run(crate::features::sessions::ScheduledRunProfile {
                    task_id: "automation-1".to_string(),
                    model: "model-1".to_string(),
                    model_id: None,
                    workspace: dir.join("ignored-workspace"),
                    mode: crate::features::sessions::ScheduledRunMode::Agent,
                    allow_shell: false,
                    trust_mode: false,
                    auto_approve: false,
                })
                .expect("create scheduled session")
                .metadata
                .id
        };
        let session_1 = make_session();
        let session_2 = make_session();
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
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
                .expect("open read state");

        assert!(scheduled_run_is_unread(&run_1, &sessions, &read_state));
        assert!(scheduled_run_is_unread(&run_2, &sessions, &read_state));
        assert!(ensure_scheduled_run_is_viewable(&run_1, &sessions).is_ok());
        assert!(has_unread_scheduled_runs(
            &[run_1.clone(), run_2.clone()],
            &sessions,
            &read_state
        ));

        sessions.set_hidden(&session_1, true);
        sessions.set_hidden(&session_2, true);
        assert!(!scheduled_run_is_unread(&run_1, &sessions, &read_state));
        assert!(!has_unread_scheduled_runs(
            &[run_1.clone(), run_2.clone()],
            &sessions,
            &read_state
        ));
        sessions.set_hidden(&session_1, false);
        sessions.set_hidden(&session_2, false);

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
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
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
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
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
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        let sessions = SessionStore::boot().expect("session store");
        sessions
            .reconcile_scheduled_profiles()
            .expect("reconcile scheduled profiles");
        let read_state =
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
                .expect("read state");
        let model_bindings = ScheduledTaskModelBindingStore::open(scheduled_model_bindings_path())
            .expect("model bindings");
        let task_kinds =
            ScheduledTaskKindStore::open(scheduled_task_kinds_path()).expect("task kinds");
        let ui_metadata = ScheduledTaskUiMetadataStore::open(scheduled_task_ui_metadata_path())
            .expect("ui metadata");
        let manager =
            open_scheduled_automation_manager(scheduled_automation_root()).expect("automations");
        let state = ScheduledTaskState {
            automations: Arc::new(tokio::sync::Mutex::new(manager)),
            task_manager: None,
            sessions,
            read_state,
            model_bindings,
            task_kinds,
            ui_metadata,
            history_archive: ScheduledHistoryArchiveStore::open(scheduled_history_archive_path())
                .expect("history archive"),
            operation_locks: ParkingMutex::new(HashMap::new()),
            pool: None,
            fallback_model: default_automation_model(None),
            scheduler_cancel: None,
            scheduler_handle: None,
            retention_handle: None,
        };
        let created = state
            .create_for_test(CreateScheduledTaskInput {
                name: "serialized delete".to_string(),
                prompt: "test operation lock".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                kind: None,
                model: None,
                model_id: None,
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
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn delete_success_is_not_reverted_by_read_state_cleanup_failure() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        let sessions = SessionStore::boot().expect("session store");
        sessions
            .reconcile_scheduled_profiles()
            .expect("reconcile scheduled profiles");
        let read_state =
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
                .expect("read state");
        let model_bindings = ScheduledTaskModelBindingStore::open(scheduled_model_bindings_path())
            .expect("model bindings");
        let task_kinds =
            ScheduledTaskKindStore::open(scheduled_task_kinds_path()).expect("task kinds");
        let ui_metadata = ScheduledTaskUiMetadataStore::open(scheduled_task_ui_metadata_path())
            .expect("ui metadata");
        let manager =
            open_scheduled_automation_manager(scheduled_automation_root()).expect("automations");
        let mut state = ScheduledTaskState {
            automations: Arc::new(tokio::sync::Mutex::new(manager)),
            task_manager: None,
            sessions,
            read_state,
            model_bindings,
            task_kinds,
            ui_metadata,
            history_archive: ScheduledHistoryArchiveStore::open(scheduled_history_archive_path())
                .expect("history archive"),
            operation_locks: ParkingMutex::new(HashMap::new()),
            pool: None,
            fallback_model: default_automation_model(None),
            scheduler_cancel: None,
            scheduler_handle: None,
            retention_handle: None,
        };
        let created = state
            .create_for_test(CreateScheduledTaskInput {
                name: "cleanup failure".to_string(),
                prompt: "delete remains successful".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                kind: None,
                model: None,
                model_id: None,
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
        assert!(
            state
                .automations
                .lock()
                .await
                .get_automation(&created.id)
                .is_err()
        );

        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    // Memory organize tasks follow the same rule as the settings-page manual entry:
    // refuse creation while memory is disabled (off by default, force-off for en-ja),
    // or the task is created fine but every run logs a "memory disabled" failure.
    #[tokio::test]
    async fn memory_organize_task_creation_requires_memory_enabled() {
        let _guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = temp_home();
        let previous = std::env::var("PINVOU3_HOME").ok();
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &dir) };
        let sessions = SessionStore::boot().expect("session store");
        sessions
            .reconcile_scheduled_profiles()
            .expect("reconcile scheduled profiles");
        let read_state =
            ScheduledRunReadStore::open(crate::platform::paths::scheduled_run_read_state_path())
                .expect("read state");
        let model_bindings = ScheduledTaskModelBindingStore::open(scheduled_model_bindings_path())
            .expect("model bindings");
        let task_kinds =
            ScheduledTaskKindStore::open(scheduled_task_kinds_path()).expect("task kinds");
        let ui_metadata = ScheduledTaskUiMetadataStore::open(scheduled_task_ui_metadata_path())
            .expect("ui metadata");
        let manager =
            open_scheduled_automation_manager(scheduled_automation_root()).expect("automations");
        let state = ScheduledTaskState {
            automations: Arc::new(tokio::sync::Mutex::new(manager)),
            task_manager: None,
            sessions,
            read_state,
            model_bindings,
            task_kinds,
            ui_metadata,
            history_archive: ScheduledHistoryArchiveStore::open(scheduled_history_archive_path())
                .expect("history archive"),
            operation_locks: ParkingMutex::new(HashMap::new()),
            pool: None,
            fallback_model: default_automation_model(None),
            scheduler_cancel: None,
            scheduler_handle: None,
            retention_handle: None,
        };
        let input = |kind: Option<String>| CreateScheduledTaskInput {
            name: "记忆整理".to_string(),
            prompt: "整理长期记忆".to_string(),
            rrule: "FREQ=WEEKLY;BYDAY=MO;BYHOUR=9;BYMINUTE=30".to_string(),
            cwds: vec![dir.join("workspace").to_string_lossy().into_owned()],
            kind,
            model: None,
            model_id: None,
            mode: Some("agent".to_string()),
            allow_shell: Some(false),
            trust_mode: Some(false),
            auto_approve: Some(false),
            paused: Some(false),
        };

        // Default config (memory_enabled=false): creation is refused.
        let refused = state
            .create_for_test(input(Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE.to_string())))
            .await;
        assert!(
            refused.is_err(),
            "memory organize tasks must be refused while memory is disabled"
        );

        // With memory enabled, the same input creates successfully.
        let settings = crate::platform::paths::settings_path();
        std::fs::create_dir_all(settings.parent().unwrap()).expect("settings dir");
        std::fs::write(
            &settings,
            "{\"language\":\"zh-Hans\",\"memory_enabled\":true}",
        )
        .expect("write settings");
        let created = state
            .create_for_test(input(Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE.to_string())))
            .await
            .expect("create with memory enabled");
        assert_eq!(
            created.kind.as_deref(),
            Some(SCHEDULED_TASK_KIND_MEMORY_ORGANIZE)
        );

        match previous {
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
