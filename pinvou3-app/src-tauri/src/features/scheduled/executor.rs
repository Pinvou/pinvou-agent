use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use deepseek_tui::automation_manager::SharedAutomationManager;
use deepseek_tui::core::events::TurnOutcomeStatus;
use deepseek_tui::task_manager::{
    ExecutionTask, TaskExecutionEvent, TaskExecutionResult, TaskExecutor, TaskStatus,
    TaskTerminalReason,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::features::assistant::engine_pool::{EnginePool, ScheduledTurnCompletion};
use crate::features::assistant::platform::bridge::Pinvou3Bridge;
use crate::features::memory::MemoryOrganizeReport;
use crate::features::scheduled::tasks::{
    SCHEDULED_MESSAGE_MAX_CHARS, ScheduledTaskKindLookup, ScheduledTaskKindStore,
};
use crate::features::sessions::{ScheduledRunMode, ScheduledRunProfile, SessionStore};
use crate::platform::prefs::{SavedModel, UserPrefs};

type StartedCallback =
    Box<dyn FnMut(String) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send>;
type ModelIdResolver = Arc<dyn Fn(&str, &str) -> Option<String> + Send + Sync>;
/// automation_id → task kind (None = ordinary chat task).
type KindResolver = Arc<dyn Fn(&str) -> ScheduledTaskKindLookup + Send + Sync>;

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

    /// Runs one app-side memory organization pass (the run body of a `memory_organize`
    /// kind task). Unlike a conversation turn: no session is created and the engine is
    /// not involved. See [`ScheduledChatExecutor::execute_memory_organize`] for
    /// cancellation semantics.
    async fn organize_memory(&self, cancel: CancellationToken) -> Result<MemoryOrganizeReport>;

    /// Delivers one scheduled message into the target session (the run body of a
    /// `session_message` kind task): steer into the current turn when the target is
    /// mid-turn, otherwise dispatch a new turn — the same delivery semantics as
    /// features::messaging, with one stated parity delta (doc §10.2): messaging
    /// bounds each leg at 30s while steer and dispatch share one 30s budget here.
    /// No session is created for the run itself.
    async fn deliver_session_message(
        &self,
        target_session: &str,
        message: String,
        cancel: CancellationToken,
    ) -> Result<()>;
}

/// Production implementation backed by the existing session store and engine
/// pool. The pool owns terminal persistence and always evicts the engine while
/// this runtime deliberately retains the durable scheduled session.
#[derive(Clone)]
pub(crate) struct EngineScheduledRuntime {
    store: SessionStore,
    pool: EnginePool,
    model_id_resolver: ModelIdResolver,
}

impl EngineScheduledRuntime {
    pub(crate) fn new(
        store: SessionStore,
        pool: EnginePool,
        model_id_resolver: ModelIdResolver,
    ) -> Self {
        Self {
            store,
            pool,
            model_id_resolver,
        }
    }
}

/// Shell 开关的唯一读取点：与普通 Yolo 会话同口径（`Pinvou3Bridge::allow_shell_for_prefs`）。
/// 任务创建（tasks.rs 的 build_create_request）与运行时（下面的 trait impl）共用，
/// 避免两份同样的偏好读取各自漂移。
pub(crate) fn current_yolo_allow_shell() -> bool {
    Pinvou3Bridge::allow_shell_for_prefs(&UserPrefs::load())
}

#[async_trait]
impl ScheduledConversationRuntime for EngineScheduledRuntime {
    fn yolo_allow_shell(&self) -> bool {
        current_yolo_allow_shell()
    }

    fn model_id_for_automation(&self, automation_id: &str, model: &str) -> Option<String> {
        (self.model_id_resolver)(automation_id, model)
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
        // Same policy as the chat command layer's organize_memory: refuse up front when
        // memory is disabled.
        if !crate::features::memory::memory_enabled() {
            anyhow::bail!("memory disabled");
        }
        // Same bridge resolution as commands::memory's organize_memory: with an active
        // session use its bound model (fresh bridge, incl. runtime credential prep);
        // otherwise fall back to the shared bridge plus a direct read of global prefs.
        let bridge = match self.store.active_id() {
            Some(session_id) => self.pool.fresh_bridge_for(&session_id).await?,
            None => {
                let mut bridge = self.pool.bridge.clone();
                bridge.prefs = UserPrefs::load();
                bridge.session_model = None;
                bridge
            }
        };
        let report =
            crate::features::memory::organize_memory_with_llm(&bridge, Some(&cancel)).await?;
        // Parity with the manual command's post-organize refresh: organize may have
        // deleted or merged items, and `ensure_runtime_prompt` only writes the runtime
        // memory file when it is missing, so the active session's file must be
        // re-rendered here or it keeps serving removed items on the next turns.
        // Best-effort, like the command layer's refresh: a render failure must not
        // fail a run whose organize itself succeeded.
        if let Some(session_id) = self.store.active_id() {
            if let Err(error) = crate::features::memory::runtime_snapshot(&session_id) {
                eprintln!("[memory] refresh runtime memory after scheduled organize: {error}");
            }
        }
        Ok(report)
    }

    /// Steer-first delivery with the messaging channel's semantics and bounds:
    /// a steer error is the normal idle signal (no live engine / no turn
    /// accepting) and falls back to a new-turn dispatch; both paths are
    /// bounded so a wedged engine cannot hang the scheduled run. The isolated
    /// prefixes are re-checked here (the kind sidecar is app-owned state, not
    /// trusted): waking an unattended session on a schedule is the recursion
    /// direction the creation path already rejects.
    async fn deliver_session_message(
        &self,
        target_session: &str,
        message: String,
        _cancel: CancellationToken,
    ) -> Result<()> {
        ensure_deliverable_target(&self.store, target_session)?;
        tokio::time::timeout(SCHEDULED_MESSAGE_DELIVERY_TIMEOUT, async {
            match self.pool.steer(target_session, message.clone()).await {
                Ok(_) => Ok(()),
                Err(steer_error) => {
                    // The dispatch path re-checks deletion/lifecycle gates
                    // itself (send_reserved_user_message loads the store under
                    // the turn lock); a genuinely gone session fails the run
                    // loudly instead of being retried forever.
                    self.pool
                        .deliver_messaging_turn(target_session, message, false)
                        .await
                        .map_err(|dispatch_error| {
                            anyhow::anyhow!(dispatch_error)
                                .context(format!("dispatch after steer failure ({steer_error:#})"))
                        })
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "delivery timed out after {SCHEDULED_MESSAGE_DELIVERY_TIMEOUT:?} (engine wedged?)"
            ))
        })
    }
}

/// Upper bound for one scheduled-message delivery attempt (same reasoning as
/// features::messaging's delivery timeout: a wedged engine must not hang the
/// scheduled run; the run is marked failed; recurring tasks can try at their next slot).
const SCHEDULED_MESSAGE_DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Run-time re-check for a scheduled-message target — the delivery-side twin
/// of the messaging channel's `check_participant_id` gate (charset + length
/// + isolation, case-insensitive on the prefixes): the kind sidecar is
/// app-owned state, not trusted, so a hand-tampered target must fail here,
/// before any engine is woken. Waking an unattended session on a schedule is
/// the recursion direction the creation path already rejects. Split into a
/// free function so tests can drive it with a real store without an
/// EnginePool.
pub(crate) fn ensure_deliverable_target(store: &SessionStore, target_session: &str) -> Result<()> {
    super::message::validate_target_identity(target_session).map_err(anyhow::Error::msg)?;
    if store.scheduled_profile(target_session).is_some() {
        anyhow::bail!(
            "target {target_session} is a scheduled-run session and cannot receive scheduled messages"
        );
    }
    // Review round-4 B1 / round-5 M-A disposition: ACP-backend and
    // code-session targets are owned by the independent code page — a failed
    // steer falls back to a NATIVE turn on a session whose transcript and
    // acp-state another runtime owns, and a recurring task would repeat it
    // every fire. PARITY DELTA vs messaging's twin gate (stated per round-5
    // M-A): this check reads the sidecar `session-agents.json` from disk,
    // not the live AcpPool — it therefore misses sessions classified ACP
    // only by the pool's in-memory metadata recovery (index record lost AND
    // boot recovery failed — a double fault). A present unreadable/corrupt
    // index now fails closed; a missing index remains an empty-index case.
    // This is narrower than messaging's live-pool gate.
    let agents = crate::features::codex_acp::SessionAgentStore::load()
        .map_err(|_| anyhow!("target session agent index unavailable"))?;
    if agents.backend(target_session).is_acp() || agents.is_code_session(target_session) {
        anyhow::bail!(
            "target session {target_session} is an ACP/code session; deliver through the independent code page, not scheduled messages"
        );
    }
    super::tasks::ensure_exact_message_target_exists(store, target_session)
        .map_err(anyhow::Error::msg)?;
    Ok(())
}

/// Host executor installed into CodeWhale's `TaskManager` for scheduled
/// automations. Scheduling and durable task/run state remain base-owned.
pub(crate) struct ScheduledChatExecutor {
    runtime: Arc<dyn ScheduledConversationRuntime>,
    kind_resolver: Option<KindResolver>,
    dispatch_store: Option<(SharedAutomationManager, ScheduledTaskKindStore)>,
}

impl ScheduledChatExecutor {
    pub(crate) fn new(runtime: Arc<dyn ScheduledConversationRuntime>) -> Self {
        Self {
            runtime,
            kind_resolver: None,
            dispatch_store: None,
        }
    }

    /// Injects a kind resolver for isolated executor fixtures.
    #[cfg(test)]
    pub(crate) fn with_kind_resolver(mut self, kind_resolver: KindResolver) -> Self {
        self.kind_resolver = Some(kind_resolver);
        self
    }

    pub(crate) fn from_services(
        store: SessionStore,
        pool: EnginePool,
        model_id_resolver: ModelIdResolver,
        dispatch_store: Option<(SharedAutomationManager, ScheduledTaskKindStore)>,
    ) -> Self {
        let mut executor = Self::new(Arc::new(EngineScheduledRuntime::new(
            store,
            pool,
            model_id_resolver,
        )));
        executor.dispatch_store = dispatch_store;
        executor
    }

    fn kind_for(&self, automation_id: &str) -> ScheduledTaskKindLookup {
        match self.kind_resolver.as_ref() {
            Some(resolver) => resolver(automation_id),
            None => ScheduledTaskKindLookup::Chat,
        }
    }

    /// Snapshot the current body and destination under the same lock that
    /// spans app updates. Queued TaskRecord prompts remain enqueue-time input;
    /// a scheduled message uses the complete configuration at dispatch time.
    async fn dispatch_snapshot(
        &self,
        id: &str,
        queued_prompt: &str,
    ) -> Result<(ScheduledTaskKindLookup, String)> {
        match &self.dispatch_store {
            Some((automations, kinds)) => {
                let manager = automations.lock().await;
                let record = manager.get_automation(id)?;
                Ok((kinds.checked_kind_lookup_for(id)?, record.prompt))
            }
            None => Ok((self.kind_for(id), queued_prompt.to_string())),
        }
    }

    /// One run of the `session_message` kind: no conversation is created for
    /// the run itself — the stored prompt is delivered into the target session
    /// (steer into its current turn, or a new turn when idle; the messaging
    /// channel's delivery semantics). The run record stays thread-less like
    /// memory organize: the target session is the thing to open, and the
    /// result text names it. Cancellation mirrors
    /// [`Self::execute_memory_organize`]: the select drops the in-flight
    /// future so a mid-delivery cancel is recorded as canceled instead of
    /// waiting out the delivery bound.
    async fn execute_session_message(
        &self,
        target_session: &str,
        message: &str,
        cancel: CancellationToken,
    ) -> TaskExecutionResult {
        if cancel.is_cancelled() {
            return canceled();
        }
        let message = message.trim();
        if message.is_empty() {
            return failed("scheduled message body is empty");
        }
        // Belt re-check of the create/update cap: a body that slipped past
        // domain validation (hand-edited sidecar, older store) must not be
        // silently delivered oversized — same bound as the messaging channel.
        if message.chars().count() > SCHEDULED_MESSAGE_MAX_CHARS {
            return failed(&format!(
                "prompt exceeds the {SCHEDULED_MESSAGE_MAX_CHARS} character limit"
            ));
        }
        let delivered = tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            delivered = self.runtime.deliver_session_message(
                target_session,
                message.to_string(),
                cancel.clone(),
            ) => Some(delivered),
        };
        match delivered {
            None => canceled(),
            Some(Ok(())) => TaskExecutionResult {
                status: TaskStatus::Completed,
                result_text: Some(super::message::delivered_result(target_session)),
                error: None,
                terminal_reason: TaskTerminalReason::Completed,
            },
            Some(Err(error)) => TaskExecutionResult {
                status: TaskStatus::Failed,
                result_text: None,
                // Secret-only redaction here, matching what this chain can
                // actually carry today: ensure_deliverable_target collapses
                // store-load failures into static text and the per-session
                // sidecar reads surface only error kinds, so no proven
                // absolute-path carrier exists on this chain. (Round-20 复审
                // 订正:此前"sidecar error contexts use bare file names"失实
                // ——session-agent 索引仓的 SessionAgentStore::load_from 在
                // 上下文里嵌 path.display();今天不泄漏仅仅因为上面那道收
                // 敛把它折成了静态文案,若拆掉收敛本臂须像 organize 臂一样
                // 接 redact_storage_prefix。)Round-19 复审:此前注释声称
                // 与本文件 organize 臂同一纪律——失实,organize 臂自
                // round-18 起额外折叠存储根(其链上的 bounded-read io
                // Display 确有绝对路径);本臂将来若引入携带路径的错误构造,
                // 须同步接 redact_storage_prefix(见 organize 臂注释)。
                error: Some(format!(
                    "scheduled message delivery: {}",
                    crate::platform::credential_store::redact_secret(&format!("{error:#}"))
                )),
                terminal_reason: TaskTerminalReason::Failed,
            },
        }
    }

    /// One run of the `memory_organize` kind: no session is created, no engine turn is
    /// issued, and no ThreadCreated / ThreadLinked events are emitted — the run lifecycle
    /// only needs the base Task's terminal state (reconcile writes a Task whose
    /// thread_id/turn_id are unset back to the run record); the organize report itself is
    /// viewed via the memory history commands, keeping run records clean.
    ///
    /// Cancellation semantics: the select makes cancellation take effect immediately
    /// instead of waiting up to 75s for the organize call to return (otherwise the 15s
    /// terminal-state wait when deleting a running task would almost certainly time out).
    /// The cancel branch drops the in-flight future, aborting the unanswered LLM request
    /// with it. A cancellation after the LLM has returned but before actions are applied
    /// is caught by organize's pre-apply check; only the synchronous apply segment
    /// (milliseconds) cannot be interrupted — that case records the run as canceled, and
    /// the report has already landed in organize_history.json for auditing.
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
                result_text: Some(report.summary()),
                error: None,
                terminal_reason: TaskTerminalReason::Completed,
            },
            Err(error) => TaskExecutionResult {
                status: TaskStatus::Failed,
                result_text: None,
                // Same redaction as the chat command layer's organize_memory: the
                // bridge-resolution and LLM error chains can carry endpoint or
                // credential-probing details, and run records are persisted and
                // rendered verbatim in the task-detail UI. Round-18 复审:链上
                // 还有 bounded-read 原语的 io Display(path.display() 的绝对
                // 存储路径,来自 refresh_recent_work_expiry / snapshot load
                // 的 context)——run record 经 list_scheduled_task_runs 出站
                // (Web 允许清单内),与命令层同一口径折叠存储根。Round-21
                // 复审:与命令层 S10-1 同序修正为先折叠后扫描——secret 先行
                // 的旧序在含空白的家目录上会让 ≥24 长度族先吃掉折叠 needle
                // (round-20 源钉 arm<fold<secret 钉住的正是该漏洞序)。
                error: Some(crate::platform::credential_store::redact_secret(&format!(
                    "memory organize: {}",
                    crate::platform::paths::redact_storage_prefix(&format!("{error:#}"))
                ))),
                terminal_reason: TaskTerminalReason::Failed,
            },
        }
    }
}

#[async_trait]
impl TaskExecutor for ScheduledChatExecutor {
    async fn execute(
        &self,
        task: ExecutionTask,
        events: mpsc::Sender<TaskExecutionEvent>,
        cancel: CancellationToken,
    ) -> TaskExecutionResult {
        let allow_shell = self.runtime.yolo_allow_shell();
        let automation_id = task
            .conversation_key()
            .unwrap_or_else(|| task.id())
            .to_string();
        // Memory organize tasks branch off here: no session creation, no model binding
        // resolution. A stored kind this build does not support fails the run outright:
        // degrading it to a chat task would execute the kind-specific stored prompt as
        // an unattended full-permission agent conversation, which is the unsafe
        // direction when the sidecar and the binary disagree.
        let (kind, message) = match self.dispatch_snapshot(&automation_id, task.prompt()).await {
            Ok(snapshot) => snapshot,
            Err(_) => return failed("scheduled task configuration unavailable before dispatch"),
        };
        match kind {
            ScheduledTaskKindLookup::MemoryOrganize => {
                return self.execute_memory_organize(cancel).await;
            }
            ScheduledTaskKindLookup::SessionMessage { target_session } => {
                return self
                    .execute_session_message(&target_session, &message, cancel)
                    .await;
            }
            ScheduledTaskKindLookup::Unsupported(raw) => {
                // Round-21 复审:kind 字段是 app-owned-but-not-trusted 的
                // sidecar 状态(上方声明两处),Debug 展开的原始字节此前
                // 未经折叠/扫描直达 Web 可见的 run record——本文件错误构造
                // 的统一纪律(先折叠后扫描)在此同样适用;短 kind 经两
                // 层 no-op 后仍原样可读(≥24 字符的字母数字族仍会被
                // is_secret_like 打码,这是扫描的既定语义而非本臂的回归)。
                let detail = format!(
                    "unsupported scheduled task kind {raw:?}; update the app or recreate the task"
                );
                return TaskExecutionResult {
                    status: TaskStatus::Failed,
                    result_text: None,
                    error: Some(fold_then_redact(&detail)),
                    terminal_reason: TaskTerminalReason::Failed,
                };
            }
            ScheduledTaskKindLookup::Chat => {}
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
        let _ = events
            .send(TaskExecutionEvent::ThreadCreated {
                thread_id: session_id.clone(),
            })
            .await;

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
                            .await
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
            // Round-20 复审 MAJOR-2:取消路径携带的引擎错误链同样落 run
            // record(list_scheduled_task_runs 出站),与 failed( 同一折叠
            // 口径。Round-21 复审:口径统一为先折叠后扫描(下方 failed(
            // 同批修正)——secret 先行会在含空白的家目录上让 ≥24 长度族
            // 先吃掉折叠 needle(记忆命令臂 S10-1 实证形态)。
            error: completion.error.map(|error| fold_then_redact(&error)),
            terminal_reason: TaskTerminalReason::Canceled,
        };
    }

    match completion.status {
        TurnOutcomeStatus::Completed => TaskExecutionResult {
            status: TaskStatus::Completed,
            result_text: Some("Scheduled conversation completed".to_string()),
            error: None,
            terminal_reason: TaskTerminalReason::Completed,
        },
        TurnOutcomeStatus::Failed => TaskExecutionResult {
            status: TaskStatus::Failed,
            result_text: None,
            error: Some(fold_then_redact(
                &completion
                    .error
                    .unwrap_or_else(|| "Scheduled conversation failed".to_string()),
            )),
            terminal_reason: TaskTerminalReason::Failed,
        },
        TurnOutcomeStatus::Interrupted => unreachable!("handled above"),
    }
}

/// Run-record 错误出站的统一纪律(round-21 复审 MAJOR-2 定序,round-22
/// 复审 m1/m2 收拢为单 helper):先折叠存储根,再对折叠后文本做 secret
/// 扫描——旧序(secret 先行)在含空白的家目录上会让 ≥24 长度族的空白
/// 分词吃掉折叠 needle(`...John [REDACTED]` 形态),盘符/用户名碎片
/// 明文落 Web 可见的 run record。顺序写死在本 helper 内,臂级重新内联
/// 旧序会被 census 钉拦下;空白 home 行为牙见
/// failed_folds_before_the_secret_scan_on_whitespace_homes。已折叠文本
/// 二次折叠幂等。organize 臂(format! 前缀拼接)与 secret-only 的
/// delivery 臂不走本 helper(各有独立源钉),census 钉死全文件拼写数。
fn fold_then_redact(raw: &str) -> String {
    crate::platform::credential_store::redact_secret(
        &crate::platform::paths::redact_storage_prefix(raw),
    )
}

fn failed(error: impl std::fmt::Display) -> TaskExecutionResult {
    TaskExecutionResult {
        status: TaskStatus::Failed,
        result_text: None,
        // Round-20 复审 MAJOR-2:create_scheduled_run 的最外层 anyhow
        // context 携带 `<home>/.../scheduled-runs/...` 绝对路径
        // (sessions/retention.rs),run_turn 错误链同类;run record 经
        // list_scheduled_task_runs 出站(Web 允许清单内),与上方 organize
        // 臂同一口径。Round-21 复审定序为先折叠后扫描,Round-22 复审
        // (m1/m2)把该纪律收拢进 fold_then_redact——臂级顺序漂移在
        // 结构上不可能;空白 home 行为牙与本臂的 helper 接线各有独立钉。
        error: Some(fold_then_redact(&error.to_string())),
        terminal_reason: TaskTerminalReason::Failed,
    }
}

fn canceled() -> TaskExecutionResult {
    TaskExecutionResult {
        status: TaskStatus::Canceled,
        result_text: None,
        error: None,
        terminal_reason: TaskTerminalReason::Canceled,
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
        NewTaskRequest, SharedTaskManager, TaskExecutionLimits, TaskManager, TaskManagerConfig,
        TaskRecord, TaskStatus,
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
        deliveries: Mutex<Vec<(String, String)>>,
        delivery_error: Mutex<Option<String>>,
        /// When set, the scripted delivery runs the SAME run-time target gate
        /// as the live runtime first (review round-3 M5: the stub must honor
        /// the gate so the executor-level tests pin the isolation check on
        /// the delivery path, not only as a free function).
        delivery_gate: Mutex<Option<SessionStore>>,
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
                deliveries: Mutex::new(Vec::new()),
                delivery_error: Mutex::new(None),
                delivery_gate: Mutex::new(None),
                started: Notify::new(),
            }
        }

        /// Arm the run-time target gate with a real store (the live runtime
        /// always gates; the stub only when armed).
        fn set_delivery_gate(&self, store: SessionStore) {
            *self.delivery_gate.lock().unwrap() = Some(store);
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

        fn set_delivery_error(&self, error: Option<String>) {
            *self.delivery_error.lock().unwrap() = error;
        }

        fn deliveries(&self) -> Vec<(String, String)> {
            self.deliveries.lock().unwrap().clone()
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

        async fn deliver_session_message(
            &self,
            target_session: &str,
            message: String,
            _cancel: CancellationToken,
        ) -> Result<()> {
            // Gate-honoring stub (review M5): same first line as the live
            // runtime — an isolated/hostile target never reaches the
            // recorded delivery.
            if let Some(store) = self.delivery_gate.lock().unwrap().as_ref() {
                ensure_deliverable_target(store, target_session)?;
            }
            self.deliveries
                .lock()
                .unwrap()
                .push((target_session.to_string(), message));
            match self.delivery_error.lock().unwrap().clone() {
                Some(error) => anyhow::bail!("{error}"),
                None => Ok(()),
            }
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
                // Simulate a long-running LLM call that never returns: notify the test that
                // organize has started, then trigger cancellation and hang forever. Before
                // the fix the executor waited here, so the 15s terminal-state wait when
                // deleting a running task almost certainly timed out.
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
            execution_limits: TaskExecutionLimits::default(),
            human_waits_answerable: false,
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
            model_provider: None,
            model_provider_id: None,
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

    /// A `session_message` run delivers the stored prompt into the target
    /// session and completes WITHOUT creating a run session: no profile, no
    /// run_turn call, and a thread-less run record (the delivery result names
    /// the target).
    #[tokio::test]
    async fn session_message_run_delivers_without_creating_a_session() -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([]));
        let executor = Arc::new(
            ScheduledChatExecutor::new(runtime.clone()).with_kind_resolver(Arc::new(|_| {
                ScheduledTaskKindLookup::SessionMessage {
                    target_session: "target001".to_string(),
                }
            })),
        );
        let (root, manager) = manager_with_executor(executor).await?;

        let queued = manager.add_task(request("每天早上问一下进展")).await?;
        let finished =
            wait_for_terminal_state(&manager, &queued.id, std::time::Duration::from_secs(5))
                .await?;

        assert_eq!(finished.status, TaskStatus::Completed);
        assert_eq!(
            finished.thread_id, None,
            "a delivery run owns no conversation"
        );
        assert_eq!(
            runtime.deliveries(),
            vec![("target001".to_string(), "每天早上问一下进展".to_string())]
        );
        assert!(
            runtime.profiles().is_empty() && runtime.calls().is_empty(),
            "no run session may be created"
        );
        manager.shutdown();
        drop(root);
        Ok(())
    }

    /// A delivery failure (wedge / gone target) fails the run with the
    /// sanitized reason instead of completing silently.
    #[tokio::test]
    async fn session_message_delivery_failure_fails_the_run() -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([]));
        runtime.set_delivery_error(Some("delivery timed out (engine wedged?)".to_string()));
        let executor = Arc::new(
            ScheduledChatExecutor::new(runtime.clone()).with_kind_resolver(Arc::new(|_| {
                ScheduledTaskKindLookup::SessionMessage {
                    target_session: "target001".to_string(),
                }
            })),
        );
        let (root, manager) = manager_with_executor(executor).await?;

        let queued = manager.add_task(request("问一下")).await?;
        let finished =
            wait_for_terminal_state(&manager, &queued.id, std::time::Duration::from_secs(5))
                .await?;

        assert_eq!(finished.status, TaskStatus::Failed);
        assert!(
            finished
                .error
                .as_deref()
                .is_some_and(|error| error.contains("scheduled message delivery")),
            "the failure names the delivery stage: {:?}",
            finished.error
        );
        manager.shutdown();
        drop(root);
        Ok(())
    }

    /// Review round-3 M5: the per-delivery isolation gate must sit on the
    /// delivery path an executor run actually takes. The scripted runtime is
    /// armed with a real store and gates exactly like the live impl — an
    /// isolated target FAILS the run and nothing is delivered. Removing the
    /// gate call (from the double or the live impl, see the source pin
    /// below) turns this red.
    /// Round-4 M5: the oversize body belt is pinned at run level — a stored
    /// oversized prompt (hand-edited store / older record, the exact
    /// scenario the belt exists for) fails the run without any delivery.
    /// The belt's EMPTY arm has no run-level drive (the foundation queue
    /// refuses empty prompts before the executor sees one) — it is driven
    /// directly on the executor by
    /// `session_message_empty_body_belt_fails_without_delivery` below.
    #[tokio::test]
    async fn session_message_run_fails_empty_and_oversized_bodies() -> Result<()> {
        // The empty arm cannot be driven through task creation (creation
        // rejects empty prompts); it stays belt-only for hand-edited
        // stores. The oversize arm IS creation-passable and pins the belt.
        for (label, body) in [("oversize", &"x".repeat(SCHEDULED_MESSAGE_MAX_CHARS + 1))] {
            let runtime = Arc::new(ScriptedRuntime::new([]));
            let executor = Arc::new(
                ScheduledChatExecutor::new(runtime.clone()).with_kind_resolver(Arc::new(|_| {
                    ScheduledTaskKindLookup::SessionMessage {
                        target_session: "target001".to_string(),
                    }
                })),
            );
            let (root, manager) = manager_with_executor(executor).await?;
            let queued = manager.add_task(request(body)).await?;
            let finished =
                wait_for_terminal_state(&manager, &queued.id, std::time::Duration::from_secs(5))
                    .await?;
            assert_eq!(
                finished.status,
                TaskStatus::Failed,
                "{label} body must fail"
            );
            assert!(
                finished
                    .error
                    .as_deref()
                    // Round-6 nit: aligned with the domain cap message —
                    // both Rust sites say "prompt exceeds the …".
                    .is_some_and(|error| error.contains("prompt exceeds the")),
                "{label}: the failure names the body gate: {:?}",
                finished.error
            );
            assert!(
                runtime.deliveries().is_empty(),
                "{label}: nothing may be delivered"
            );
            manager.shutdown();
            drop(root);
        }
        Ok(())
    }

    /// Review round-15 N10: the belt's EMPTY arm, driven directly on the
    /// executor — the foundation queue refuses empty prompts, so the
    /// run-level test above can only carry the oversize arm, while the
    /// belt must still catch a blank body from a hand-edited store /
    /// older record. Deleting the empty arm turns this red.
    #[tokio::test]
    async fn session_message_empty_body_belt_fails_without_delivery() {
        for body in ["", "   "] {
            let runtime = Arc::new(ScriptedRuntime::new([]));
            let executor = ScheduledChatExecutor::new(runtime.clone());
            let result = executor
                .execute_session_message("target001", body, CancellationToken::new())
                .await;
            assert_eq!(
                result.status,
                TaskStatus::Failed,
                "{body:?} body must fail at the belt"
            );
            assert!(
                result
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("body is empty")),
                "{body:?}: the failure names the empty belt: {:?}",
                result.error
            );
            assert!(
                runtime.deliveries().is_empty(),
                "{body:?}: nothing may be delivered"
            );
        }
    }

    #[tokio::test]
    async fn session_message_run_gates_isolated_targets_before_delivery() -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([]));
        let gate_root = TestRoot::new()?;
        runtime.set_delivery_gate(SessionStore::boot_at_test_dir(&gate_root.0)?);
        let executor = Arc::new(
            ScheduledChatExecutor::new(runtime.clone()).with_kind_resolver(Arc::new(|_| {
                ScheduledTaskKindLookup::SessionMessage {
                    target_session: "sched-run1".to_string(),
                }
            })),
        );
        let (root, manager) = manager_with_executor(executor).await?;

        let queued = manager.add_task(request("定时投递")).await?;
        let finished =
            wait_for_terminal_state(&manager, &queued.id, std::time::Duration::from_secs(5))
                .await?;

        assert_eq!(finished.status, TaskStatus::Failed);
        assert!(
            finished
                .error
                .as_deref()
                .is_some_and(|error| error.contains("isolated")),
            "the failure must name the isolation gate: {:?}",
            finished.error
        );
        assert!(
            runtime.deliveries().is_empty(),
            "an isolated target must never reach a delivery"
        );
        manager.shutdown();
        drop(root);
        drop(gate_root);
        Ok(())
    }

    /// Review round-3 M5, source pin on the LIVE delivery body: gate before
    /// steer, dispatch fallback after steer failure, all under the delivery
    /// timeout — deleting or reordering any leg of
    /// `EngineScheduledRuntime::deliver_session_message` turns this red
    /// (behavioral coverage of the stub lives in the tests above; the 30s
    /// wall-clock timeout leg stays unpinned by design).
    #[test]
    fn live_delivery_body_shape_is_pinned() {
        let source = include_str!("executor.rs");
        let body = source
            .split("impl ScheduledConversationRuntime for EngineScheduledRuntime")
            .nth(1)
            .expect("live runtime impl present");
        let gate = body
            .find("ensure_deliverable_target(&self.store, target_session)?;")
            .expect("the live delivery must gate the target first");
        let steer = body
            .find("self.pool.steer(target_session, message.clone())")
            .expect("steer-first delivery");
        // Round-34 MAJOR 4: no or_else fallback — the loose needle matched
        // this test's own string literals (3 hits in-file), so the pin
        // survived flipping the production argument to true. The exact
        // production-format needle only matches the runtime impl.
        let dispatch = body
            .find("self.pool\n                        .deliver_messaging_turn(target_session, message, false)")
            .expect("dispatch fallback after steer failure");
        let timeout = body
            .find("SCHEDULED_MESSAGE_DELIVERY_TIMEOUT, async")
            .expect("the delivery bound wraps the whole body");
        assert!(gate < steer, "gate precedes the steer");
        assert!(steer < dispatch, "steer precedes the dispatch fallback");
        assert!(
            gate < timeout && timeout < steer,
            "textual order is gate → timeout wrapper → steer/dispatch body"
        );
    }

    /// Review round-15 N10: the delivery-failure reason passes through
    /// `redact_secret` before it reaches the run record (credential /
    /// endpoint-probing detail; Round-20 复审订正:旧括注"app error chains
    /// carry store paths"与本臂自身注释的实勘结论相反——该链今天无已证实
    /// 的绝对路径载体,折叠存储根不在本臂纪律内) — deleting the call kept
    /// every behavioral test green, so the call site is pinned textually,
    /// the same way as the live body above.
    /// Review round-15 N10, rewritten in round-17 M5: the delivery-failure
    /// reason passes through `redact_secret` before it reaches the run
    /// record. The textual pin needs TWO bounds to be mutation-sensitive:
    /// `include_str!` includes the WHOLE file — the test module's own
    /// source at the bottom carries every one of these literal patterns
    /// (the old unbounded slice self-satisfied through its own find line,
    /// verified by deleting the arm's call), and the plain `.nth(1)`
    /// additionally spanned the memory_organize arm below, whose
    /// byte-identical redact call self-satisfied the same way. Bound to
    /// the production half first, then to this arm.
    #[test]
    fn delivery_failure_reason_is_redacted_in_source() {
        let source = include_str!("executor.rs");
        let production = source
            .split("\n#[cfg(test)]\nmod tests {")
            .next()
            .expect("the production half present");
        let body = production
            .split("async fn execute_session_message")
            .nth(1)
            .expect("the session-message run body present")
            .split("async fn execute_memory_organize")
            .next()
            .expect("the session-message arm is bounded by the organize arm");
        let failure_arm = body
            .find("Some(Err(error))")
            .expect("the failure arm present");
        let redact = body
            .find("redact_secret(&format!(\"{error:#}\"))")
            .expect("the failure reason must pass through redact_secret");
        assert!(
            failure_arm < redact,
            "the redaction sits inside the delivery-failure arm"
        );
    }

    /// Round-18 复审:memory_organize 的失败原因此前只过 `redact_secret`
    /// (其注释还声称与命令层同一处理——命令层自 round-15 起额外折叠存储
    /// 根,注释失实)。organize 的错误链携带 bounded-read 原语的 io
    /// Display(path.display() 的绝对存储路径),run record 经
    /// `list_scheduled_task_runs` 出站(Web 允许清单内)。本钉与
    /// `memory_organize_failure_folds_the_storage_root_in_the_record`
    /// (Round-19 复审补的行为钉,同一文件内即可构造失败 organize)
    /// 双重兜底:源码钉锚生产半边与臂内 redact 顺序,删任一层 redact
    /// 即红;行为钉走真实执行路径断言 record 内容。Round-21 复审:顺序
    /// 断言翻转为 fold 先行(secret 外层、fold 内层)——round-20 版本
    /// 钉住的 arm<fold<secret 恰是 S10-1 实证过的漏洞序,本钉此前在
    /// 保护错误方向;空白 home 行为钉
    /// (`memory_organize_failure_folds_before_the_secret_scan_on_whitespace_homes`)
    /// 给顺序补上行为级红。
    #[test]
    fn memory_organize_failure_reason_folds_the_storage_prefix() {
        let source = include_str!("executor.rs");
        let production = source
            .split("\n#[cfg(test)]\nmod tests {")
            .next()
            .expect("the production half present");
        let body = production
            .split("async fn execute_memory_organize")
            .nth(1)
            .expect("the memory-organize run body present")
            .split("\n    async fn ")
            .next()
            .expect("the memory-organize arm must be bounded by the next item");
        let arm = body
            .find("Err(error) => TaskExecutionResult")
            .expect("the failure arm present");
        let secret = body
            .find("redact_secret(&format!(")
            .expect("the failure reason must still pass through redact_secret");
        let fold = body
            .find("crate::platform::paths::redact_storage_prefix(&format!(\"{error:#}\"))")
            .expect(
                "the failure reason must fold the storage root via redact_storage_prefix; \
                 io error Displays carry the absolute path",
            );
        assert!(
            arm < secret && secret < fold,
            "the fold must run BEFORE the secret scan (outer redact_secret over \
             inner redact_storage_prefix): scan-then-fold lets the >=24-char \
             secret family destroy the fold needle on whitespace-containing \
             homes and leak a drive+username fragment (the S10-1 shape)"
        );
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
                    model_provider: None,
                    model_provider_id: None,
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
                    model_provider: None,
                    model_provider_id: None,
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
            ScheduledChatExecutor::new(runtime.clone())
                .with_kind_resolver(Arc::new(|_| ScheduledTaskKindLookup::MemoryOrganize)),
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
        // Organize runs create no session and run no engine turn: thread/turn stay unset.
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
    async fn canceled_session_message_wins_over_invalid_body_checks() {
        let runtime = Arc::new(ScriptedRuntime::new([]));
        let executor = ScheduledChatExecutor::new(runtime.clone());
        for message in [String::new(), "x".repeat(SCHEDULED_MESSAGE_MAX_CHARS + 1)] {
            let cancel = CancellationToken::new();
            cancel.cancel();
            let result = executor
                .execute_session_message("target001", &message, cancel)
                .await;
            assert_eq!(result.status, TaskStatus::Canceled);
        }
        assert!(runtime.deliveries().is_empty());
    }

    #[tokio::test]
    async fn memory_organize_failure_maps_to_a_failed_run() -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([]));
        runtime.set_organize_error(Some(
            "model unavailable sk-test-redaction-fixture".to_string(),
        ));
        let executor = Arc::new(
            ScheduledChatExecutor::new(runtime.clone())
                .with_kind_resolver(Arc::new(|_| ScheduledTaskKindLookup::MemoryOrganize)),
        );
        let (_root, manager) = manager_with_executor(executor).await?;

        let queued = manager.add_task(request("organize memory")).await?;
        let finished =
            wait_for_terminal_state(&manager, &queued.id, std::time::Duration::from_secs(5))
                .await?;

        assert_eq!(finished.status, TaskStatus::Failed);
        assert_eq!(
            finished.error.as_deref(),
            Some("memory organize: model unavailable [REDACTED]")
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
    async fn memory_organize_failure_folds_the_storage_root_in_the_record() -> Result<()> {
        // Round-19 复审 M2:行为钉。round-18 源码钉注释曾声称「无 AppHandle
        // 无法行为构造失败 organize」——不成立,上一测试的
        // set_organize_error 就是同一构造。本测试注入携带存储路径的失败,
        // 断言 run record(经 list_scheduled_task_runs 出站的 Web 可见面)
        // 落的是折叠后的 ~/.pinvou3 形态而非原始路径。fixture 用短 home:
        // 内层 redact_secret 的 ≥24 长度族会把长路径先打成 [REDACTED],
        // 折叠层无从观察;短路径(如真实短安装/短用户名)正是折叠层独力
        // 兜底的形态。ENV_LOCK 保证 worker 线程读到与本测试构造一致的
        // PINVOU3_HOME(paths 测试会在同进程并行改写该变量)。
        let (_env_guard, _env_restore) =
            crate::platform::test_support::locked_env(&["PINVOU3_HOME"]);
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes
        // serialized in-process (platform::test_support discipline).
        unsafe { std::env::set_var("PINVOU3_HOME", "p8") };
        let runtime = Arc::new(ScriptedRuntime::new([]));
        runtime.set_organize_error(Some(format!(
            "snapshot load failed: {}",
            crate::platform::paths::pinvou3_home().join("p").display()
        )));
        let executor = Arc::new(
            ScheduledChatExecutor::new(runtime.clone())
                .with_kind_resolver(Arc::new(|_| ScheduledTaskKindLookup::MemoryOrganize)),
        );
        let (_root, manager) = manager_with_executor(executor).await?;

        let queued = manager.add_task(request("organize memory")).await?;
        let finished =
            wait_for_terminal_state(&manager, &queued.id, std::time::Duration::from_secs(5))
                .await?;

        assert_eq!(finished.status, TaskStatus::Failed);
        let error = finished
            .error
            .as_deref()
            .expect("failed run carries the error");
        assert!(
            error.contains("memory organize: snapshot load failed: ~/.pinvou3/p"),
            "the run record must carry the folded storage root, got: {error}"
        );
        assert!(
            !error.contains("p8/p"),
            "the run record must not carry the raw home prefix, got: {error}"
        );
        assert!(
            !error.contains("[REDACTED]"),
            "the path token must survive the inner secret redaction for the \
             fold to be the layer under test, got: {error}"
        );
        manager.shutdown();
        Ok(())
    }

    #[tokio::test]
    async fn memory_organize_failure_folds_before_the_secret_scan_on_whitespace_homes() -> Result<()>
    {
        // Round-21 复审(S10-1 补齐)行为钉:含空白的家目录上,secret 先行
        // 的旧序会让 ≥24 长度族的空白分词吃掉折叠 needle(`...John
        // [REDACTED]` 形态),盘符/用户名碎片明文落 run record。生产臂
        // (round-21 起与命令层同序)先折叠后扫描:折叠形态的短 token 存活
        // 为 ~/.pinvou3 形态,长 token 整体 [REDACTED]——两个方向都不残留
        // 家目录碎片。对照腿用旧序手工复现泄漏形状(空白前段短于 ≥24 族时
        // 可构造;临时目录本身超长的平台(macOS /var/folders…)跳过对照腿,
        // 且那里空白前段自身已入 ≥24 族,两序产出字节不同(旧序两个
        // [REDACTED]、新序折叠后一个)但均无碎片残留——生产不变式腿
        // (!contains 碎片)在该平台上对顺序回归无信号,顺序牙齿依赖
        // Linux CI 的短 /tmp(round-27 复审 m11 订正「两序产出相同」);
        // round-23 复审口径注记)。
        // 把顺序改回 secret 先行即红。ENV_LOCK 保证 worker 线程读到与本测试
        // 构造一致的 PINVOU3_HOME。
        let base = std::env::temp_dir().join(format!("jd{}", std::process::id()));
        let home = base.join("John Doe").join(".pinvou3");
        std::fs::create_dir_all(&home).unwrap();
        let (_env_guard, _env_restore) =
            crate::platform::test_support::locked_env(&["PINVOU3_HOME"]);
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes
        // serialized in-process (platform::test_support discipline).
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };
        // 路径一律取 pinvou3_home() 的读数(macOS /tmp 符号链接等规范化差
        // 异),保证折叠 needle 与注入的失败 detail 同源。尾段补足到 ≥24
        // 含数字+字母,让旧序的 ≥24 族必定命中(泄漏形状与临时目录长度
        // 无关)。
        let home_now = crate::platform::paths::pinvou3_home();
        let detail = format!(
            "snapshot load failed: {}/sessions2/x.json12345678901234567890123456789",
            home_now.display()
        );
        let runtime = Arc::new(ScriptedRuntime::new([]));
        runtime.set_organize_error(Some(detail));
        let executor = Arc::new(
            ScheduledChatExecutor::new(runtime.clone())
                .with_kind_resolver(Arc::new(|_| ScheduledTaskKindLookup::MemoryOrganize)),
        );
        let (_root, manager) = manager_with_executor(executor).await?;

        let queued = manager.add_task(request("organize memory")).await?;
        let finished =
            wait_for_terminal_state(&manager, &queued.id, std::time::Duration::from_secs(5))
                .await?;

        assert_eq!(finished.status, TaskStatus::Failed);
        let error = finished
            .error
            .as_deref()
            .expect("failed run carries the error");
        assert!(
            !error.contains("John") && !error.contains("Doe"),
            "fold-first must not leak any home fragment onto the web-visible \
             run record, got: {error}"
        );
        assert!(
            error.contains("memory organize: "),
            "the failure arm must still format the organize context, got: {error}"
        );

        // 对照腿:旧序(secret 先行)确实产生碎片泄漏——本测试钉的就是该
        // 形态不再出现在生产臂。空白前段(temp/jd<pid>/John)短于 ≥24 族
        // 时形状可复现;平台上临时目录本身超长时跳过(macOS /var/folders…),
        // 与 memory.rs 同名钉的既有口径一致。
        let pre_space = format!("{}/John", base.display());
        if pre_space.len() < 24 {
            let leaked = crate::platform::paths::redact_storage_prefix(
                &crate::platform::credential_store::redact_secret(&format!(
                    "memory organize: snapshot load failed: {}/sessions2/x.json12345678901234567890123456789",
                    home_now.display()
                )),
            );
            assert!(
                leaked.contains("John"),
                "control: the reversed order must reproduce the fragment-leak \
                 shape this pin forbids, got: {leaked}"
            );
        }

        // 短尾段腿:折叠形态完整出站(折叠层在扫描前已把家目录替换干净)。
        let plain_detail = format!(
            "snapshot load failed: {}/mem/x.json: boom",
            home_now.display()
        );
        let plain_runtime = Arc::new(ScriptedRuntime::new([]));
        plain_runtime.set_organize_error(Some(plain_detail));
        let plain_executor = Arc::new(
            ScheduledChatExecutor::new(plain_runtime.clone())
                .with_kind_resolver(Arc::new(|_| ScheduledTaskKindLookup::MemoryOrganize)),
        );
        let (_plain_root, plain_manager) = manager_with_executor(plain_executor).await?;
        let plain_queued = plain_manager.add_task(request("organize memory")).await?;
        let plain_finished = wait_for_terminal_state(
            &plain_manager,
            &plain_queued.id,
            std::time::Duration::from_secs(5),
        )
        .await?;
        let plain_error = plain_finished
            .error
            .as_deref()
            .expect("failed run carries the error");
        assert!(
            plain_error.contains("memory organize: snapshot load failed: ~/.pinvou3/mem/x.json"),
            "the short folded tail must survive the secret scan intact, got: {plain_error}"
        );
        plain_manager.shutdown();
        manager.shutdown();
        Ok(())
    }

    /// Round-22 复审(m1/m2)census 钉:四个 run-record 错误臂统一经
    /// fold_then_redact 折叠(secret-only 的 delivery 臂与自带 format!
    /// 前缀的 organize 臂除外)。全文件生产半边的两个原语拼写数钉死:
    /// 新错误臂若内联旧序或绕开 helper,census 计数即变红,作者必须
    /// 显式并入 helper(census 盲区:改写拼写——如 use 引入短名——不
    /// 计数;组织形态改变时由编译器与行为钉兜底)。
    #[test]
    fn run_record_fold_census_routes_through_the_helper() {
        let source = include_str!("executor.rs");
        let production = source
            .split("\n#[cfg(test)]")
            .next()
            .expect("executor.rs must keep its tests behind #[cfg(test)]");
        // Round-27 复审 m4:纯注释行先滤除——生产半边的注释提及(措辞
        // 巧合避开了被计拼写)此前能垫高/掩饰计数,与 tasks.rs 钉的过滤
        // 口径对齐。
        let production = crate::platform::test_support::comment_filtered(production);
        assert_eq!(
            production
                .matches("crate::platform::credential_store::redact_secret(")
                .count(),
            3,
            "exactly the helper, the organize arm and the secret-only delivery \
             arm may scan; new error arms must route through fold_then_redact"
        );
        assert_eq!(
            production
                .matches("crate::platform::paths::redact_storage_prefix(")
                .count(),
            2,
            "exactly the helper and the organize arm may fold; new error arms \
             must route through fold_then_redact"
        );
        // Round-25 复审 MINOR:原语拼写 census 之上补 helper 调用点
        // census——unsupported-kind 臂的 fold_then_redact 调用此前对两个
        // 原语拼写 census 均不可见(删掉该调用、退回裸 detail 时全套件
        // 保持绿,违背本文件「错误构造一律经 helper」的自钉标准)。钉死
        // 生产半边的引用总数(4 个错误臂 + helper 定义自身):删除调用
        // 或新增绕行臂即红。
        assert_eq!(
            production.matches("fold_then_redact(").count(),
            5,
            "exactly the four error arms (organize, canceled completion, \
             failed completion, unsupported-kind) plus the helper definition \
             may reference fold_then_redact; deleting a call site changes \
             this census. Disclosed blindness: a NEW raw arm emitting \
             neither primitive nor the helper stays invisible here — the \
             per-arm behavioral pins are the second line of defense"
        );
    }

    /// Round-22 复审(m1)行为钉:failed() 出口的空白 home 顺序牙——
    /// 此前只有 organize 臂有(本臂单元钉的短 home 下两序字节相同,顺序
    /// 回归无信号,突变实证 323 全绿)。直接驱动 failed():生产 helper
    /// (先折叠后扫描)两个方向都不残留家目录碎片;对照腿用旧序手工
    /// 复现泄漏形状,临时目录超长的平台跳过对照腿(与 organize 钉同
    /// 口径)。改回旧序或绕开 helper 内联均红。
    #[test]
    fn failed_folds_before_the_secret_scan_on_whitespace_homes() {
        let base = std::env::temp_dir().join(format!("jd{}", std::process::id()));
        let home = base.join("John Doe").join(".pinvou3");
        std::fs::create_dir_all(&home).unwrap();
        let (_env_guard, _env_restore) =
            crate::platform::test_support::locked_env(&["PINVOU3_HOME"]);
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes
        // serialized in-process (platform::test_support discipline).
        unsafe { std::env::set_var("PINVOU3_HOME", &home) };
        let home_now = crate::platform::paths::pinvou3_home();
        let detail = format!(
            "create_scheduled_run: {}/scheduled-runs/r.json12345678901234567890123456789",
            home_now.display()
        );
        let error = failed(&detail).error.expect("failed() carries the error");
        assert!(
            !error.contains("John") && !error.contains("Doe"),
            "fold-first must not leak any home fragment onto the web-visible \
             run record, got: {error}"
        );

        // 对照腿:旧序(secret 先行)确实产生碎片泄漏(空白前段短于
        // ≥24 族时可构造;临时目录超长的平台造不出,跳过)。
        let pre_space = format!("{}/John", base.display());
        if pre_space.len() < 24 {
            let leaked = crate::platform::paths::redact_storage_prefix(
                &crate::platform::credential_store::redact_secret(&detail),
            );
            assert!(
                leaked.contains("John"),
                "control: the reversed order must reproduce the fragment-leak \
                 shape this pin forbids, got: {leaked}"
            );
        }

        // 短尾段腿:折叠形态完整出站(折叠层在扫描前已替换干净)。
        let plain = failed(format!(
            "create_scheduled_run: {}/mem/x.json: boom",
            home_now.display()
        ))
        .error
        .expect("failed() carries the error");
        assert!(
            plain.contains("~/.pinvou3/mem/x.json"),
            "the short folded tail must survive the secret scan intact, got: {plain}"
        );
    }

    #[test]
    fn failed_reason_folds_the_storage_root_like_the_organize_arm() {
        // Round-20 复审 MAJOR-2 行为钉(单元半):failed( 是
        // create_session(:505)与 run_turn(:545)错误臂的共同出口——
        // create_scheduled_run 的最外层 anyhow context 携带绝对存储路径,
        // 此前该出口无任何折叠。删除 failed( 内的折叠本断言即红。
        // (Round-21 复审修正:round-20 注释声称「内外层序对调本断言即红」
        // 不成立——短 home 下两层 token 都短于 ≥24 族,对调是字节级
        // no-op。Round-22 复审(m1):顺序牙由 fold_then_redact helper
        // census 钉与本文件
        // failed_folds_before_the_secret_scan_on_whitespace_homes 行为钉
        // 提供,本钉只负责折叠层存在性。)
        let (_env_guard, _env_restore) =
            crate::platform::test_support::locked_env(&["PINVOU3_HOME"]);
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes
        // serialized in-process (platform::test_support discipline).
        unsafe { std::env::set_var("PINVOU3_HOME", "p8") };
        // 路径 token 保持短于内层 redact_secret 的 ≥24 长度族,让折叠层
        // 独力可观察(与上方 organize 行为钉同一纪律)。
        let result = failed(format!(
            "create scheduled task workspace {}",
            crate::platform::paths::pinvou3_home()
                .join("sr/w")
                .display()
        ));
        let error = result.error.expect("failed() must carry the error");
        assert!(
            error.contains("create scheduled task workspace ~/.pinvou3/sr/w"),
            "the failed() exit must fold the storage root before the secret scan, got: {error}"
        );
        assert!(
            !error.contains("p8/sr"),
            "the failed() exit must not retain the raw home prefix, got: {error}"
        );
    }

    #[test]
    fn completion_error_arms_fold_the_storage_root() {
        // Round-20 复审 MAJOR-2 行为钉(单元半):map_completion 的
        // Failed 臂与 Canceled 臂携带引擎错误链,同样落 run record——
        // 删除任一臂的折叠本测试即红。
        let (_env_guard, _env_restore) =
            crate::platform::test_support::locked_env(&["PINVOU3_HOME"]);
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes
        // serialized in-process (platform::test_support discipline).
        unsafe { std::env::set_var("PINVOU3_HOME", "p8") };
        let raw = format!(
            "turn failed at {}",
            crate::platform::paths::pinvou3_home()
                .join("sr/w")
                .display()
        );
        let failed_result = map_completion(completion(
            "turn_1".to_string(),
            TurnOutcomeStatus::Failed,
            Some(raw.clone()),
            false,
        ));
        let failed_error = failed_result
            .error
            .expect("failed completion carries the error");
        assert!(
            failed_error.contains("turn failed at ~/.pinvou3/sr/w"),
            "the completion Failed arm must fold the storage root, got: {failed_error}"
        );
        assert!(
            !failed_error.contains("p8/sr"),
            "the completion Failed arm must not retain the raw home prefix, got: {failed_error}"
        );
        let canceled_result = map_completion(completion(
            "turn_2".to_string(),
            TurnOutcomeStatus::Completed,
            Some(raw),
            true,
        ));
        assert_eq!(canceled_result.status, TaskStatus::Canceled);
        let canceled_error = canceled_result
            .error
            .expect("canceled completion carries the error");
        assert!(
            canceled_error.contains("~/.pinvou3/sr/w"),
            "the completion Canceled arm must fold the storage root, got: {canceled_error}"
        );
        assert!(
            !canceled_error.contains("p8/sr"),
            "the completion Canceled arm must not retain the raw home prefix, got: {canceled_error}"
        );
    }

    #[tokio::test]
    async fn unsupported_kind_fails_the_run_without_a_session_or_engine_turn() -> Result<()> {
        // A kind entry this build does not support must fail the run instead of
        // degrading to a chat task: the stored prompt was authored for its kind,
        // and running it as an unattended full-permission agent conversation is
        // the unsafe direction.
        let runtime = Arc::new(ScriptedRuntime::new([]));
        let executor = Arc::new(
            ScheduledChatExecutor::new(runtime.clone()).with_kind_resolver(Arc::new(|_| {
                ScheduledTaskKindLookup::Unsupported("legacy_kind".to_string())
            })),
        );
        let (_root, manager) = manager_with_executor(executor).await?;

        let queued = manager.add_task(request("organize memory")).await?;
        let finished =
            wait_for_terminal_state(&manager, &queued.id, std::time::Duration::from_secs(5))
                .await?;

        assert_eq!(finished.status, TaskStatus::Failed);
        let error = finished.error.as_deref().unwrap_or_default();
        assert!(
            error.contains("unsupported scheduled task kind"),
            "error must name the unsupported kind, got: {error}"
        );
        assert!(
            error.contains("legacy_kind"),
            "error must carry the raw kind value"
        );
        assert_eq!(finished.thread_id, None, "must not create a session");
        assert_eq!(finished.turn_id, None);
        assert!(
            runtime.profiles().is_empty(),
            "unsupported kind must not create a session"
        );
        assert!(
            runtime.calls().is_empty(),
            "unsupported kind must not run an engine turn"
        );
        assert_eq!(
            runtime.organize_call_count(),
            0,
            "unsupported kind must not reach the organize routine"
        );
        manager.shutdown();
        Ok(())
    }

    #[tokio::test]
    async fn memory_organize_run_cancels_without_waiting_for_the_llm_call() -> Result<()> {
        let runtime = Arc::new(ScriptedRuntime::new([]));
        runtime.set_organize_hang_after_cancel(true);
        let executor = Arc::new(
            ScheduledChatExecutor::new(runtime.clone())
                .with_kind_resolver(Arc::new(|_| ScheduledTaskKindLookup::MemoryOrganize)),
        );
        let (_root, manager) = manager_with_executor(executor).await?;

        let queued = manager.add_task(request("organize memory")).await?;
        // Wait for the runtime to enter the never-returning organize call, then cancel:
        // before the fix the executor kept waiting on the 75s organize call itself, so the
        // 15s terminal-state wait when deleting a running task always timed out.
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

    /// Round-5 M-E (Rust side): the ACP/code gate is pinned store-backed —
    /// a session-agents.json carrying an ACP-backend record and a code-mode
    /// record drives both production rejections (mirroring messaging's
    /// real_gate_inputs test).
    #[test]
    fn deliverable_target_rejects_acp_and_code_records() {
        crate::platform::test_support::with_temp_home("sched-acp-gate", || {
            let sidecar = crate::platform::paths::pinvou3_home().join("session-agents.json");
            std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
            // AgentStoreFile envelope: {version, sessions: {id: record}};
            // the mode field serializes as the legacy `code_session` bool.
            std::fs::write(
                &sidecar,
                serde_json::json!({
                    "version": 1,
                    "sessions": {
                        "acp-tgt": {"backend": "claude-acp"},
                        "code-tgt": {"backend": "deepseek", "code_session": true},
                    }
                })
                .to_string(),
            )
            .unwrap();
            let store =
                crate::features::sessions::SessionStore::boot_for_process_startup().expect("store");
            let err = ensure_deliverable_target(&store, "acp-tgt").unwrap_err();
            assert!(err.to_string().contains("ACP/code"), "{err}");
            let err = ensure_deliverable_target(&store, "code-tgt").unwrap_err();
            assert!(err.to_string().contains("ACP/code"), "{err}");
            std::fs::write(store.manager.sessions_dir().join("chat-tgt.json"), "{}").unwrap();
            assert!(ensure_deliverable_target(&store, "chat-tgt").is_ok());
            std::fs::write(&sidecar, "{broken").unwrap();
            assert!(
                ensure_deliverable_target(&store, "chat-tgt")
                    .unwrap_err()
                    .to_string()
                    .contains("index unavailable")
            );
            std::fs::write(&sidecar, r#"{"version":999,"sessions":{}}"#).unwrap();
            assert!(ensure_deliverable_target(&store, "chat-tgt").is_err());
        });
    }

    /// Round-6 M-2: this function existed since round 5 with NO test
    /// attribute — dead code that every gate tolerated (clippy --tests
    /// does not flag it) while suggesting the executor's case-variant and
    /// hostile-charset arms were pinned. The attribute is the fix.
    #[test]
    fn delivery_target_gate_rejects_isolated_and_hostile_ids() -> Result<()> {
        let root = TestRoot::new()?;
        let store = SessionStore::boot_at_test_dir(&root.0)?;
        // A plain session id passes the gate (no profile registered).
        std::fs::write(store.manager.sessions_dir().join("tgt0001.json"), "{}")?;
        assert!(ensure_deliverable_target(&store, "tgt0001").is_ok());
        // Isolated prefixes — case variants included — are rejected before
        // any engine is woken (the recursion direction; same discipline as
        // the messaging channel's check_participant_id).
        for hostile in [
            "sched-run1",
            "SCHED-run1",
            "aux-side1",
            "AUX-side1",
            "eval_b1",
            "EVAL_b1",
        ] {
            assert!(
                ensure_deliverable_target(&store, hostile).is_err(),
                "{hostile} must be rejected"
            );
        }
        // Hostile charset/padding poisons at the gate instead of failing
        // somewhere inside the delivery path.
        for hostile in ["", "  ", "../escape", "with space"] {
            assert!(
                ensure_deliverable_target(&store, hostile).is_err(),
                "{hostile:?} must be rejected"
            );
        }
        Ok(())
    }
    async fn message_snapshot_fixture() -> Result<(
        TestRoot,
        SharedAutomationManager,
        ScheduledTaskKindStore,
        String,
    )> {
        let root = TestRoot::new()?;
        let automations = Arc::new(tokio::sync::Mutex::new(
            deepseek_tui::automation_manager::AutomationManager::open(root.0.join("automations"))?,
        ));
        let created = automations
            .lock()
            .await
            .create_automation(serde_json::from_value(serde_json::json!({
                "name":"snapshot", "prompt":"old body", "rrule":"FREQ=HOURLY", "status":"paused"
            }))?)?;
        let kinds = ScheduledTaskKindStore::open(root.0.join("task-kinds.json"))?;
        kinds.set_kind_with_target(&created.id, "session_message", "target001")?;
        Ok((root, automations, kinds, created.id))
    }

    #[tokio::test]
    async fn dispatch_waits_for_both_retarget_writes_before_reading_a_pair() -> Result<()> {
        let (_root, automations, kinds, id) = message_snapshot_fixture().await?;
        let runtime = Arc::new(ScriptedRuntime::new([]));
        let mut executor = ScheduledChatExecutor::new(runtime);
        executor.dispatch_store = Some((automations.clone(), kinds.clone()));
        let update = automations.lock().await;
        update.update_automation(
            &id,
            deepseek_tui::automation_manager::UpdateAutomationRequest {
                prompt: Some("new body".to_string()),
                ..Default::default()
            },
        )?;
        let dispatched =
            tokio::spawn(async move { executor.dispatch_snapshot(&id, "old queued body").await });
        tokio::task::yield_now().await;
        assert!(
            !dispatched.is_finished(),
            "dispatch must wait at the update's critical section"
        );
        kinds.set_kind_with_target(
            &update.list_automations()?[0].id,
            "session_message",
            "target002",
        )?;
        drop(update);
        let (kind, prompt) = dispatched.await??;
        assert_eq!(prompt, "new body");
        assert_eq!(
            kind,
            ScheduledTaskKindLookup::SessionMessage {
                target_session: "target002".to_string()
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn real_task_manager_delivery_result_round_trips_the_target_contract() -> Result<()> {
        let (_root, automations, kinds, id) = message_snapshot_fixture().await?;
        let runtime = Arc::new(ScriptedRuntime::new([]));
        let mut executor = ScheduledChatExecutor::new(runtime.clone());
        executor.dispatch_store = Some((automations.clone(), kinds));
        let (_task_root, manager) = manager_with_executor(Arc::new(executor)).await?;
        let run =
            deepseek_tui::automation_manager::run_now_shared(&automations, &id, &manager).await?;
        let finished = wait_for_terminal_state(
            &manager,
            run.task_id.as_deref().expect("linked task"),
            Duration::from_secs(5),
        )
        .await?;
        assert_eq!(finished.status, TaskStatus::Completed);
        assert_eq!(
            runtime.deliveries(),
            vec![("target001".to_string(), "old body".to_string())]
        );
        assert_eq!(
            super::super::message::delivered_target(
                finished.result_summary.as_deref().expect("result")
            ),
            Some("target001")
        );
        manager.shutdown();
        Ok(())
    }
}
