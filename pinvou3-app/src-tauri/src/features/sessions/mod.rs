//! 多对话管理 wrapper。
//!
//! 复用 deepseek-tui 上游 [`SessionManager`]（已支持 `new(custom_dir)`），
//! 把 sessions 目录定向到 `~/.pinvou3/sessions/`（隔离 `~/.deepseek/`）。
//!
//! 暴露给 pinvou3-app Tauri commands 的能力：
//! - `list` —— 列出所有会话元数据（前端历史面板）
//! - `create_new` —— 新建空会话（首次未发送消息前）
//! - `load` —— 读完整对话（切换 session 时给 engine 通过 `Op::SyncSession` 注入）
//! - `save` —— 持久化（每轮 turn 完成 auto-save）
//! - `delete` —— 删除会话 + artifacts 目录
//! - `set_title` —— 重命名
//! - `active_id` / `set_active` —— 跟踪当前 active session（chat command 用）
//!
//! **Arc + RwLock 包装**：所有字段都是 `Arc`，整个 `SessionStore` 可以
//! 廉价 Clone 进 Tauri State + 多个 task 共享。

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use deepseek_tui::artifacts::{ArtifactKind, ArtifactRecord};
use deepseek_tui::models::{Message, SystemPrompt};
use deepseek_tui::session_manager::{
    create_saved_session_with_id_and_mode, SavedSession, SessionManager, SessionMetadata,
};
use deepseek_tui::tui::app::AppMode;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::mode_state::{
    ActiveSkillBinding, MountedCollection, MountedCollectionsSnapshot, SerializableMode,
    SessionModeState,
};
use crate::platform::paths;
use crate::platform::prefs::{CodePermissionPrefs, UserPrefs};

const SCHEDULED_PROFILE_SCHEMA_VERSION: u32 = 1;
const MAX_SESSIONS_PER_KIND: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Chat,
    ScheduledRun,
}

/// Execution mode captured by an immutable scheduled-run profile.
///
/// Unlike the legacy two-state frontend mode, scheduled execution must retain
/// `agent` as a distinct value across restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledRunMode {
    Agent,
    Plan,
    Yolo,
}

impl ScheduledRunMode {
    pub(crate) const fn for_scheduled_auto_approve(auto_approve: bool) -> Self {
        if auto_approve {
            Self::Yolo
        } else {
            Self::Agent
        }
    }

    pub const fn as_label(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Plan => "plan",
            Self::Yolo => "yolo",
        }
    }

    pub const fn to_app_mode(self) -> AppMode {
        match self {
            Self::Agent => AppMode::Agent,
            Self::Plan => AppMode::Plan,
            Self::Yolo => AppMode::Yolo,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledRunProfile {
    pub task_id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    pub workspace: PathBuf,
    pub mode: ScheduledRunMode,
    pub allow_shell: bool,
    pub trust_mode: bool,
    pub auto_approve: bool,
}

impl ScheduledRunProfile {
    /// Scheduled execution has no interactive mode selector. Approval policy is
    /// the authority: runs that may auto-approve use Yolo, while every other run
    /// stays in Agent so the engine cannot bypass the persisted approval gate.
    pub(crate) const fn execution_mode(&self) -> ScheduledRunMode {
        ScheduledRunMode::for_scheduled_auto_approve(self.auto_approve)
    }
}

/// How an engine-owned scheduled session update changes the durable token total.
///
/// `SessionUpdated` does not carry usage, so callers must preserve the last durable
/// total. A final engine snapshot reports usage accumulated since that engine was
/// spawned; combining it with the spawn-time base produces an absolute lifetime
/// total without double-counting later turns from the same engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduledTokenAccounting {
    PreservePersisted,
    EngineCumulative {
        base_total_tokens: u64,
        engine_total_tokens: u64,
    },
}

/// Authoritative engine state to persist for one scheduled-run session.
///
/// This mirrors the durable fields available from `Event::SessionUpdated` plus a
/// final `SessionSnapshot`. Identity, title, creation time, artifacts, and the
/// immutable scheduled profile remain owned by the existing saved session/store.
#[derive(Debug, Clone)]
pub struct ScheduledEngineState {
    pub messages: Vec<Message>,
    pub system_prompt: Option<SystemPrompt>,
    pub model: String,
    pub workspace: PathBuf,
    pub mode: ScheduledRunMode,
    pub token_accounting: ScheduledTokenAccounting,
}

/// Authoritative engine snapshot for an ordinary chat session. The event
/// forwarder sanitizes engine-only user prompt injections before constructing
/// this value; persistence therefore never depends on a WebView staying alive.
#[derive(Debug, Clone)]
pub struct ChatEngineState {
    pub messages: Vec<Message>,
    pub system_prompt: Option<SystemPrompt>,
    pub model: String,
    pub workspace: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScheduledProfileRegistry {
    schema_version: u32,
    #[serde(default)]
    sessions: HashMap<String, ScheduledRunProfile>,
}

impl Default for ScheduledProfileRegistry {
    fn default() -> Self {
        Self {
            schema_version: SCHEDULED_PROFILE_SCHEMA_VERSION,
            sessions: HashMap::new(),
        }
    }
}

/// pinvou3 session 存储：包 SessionManager + active id 跟踪 + per-session mode 状态。
///
/// mode 状态分两层（品悟原生 code 会话权限模式的产品语义，已拍板）：
/// - plain（work）会话：mode 仅驻内存、默认 Yolo，不持久化（现状逐字节不变）；
/// - code 会话：显式切 mode（`set_mode`）时持久化到
///   `~/.pinvou3/sessions/_code_mode_states.json`（重开会话恢复它自己上次的
///   mode），并更新 settings.json `code_permission.last_mode`（新建 code 会话的
///   全局默认；从未用过 code 模式 → Plan 只读）。yolo 一次性确认标志
///   `code_permission.yolo_confirmed` 同样在 settings.json，由确认命令写入。
/// 其余运行时交互状态（pending_plan、persona、知识库挂载等）仍 in-memory only。
///
/// `auto_continue_count`：M2 弱模型加固——Executing 态 LLM 调一次工具就停时,
/// bridge 自动 send "继续"消息驱动 agent loop。每个用户主动消息重置为 0,
#[derive(Clone)]
pub struct SessionStore {
    manager: Arc<SessionManager>,
    scheduled_profiles: Arc<RwLock<HashMap<String, ScheduledRunProfile>>>,
    scheduled_profiles_path: Arc<PathBuf>,
    scheduled_root: Arc<PathBuf>,
    scheduled_mutation: Arc<Mutex<()>>,
    active: Arc<RwLock<Option<String>>>,
    mode_states: Arc<RwLock<HashMap<String, SessionModeState>>>,
    /// per-session 模型绑定:session_id → SavedModel.id。某 session 显式选过模型
    /// 才有条目;没选的回退全局 active_model_id。落盘到 `_session_models.json`
    /// (仿 skill_bindings),底座 SavedSession 不能加字段故独立存。
    session_models: Arc<RwLock<HashMap<String, String>>>,
    /// 历史对话置顶表:session_id -> pinned_at。独立落盘到 `_pinned_sessions.json`,
    /// 不改 SavedSession 结构。
    pinned_sessions: Arc<RwLock<HashMap<String, String>>>,
    /// 从左侧任务列表收起的会话:session_id -> hidden_at。独立落盘到
    /// `_hidden_sessions.json`,不改 SavedSession 结构。
    hidden_sessions: Arc<RwLock<HashMap<String, String>>>,
    /// 原生代码会话绑定的项目目录解析器,由 app 组合根(lib.rs)在 AcpPool 就绪
    /// 后注入;None = 无代码会话项目绑定,所有会话的执行根都是会话私有目录。
    /// 账本根(附件/审计/产物/远程授权)不受其影响,恒为会话私有目录。
    execution_root_resolver: Arc<RwLock<Option<ExecutionRootResolver>>>,
    /// 品悟原生 code 会话判定（ACP 会话恒为 plain，见 codex_acp store）。
    /// 与 Engine bridge / 远程端共用同一份 `SessionAgentStore` 闭包，由 app 组合根
    /// (lib.rs) 注入；None = 无 code 会话判定（测试/启动早期），全部按 plain 语义。
    code_session_predicate: Arc<RwLock<Option<CodeSessionPredicate>>>,
    /// `_code_mode_states.json` 的内存事实源：只存 code 会话的显式 mode。
    /// 启动时 load 合并进 `mode_states`；set_mode / 删除会话时维护并落盘。
    code_mode_states: Arc<RwLock<HashMap<String, SerializableMode>>>,
    /// settings.json `code_permission` 的进程内镜像。`mode_state` 在 chat 发送
    /// 路径上每轮被调，默认值解析只读这块内存（加锁读，不触盘）；写入经
    /// `UserPrefs::update_transaction` 落盘后同步本镜像。
    code_permission: Arc<RwLock<CodePermissionPrefs>>,
    /// `_multi_agent.json` 的持久化互斥：内存快照与 tmp+rename 必须在同一临界
    /// 区内完成。少了它，两个并发保存会各自读到不同时刻的快照，**后完成写盘的
    /// 旧快照**会覆盖新快照——重启后部分会话的开关状态消失。
    multi_agent_flags_io: Arc<Mutex<()>>,
}

/// 原生代码会话(品悟 Engine)的执行根解析器:绑定了项目目录的原生代码会话
/// 返回 `Some(项目目录)`;其余会话返回 `None`,调用方回退到会话私有目录。
///
/// 用闭包而非直接依赖 `codex_acp::SessionAgentStore`:`sessions` 与 `codex_acp`
/// 两个 feature 互相引用会成环,解析器由 app 组合根(lib.rs)注入并共享 AcpPool
/// 持有的同一份 store(clone 共享 Arc,运行时读到最新绑定)。
pub type ExecutionRootResolver = Arc<dyn Fn(&str) -> Option<PathBuf> + Send + Sync>;

/// 品悟原生 code 会话判定闭包：与 `ExecutionRootResolver` 同样的注入理由
/// （避免 sessions ↔ codex_acp 成环），由 lib.rs 共享同一份 `SessionAgentStore`。
/// ACP 会话在其 store 里恒为 plain（`bind_*` 时显式重置），故本判定命中即
/// "品悟原生 code 会话"，不会误伤 ACP 会话自己的权限模式。
pub type CodeSessionPredicate = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// 一个会话的两个根:
/// - `execution`:Engine cwd / shell 执行目录。绑了项目目录的原生代码会话 = 项目
///   目录;其余会话 = 会话私有目录(scheduled 会话 = 其 automation workspace)。
/// - `ledger`:应用账本根(附件/审计/产物/远程授权)。绑了项目目录的原生代码会话
///   恒为会话私有目录(不污染用户项目);其余会话与 execution 相同。
///
/// 由 [`SessionStore::session_roots`] 统一解析,调用方按用途显式选择用哪个根,
/// 避免把执行根误当账本根写盘(或反之)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRoots {
    pub execution: PathBuf,
    pub ledger: PathBuf,
}

/// 两个根的纯解析:给定原生代码会话绑定的项目目录(无绑定传 `None`),返回
/// 执行根与账本根。不感知 scheduled 会话——scheduled 的两个根都是其
/// automation workspace,由 [`SessionStore::session_roots`] 在上层处理。
pub fn session_roots_for(session_id: &str, bound_project_root: Option<PathBuf>) -> SessionRoots {
    let private = paths::session_workspace_dir(session_id);
    match bound_project_root {
        Some(project) => SessionRoots {
            execution: project,
            ledger: private,
        },
        None => SessionRoots {
            execution: private.clone(),
            ledger: private,
        },
    }
}

/// Transactional checkout of the two per-session one-shot prompt injections.
/// Unless committed after Engine submission, Drop restores values that still
/// belong to the same skill/persona and have not been replaced meanwhile.
pub(crate) struct PendingTurnInjections {
    store: SessionStore,
    session_id: String,
    skill: Option<(String, String)>,
    persona: Option<(Option<String>, String)>,
    committed: bool,
}

impl PendingTurnInjections {
    pub(crate) fn skill_instruction(&self) -> Option<&str> {
        self.skill
            .as_ref()
            .map(|(_, instruction)| instruction.as_str())
    }

    pub(crate) fn persona_body(&self) -> Option<&str> {
        self.persona.as_ref().map(|(_, body)| body.as_str())
    }

    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for PendingTurnInjections {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        self.store.restore_pending_turn_injections(
            &self.session_id,
            self.skill.take(),
            self.persona.take(),
        );
    }
}

/// Atomic checkout of the currently actionable Plan ticket. Claiming switches
/// the session to Yolo before the execution turn is submitted. Drop restores
/// Plan + ticket on every pre-submission error or cancelled command future.
pub(crate) struct PendingPlanClaim {
    store: SessionStore,
    session_id: String,
    plan_id: String,
    accepted_state: SessionModeState,
    settled: bool,
}

impl PendingPlanClaim {
    pub(crate) fn accepted_state(&self) -> &SessionModeState {
        &self.accepted_state
    }

    pub(crate) fn commit(mut self) {
        self.store
            .finish_pending_plan_claim(&self.session_id, &self.plan_id);
        self.settled = true;
    }

    pub(crate) fn rollback(mut self) -> Result<()> {
        let result = self
            .store
            .restore_pending_plan_claim(&self.session_id, &self.plan_id);
        self.settled = true;
        result
    }
}

impl Drop for PendingPlanClaim {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        if let Err(error) = self
            .store
            .restore_pending_plan_claim(&self.session_id, &self.plan_id)
        {
            eprintln!(
                "[sessions] restore dropped plan claim for {} failed: {error:#}",
                self.session_id
            );
        }
    }
}

impl SessionStore {
    fn save_session_atomic(&self, session: &SavedSession) -> Result<PathBuf> {
        validate_session_id(&session.metadata.id)?;
        let path = self
            .manager
            .sessions_dir()
            .join(format!("{}.json", session.metadata.id));
        let payload = serde_json::to_vec_pretty(session).context("serialize saved session")?;
        deepseek_tui::utils::write_atomic(&path, &payload)
            .with_context(|| format!("write session {}", path.display()))?;
        Ok(path)
    }

    /// Keep ordinary and scheduled histories in one directory without letting
    /// one class consume the other's retention budget. The upstream manager's
    /// default cleanup cannot distinguish the two once storage is unified.
    fn enforce_session_retention_locked(&self) -> Result<()> {
        let sessions = self
            .manager
            .list_sessions()
            .context("list sessions for retention")?;
        let mut chat_count = 0usize;
        let mut deleted_ids = Vec::new();
        let mut delete_error = None;
        for metadata in sessions {
            // Scheduled sessions own additional records outside sessions/.
            // Generic chat cleanup must not delete only the transcript and
            // strand the other half of their history.
            if metadata.id.starts_with("sched-") {
                continue;
            }
            chat_count += 1;
            if chat_count > MAX_SESSIONS_PER_KIND {
                match self.manager.delete_session(&metadata.id) {
                    Ok(()) => deleted_ids.push(metadata.id),
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => {
                        if delete_error.is_none() {
                            delete_error = Some(
                                anyhow::anyhow!(error)
                                    .context(format!("delete retained session {}", metadata.id)),
                            );
                        }
                    }
                }
            }
        }
        self.purge_session_side_maps(&deleted_ids);
        let reconcile_error = self.reconcile_scheduled_profiles_locked().err();
        match (delete_error, reconcile_error) {
            (Some(delete), Some(reconcile)) => Err(anyhow::anyhow!(
                "{delete:#}; scheduled profile reconciliation also failed: {reconcile:#}"
            )),
            (Some(delete), None) => Err(delete),
            (None, Some(reconcile)) => Err(reconcile),
            (None, None) => Ok(()),
        }
    }

    /// 用 `~/.pinvou3/sessions/` 初始化。如果目录不存在会自动创建。
    pub fn boot() -> Result<Self> {
        let store = Self::from_paths(
            paths::sessions_root(),
            paths::scheduled_run_profiles_path(),
            paths::scheduled_tasks_root(),
        )?;
        // Sidecars historically load later in the Tauri setup hook. Loading
        // them here too lets reconciliation discard scheduled-only runtime
        // state immediately instead of resurrecting it after stale profiles
        // have already been removed.
        store.load_skill_bindings();
        store.load_multi_agent_flags();
        store.load_session_models();
        store.load_pinned_sessions();
        store.load_hidden_sessions();
        store.load_code_mode_states();
        {
            let _mutation = store.scheduled_mutation.lock();
            store.enforce_session_retention_locked()?;
        }
        store.purge_all_scheduled_side_maps();
        Ok(store)
    }

    /// 测试专用：以隔离目录初始化，不触碰真实 `~/.pinvou3` 数据
    /// （评审测试建议：boot() 直连真实数据目录污染用户环境）。
    #[cfg(test)]
    pub(crate) fn boot_at_test_dir(root: &std::path::Path) -> Result<Self> {
        Self::from_paths(
            root.join("sessions"),
            root.join("scheduled-run-profiles.json"),
            root.join("scheduled"),
        )
    }

    #[cfg(test)]
    pub(crate) fn boot_with_scheduled_root(scheduled_root: PathBuf) -> Result<Self> {
        let store = Self::from_paths(
            paths::sessions_root(),
            paths::scheduled_run_profiles_path(),
            scheduled_root,
        )?;
        store.load_skill_bindings();
        store.load_multi_agent_flags();
        store.load_session_models();
        store.load_pinned_sessions();
        store.load_hidden_sessions();
        store.load_code_mode_states();
        {
            let _mutation = store.scheduled_mutation.lock();
            store.enforce_session_retention_locked()?;
        }
        store.purge_all_scheduled_side_maps();
        Ok(store)
    }

    fn from_paths(
        sessions_dir: PathBuf,
        scheduled_profiles_path: PathBuf,
        scheduled_root: PathBuf,
    ) -> Result<Self> {
        let manager = SessionManager::new(sessions_dir.clone())
            .with_context(|| format!("SessionManager::new({}) failed", sessions_dir.display()))?;
        let store = Self {
            manager: Arc::new(manager),
            scheduled_profiles: Arc::new(RwLock::new(HashMap::new())),
            scheduled_profiles_path: Arc::new(scheduled_profiles_path),
            scheduled_root: Arc::new(scheduled_root),
            scheduled_mutation: Arc::new(Mutex::new(())),
            active: Arc::new(RwLock::new(None)),
            mode_states: Arc::new(RwLock::new(HashMap::new())),
            multi_agent_flags_io: Arc::new(Mutex::new(())),
            session_models: Arc::new(RwLock::new(HashMap::new())),
            pinned_sessions: Arc::new(RwLock::new(HashMap::new())),
            hidden_sessions: Arc::new(RwLock::new(HashMap::new())),
            execution_root_resolver: Arc::new(RwLock::new(None)),
            code_session_predicate: Arc::new(RwLock::new(None)),
            code_mode_states: Arc::new(RwLock::new(HashMap::new())),
            code_permission: Arc::new(RwLock::new(UserPrefs::load().code_permission)),
        };
        store.load_scheduled_profiles()?;
        store.reconcile_scheduled_profiles_locked()?;
        Ok(store)
    }

    /// 列出所有 session 元数据，按 updated_at 倒序（最新在前）。
    pub fn list(&self) -> Result<Vec<SessionMetadata>> {
        let mut out = self
            .manager
            .list_sessions()
            .context("list_sessions failed")?;
        // Scheduled conversations share the durable store so detail/history can
        // load them normally, but remain owned by the Scheduled Tasks surface.
        // 多智能体是普通会话的持久开关，不是独立会话类型；这里只隔离定时
        // 会话，其余历史统一进入普通列表。
        out.retain(|metadata| !metadata.id.starts_with("sched-"));
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(out)
    }

    /// 列出所有定时运行会话的元数据，按 updated_at 倒序。归档列表需要它，
    /// 因为 [`Self::list`] 刻意把 sched-* 会话隔离在普通历史之外。
    pub fn list_scheduled(&self) -> Result<Vec<SessionMetadata>> {
        let mut out = self
            .manager
            .list_sessions()
            .context("list_sessions failed")?;
        out.retain(|metadata| metadata.id.starts_with("sched-"));
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(out)
    }

    /// 加载完整 session（包含所有 messages）。
    pub fn load(&self, id: &str) -> Result<SavedSession> {
        self.manager
            .load_session(id)
            .with_context(|| format!("load_session({id})"))
    }

    /// Return the persisted Session size without loading its transcript. Web
    /// downloads use this to reserve bounded transfer capacity before the
    /// comparatively expensive deserialize/serialize step begins.
    pub(crate) fn persisted_size(&self, id: &str) -> Result<u64> {
        validate_session_id(id)?;
        let path = self.manager.sessions_dir().join(format!("{id}.json"));
        std::fs::metadata(&path)
            .with_context(|| format!("read Session metadata {}", path.display()))
            .map(|metadata| metadata.len())
    }

    /// 落盘整个 session（atomic write 由上游处理）。
    pub fn save(&self, session: &SavedSession) -> Result<PathBuf> {
        if self.is_scheduled_session(&session.metadata.id)? {
            let _mutation = self.scheduled_mutation.lock();
            let path = self.save_session_atomic(session)?;
            if let Err(error) = self.enforce_session_retention_locked() {
                eprintln!(
                    "[sessions] scheduled retention reconciliation failed after committed save: {error:#}"
                );
            }
            return Ok(path);
        }
        let _mutation = self.scheduled_mutation.lock();
        let path = self.save_session_atomic(session)?;
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!(
                "[sessions] session retention reconciliation failed after session save: {error:#}"
            );
        }
        Ok(path)
    }

    /// 删除 session（含 artifacts 子目录）。
    pub fn delete(&self, id: &str) -> Result<()> {
        if self.is_scheduled_session(id)? {
            bail!("Scheduled-run sessions are deleted through their automation");
        }
        match self.manager.delete_session(id) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {
                // The session JSON may already have been removed by an earlier
                // delete or interrupted cleanup. Treat that as success, but
                // still remove an orphaned workspace/artifacts directory.
                validate_session_id(id)?;
                let session_dir = self.manager.sessions_dir().join(id);
                match std::fs::remove_dir_all(&session_dir) {
                    Ok(()) => {}
                    Err(dir_err) if dir_err.kind() == ErrorKind::NotFound => {}
                    Err(dir_err) => {
                        return Err(dir_err).with_context(|| {
                            format!("remove stale session dir {}", session_dir.display())
                        });
                    }
                }
            }
            Err(err) => return Err(err).with_context(|| format!("delete_session({id})")),
        }
        // 如果删的是 active session，清理 active 标记
        let mut active = self.active.write();
        if active.as_deref() == Some(id) {
            *active = None;
        }
        drop(active);
        let removed_multi_agent = self
            .mode_states
            .write()
            .remove(id)
            .is_some_and(|state| state.multi_agent);
        if removed_multi_agent {
            if let Err(error) = self.save_multi_agent_flags() {
                eprintln!(
                    "[sessions] update _multi_agent.json after delete({id}) failed: {error:#}"
                );
            }
        }
        if self.code_mode_states.write().remove(id).is_some() {
            self.save_code_mode_states();
        }
        let removed_session_model = {
            let mut session_models = self.session_models.write();
            session_models.remove(id).is_some()
        };
        if removed_session_model {
            self.save_session_models();
        }
        let removed_pin = {
            let mut pinned_sessions = self.pinned_sessions.write();
            pinned_sessions.remove(id).is_some()
        };
        if removed_pin {
            self.save_pinned_sessions();
        }
        let removed_hidden = {
            let mut hidden_sessions = self.hidden_sessions.write();
            hidden_sessions.remove(id).is_some()
        };
        if removed_hidden {
            self.save_hidden_sessions();
        }
        Ok(())
    }

    pub fn session_kind(&self, id: &str) -> Result<SessionKind> {
        if self.is_scheduled_session(id)? {
            Ok(SessionKind::ScheduledRun)
        } else {
            Ok(SessionKind::Chat)
        }
    }

    pub fn scheduled_profile(&self, id: &str) -> Option<ScheduledRunProfile> {
        self.scheduled_profiles.read().get(id).cloned()
    }

    fn scheduled_workspace_for_task(&self, task_id: &str) -> Result<PathBuf> {
        validate_scheduled_task_id(task_id)?;
        Ok(self.scheduled_root.join(task_id).join("workspace"))
    }

    /// 注入原生代码会话的执行根解析器;由 app 组合根在 AcpPool 就绪后调用一次。
    /// 与 Engine bridge 共用同一份 `SessionAgentStore` 闭包,两侧解析结果一致。
    pub fn set_execution_root_resolver(&self, resolver: ExecutionRootResolver) {
        *self.execution_root_resolver.write() = Some(resolver);
    }

    /// 注入品悟原生 code 会话判定；由 app 组合根与执行根解析器同点注入，
    /// 与 Engine bridge / 远程端共用同一份 `SessionAgentStore` 闭包。
    ///
    /// `load_skill_bindings` 在启动早期、谓词注入前执行，对绑过 skill 的 code
    /// 会话用 `or_default()` 物化出 `mode=Yolo` 的条目（`SessionModeState` 默认
    /// mode 即 Yolo）；`load_code_mode_states` 只覆盖 `_code_mode_states.json`
    /// 里有显式记录的会话。于是「code 会话 + 绑 skill + 从未显式切 mode」重启后
    /// 会带着 Yolo 残留绕过 `resolved_default_mode`，错误回到 Yolo 而非 Plan 首启。
    /// 谓词注入后立刻 reconcile 这类残留：无持久化记录的 code 会话 mode 拨回 Plan。
    pub fn set_code_session_predicate(&self, predicate: CodeSessionPredicate) {
        *self.code_session_predicate.write() = Some(predicate);
        self.reconcile_code_default_modes();
    }

    /// 把启动期被 `load_skill_bindings` 物化成 Yolo、但没有 per-session 持久化
    /// 记录的 code 会话 mode 拨回 Plan 首启默认。仅修 `mode` 字段，保留
    /// `active_skill`/`pinvou_review_enabled` 等其他字段。
    fn reconcile_code_default_modes(&self) {
        // 有显式 per-session 记录的 code 会话：交给 load_code_mode_states 覆盖，
        // 不在此处理。
        let persisted: HashSet<String> = self.code_mode_states.read().keys().cloned().collect();
        let mut m = self.mode_states.write();
        for (id, state) in m.iter_mut() {
            if state.mode == SerializableMode::Yolo
                && !persisted.contains(id)
                && self.is_code_session(id)
            {
                state.mode = SerializableMode::Plan;
            }
        }
    }

    /// 统一解析一个会话的两个根(执行根 + 账本根)。调用方按用途显式选择
    /// [`SessionRoots::execution`] 或 [`SessionRoots::ledger`],避免把执行根误当
    /// 账本根写盘(或反之)。
    ///
    /// - scheduled 会话两个根都是其 automation workspace;
    /// - 绑了项目目录的原生代码会话:execution = 项目目录,ledger = 会话私有目录;
    /// - 其余会话两个根都是会话私有目录。
    pub fn session_roots(&self, id: &str) -> Result<SessionRoots> {
        // This helper is a path authority boundary, not merely a convenience
        // accessor. Validate before any join so callers can never turn a
        // Session id such as `../outside` into an escaping workspace path.
        validate_session_id(id)?;
        if let Some(profile) = self.scheduled_profile(id) {
            return Ok(SessionRoots {
                execution: profile.workspace.clone(),
                ledger: profile.workspace,
            });
        }
        if self.is_scheduled_session(id)? {
            bail!("Scheduled-run session '{id}' has no persisted execution profile");
        }
        let bound_project_root = self
            .execution_root_resolver
            .read()
            .as_ref()
            .and_then(|resolver| resolver(id));
        Ok(session_roots_for(id, bound_project_root))
    }

    /// The ledger root (attachments/audit/artifacts) for a session's own files,
    /// and the execution root for ordinary/scheduled sessions.
    ///
    /// For project-bound native code sessions this is NOT the engine execution
    /// root — the engine runs in the bound project directory. Use
    /// [`Self::session_roots`] when the caller needs to pick a root explicitly;
    /// this helper remains the ledger root and the fallback execution root for
    /// non-project sessions. Each scheduled run has an independent conversation,
    /// while all runs owned by the same automation share that automation's
    /// workspace.
    pub fn ledger_root(&self, id: &str) -> Result<PathBuf> {
        Ok(self.session_roots(id)?.ledger)
    }

    pub fn scheduled_session_ids_for_task(&self, task_id: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .scheduled_profiles
            .read()
            .iter()
            .filter(|(_, profile)| profile.task_id == task_id)
            .map(|(session_id, _)| session_id.clone())
            .collect();
        ids.sort();
        ids
    }

    pub fn scheduled_session_exists(&self, id: &str) -> bool {
        self.scheduled_profile(id).is_some() && self.manager.load_session(id).is_ok()
    }

    /// Persist authoritative engine state for an existing scheduled-run session.
    ///
    /// The scheduled mutation lock makes the read/modify/atomic-save operation
    /// exclusive with scheduled creation, deletion, and retention reconciliation.
    /// Ordinary chat sessions are deliberately rejected by this specialized entry.
    /// `SessionUpdated` callers should use `PreservePersisted`; terminal usage can
    /// then be committed independently with [`Self::persist_scheduled_token_total`].
    pub fn persist_scheduled_engine_state(
        &self,
        id: &str,
        state: ScheduledEngineState,
    ) -> Result<SavedSession> {
        let _mutation = self.scheduled_mutation.lock();
        let profile = self
            .scheduled_profiles
            .read()
            .get(id)
            .cloned()
            .with_context(|| format!("Session '{id}' is not a scheduled-run session"))?;
        validate_scheduled_session_id(id)?;

        let mut session = self
            .manager
            .load_session(id)
            .with_context(|| format!("load scheduled session {id} for engine persistence"))?;
        let total_tokens = match state.token_accounting {
            ScheduledTokenAccounting::PreservePersisted => session.metadata.total_tokens,
            ScheduledTokenAccounting::EngineCumulative {
                base_total_tokens,
                engine_total_tokens,
            } => base_total_tokens.saturating_add(engine_total_tokens),
        };
        let mode_label = state.mode.as_label();

        session.metadata.updated_at = Utc::now();
        session.metadata.message_count = state.messages.len();
        session.metadata.total_tokens = total_tokens;
        session.metadata.model = state.model;
        session.metadata.workspace = profile.workspace;
        session.metadata.mode = Some(mode_label.to_string());
        session.messages = state.messages;
        session.system_prompt = persisted_system_prompt(state.system_prompt.as_ref());

        self.save_session_atomic(&session)
            .with_context(|| format!("persist scheduled engine state for {id}"))?;
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!(
                "[sessions] scheduled retention reconciliation failed after committed engine state save: {error:#}"
            );
        }
        Ok(session)
    }

    /// Persist a sanitized `SessionUpdated` snapshot for an ordinary chat.
    ///
    /// The same mutation lock used by UI CAS, metadata edits, artifacts, and
    /// scheduled persistence covers the complete read/modify/atomic-save chain.
    /// Engine snapshots are authoritative and may legitimately truncate the
    /// transcript for edit-last-turn or compaction, so the UI overwrite guard
    /// is intentionally not applied here.
    pub fn persist_chat_engine_state(
        &self,
        id: &str,
        state: ChatEngineState,
    ) -> Result<SavedSession> {
        let _mutation = self.scheduled_mutation.lock();
        if self.scheduled_profiles.read().contains_key(id) {
            bail!("Session '{id}' is a scheduled-run session");
        }
        validate_session_id(id)?;

        let mut session = self
            .manager
            .load_session(id)
            .with_context(|| format!("load chat session {id} for engine persistence"))?;
        session.metadata.updated_at = Utc::now();
        session.metadata.message_count = state.messages.len();
        session.metadata.model = state.model;
        session.metadata.workspace = state.workspace;
        session.messages = state.messages;
        session.system_prompt = persisted_system_prompt(state.system_prompt.as_ref());

        self.save_session_atomic(&session)
            .with_context(|| format!("persist chat engine state for {id}"))?;
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!(
                "[sessions] chat retention reconciliation failed after committed engine state save: {error:#}"
            );
        }
        Ok(session)
    }

    /// Terminal fallback for an admitted chat or interactive scheduled turn
    /// whose Engine failed before
    /// emitting a user-bearing `SessionUpdated` snapshot (for example, an
    /// unconfigured model client). The content revision captured before
    /// submission prevents a duplicate append or stale edit if another
    /// authoritative writer already advanced the transcript.
    pub(crate) fn persist_admitted_chat_display(
        &self,
        id: &str,
        expected_revision: &str,
        display_message: Message,
        edit_last: bool,
    ) -> Result<SavedSession> {
        let _mutation = self.scheduled_mutation.lock();
        validate_session_id(id)?;
        let mut session = self
            .manager
            .load_session(id)
            .with_context(|| format!("load chat session {id} for admitted display fallback"))?;
        if transcript_revision(&session.messages)? != expected_revision {
            return Ok(session);
        }
        if edit_last {
            if let Some(index) = session
                .messages
                .iter()
                .rposition(|message| message.role == "user")
            {
                session.messages.truncate(index);
            }
        }
        session.messages.push(display_message);
        session.metadata.message_count = session.messages.len();
        session.metadata.updated_at = Utc::now();
        self.save_session_atomic(&session)
            .with_context(|| format!("persist admitted chat display for {id}"))?;
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!(
                "[sessions] chat retention reconciliation failed after admitted display save: {error:#}"
            );
        }
        Ok(session)
    }

    /// Persist a terminal engine's absolute lifetime token total without replacing
    /// the last `SessionUpdated` transcript or any other engine-owned state.
    ///
    /// `engine_total_tokens` is cumulative for the current engine instance and
    /// `base_total_tokens` is the durable total captured when that engine spawned.
    pub fn persist_scheduled_token_total(
        &self,
        id: &str,
        base_total_tokens: u64,
        engine_total_tokens: u64,
    ) -> Result<SavedSession> {
        let _mutation = self.scheduled_mutation.lock();
        if !self.scheduled_profiles.read().contains_key(id) {
            bail!("Session '{id}' is not a scheduled-run session");
        }
        validate_scheduled_session_id(id)?;

        let mut session = self
            .manager
            .load_session(id)
            .with_context(|| format!("load scheduled session {id} for token persistence"))?;
        session.metadata.updated_at = Utc::now();
        session.metadata.total_tokens = base_total_tokens.saturating_add(engine_total_tokens);

        self.save_session_atomic(&session)
            .with_context(|| format!("persist scheduled token total for {id}"))?;
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!(
                "[sessions] scheduled retention reconciliation failed after committed token save: {error:#}"
            );
        }
        Ok(session)
    }

    pub fn create_scheduled_run(&self, mut profile: ScheduledRunProfile) -> Result<SavedSession> {
        if profile.task_id.trim().is_empty() {
            bail!("Scheduled run task id is required");
        }
        if profile.model.trim().is_empty() {
            bail!("Scheduled run model is required");
        }

        let _mutation = self.scheduled_mutation.lock();
        // 每次运行创建独立对话；同一 automation 的所有对话共享任务工作间。
        // workspace 只由稳定 task_id(automation_id)派生，不接受调用方路径。
        profile.workspace = self.scheduled_workspace_for_task(&profile.task_id)?;
        std::fs::create_dir_all(&profile.workspace).with_context(|| {
            format!(
                "create scheduled task workspace {}",
                profile.workspace.display()
            )
        })?;
        let id = format!("sched-{}", generate_session_id());
        let mode = profile.mode.as_label();
        let mut session = create_saved_session_with_id_and_mode(
            id.clone(),
            &[],
            &profile.model,
            &profile.workspace,
            0,
            None,
            Some(mode),
        );
        session.metadata.title = "Scheduled run".to_string();
        self.save_session_atomic(&session)
            .context("save new scheduled session")?;

        self.scheduled_profiles
            .write()
            .insert(id.clone(), profile.clone());
        if let Err(err) = self.save_scheduled_profiles() {
            self.scheduled_profiles.write().remove(&id);
            if let Err(rollback_error) = self.manager.delete_session(&id) {
                return Err(anyhow::anyhow!(
                    "save scheduled session profile: {err:#}; rollback scheduled session {id} also failed: {rollback_error}"
                ));
            }
            return Err(err).context("save scheduled session profile");
        }
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!(
                "[sessions] scheduled retention reconciliation failed after committed create: {error:#}"
            );
        }
        Ok(session)
    }

    pub fn delete_scheduled_run(&self, id: &str, expected_task_id: &str) -> Result<()> {
        let _mutation = self.scheduled_mutation.lock();
        let Some(profile) = self.scheduled_profile(id) else {
            return Ok(());
        };
        if profile.task_id != expected_task_id {
            bail!(
                "Scheduled session task ownership mismatch: expected {expected_task_id}, found {}",
                profile.task_id
            );
        }

        match self.manager.delete_session(id) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("delete scheduled session {id}")),
        }
        self.remove_scheduled_runtime_dir(id)?;

        self.scheduled_profiles.write().remove(id);
        self.purge_session_side_maps(&[id.to_string()]);
        self.save_scheduled_profiles()?;
        Ok(())
    }

    pub fn reconcile_scheduled_profiles(&self) -> Result<()> {
        let _mutation = self.scheduled_mutation.lock();
        self.reconcile_scheduled_profiles_locked()
    }

    fn reconcile_scheduled_profiles_locked(&self) -> Result<()> {
        let stale_ids: Vec<String> = self
            .scheduled_profiles
            .read()
            .keys()
            .filter(|id| scheduled_session_file(&self.manager, id).is_ok_and(|path| !path.exists()))
            .cloned()
            .collect();

        let mut removed = Vec::new();
        for id in stale_ids {
            self.remove_scheduled_runtime_dir(&id)?;
            removed.push(id);
        }
        {
            let mut profiles = self.scheduled_profiles.write();
            for id in &removed {
                profiles.remove(id);
            }
        }
        if !removed.is_empty() {
            self.save_scheduled_profiles()?;
        }
        // A `sched-*` JSON without a profile is deliberately retained. It can
        // arise if the process dies between the two atomic commits, and keeping
        // the transcript is safer than treating an incomplete transaction as
        // permission to delete user history. The id prefix keeps it out of the
        // ordinary chat list until it can be recovered or removed explicitly.
        self.purge_session_side_maps(&removed);
        Ok(())
    }

    fn purge_session_side_maps(&self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        let contains = |candidate: &str| ids.iter().any(|id| id == candidate);

        let (removed_modes, removed_multi_agent) = {
            let mut modes = self.mode_states.write();
            let before = modes.len();
            let mut removed_multi_agent = false;
            modes.retain(|id, state| {
                let keep = !contains(id.as_str());
                if !keep && state.multi_agent {
                    removed_multi_agent = true;
                }
                keep
            });
            (modes.len() != before, removed_multi_agent)
        };
        if removed_modes {
            self.save_skill_bindings();
        }
        if removed_multi_agent {
            // 保留策略清掉的会话必须同步移出 _multi_agent.json：残留的幽灵
            // id 会在重启后复活开关状态，专家池变更联动还会给它重建工作区。
            if let Err(error) = self.save_multi_agent_flags() {
                eprintln!(
                    "[sessions] update _multi_agent.json after retention purge failed: {error:#}"
                );
            }
        }

        let removed_code_modes = {
            let mut modes = self.code_mode_states.write();
            let before = modes.len();
            modes.retain(|id, _| !contains(id.as_str()));
            modes.len() != before
        };
        if removed_code_modes {
            self.save_code_mode_states();
        }

        let removed_models = {
            let mut models = self.session_models.write();
            let before = models.len();
            models.retain(|id, _| !contains(id.as_str()));
            models.len() != before
        };
        if removed_models {
            self.save_session_models();
        }

        {
            let mut active = self.active.write();
            if active.as_deref().is_some_and(contains) {
                *active = None;
            }
        }

        let removed_pins = {
            let mut pins = self.pinned_sessions.write();
            let before = pins.len();
            pins.retain(|id, _| !contains(id.as_str()));
            pins.len() != before
        };
        if removed_pins {
            self.save_pinned_sessions();
        }

        let removed_hidden = {
            let mut hidden = self.hidden_sessions.write();
            let before = hidden.len();
            hidden.retain(|id, _| !contains(id.as_str()));
            hidden.len() != before
        };
        if removed_hidden {
            self.save_hidden_sessions();
        }
    }

    fn purge_all_scheduled_side_maps(&self) {
        let live_ids = self
            .scheduled_profiles
            .read()
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        let is_stale_scheduled_id =
            |id: &&String| id.starts_with("sched-") && !live_ids.contains(id.as_str());
        let mut ids = Vec::new();
        ids.extend(
            self.mode_states
                .read()
                .keys()
                .filter(is_stale_scheduled_id)
                .cloned(),
        );
        ids.extend(
            self.session_models
                .read()
                .keys()
                .filter(is_stale_scheduled_id)
                .cloned(),
        );
        ids.extend(
            self.pinned_sessions
                .read()
                .keys()
                .filter(is_stale_scheduled_id)
                .cloned(),
        );
        ids.extend(
            self.hidden_sessions
                .read()
                .keys()
                .filter(is_stale_scheduled_id)
                .cloned(),
        );
        ids.sort();
        ids.dedup();
        self.purge_session_side_maps(&ids);
    }

    fn is_scheduled_session(&self, id: &str) -> Result<bool> {
        if self.scheduled_profiles.read().contains_key(id) {
            return Ok(true);
        }
        if !id.starts_with("sched-") {
            return Ok(false);
        }
        Ok(scheduled_session_file(&self.manager, id)?.exists())
    }

    fn load_scheduled_profiles(&self) -> Result<()> {
        if !self.scheduled_profiles_path.exists() {
            return Ok(());
        }
        let raw =
            std::fs::read_to_string(self.scheduled_profiles_path.as_ref()).with_context(|| {
                format!(
                    "read scheduled profiles {}",
                    self.scheduled_profiles_path.display()
                )
            })?;
        let registry: ScheduledProfileRegistry =
            serde_json::from_str(&raw).context("parse scheduled session profiles")?;
        if registry.schema_version != SCHEDULED_PROFILE_SCHEMA_VERSION {
            bail!(
                "Scheduled profile schema v{} does not match supported v{}",
                registry.schema_version,
                SCHEDULED_PROFILE_SCHEMA_VERSION
            );
        }
        for (id, profile) in &registry.sessions {
            validate_scheduled_session_id(id)?;
            validate_scheduled_workspace_path(&self.scheduled_root, &profile.workspace)
                .with_context(|| format!("validate scheduled profile workspace for {id}"))?;
            std::fs::create_dir_all(&profile.workspace).with_context(|| {
                format!(
                    "create scheduled task workspace {}",
                    profile.workspace.display()
                )
            })?;
        }
        *self.scheduled_profiles.write() = registry.sessions;
        Ok(())
    }

    fn save_scheduled_profiles(&self) -> Result<()> {
        if let Some(parent) = self.scheduled_profiles_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create scheduled profile dir {}", parent.display()))?;
        }
        let registry = ScheduledProfileRegistry {
            schema_version: SCHEDULED_PROFILE_SCHEMA_VERSION,
            sessions: self.scheduled_profiles.read().clone(),
        };
        let payload =
            serde_json::to_vec_pretty(&registry).context("serialize scheduled profiles")?;
        deepseek_tui::utils::write_atomic(self.scheduled_profiles_path.as_ref(), &payload)
            .with_context(|| {
                format!(
                    "write scheduled profiles {}",
                    self.scheduled_profiles_path.display()
                )
            })
    }

    fn remove_scheduled_runtime_dir(&self, id: &str) -> Result<()> {
        validate_scheduled_session_id(id)?;
        if !self.scheduled_profiles.read().contains_key(id)
            && chat_session_file(&self.manager, id)?.exists()
        {
            bail!("Refusing to remove runtime data for ordinary chat session '{id}'");
        }
        let runtime_dir = self.manager.sessions_dir().join(id);
        if runtime_dir.exists() {
            std::fs::remove_dir_all(&runtime_dir).with_context(|| {
                format!("remove scheduled runtime dir {}", runtime_dir.display())
            })?;
        }
        Ok(())
    }

    /// 重命名：load → 改 metadata.title → save。
    pub fn set_title(&self, id: &str, title: String) -> Result<()> {
        // 标题和 transcript 存在同一个 JSON 中。定时会话生成期间 Engine 也会写这个
        // 文件，所以必须把 load / modify / save 放在同一把锁里；否则重命名可能把
        // Engine 刚落盘的新消息用旧快照覆盖掉。
        let _mutation = self.scheduled_mutation.lock();
        let mut session = self
            .manager
            .load_session(id)
            .with_context(|| format!("load_session({id}) for title update"))?;
        session.metadata.title = title;
        self.save_session_atomic(&session)?;
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!("[sessions] retention reconciliation failed after title update: {error:#}");
        }
        Ok(())
    }

    /// 更新会话最近活跃时间，不改动 transcript 或其他元数据。
    ///
    /// ACP 对话把时间线持久化在独立 sidecar 中，不会经过普通聊天的
    /// `persist_chat_engine_state`，因此接受新回合时需要显式触碰主会话元数据，
    /// 让统一侧边栏仍能按 `updated_at` 正确排序。
    pub fn touch_activity(&self, id: &str) -> Result<()> {
        let _mutation = self.scheduled_mutation.lock();
        validate_session_id(id)?;
        let mut session = self
            .manager
            .load_session(id)
            .with_context(|| format!("load_session({id}) for activity update"))?;
        session.metadata.updated_at = Utc::now();
        self.save_session_atomic(&session)?;
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!(
                "[sessions] retention reconciliation failed after activity update: {error:#}"
            );
        }
        Ok(())
    }

    /// 新建空 session（无 messages）。返回 SavedSession 让调用方
    /// 立刻 `Op::SyncSession` 同步给 engine，并 set_active(id)。
    /// 上游空消息时 title 默认 "New Session"，pinvou3 覆写成中文。
    pub fn create_new(
        &self,
        model: String,
        model_id: Option<String>,
        workspace: PathBuf,
    ) -> Result<SavedSession> {
        let id = generate_session_id();
        let mut session = create_saved_session_with_id_and_mode(
            id.clone(),
            &[],
            &model,
            &workspace,
            0,
            None,
            None,
        );
        session.metadata.title = "新对话".to_string();
        // per-session 模型：先落 sidecar 再公开 Session JSON，避免写盘失败后
        // 留下一条看似创建成功、重启却切回其它模型的会话。
        if let Some(mid) = model_id {
            self.set_session_model_id(&id, Some(mid))?;
        }
        if let Err(error) = self.save(&session) {
            let rollback = self.delete(&id);
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback_error) => {
                    anyhow::anyhow!("{error:#}; rollback Session {id}: {rollback_error:#}")
                }
            });
        }
        Ok(session)
    }

    /// Replace a transcript for explicit store-maintenance flows. Live ordinary
    /// turns use [`Self::persist_chat_engine_state`]; Web edits use the revision
    /// CAS entry point. `total_tokens` is preserved.
    pub fn update_messages(&self, id: &str, messages: Vec<Message>) -> Result<()> {
        let _mutation = self.scheduled_mutation.lock();
        let mut session = self
            .manager
            .load_session(id)
            .with_context(|| format!("load_session({id}) for transcript update"))?;
        if looks_like_truncating_overwrite(&session.messages, &messages) {
            anyhow::bail!(
                "refusing to overwrite {} existing messages with {} unrelated messages",
                session.messages.len(),
                messages.len()
            );
        }
        session.metadata.message_count = messages.len();
        session.metadata.updated_at = Utc::now();
        session.messages = messages;
        self.save_session_atomic(&session)?;
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!(
                "[sessions] retention reconciliation failed after transcript update: {error:#}"
            );
        }
        Ok(())
    }

    /// Atomically replace a normal chat transcript only when the caller's
    /// content-derived revision still matches the durable transcript.
    ///
    /// The mutation lock deliberately covers load, revision comparison,
    /// truncation protection, and the atomic file replacement. Metadata-only
    /// changes (for example title or artifacts) never create false conflicts.
    pub fn compare_and_swap_messages(
        &self,
        id: &str,
        expected_revision: &str,
        messages: Vec<Message>,
    ) -> Result<String> {
        let _mutation = self.scheduled_mutation.lock();
        if self.is_scheduled_session(id)? {
            bail!("Cannot replace messages for scheduled-run session '{id}'");
        }
        let mut session = self
            .manager
            .load_session(id)
            .with_context(|| format!("load_session({id}) for transcript CAS"))?;
        let current_revision = transcript_revision(&session.messages)?;
        if current_revision != expected_revision {
            bail!("session_revision_conflict: 会话内容已在远程控制编辑期间发生变化");
        }
        if looks_like_truncating_overwrite(&session.messages, &messages) {
            bail!(
                "refusing to overwrite {} existing messages with {} unrelated messages",
                session.messages.len(),
                messages.len()
            );
        }

        let next_revision = transcript_revision(&messages)?;
        session.metadata.message_count = messages.len();
        session.metadata.updated_at = Utc::now();
        session.messages = messages;
        self.save_session_atomic(&session)?;
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!("[sessions] retention reconciliation failed after transcript CAS: {error:#}");
        }
        Ok(next_revision)
    }

    /// 替换 session 的产物列表。前端跟踪 File.write / File.edit 工具调用积累的 paths,
    /// 每轮 TurnComplete 一起落盘。重启 / 切换 session 后能从 SavedSession.artifacts
    /// 恢复列表(让用户感知产物跟 session 是一对一的)。
    pub fn update_artifacts(&self, id: &str, paths: Vec<String>) -> Result<()> {
        let _mutation = self.scheduled_mutation.lock();
        if self.is_scheduled_session(id)? {
            bail!("Cannot replace artifacts for scheduled-run session '{id}'");
        }
        let mut session = self
            .manager
            .load_session(id)
            .with_context(|| format!("load_session({id}) for artifact update"))?;
        let session_id = session.metadata.id.clone();
        let now = Utc::now();
        session.artifacts = paths
            .into_iter()
            .enumerate()
            .map(|(idx, p)| {
                let path = PathBuf::from(&p);
                let byte_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                ArtifactRecord {
                    id: format!("p3art_{session_id}_{idx}"),
                    kind: ArtifactKind::ToolOutput,
                    session_id: session_id.clone(),
                    tool_call_id: format!("p3_{idx}"),
                    tool_name: "write_file".into(),
                    created_at: now,
                    byte_size,
                    preview: String::new(),
                    storage_path: path,
                }
            })
            .collect();
        session.metadata.updated_at = now;
        self.save_session_atomic(&session)?;
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!(
                "[sessions] retention reconciliation failed after artifact update: {error:#}"
            );
        }
        Ok(())
    }

    /// Merge one backend-observed artifact into the durable session record.
    /// This is used by headless scheduled runs where no WebView is present to
    /// call `save_session_artifacts` after `chat:tool_end`.
    pub(crate) fn append_scheduled_artifact_path(&self, id: &str, path: PathBuf) -> Result<()> {
        let _mutation = self.scheduled_mutation.lock();
        if !self.scheduled_profiles.read().contains_key(id) {
            bail!("Session '{id}' is not a scheduled-run session");
        }
        validate_scheduled_session_id(id)?;
        let mut session = self
            .manager
            .load_session(id)
            .with_context(|| format!("load scheduled session {id} for artifact append"))?;
        if session
            .artifacts
            .iter()
            .any(|artifact| artifact.storage_path == path)
        {
            return Ok(());
        }
        let now = Utc::now();
        let index = session.artifacts.len();
        let byte_size = std::fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        session.artifacts.push(ArtifactRecord {
            id: format!("p3art_{id}_{index}"),
            kind: ArtifactKind::ToolOutput,
            session_id: id.to_string(),
            tool_call_id: format!("p3_{index}"),
            tool_name: "write_file".to_string(),
            created_at: now,
            byte_size,
            preview: String::new(),
            storage_path: path,
        });
        session.metadata.updated_at = now;
        self.save_session_atomic(&session)
            .with_context(|| format!("persist scheduled artifact for {id}"))?;
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!(
                "[sessions] scheduled retention reconciliation failed after committed artifact append: {error:#}"
            );
        }
        Ok(())
    }

    pub fn active_id(&self) -> Option<String> {
        self.active.read().clone()
    }

    pub fn set_active(&self, id: Option<String>) {
        *self.active.write() = id;
    }

    // ===================== Mode 状态机 =====================

    /// 取当前 session 的 mode 状态。无内存条目时按 [`Self::resolved_default_mode`]
    /// 解析默认值（code 会话 → 全局 `code_last_mode`，从未用过 → Plan 只读；
    /// plain 会话 → Yolo 现状）。本函数在 chat 发送路径上每轮被调：解析只做
    /// 几次 RwLock 读，不触盘，也不物化条目（既有条目语义不变，原样返回）。
    pub fn mode_state(&self, id: &str) -> SessionModeState {
        self.mode_states
            .read()
            .get(id)
            .cloned()
            .unwrap_or_else(|| SessionModeState {
                mode: self.resolved_default_mode(id),
                ..SessionModeState::default()
            })
    }

    /// 品悟原生 code 会话判定。谓词未注入（测试/启动早期）时按 plain 处理，
    /// 保持历史行为。
    fn is_code_session(&self, id: &str) -> bool {
        self.code_session_predicate
            .read()
            .as_ref()
            .is_some_and(|predicate| predicate(id))
    }

    /// 无条目时的默认 mode 解析：code 会话回落全局 `code_permission.last_mode`
    /// （None = 用户从未用过 code 模式 → Plan 只读首启）；plain 会话恒 Yolo。
    fn resolved_default_mode(&self, id: &str) -> SerializableMode {
        if self.is_code_session(id) {
            self.code_permission
                .read()
                .last_mode
                .unwrap_or(SerializableMode::Plan)
        } else {
            SerializableMode::Yolo
        }
    }

    /// `mode_states` 的 entry 助手：新建条目时携带按会话类型解析的默认 mode，
    /// 避免 `or_default()` 把从未显式切过 mode 的 code 会话物化成 Yolo
    /// （否则 register_pending_plan 等流程会静默丢失 Plan 语义）。
    /// 调用方在拿 `mode_states` 写锁前先算好 default（谓词/镜像锁不入临界区）。
    fn mode_state_entry<'m>(
        states: &'m mut HashMap<String, SessionModeState>,
        id: &str,
        default_mode: SerializableMode,
    ) -> &'m mut SessionModeState {
        states
            .entry(id.to_string())
            .or_insert_with(|| SessionModeState {
                mode: default_mode,
                ..SessionModeState::default()
            })
    }

    /// 设置 mode。砍 PlanPhase 后是 Plan/Yolo 唯一 setter(流转命令都调它),
    /// 只改 mode,保留 pinvou_review_enabled 等其他字段。
    ///
    /// 仅品悟原生 code 会话持久化（产品已拍板的两层语义）：per-session 写
    /// `_code_mode_states.json`（重开恢复它自己上次的 mode）+ 更新全局
    /// `code_permission.last_mode`（新建 code 会话的默认）。plain 会话维持
    /// 内存态不持久化；ACP 会话不经此命令（有自己的权限模式）。落盘失败只
    /// 记日志不打断交互——内存切换已生效，与 save_skill_bindings 同级容错。
    pub fn set_mode(&self, id: &str, mode: SerializableMode) -> Result<()> {
        {
            let mut m = self.mode_states.write();
            let entry = m.entry(id.to_string()).or_default();
            entry.mode = mode;
            entry.pending_plan_id = None;
            entry.plan_claim_in_flight = None;
        }
        if self.is_code_session(id) {
            self.code_mode_states.write().insert(id.to_string(), mode);
            self.save_code_mode_states();
            self.record_code_last_mode(mode);
        }
        Ok(())
    }

    /// 多智能体模式开关（ADR-0006）。**必须持久化**：Web 门禁与每轮委派
    /// 注入都依据它，只驻内存的话重启后开关静默关闭、名册与门禁一起失效。
    ///
    /// 落盘失败即整体失败并**回滚内存**：否则界面显示已开启、重启后却静默
    /// 关闭，Web 门禁也跟着失守。
    ///
    /// 「改内存 → 落盘 → 失败回滚」**整个事务**持有 `multi_agent_flags_io`：
    /// 只锁保存的话，并发翻转同一会话且恰逢落盘失败时，一方的回滚会把另一
    /// 方已提交的新状态覆盖回旧值（复核点名）。
    pub fn set_multi_agent(&self, id: &str, enabled: bool) -> Result<()> {
        let _io = self.multi_agent_flags_io.lock();
        let previous = {
            let mut m = self.mode_states.write();
            let entry = m.entry(id.to_string()).or_default();
            let previous = entry.multi_agent;
            entry.multi_agent = enabled;
            previous
        };
        if let Err(error) = self.save_multi_agent_flags_locked() {
            let mut m = self.mode_states.write();
            if let Some(entry) = m.get_mut(id) {
                entry.multi_agent = previous;
            }
            return Err(error).context("persist multi-agent flag");
        }
        Ok(())
    }

    /// 开着多智能体开关的会话清单（持久化与专家池变更联动共用）。
    pub fn multi_agent_session_ids(&self) -> Vec<String> {
        let m = self.mode_states.read();
        let mut ids: Vec<String> = m
            .iter()
            .filter(|(_, state)| state.multi_agent)
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// 把开关清单落到 `sessions/_multi_agent.json`（空清单删文件，避免残留
    /// 空壳）。写入走 tmp + rename 原子替换：进程恰在写一半退出时不能给下次
    /// 启动留半个 JSON。**内存快照与写盘在 `multi_agent_flags_io` 同一临界区
    /// 内完成**：保存因此全序化，最后完成的保存必然持有不早于任何先前保存的
    /// 快照——并发「开启/删除」不会让旧快照覆盖新快照。
    pub fn save_multi_agent_flags(&self) -> Result<()> {
        let _io = self.multi_agent_flags_io.lock();
        self.save_multi_agent_flags_locked()
    }

    /// [`save_multi_agent_flags`] 的临界区本体：调用方必须已持有
    /// `multi_agent_flags_io`（parking_lot Mutex 不可重入）。
    fn save_multi_agent_flags_locked(&self) -> Result<()> {
        let file = crate::platform::paths::sessions_root().join("_multi_agent.json");
        let ids = self.multi_agent_session_ids();
        if ids.is_empty() {
            return match std::fs::remove_file(&file) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error).context("remove _multi_agent.json"),
            };
        }
        let json = serde_json::to_string_pretty(&ids).context("serialize multi-agent flags")?;
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, json).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &file)
            .with_context(|| format!("commit {} -> {}", tmp.display(), file.display()))
    }

    /// 启动时恢复开关清单（与 `load_skill_bindings` 同点调用）。
    ///
    /// 顺带自愈幽灵 id：删除/清理路径的侧车更新失败只记日志（会话本体已删，
    /// 报错也无从回滚），残留的 id 靠这里对账剔除——会话 JSON 已不存在的
    /// 条目不恢复，且当场重写清单，不再传染给下一次启动。
    pub fn load_multi_agent_flags(&self) {
        let file = crate::platform::paths::sessions_root().join("_multi_agent.json");
        let Ok(content) = std::fs::read_to_string(&file) else {
            return;
        };
        let ids: Vec<String> = match serde_json::from_str(&content) {
            Ok(ids) => ids,
            Err(e) => {
                eprintln!("[sessions] load_multi_agent_flags failed: {e}");
                return;
            }
        };
        let sessions_dir = self.manager.sessions_dir().to_path_buf();
        let mut ghosts = false;
        {
            let mut m = self.mode_states.write();
            for id in ids {
                if sessions_dir.join(format!("{id}.json")).is_file() {
                    m.entry(id).or_default().multi_agent = true;
                } else {
                    ghosts = true;
                }
            }
        }
        if ghosts {
            if let Err(error) = self.save_multi_agent_flags() {
                eprintln!(
                    "[sessions] rewrite _multi_agent.json after ghost cleanup failed: {error:#}"
                );
            }
        }
    }

    /// Register the newest actionable plan only while the session is still in
    /// Plan mode. A newer TurnComplete supersedes the previous ticket.
    pub(crate) fn register_pending_plan(
        &self,
        id: &str,
        plan_id: String,
    ) -> Option<SessionModeState> {
        let default_mode = self.resolved_default_mode(id);
        let mut states = self.mode_states.write();
        let entry = Self::mode_state_entry(&mut states, id, default_mode);
        if entry.mode != SerializableMode::Plan || entry.plan_claim_in_flight.is_some() {
            return None;
        }
        entry.pending_plan_id = Some(plan_id);
        Some(entry.clone())
    }

    /// Atomically compare-and-consume a Plan ticket and switch to Yolo. The
    /// returned guard restores the ticket if Engine submission does not commit.
    pub(crate) fn claim_pending_plan(&self, id: &str, plan_id: &str) -> Result<PendingPlanClaim> {
        let accepted_state = {
            let default_mode = self.resolved_default_mode(id);
            let mut states = self.mode_states.write();
            let entry = Self::mode_state_entry(&mut states, id, default_mode);
            if entry.mode != SerializableMode::Plan
                || entry.pending_plan_id.as_deref() != Some(plan_id)
                || entry.plan_claim_in_flight.is_some()
            {
                bail!("plan_not_active");
            }
            entry.mode = SerializableMode::Yolo;
            entry.pending_plan_id = None;
            entry.plan_claim_in_flight = Some(plan_id.to_string());
            entry.clone()
        };
        Ok(PendingPlanClaim {
            store: self.clone(),
            session_id: id.to_string(),
            plan_id: plan_id.to_string(),
            accepted_state,
            settled: false,
        })
    }

    fn finish_pending_plan_claim(&self, id: &str, plan_id: &str) {
        let mut states = self.mode_states.write();
        let Some(entry) = states.get_mut(id) else {
            return;
        };
        if entry.plan_claim_in_flight.as_deref() == Some(plan_id) {
            entry.plan_claim_in_flight = None;
        }
    }

    fn restore_pending_plan_claim(&self, id: &str, plan_id: &str) -> Result<()> {
        let default_mode = self.resolved_default_mode(id);
        let mut states = self.mode_states.write();
        let entry = Self::mode_state_entry(&mut states, id, default_mode);
        if entry.mode != SerializableMode::Yolo
            || entry.pending_plan_id.is_some()
            || entry.plan_claim_in_flight.as_deref() != Some(plan_id)
        {
            bail!("restore plan claim conflict");
        }
        entry.mode = SerializableMode::Plan;
        entry.pending_plan_id = Some(plan_id.to_string());
        entry.plan_claim_in_flight = None;
        Ok(())
    }

    pub(crate) fn discard_pending_plan(&self, id: &str, plan_id: &str) -> Result<SessionModeState> {
        let default_mode = self.resolved_default_mode(id);
        let mut states = self.mode_states.write();
        let entry = Self::mode_state_entry(&mut states, id, default_mode);
        if entry.mode != SerializableMode::Plan
            || entry.pending_plan_id.as_deref() != Some(plan_id)
            || entry.plan_claim_in_flight.is_some()
        {
            bail!("plan_not_active");
        }
        entry.pending_plan_id = None;
        Ok(entry.clone())
    }

    /// 设置品悟 review 开关（用户在 UI 顶部 toggle 切换）。
    /// 与 Plan/YOLO 切换正交：品悟 toggle 不动 mode/phase。
    pub fn set_pinvou_review(&self, id: &str, enabled: bool) {
        let default_mode = self.resolved_default_mode(id);
        let mut m = self.mode_states.write();
        let entry = Self::mode_state_entry(&mut m, id, default_mode);
        entry.pinvou_review_enabled = enabled;
    }

    /// 重置到默认（Yolo + None）。delete_session 时调用。
    pub fn reset_mode_state(&self, id: &str) {
        self.mode_states.write().remove(id);
        if self.code_mode_states.write().remove(id).is_some() {
            self.save_code_mode_states();
        }
    }

    // ===================== 工作流 skill 绑定 (per-session) =====================

    /// 把一个 skill 绑定到指定 session。`start_skill_session` 在 create_new
    /// 之后立刻调,挂 pending_instruction 让该 session 第一条 chat 自动 prepend。
    pub fn bind_skill(&self, id: &str, binding: ActiveSkillBinding) {
        let default_mode = self.resolved_default_mode(id);
        let mut m = self.mode_states.write();
        let entry = Self::mode_state_entry(&mut m, id, default_mode);
        entry.active_skill = Some(binding);
    }

    /// 取该 session 当前绑定的 skill 信息(给前端渲染 chips strip)。
    /// 注意:返回的 binding 里 pending_instruction 是 None(serde skip + 一次性消费)。
    pub fn active_skill(&self, id: &str) -> Option<ActiveSkillBinding> {
        self.mode_states.read().get(id)?.active_skill.clone()
    }

    /// 一次性消费 session 绑定 skill 的 pending instruction。
    /// commands::chat 在发用户消息前调,prepend 到 message content 后置空,
    /// 后续 turn 不再重复(LLM 已经看到过,靠 session 上下文保持)。
    pub fn take_pending_skill_instruction(&self, id: &str) -> Option<String> {
        let mut m = self.mode_states.write();
        let entry = m.get_mut(id)?;
        let skill = entry.active_skill.as_mut()?;
        skill.pending_instruction.take()
    }

    /// Atomically checkout every one-shot prompt injection for a turn. The
    /// returned guard restores them on any pre-submission error or cancelled
    /// future; callers commit it only after EngineHandle accepts the operation.
    pub(crate) fn take_pending_turn_injections(&self, id: &str) -> PendingTurnInjections {
        let (skill, persona) = {
            let mut states = self.mode_states.write();
            match states.get_mut(id) {
                Some(state) => {
                    let skill = state.active_skill.as_mut().and_then(|binding| {
                        binding
                            .pending_instruction
                            .take()
                            .map(|instruction| (binding.name.clone(), instruction))
                    });
                    let persona = state
                        .pending_persona_body
                        .take()
                        .map(|body| (state.active_persona.clone(), body));
                    (skill, persona)
                }
                None => (None, None),
            }
        };
        PendingTurnInjections {
            store: self.clone(),
            session_id: id.to_string(),
            skill,
            persona,
            committed: false,
        }
    }

    fn restore_pending_turn_injections(
        &self,
        id: &str,
        skill: Option<(String, String)>,
        persona: Option<(Option<String>, String)>,
    ) {
        if skill.is_none() && persona.is_none() {
            return;
        }
        let mut states = self.mode_states.write();
        let Some(state) = states.get_mut(id) else {
            return;
        };
        if let Some((skill_name, instruction)) = skill {
            if let Some(binding) = state.active_skill.as_mut() {
                if binding.name == skill_name && binding.pending_instruction.is_none() {
                    binding.pending_instruction = Some(instruction);
                }
            }
        }
        if let Some((persona_id, body)) = persona {
            if state.active_persona == persona_id && state.pending_persona_body.is_none() {
                state.pending_persona_body = Some(body);
            }
        }
    }

    /// 解除 session 的 skill 绑定(用户点 chips 区 ✕ 时调用)。
    /// 不删 session 本身,只清掉绑定 — chips strip 在前端会因此隐藏。
    // ── Side B 卡片池(persona,远端体系) ──
    pub fn set_active_persona(&self, id: &str, persona_id: Option<String>) {
        let default_mode = self.resolved_default_mode(id);
        Self::mode_state_entry(&mut self.mode_states.write(), id, default_mode).active_persona =
            persona_id;
    }
    pub fn active_persona_id(&self, id: &str) -> Option<String> {
        self.mode_states.read().get(id)?.active_persona.clone()
    }
    pub fn set_pending_persona_body(&self, id: &str, body: Option<String>) {
        let default_mode = self.resolved_default_mode(id);
        Self::mode_state_entry(&mut self.mode_states.write(), id, default_mode)
            .pending_persona_body = body;
    }
    pub fn take_pending_persona_body(&self, id: &str) -> Option<String> {
        self.mode_states
            .write()
            .get_mut(id)?
            .pending_persona_body
            .take()
    }

    // ── 知识库挂载(会话级粘连,仿 persona,仅驻内存) ──
    pub fn set_mounted_collection(&self, id: &str, collection_id: Option<i64>) {
        let mounted = collection_id
            .filter(|collection_id| *collection_id > 0)
            .map(|collection_id| MountedCollection {
                collection_id,
                enabled: true,
            })
            .into_iter()
            .collect();
        self.set_mounted_collections(id, mounted);
    }

    pub fn set_mounted_collections(
        &self,
        id: &str,
        collections: Vec<MountedCollection>,
    ) -> MountedCollectionsSnapshot {
        self.update_mounted_collections(id, |_| collections)
    }

    pub fn add_mounted_collection(
        &self,
        id: &str,
        collection_id: i64,
    ) -> MountedCollectionsSnapshot {
        self.update_mounted_collections(id, |mut collections| {
            if let Some(collection) = collections
                .iter_mut()
                .find(|collection| collection.collection_id == collection_id)
            {
                collection.enabled = true;
            } else {
                collections.push(MountedCollection {
                    collection_id,
                    enabled: true,
                });
            }
            collections
        })
    }

    pub fn set_mounted_collection_enabled(
        &self,
        id: &str,
        collection_id: i64,
        enabled: bool,
    ) -> MountedCollectionsSnapshot {
        self.update_mounted_collections(id, |mut collections| {
            if let Some(collection) = collections
                .iter_mut()
                .find(|collection| collection.collection_id == collection_id)
            {
                collection.enabled = enabled;
            }
            collections
        })
    }

    pub fn remove_mounted_collection(
        &self,
        id: &str,
        collection_id: i64,
    ) -> MountedCollectionsSnapshot {
        self.update_mounted_collections(id, |mut collections| {
            collections.retain(|collection| collection.collection_id != collection_id);
            collections
        })
    }

    /// Remove a deleted knowledge collection from every in-memory session in one write lock.
    ///
    /// Knowledge mounts are session-ephemeral, so `mode_states` is the complete fact source that
    /// needs cascading. Returning only changed snapshots lets the Tauri boundary publish one
    /// revisioned event per affected session without coupling the sessions domain to `AppHandle`.
    pub fn remove_mounted_collection_from_all(
        &self,
        collection_id: i64,
    ) -> Vec<(String, MountedCollectionsSnapshot)> {
        let mut states = self.mode_states.write();
        let mut changed = Vec::new();
        for (session_id, state) in states.iter_mut() {
            let mut collections = if state.mounted_collections.is_empty() {
                state
                    .mounted_collection
                    .map(|mounted_id| MountedCollection {
                        collection_id: mounted_id,
                        enabled: true,
                    })
                    .into_iter()
                    .collect::<Vec<_>>()
            } else {
                state.mounted_collections.clone()
            };
            let previous_len = collections.len();
            collections.retain(|collection| collection.collection_id != collection_id);
            if collections.len() == previous_len {
                continue;
            }

            state.mounted_collection = collections
                .iter()
                .find(|collection| collection.enabled)
                .map(|collection| collection.collection_id);
            state.mounted_collections = collections.clone();
            state.mounted_collections_revision =
                state.mounted_collections_revision.wrapping_add(1).max(1);
            changed.push((
                session_id.clone(),
                MountedCollectionsSnapshot {
                    revision: state.mounted_collections_revision,
                    collections,
                },
            ));
        }
        changed.sort_by(|left, right| left.0.cmp(&right.0));
        changed
    }

    fn update_mounted_collections<F>(&self, id: &str, update: F) -> MountedCollectionsSnapshot
    where
        F: FnOnce(Vec<MountedCollection>) -> Vec<MountedCollection>,
    {
        let default_mode = self.resolved_default_mode(id);
        let mut states = self.mode_states.write();
        let state = Self::mode_state_entry(&mut states, id, default_mode);
        let current = if state.mounted_collections.is_empty() {
            state
                .mounted_collection
                .map(|collection_id| MountedCollection {
                    collection_id,
                    enabled: true,
                })
                .into_iter()
                .collect()
        } else {
            state.mounted_collections.clone()
        };
        let mut normalized = Vec::new();
        for collection in update(current) {
            if collection.collection_id <= 0
                || normalized.iter().any(|mounted: &MountedCollection| {
                    mounted.collection_id == collection.collection_id
                })
            {
                continue;
            }
            normalized.push(collection);
        }
        let legacy = normalized
            .iter()
            .find(|collection| collection.enabled)
            .map(|collection| collection.collection_id);
        state.mounted_collection = legacy;
        state.mounted_collections = normalized.clone();
        state.mounted_collections_revision =
            state.mounted_collections_revision.wrapping_add(1).max(1);
        MountedCollectionsSnapshot {
            revision: state.mounted_collections_revision,
            collections: normalized,
        }
    }

    pub fn mounted_collections(&self, id: &str) -> Vec<MountedCollection> {
        self.mounted_collections_snapshot(id).collections
    }

    pub fn mounted_collections_snapshot(&self, id: &str) -> MountedCollectionsSnapshot {
        let states = self.mode_states.read();
        let Some(state) = states.get(id) else {
            return MountedCollectionsSnapshot {
                revision: 0,
                collections: Vec::new(),
            };
        };
        let collections = if !state.mounted_collections.is_empty() {
            state.mounted_collections.clone()
        } else {
            state
                .mounted_collection
                .map(|collection_id| MountedCollection {
                    collection_id,
                    enabled: true,
                })
                .into_iter()
                .collect()
        };
        MountedCollectionsSnapshot {
            revision: state.mounted_collections_revision,
            collections,
        }
    }

    pub fn mounted_collection_ids(&self, id: &str) -> Vec<i64> {
        self.mounted_collections(id)
            .into_iter()
            .filter(|collection| collection.enabled)
            .map(|collection| collection.collection_id)
            .collect()
    }

    pub fn mounted_collection(&self, id: &str) -> Option<i64> {
        self.mounted_collection_ids(id).into_iter().next()
    }

    pub fn unbind_skill(&self, id: &str) {
        if let Some(entry) = self.mode_states.write().get_mut(id) {
            entry.active_skill = None;
        }
        self.save_skill_bindings();
    }

    /// 查找已有绑定指定 skill 的 session ID（用于恢复工作流）。
    pub fn find_session_with_skill(&self, skill_name: &str) -> Option<String> {
        self.mode_states
            .read()
            .iter()
            .find(|(_, state)| {
                state.active_skill.as_ref().map(|s| s.name.as_str()) == Some(skill_name)
            })
            .map(|(id, _)| id.clone())
    }

    /// 持久化所有 skill binding 到磁盘。
    pub fn save_skill_bindings(&self) {
        let bindings_file = crate::platform::paths::sessions_root().join("_skill_bindings.json");
        let m = self.mode_states.read();
        let bindings: std::collections::HashMap<
            String,
            &crate::core::mode_state::ActiveSkillBinding,
        > = m
            .iter()
            .filter_map(|(id, state)| state.active_skill.as_ref().map(|s| (id.clone(), s)))
            .collect();
        if bindings.is_empty() {
            let _ = std::fs::remove_file(&bindings_file);
            return;
        }
        if let Ok(json) = serde_json::to_string_pretty(&bindings) {
            let _ = std::fs::write(bindings_file, json);
        }
    }

    /// 从磁盘恢复 skill bindings（启动时调用）。
    pub fn load_skill_bindings(&self) {
        let bindings_file = crate::platform::paths::sessions_root().join("_skill_bindings.json");
        if !bindings_file.exists() {
            return;
        }
        let content = match std::fs::read_to_string(&bindings_file) {
            Ok(c) => c,
            Err(_) => return,
        };
        let bindings: std::collections::HashMap<
            String,
            crate::core::mode_state::ActiveSkillBinding,
        > = match serde_json::from_str(&content) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[sessions] load_skill_bindings failed: {e}");
                return;
            }
        };
        let mut m = self.mode_states.write();
        for (id, binding) in bindings {
            let entry = m.entry(id).or_default();
            entry.active_skill = Some(binding);
        }
    }

    // ===================== code 会话 mode 持久化 =====================

    /// 持久化 code 会话的 per-session mode 到 `_code_mode_states.json`
    /// （仿 `_skill_bindings.json`；只存 code 会话，plain 会话 mode 恒内存态）。
    /// 空表时删文件，与 save_skill_bindings 同款语义。
    pub fn save_code_mode_states(&self) {
        let states_file = crate::platform::paths::sessions_root().join("_code_mode_states.json");
        let modes = self.code_mode_states.read();
        if modes.is_empty() {
            let _ = std::fs::remove_file(&states_file);
            return;
        }
        if let Ok(json) = serde_json::to_string_pretty(&*modes) {
            let _ = std::fs::write(states_file, json);
        }
    }

    /// 启动时恢复 code 会话的 per-session mode：合并进 `mode_states`，
    /// 重开某个 code 会话即恢复它自己上次显式使用的 mode。
    pub fn load_code_mode_states(&self) {
        let states_file = crate::platform::paths::sessions_root().join("_code_mode_states.json");
        if !states_file.exists() {
            return;
        }
        let content = match std::fs::read_to_string(&states_file) {
            Ok(c) => c,
            Err(_) => return,
        };
        let modes: std::collections::HashMap<String, SerializableMode> =
            match serde_json::from_str(&content) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("[sessions] load_code_mode_states failed: {e}");
                    return;
                }
            };
        {
            let mut persisted = self.code_mode_states.write();
            *persisted = modes.clone();
        }
        let mut states = self.mode_states.write();
        for (id, mode) in modes {
            states.entry(id).or_default().mode = mode;
        }
    }

    /// 全局 code 权限偏好（内存镜像；磁盘真相在 settings.json `code_permission`）。
    pub fn code_permission_prefs(&self) -> CodePermissionPrefs {
        *self.code_permission.read()
    }

    /// 记录"上次在 code 会话显式使用的 mode"：新建 code 会话的默认 mode。
    /// 先更新内存镜像（本次运行立即生效），再字段级事务写 settings.json；
    /// 写盘失败只记日志（与 set_mode 的容错语义一致）。
    fn record_code_last_mode(&self, mode: SerializableMode) {
        self.code_permission.write().last_mode = Some(mode);
        if let Err(error) = UserPrefs::update_transaction(|prefs| {
            prefs.code_permission.last_mode = Some(mode);
            Ok(())
        }) {
            eprintln!("[sessions] persist code_permission.last_mode failed: {error}");
        }
    }

    /// yolo 一次性确认：置 `code_permission.yolo_confirmed = true` 并落盘。
    /// 确认是 UI 层语义（与 VS Code 同款），后端不在 exit_plan_to_yolo 强制门控。
    ///
    /// 仅同步本命令负责的 `yolo_confirmed` 字段，不整体覆盖镜像：`update_transaction`
    /// 返回的快照可能已过期（并发 `record_code_last_mode` 在事务提交后、本行执行前
    /// 写入了内存镜像的 `last_mode`），整体赋值会丢弃它导致内存/磁盘漂移。
    pub fn confirm_code_yolo(&self) -> Result<CodePermissionPrefs, String> {
        UserPrefs::update_transaction(|prefs| {
            prefs.code_permission.yolo_confirmed = true;
            Ok(())
        })?;
        self.code_permission.write().yolo_confirmed = true;
        Ok(self.code_permission_prefs())
    }

    // ===================== per-session 模型绑定 =====================

    /// 取该 session 在输入栏应显示的模型 id。普通会话无绑定时返回 None；
    /// 定时会话首次打开时回退创建任务时的模型，用户手动切换后返回交互覆盖值。
    pub fn session_model_id(&self, id: &str) -> Option<String> {
        self.session_model_override(id).or_else(|| {
            self.scheduled_profile(id)
                .and_then(|profile| profile.model_id)
        })
    }

    /// 只读取用户在对话输入栏里选择的模型，不包含定时运行创建时的模型回退。
    pub fn session_model_override(&self, id: &str) -> Option<String> {
        self.session_models.read().get(id).cloned()
    }

    /// 设/清该 session 的模型 id 并落盘。`None` = 清除(回退全局默认)。
    pub fn set_session_model_id(&self, id: &str, model_id: Option<String>) -> Result<()> {
        let mut models = self.session_models.write();
        let previous = models.get(id).cloned();
        match model_id {
            Some(mid) => {
                models.insert(id.to_string(), mid);
            }
            None => {
                models.remove(id);
            }
        }
        if let Err(error) = Self::persist_session_models(&models) {
            match previous {
                Some(previous) => {
                    models.insert(id.to_string(), previous);
                }
                None => {
                    models.remove(id);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    /// 持久化 per-session 模型绑定到 `~/.pinvou3/sessions/_session_models.json`。
    fn persist_session_models(models: &HashMap<String, String>) -> Result<()> {
        let file = crate::platform::paths::sessions_root().join("_session_models.json");
        if models.is_empty() {
            return match std::fs::remove_file(&file) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error).with_context(|| format!("remove {}", file.display())),
            };
        }
        let payload =
            serde_json::to_vec_pretty(models).context("serialize per-session model bindings")?;
        deepseek_tui::utils::write_atomic(&file, &payload)
            .with_context(|| format!("persist per-session model bindings to {}", file.display()))
    }

    /// 尽力持久化由清理流程批量修改的模型绑定；交互式设置使用
    /// [`Self::set_session_model_id`] 的可失败事务路径。
    pub fn save_session_models(&self) {
        if let Err(error) = Self::persist_session_models(&self.session_models.read()) {
            eprintln!("[sessions] save_session_models failed: {error:#}");
        }
    }

    /// 启动时从磁盘恢复 per-session 模型绑定。
    pub fn load_session_models(&self) {
        let file = crate::platform::paths::sessions_root().join("_session_models.json");
        if !file.exists() {
            return;
        }
        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => return,
        };
        match serde_json::from_str::<HashMap<String, String>>(&content) {
            Ok(map) => {
                *self.session_models.write() = map;
            }
            Err(e) => eprintln!("[sessions] load_session_models failed: {e}"),
        }
    }

    // ===================== 历史对话置顶 =====================

    pub fn is_pinned(&self, id: &str) -> bool {
        self.pinned_sessions.read().contains_key(id)
    }

    pub fn pinned_at(&self, id: &str) -> Option<String> {
        self.pinned_sessions.read().get(id).cloned()
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) {
        {
            let mut pins = self.pinned_sessions.write();
            if pinned {
                pins.insert(id.to_string(), Utc::now().to_rfc3339());
            } else {
                pins.remove(id);
            }
        }
        self.save_pinned_sessions();
    }

    pub fn save_pinned_sessions(&self) {
        let file = crate::platform::paths::sessions_root().join("_pinned_sessions.json");
        let pins = self.pinned_sessions.read();
        if pins.is_empty() {
            let _ = std::fs::remove_file(&file);
            return;
        }
        let mut out: Vec<_> = pins
            .iter()
            .map(|(id, pinned_at)| {
                serde_json::json!({
                    "id": id,
                    "pinned_at": pinned_at,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            a.get("id")
                .and_then(|v| v.as_str())
                .cmp(&b.get("id").and_then(|v| v.as_str()))
        });
        if let Ok(json) = serde_json::to_string_pretty(&out) {
            let _ = std::fs::write(file, json);
        }
    }

    pub fn load_pinned_sessions(&self) {
        let file = crate::platform::paths::sessions_root().join("_pinned_sessions.json");
        if !file.exists() {
            return;
        }
        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => return,
        };
        match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(serde_json::Value::Array(items)) => {
                let mut pins = HashMap::new();
                for item in items {
                    match item {
                        serde_json::Value::String(id) => {
                            pins.insert(id, Utc::now().to_rfc3339());
                        }
                        serde_json::Value::Object(mut obj) => {
                            let id = obj
                                .remove("id")
                                .and_then(|v| v.as_str().map(str::to_string));
                            let pinned_at = obj
                                .remove("pinned_at")
                                .and_then(|v| v.as_str().map(str::to_string))
                                .unwrap_or_else(|| Utc::now().to_rfc3339());
                            if let Some(id) = id {
                                pins.insert(id, pinned_at);
                            }
                        }
                        _ => {}
                    }
                }
                *self.pinned_sessions.write() = pins;
            }
            Ok(_) => eprintln!("[sessions] load_pinned_sessions failed: invalid shape"),
            Err(e) => eprintln!("[sessions] load_pinned_sessions failed: {e}"),
        }
    }

    // ===================== 收起任务列表 =====================

    pub fn is_hidden(&self, id: &str) -> bool {
        self.hidden_sessions.read().contains_key(id)
    }

    pub fn hidden_at(&self, id: &str) -> Option<String> {
        self.hidden_sessions.read().get(id).cloned()
    }

    pub fn set_hidden(&self, id: &str, hidden: bool) {
        {
            let mut hidden_sessions = self.hidden_sessions.write();
            if hidden {
                hidden_sessions.insert(id.to_string(), Utc::now().to_rfc3339());
            } else {
                hidden_sessions.remove(id);
            }
        }
        if hidden {
            self.set_pinned(id, false);
        }
        self.save_hidden_sessions();
    }

    pub fn save_hidden_sessions(&self) {
        let file = crate::platform::paths::sessions_root().join("_hidden_sessions.json");
        let hidden_sessions = self.hidden_sessions.read();
        if hidden_sessions.is_empty() {
            let _ = std::fs::remove_file(&file);
            return;
        }
        let mut out: Vec<_> = hidden_sessions
            .iter()
            .map(|(id, hidden_at)| {
                serde_json::json!({
                    "id": id,
                    "hidden_at": hidden_at,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            a.get("id")
                .and_then(|v| v.as_str())
                .cmp(&b.get("id").and_then(|v| v.as_str()))
        });
        if let Ok(json) = serde_json::to_string_pretty(&out) {
            let _ = std::fs::write(file, json);
        }
    }

    pub fn load_hidden_sessions(&self) {
        let file = crate::platform::paths::sessions_root().join("_hidden_sessions.json");
        if !file.exists() {
            return;
        }
        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(_) => return,
        };
        match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(serde_json::Value::Array(items)) => {
                let mut hidden_sessions = HashMap::new();
                for item in items {
                    match item {
                        serde_json::Value::String(id) => {
                            hidden_sessions.insert(id, Utc::now().to_rfc3339());
                        }
                        serde_json::Value::Object(mut obj) => {
                            let id = obj
                                .remove("id")
                                .and_then(|v| v.as_str().map(str::to_string));
                            let hidden_at = obj
                                .remove("hidden_at")
                                .and_then(|v| v.as_str().map(str::to_string))
                                .unwrap_or_else(|| Utc::now().to_rfc3339());
                            if let Some(id) = id {
                                hidden_sessions.insert(id, hidden_at);
                            }
                        }
                        _ => {}
                    }
                }
                *self.hidden_sessions.write() = hidden_sessions;
            }
            Ok(_) => eprintln!("[sessions] load_hidden_sessions failed: invalid shape"),
            Err(e) => eprintln!("[sessions] load_hidden_sessions failed: {e}"),
        }
    }
}

/// 工作流运行宿主会话的 id 前缀。
///
/// 与 `sched-` 同源的隔离手法：宿主会话存在持久层并进入对话列表，但仍由
/// 工作流入口拥有；前缀驱动专用引擎配置、徽标与级联清理。
pub(crate) fn validate_session_id(id: &str) -> Result<()> {
    if id.trim().is_empty()
        || !id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
    {
        bail!("Invalid session id '{id}'");
    }
    Ok(())
}

fn validate_scheduled_session_id(id: &str) -> Result<()> {
    validate_session_id(id)?;
    if !id.starts_with("sched-") {
        bail!("Scheduled session id must start with 'sched-': {id}");
    }
    Ok(())
}

fn validate_scheduled_task_id(id: &str) -> Result<()> {
    if id.trim().is_empty()
        || !id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
    {
        bail!("Invalid scheduled task id '{id}'");
    }
    Ok(())
}

fn validate_scheduled_workspace_path(root: &Path, workspace: &Path) -> Result<()> {
    if !workspace.is_absolute() {
        bail!(
            "Scheduled profile workspace must be absolute: {}",
            workspace.display()
        );
    }
    if workspace
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!(
            "Scheduled profile workspace must not contain parent segments: {}",
            workspace.display()
        );
    }
    if !workspace.starts_with(root) {
        bail!(
            "Scheduled profile workspace must live under {}: {}",
            root.display(),
            workspace.display()
        );
    }
    if workspace.file_name().and_then(|name| name.to_str()) != Some("workspace") {
        bail!(
            "Scheduled profile workspace must end with 'workspace': {}",
            workspace.display()
        );
    }
    Ok(())
}

fn persisted_system_prompt(system_prompt: Option<&SystemPrompt>) -> Option<String> {
    match system_prompt {
        Some(SystemPrompt::Text(text)) => Some(text.clone()),
        Some(SystemPrompt::Blocks(blocks)) => Some(
            blocks
                .iter()
                .map(|block| block.text.clone())
                .collect::<Vec<_>>()
                .join("\n\n---\n\n"),
        ),
        None => None,
    }
}

fn chat_session_file(manager: &SessionManager, id: &str) -> Result<PathBuf> {
    validate_session_id(id)?;
    Ok(manager.sessions_dir().join(format!("{id}.json")))
}

fn scheduled_session_file(manager: &SessionManager, id: &str) -> Result<PathBuf> {
    validate_scheduled_session_id(id)?;
    Ok(manager.sessions_dir().join(format!("{id}.json")))
}

/// 生成 URL-safe session id（短 8 字节 timestamp + nanos hash）。
/// 上游 `validated_session_path` 只允许 `[A-Za-z0-9_-]`，所以走 base32-like 字符集。
fn generate_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    const ALPHA: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut n = nanos;
    let mut buf = String::with_capacity(13);
    for _ in 0..13 {
        buf.push(ALPHA[(n % 36) as usize] as char);
        n /= 36;
    }
    buf
}

/// Stable optimistic-concurrency token for transcript content only.
///
/// Session metadata and artifacts intentionally do not participate: renaming a
/// Session or discovering an artifact must not invalidate a browser transcript
/// edit that was based on the same messages.
pub fn transcript_revision(messages: &[Message]) -> Result<String> {
    let encoded = serde_json::to_vec(messages).context("serialize transcript for revision")?;
    Ok(crate::platform::encoding::hex_lower(&Sha256::digest(
        encoded,
    )))
}

fn looks_like_truncating_overwrite(existing: &[Message], incoming: &[Message]) -> bool {
    if incoming.len() >= existing.len() || existing.len() <= 2 {
        return false;
    }
    let check = incoming.len().min(2);
    if check == 0 {
        return true;
    }
    for idx in 0..check {
        if existing[idx] != incoming[idx] {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::paths::tests::ENV_LOCK;
    use deepseek_tui::models::{ContentBlock, SystemPrompt};

    /// 借用 paths 模块的进程级 env 锁——避免与其他 mutate PINVOU3_HOME
    /// 的测试并行 race。返回带 guard 的 store；guard drop 后才解锁。
    fn isolated_store() -> (SessionStore, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-sessions-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::env::set_var("PINVOU3_HOME", &tmp);
        let store = SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot");
        // 注意：不 remove_var——锁还没 drop，下面的断言需要 PINVOU3_HOME 仍是这个值。
        (store, guard)
    }

    fn user_text(text: &str) -> Message {
        Message {
            role: "user".into(),
            content: vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        }
    }

    fn assistant_text(text: &str) -> Message {
        Message {
            role: "assistant".into(),
            content: vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        }
    }

    /// Reopen the same on-disk stores without consulting the process-global
    /// PINVOU3_HOME again, so restart assertions retain the paths captured at boot.
    fn reopen_store(store: &SessionStore) -> Result<SessionStore> {
        let reopened = SessionStore::from_paths(
            store.manager.sessions_dir().to_path_buf(),
            store.scheduled_profiles_path.as_ref().clone(),
            store.scheduled_root.as_ref().clone(),
        )?;
        reopened.load_skill_bindings();
        reopened.load_session_models();
        reopened.load_pinned_sessions();
        reopened.load_hidden_sessions();
        reopened.load_code_mode_states();
        {
            let _mutation = reopened.scheduled_mutation.lock();
            reopened.enforce_session_retention_locked()?;
        }
        reopened.purge_all_scheduled_side_maps();
        Ok(reopened)
    }

    fn task_workspace(store: &SessionStore, task_id: &str) -> PathBuf {
        store
            .scheduled_workspace_for_task(task_id)
            .expect("valid scheduled task workspace")
    }

    #[test]
    fn create_new_persists_and_lists() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let list = store.list().expect("list");
        assert!(list.iter().any(|m| m.id == s.metadata.id));
    }

    #[test]
    fn session_roots_plain_session_shares_private_root() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let private = paths::session_workspace_dir(&s.metadata.id);
        let roots = store.session_roots(&s.metadata.id).expect("roots");
        assert_eq!(roots.execution, private);
        assert_eq!(roots.ledger, private);
        assert_eq!(
            store.ledger_root(&s.metadata.id).expect("ledger root"),
            private
        );
    }

    #[test]
    fn session_roots_bound_project_keeps_ledger_on_private_root() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let bound_id = s.metadata.id.clone();
        let project = std::env::temp_dir().join("pinvou3-bound-project-roots-test");
        store.set_execution_root_resolver(Arc::new(move |id: &str| {
            (id == bound_id).then(|| project.clone())
        }));
        let roots = store.session_roots(&s.metadata.id).expect("roots");
        assert_eq!(
            roots.execution,
            std::env::temp_dir().join("pinvou3-bound-project-roots-test")
        );
        // 绑了项目目录的原生代码会话：账本根恒为会话私有目录，不污染用户项目。
        let private = paths::session_workspace_dir(&s.metadata.id);
        assert_eq!(roots.ledger, private);
        assert_eq!(
            store.ledger_root(&s.metadata.id).expect("ledger root"),
            private
        );
        // 未绑定的会话不受 resolver 影响，两根仍一致。
        let other = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create other");
        let other_roots = store.session_roots(&other.metadata.id).expect("roots");
        assert_eq!(other_roots.execution, other_roots.ledger);
    }

    #[test]
    fn session_roots_scheduled_run_uses_automation_workspace_for_both_roots() {
        let (store, _g) = isolated_store();
        let saved = store
            .create_scheduled_run(scheduled_profile("task-roots"))
            .expect("scheduled run");
        let workspace = task_workspace(&store, "task-roots");
        let roots = store.session_roots(&saved.metadata.id).expect("roots");
        assert_eq!(roots.execution, workspace);
        assert_eq!(roots.ledger, workspace);
        assert_eq!(
            store.ledger_root(&saved.metadata.id).expect("ledger root"),
            workspace
        );
    }

    fn scheduled_profile(task_id: &str) -> ScheduledRunProfile {
        ScheduledRunProfile {
            task_id: task_id.to_string(),
            model: "/scheduled-model".to_string(),
            model_id: Some("scheduled-model-id".to_string()),
            workspace: std::env::temp_dir().join("scheduled-workspace"),
            mode: ScheduledRunMode::Plan,
            allow_shell: true,
            trust_mode: false,
            auto_approve: false,
        }
    }

    fn text_message(role: &str, text: &str) -> Message {
        Message {
            role: role.to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    fn scheduled_engine_state(
        messages: Vec<Message>,
        mode: ScheduledRunMode,
        token_accounting: ScheduledTokenAccounting,
    ) -> ScheduledEngineState {
        ScheduledEngineState {
            messages,
            system_prompt: Some(SystemPrompt::Text("scheduled system prompt".to_string())),
            model: "/engine-model".to_string(),
            workspace: std::env::temp_dir().join("scheduled-engine-workspace"),
            mode,
            token_accounting,
        }
    }

    fn chat_engine_state(messages: Vec<Message>) -> ChatEngineState {
        ChatEngineState {
            messages,
            system_prompt: Some(SystemPrompt::Text("ordinary system prompt".to_string())),
            model: "/ordinary-engine-model".to_string(),
            workspace: std::env::temp_dir().join("ordinary-engine-workspace"),
        }
    }

    #[test]
    fn ordinary_session_updated_snapshot_is_persisted_authoritatively() {
        let (store, _g) = isolated_store();
        let session = store
            .create_new("/initial-model".into(), None, std::env::temp_dir())
            .expect("create ordinary chat");
        store
            .update_messages(
                &session.metadata.id,
                vec![user_text("old"), assistant_text("old answer")],
            )
            .expect("seed transcript");

        let authoritative = vec![
            user_text("visible user prompt"),
            assistant_text("authoritative answer"),
        ];
        let saved = store
            .persist_chat_engine_state(
                &session.metadata.id,
                chat_engine_state(authoritative.clone()),
            )
            .expect("persist ordinary SessionUpdated");

        assert_eq!(saved.messages, authoritative);
        assert_eq!(saved.metadata.message_count, 2);
        assert_eq!(saved.metadata.model, "/ordinary-engine-model");
        assert_eq!(
            saved.system_prompt.as_deref(),
            Some("ordinary system prompt")
        );
        let reopened = reopen_store(&store).expect("reopen");
        assert_eq!(
            reopened
                .load(&session.metadata.id)
                .expect("load durable chat")
                .messages,
            authoritative
        );
    }

    #[test]
    fn admitted_display_fallback_is_revision_guarded_for_append_and_edit() {
        let (store, _g) = isolated_store();
        let session = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create chat");
        let baseline = vec![user_text("first"), assistant_text("answer")];
        store
            .update_messages(&session.metadata.id, baseline.clone())
            .unwrap();
        let baseline_revision = transcript_revision(&baseline).unwrap();

        let appended = store
            .persist_admitted_chat_display(
                &session.metadata.id,
                &baseline_revision,
                user_text("second"),
                false,
            )
            .unwrap();
        assert_eq!(
            appended.messages,
            vec![
                user_text("first"),
                assistant_text("answer"),
                user_text("second")
            ]
        );
        let unchanged = store
            .persist_admitted_chat_display(
                &session.metadata.id,
                &baseline_revision,
                user_text("must not duplicate"),
                false,
            )
            .unwrap();
        assert_eq!(unchanged.messages, appended.messages);

        let edit_revision = transcript_revision(&appended.messages).unwrap();
        let edited = store
            .persist_admitted_chat_display(
                &session.metadata.id,
                &edit_revision,
                user_text("edited second"),
                true,
            )
            .unwrap();
        assert_eq!(
            edited.messages,
            vec![
                user_text("first"),
                assistant_text("answer"),
                user_text("edited second")
            ]
        );
    }

    #[test]
    fn scheduled_session_is_isolated_but_directly_loadable() {
        let (store, _g) = isolated_store();
        let chat = store
            .create_new("/chat-model".into(), None, std::env::temp_dir())
            .expect("create chat");
        let scheduled = store
            .create_scheduled_run(scheduled_profile("task-isolated"))
            .expect("create scheduled run");

        let listed = store.list().expect("list chats");
        assert!(listed.iter().any(|item| item.id == chat.metadata.id));
        assert!(!listed.iter().any(|item| item.id == scheduled.metadata.id));
        assert!(paths::sessions_root()
            .join(format!("{}.json", scheduled.metadata.id))
            .exists());
        assert_eq!(
            store
                .load(&scheduled.metadata.id)
                .expect("direct load")
                .metadata
                .id,
            scheduled.metadata.id
        );
    }

    #[test]
    fn scheduled_profile_survives_restart_and_routes_message_updates() {
        let (store, _g) = isolated_store();
        let scheduled = store
            .create_scheduled_run(scheduled_profile("task-restart"))
            .expect("create scheduled run");
        let id = scheduled.metadata.id.clone();

        let reloaded = reopen_store(&store).expect("reboot");
        assert_eq!(
            reloaded
                .scheduled_profile(&id)
                .expect("profile after restart")
                .task_id,
            "task-restart"
        );
        reloaded
            .update_messages(&id, Vec::new())
            .expect("route scheduled update");
        assert!(reloaded
            .manager
            .sessions_dir()
            .join(format!("{id}.json"))
            .exists());
    }

    #[test]
    fn scheduled_profile_accepts_persisted_workspace_on_restart() {
        let (store, _g) = isolated_store();
        let scheduled = store
            .create_scheduled_run(scheduled_profile("task-legacy-workspace"))
            .expect("create scheduled run");
        let id = scheduled.metadata.id.clone();
        let persisted_workspace = store
            .scheduled_root
            .join("automation-legacy")
            .join("workspace");
        let raw = std::fs::read_to_string(store.scheduled_profiles_path.as_ref())
            .expect("read scheduled profile registry");
        let mut registry: ScheduledProfileRegistry =
            serde_json::from_str(&raw).expect("parse scheduled profile registry");
        registry
            .sessions
            .get_mut(&id)
            .expect("scheduled profile")
            .workspace = persisted_workspace.clone();
        std::fs::write(
            store.scheduled_profiles_path.as_ref(),
            serde_json::to_vec_pretty(&registry).expect("serialize scheduled profile registry"),
        )
        .expect("write scheduled profile registry");

        let reloaded = reopen_store(&store).expect("reboot");
        assert_eq!(
            reloaded
                .scheduled_profile(&id)
                .expect("profile after restart")
                .workspace,
            persisted_workspace
        );
        assert!(persisted_workspace.exists());
    }

    #[test]
    fn scheduled_conversation_accepts_interactive_mode_and_model_overrides() {
        let (store, _g) = isolated_store();
        let profile = scheduled_profile("task-interactive-profile");
        let scheduled = store
            .create_scheduled_run(profile.clone())
            .expect("create scheduled run");
        let id = scheduled.metadata.id;

        store
            .set_mode(&id, SerializableMode::Plan)
            .expect("scheduled conversation mode override");
        store
            .set_session_model_id(&id, Some("override-model".to_string()))
            .expect("scheduled conversation model override");
        let mut expected_profile = profile.clone();
        expected_profile.workspace = task_workspace(&store, &profile.task_id);
        assert_eq!(store.scheduled_profile(&id), Some(expected_profile));
        assert_eq!(store.mode_state(&id).mode, SerializableMode::Plan);
        assert_eq!(
            store.session_model_id(&id).as_deref(),
            Some("override-model")
        );
        assert_eq!(
            store.session_model_override(&id).as_deref(),
            Some("override-model")
        );
    }

    #[test]
    fn scheduled_conversation_model_override_precedes_profile_fallback() {
        let (store, _g) = isolated_store();
        let mut profile = scheduled_profile("task-model-authority");
        profile.model_id = None;
        let scheduled = store
            .create_scheduled_run(profile)
            .expect("create scheduled run");
        let id = scheduled.metadata.id;
        store
            .session_models
            .write()
            .insert(id.clone(), "legacy-model-id".to_string());

        assert_eq!(
            store.session_model_id(&id).as_deref(),
            Some("legacy-model-id"),
            "an explicit interactive model choice must win after opening the run as a chat"
        );
    }

    #[test]
    fn scheduled_mode_override_preserves_live_auxiliary_session_state() {
        let (store, _g) = isolated_store();
        let scheduled = store
            .create_scheduled_run(scheduled_profile("task-live-aux-state"))
            .expect("create scheduled run");
        let id = scheduled.metadata.id;
        store.set_active_persona(&id, Some("scheduled-persona".to_string()));
        store.bind_skill(
            &id,
            ActiveSkillBinding {
                name: "scheduled-skill".to_string(),
                pending_instruction: None,
                phases: Vec::new(),
                project_dir: None,
            },
        );
        store.set_mounted_collection(&id, Some(42));
        store
            .mode_states
            .write()
            .entry(id.clone())
            .or_default()
            .mode = SerializableMode::Plan;

        let state = store.mode_state(&id);
        assert_eq!(state.mode, SerializableMode::Plan);
        assert_eq!(state.active_persona.as_deref(), Some("scheduled-persona"));
        assert_eq!(
            state.active_skill.as_ref().map(|skill| skill.name.as_str()),
            Some("scheduled-skill")
        );
        assert_eq!(state.mounted_collection, Some(42));
    }

    #[test]
    fn scheduled_engine_state_persists_full_snapshot_and_preserves_identity_and_profile() {
        let (store, _g) = isolated_store();
        let profile = scheduled_profile("task-engine-state");
        let scheduled = store
            .create_scheduled_run(profile.clone())
            .expect("create scheduled run");
        let id = scheduled.metadata.id.clone();
        store
            .set_title(&id, "Kept scheduled title".to_string())
            .expect("set scheduled title");
        let before = store.load(&id).expect("load before engine state");
        let messages = vec![
            text_message("user", "run the scheduled task"),
            text_message("assistant", "scheduled result"),
        ];

        let persisted = store
            .persist_scheduled_engine_state(
                &id,
                scheduled_engine_state(
                    messages.clone(),
                    ScheduledRunMode::Yolo,
                    ScheduledTokenAccounting::EngineCumulative {
                        base_total_tokens: 40,
                        engine_total_tokens: 12,
                    },
                ),
            )
            .expect("persist scheduled engine state");

        assert_eq!(persisted.metadata.id, before.metadata.id);
        assert_eq!(persisted.metadata.title, before.metadata.title);
        assert_eq!(persisted.metadata.created_at, before.metadata.created_at);
        assert_eq!(persisted.metadata.message_count, messages.len());
        assert_eq!(persisted.metadata.total_tokens, 52);
        assert_eq!(persisted.metadata.model, "/engine-model");
        assert_eq!(
            persisted.metadata.workspace,
            task_workspace(&store, &profile.task_id)
        );
        assert_eq!(persisted.metadata.mode.as_deref(), Some("yolo"));
        assert_eq!(persisted.messages, messages);
        assert_eq!(
            persisted.system_prompt.as_deref(),
            Some("scheduled system prompt")
        );
        let mut expected_profile = profile.clone();
        expected_profile.workspace = task_workspace(&store, &profile.task_id);
        assert_eq!(store.scheduled_profile(&id), Some(expected_profile.clone()));

        let reloaded = reopen_store(&store).expect("reboot");
        assert_eq!(reloaded.scheduled_profile(&id), Some(expected_profile));
        let from_disk = reloaded.load(&id).expect("load persisted engine state");
        assert_eq!(from_disk.metadata.total_tokens, 52);
        assert_eq!(from_disk.messages, persisted.messages);
        assert_eq!(from_disk.system_prompt, persisted.system_prompt);
    }

    #[test]
    fn scheduled_engine_token_accounting_preserves_updates_and_accumulates_across_restarts() {
        let (store, _g) = isolated_store();
        let scheduled = store
            .create_scheduled_run(scheduled_profile("task-token-accounting"))
            .expect("create scheduled run");
        let id = scheduled.metadata.id.clone();

        store
            .persist_scheduled_engine_state(
                &id,
                scheduled_engine_state(
                    vec![text_message("user", "first turn")],
                    ScheduledRunMode::Plan,
                    ScheduledTokenAccounting::EngineCumulative {
                        base_total_tokens: 0,
                        engine_total_tokens: 100,
                    },
                ),
            )
            .expect("persist first engine snapshot");
        store
            .persist_scheduled_engine_state(
                &id,
                scheduled_engine_state(
                    vec![
                        text_message("user", "first turn"),
                        text_message("assistant", "incremental update"),
                    ],
                    ScheduledRunMode::Plan,
                    ScheduledTokenAccounting::PreservePersisted,
                ),
            )
            .expect("persist SessionUpdated-equivalent state");
        assert_eq!(
            store
                .load(&id)
                .expect("load after update")
                .metadata
                .total_tokens,
            100
        );

        let reloaded = reopen_store(&store).expect("restart before later turn");
        reloaded
            .persist_scheduled_engine_state(
                &id,
                scheduled_engine_state(
                    vec![text_message("assistant", "later turn")],
                    ScheduledRunMode::Yolo,
                    ScheduledTokenAccounting::EngineCumulative {
                        base_total_tokens: 100,
                        engine_total_tokens: 25,
                    },
                ),
            )
            .expect("persist later engine snapshot");
        reloaded
            .persist_scheduled_engine_state(
                &id,
                scheduled_engine_state(
                    vec![text_message("assistant", "same engine next turn")],
                    ScheduledRunMode::Yolo,
                    ScheduledTokenAccounting::EngineCumulative {
                        base_total_tokens: 100,
                        engine_total_tokens: 40,
                    },
                ),
            )
            .expect("persist cumulative same-engine snapshot");

        assert_eq!(
            reloaded
                .load(&id)
                .expect("load accumulated total")
                .metadata
                .total_tokens,
            140,
            "same-engine cumulative usage must not be added twice"
        );
    }

    #[test]
    fn scheduled_engine_state_entry_rejects_normal_chat_without_mutation() {
        let (store, _g) = isolated_store();
        let chat = store
            .create_new(
                "/chat-model".to_string(),
                None,
                std::env::temp_dir().join("chat-workspace"),
            )
            .expect("create chat");

        let error = store
            .persist_scheduled_engine_state(
                &chat.metadata.id,
                scheduled_engine_state(
                    vec![text_message("user", "must not persist")],
                    ScheduledRunMode::Plan,
                    ScheduledTokenAccounting::EngineCumulative {
                        base_total_tokens: 0,
                        engine_total_tokens: 99,
                    },
                ),
            )
            .expect_err("normal chat must not use scheduled persistence");

        assert!(error.to_string().contains("not a scheduled-run session"));
        let token_error = store
            .persist_scheduled_token_total(&chat.metadata.id, 0, 99)
            .expect_err("normal chat must not use scheduled token persistence");
        assert!(token_error
            .to_string()
            .contains("not a scheduled-run session"));
        let unchanged = store.load(&chat.metadata.id).expect("load unchanged chat");
        assert_eq!(unchanged.metadata.title, chat.metadata.title);
        assert_eq!(unchanged.metadata.model, chat.metadata.model);
        assert_eq!(unchanged.metadata.workspace, chat.metadata.workspace);
        assert_eq!(unchanged.metadata.total_tokens, 0);
        assert!(unchanged.messages.is_empty());
        assert!(unchanged.system_prompt.is_none());
    }

    #[test]
    fn public_artifact_replace_rejects_scheduled_without_mutation() {
        let (store, _g) = isolated_store();
        let scheduled = store
            .create_scheduled_run(scheduled_profile("task-artifact-owner"))
            .expect("create scheduled run");
        let id = scheduled.metadata.id;
        let original = std::env::temp_dir().join("scheduled-original-artifact.md");
        store
            .append_scheduled_artifact_path(&id, original.clone())
            .expect("backend artifact append");
        let before = store.load(&id).expect("load before replacement");

        let error = store
            .update_artifacts(
                &id,
                vec![std::env::temp_dir()
                    .join("ui-replacement.md")
                    .to_string_lossy()
                    .into_owned()],
            )
            .expect_err("public replacement must reject scheduled sessions");

        assert!(error.to_string().contains("scheduled-run"));
        let after = store.load(&id).expect("load after rejection");
        assert_eq!(after.artifacts.len(), before.artifacts.len());
        assert_eq!(
            after
                .artifacts
                .iter()
                .map(|artifact| artifact.storage_path.clone())
                .collect::<Vec<_>>(),
            before
                .artifacts
                .iter()
                .map(|artifact| artifact.storage_path.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(after.metadata.updated_at, before.metadata.updated_at);
        assert_eq!(before.artifacts[0].storage_path, original);
    }

    #[test]
    fn ordinary_artifact_replace_behavior_is_unchanged() {
        let (store, _g) = isolated_store();
        let chat = store
            .create_new("/chat-model".into(), None, std::env::temp_dir())
            .expect("create chat");
        let artifact = std::env::temp_dir().join("ordinary-artifact.md");

        store
            .update_artifacts(
                &chat.metadata.id,
                vec![artifact.to_string_lossy().into_owned()],
            )
            .expect("ordinary replacement remains supported");

        assert_eq!(
            store.load(&chat.metadata.id).expect("load chat").artifacts[0].storage_path,
            artifact
        );
    }

    #[test]
    fn scheduled_agent_mode_round_trips_without_collapsing_profile_or_metadata() {
        let (store, _g) = isolated_store();
        let mut profile = scheduled_profile("task-agent-mode");
        profile.mode = ScheduledRunMode::Agent;
        let scheduled = store
            .create_scheduled_run(profile.clone())
            .expect("create agent scheduled run");
        let id = scheduled.metadata.id.clone();

        assert_eq!(scheduled.metadata.mode.as_deref(), Some("agent"));
        assert_eq!(
            profile.mode.to_app_mode(),
            deepseek_tui::tui::app::AppMode::Agent
        );
        let persisted = store
            .persist_scheduled_engine_state(
                &id,
                ScheduledEngineState {
                    messages: vec![text_message("assistant", "agent result")],
                    system_prompt: Some(SystemPrompt::Text("agent prompt".to_string())),
                    model: "/agent-model".to_string(),
                    workspace: std::env::temp_dir().join("agent-workspace"),
                    mode: ScheduledRunMode::Agent,
                    token_accounting: ScheduledTokenAccounting::PreservePersisted,
                },
            )
            .expect("persist agent engine state");

        assert_eq!(persisted.metadata.mode.as_deref(), Some("agent"));
        assert_eq!(
            store.scheduled_profile(&id).expect("agent profile").mode,
            ScheduledRunMode::Agent
        );
        assert_eq!(store.mode_state(&id).mode, SerializableMode::Yolo);

        let reloaded = reopen_store(&store).expect("restart after agent persistence");
        assert_eq!(
            reloaded
                .scheduled_profile(&id)
                .expect("agent profile after restart")
                .mode,
            ScheduledRunMode::Agent
        );
        assert_eq!(reloaded.mode_state(&id).mode, SerializableMode::Yolo);
        assert_eq!(
            reloaded
                .load(&id)
                .expect("agent session after restart")
                .metadata
                .mode
                .as_deref(),
            Some("agent")
        );
    }

    #[test]
    fn scheduled_terminal_token_persistence_does_not_replace_engine_state() {
        let (store, _g) = isolated_store();
        let profile = scheduled_profile("task-terminal-token");
        let scheduled = store
            .create_scheduled_run(profile.clone())
            .expect("create scheduled run");
        let id = scheduled.metadata.id.clone();
        store
            .persist_scheduled_engine_state(
                &id,
                scheduled_engine_state(
                    vec![
                        text_message("user", "retain this request"),
                        text_message("assistant", "retain this response"),
                    ],
                    ScheduledRunMode::Plan,
                    ScheduledTokenAccounting::PreservePersisted,
                ),
            )
            .expect("persist cached SessionUpdated state");
        let before = store.load(&id).expect("load before terminal usage");

        let after = store
            .persist_scheduled_token_total(&id, 40, 9)
            .expect("persist terminal token total");

        assert_eq!(after.metadata.total_tokens, 49);
        assert_eq!(after.metadata.id, before.metadata.id);
        assert_eq!(after.metadata.title, before.metadata.title);
        assert_eq!(after.metadata.created_at, before.metadata.created_at);
        assert_eq!(after.metadata.message_count, before.metadata.message_count);
        assert_eq!(after.metadata.model, before.metadata.model);
        assert_eq!(after.metadata.workspace, before.metadata.workspace);
        assert_eq!(after.metadata.mode, before.metadata.mode);
        assert_eq!(after.messages, before.messages);
        assert_eq!(after.system_prompt, before.system_prompt);
        assert_eq!(after.artifacts, before.artifacts);
        let mut expected_profile = profile.clone();
        expected_profile.workspace = task_workspace(&store, &profile.task_id);
        assert_eq!(store.scheduled_profile(&id), Some(expected_profile));

        let reloaded = reopen_store(&store).expect("restart after terminal usage");
        let from_disk = reloaded
            .load(&id)
            .expect("load terminal usage after restart");
        assert_eq!(from_disk.metadata.total_tokens, 49);
        assert_eq!(from_disk.messages, before.messages);
        assert_eq!(from_disk.system_prompt, before.system_prompt);

        let later = reloaded
            .persist_scheduled_token_total(&id, 49, 11)
            .expect("persist later engine token total");
        assert_eq!(later.metadata.total_tokens, 60);
        assert_eq!(later.messages, before.messages);
        assert_eq!(later.system_prompt, before.system_prompt);
    }

    #[test]
    fn checked_scheduled_delete_removes_profile_json_and_runtime_directory() {
        let (store, _g) = isolated_store();
        let scheduled = store
            .create_scheduled_run(scheduled_profile("task-delete"))
            .expect("create scheduled run");
        let id = scheduled.metadata.id.clone();
        let runtime_dir = paths::sessions_root().join(&id);
        std::fs::create_dir_all(runtime_dir.join("artifacts")).expect("runtime dir");
        store.set_active(Some(id.clone()));
        store
            .set_session_model_id(&id, Some("override-model".to_string()))
            .expect("scheduled conversation model override");
        store.set_hidden(&id, true);
        store.set_pinned(&id, true);

        let err = store
            .delete(&id)
            .expect_err("ordinary chat deletion must reject scheduled runs");
        assert!(err.to_string().contains("through their automation"));

        let err = store
            .delete_scheduled_run(&id, "another-task")
            .expect_err("wrong owner must fail");
        assert!(err.to_string().contains("task ownership"));
        assert!(runtime_dir.exists());

        store
            .delete_scheduled_run(&id, "task-delete")
            .expect("delete scheduled run");
        assert!(store.scheduled_profile(&id).is_none());
        assert!(store.active_id().is_none());
        assert!(store.session_model_id(&id).is_none());
        assert!(!store.is_hidden(&id));
        assert!(!store.is_pinned(&id));
        assert!(!runtime_dir.exists());
        assert!(!paths::sessions_root().join(format!("{id}.json")).exists());
    }

    #[test]
    fn scheduled_creation_rolls_back_when_profile_write_fails() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-scheduled-rollback-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let profile_path = root.join("profiles.json");
        let store = SessionStore::from_paths(
            root.join("sessions"),
            profile_path.clone(),
            root.join("scheduled"),
        )
        .expect("store");
        std::fs::create_dir_all(&profile_path).expect("make profile path a directory");

        let err = store
            .create_scheduled_run(scheduled_profile("task-rollback"))
            .expect_err("profile write must fail");

        assert!(err.to_string().contains("save scheduled session profile"));
        assert!(
            store
                .manager
                .list_sessions()
                .expect("session list")
                .is_empty(),
            "the SavedSession must be removed when profile persistence fails"
        );
        assert!(store.scheduled_profiles.read().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scheduled_sessions_wait_for_coordinated_run_retention() {
        let (store, _g) = isolated_store();
        let chat = store
            .create_new("/chat-model".into(), None, std::env::temp_dir())
            .expect("create chat");
        let mut scheduled_ids = Vec::new();

        for index in 0..51 {
            let scheduled = store
                .create_scheduled_run(scheduled_profile(&format!("task-{index}")))
                .expect("create scheduled run");
            std::fs::create_dir_all(paths::sessions_root().join(&scheduled.metadata.id))
                .expect("runtime dir");
            store
                .mode_states
                .write()
                .insert(scheduled.metadata.id.clone(), SessionModeState::default());
            store
                .session_models
                .write()
                .insert(scheduled.metadata.id.clone(), "stale-model".to_string());
            store
                .pinned_sessions
                .write()
                .insert(scheduled.metadata.id.clone(), "stale-pin".to_string());
            store
                .hidden_sessions
                .write()
                .insert(scheduled.metadata.id.clone(), "stale-hidden".to_string());
            scheduled_ids.push(scheduled.metadata.id);
        }

        assert_eq!(
            store.manager.list_sessions().expect("session list").len(),
            52
        );
        assert_eq!(store.scheduled_profiles.read().len(), 51);
        assert_eq!(
            store
                .load(&chat.metadata.id)
                .expect("chat retained")
                .metadata
                .id,
            chat.metadata.id,
            "scheduled retention must not consume the ordinary-chat budget"
        );

        assert!(scheduled_ids.iter().all(|id| {
            store.scheduled_profile(id).is_some()
                && store.mode_states.read().contains_key(id)
                && store.session_models.read().contains_key(id)
                && store.pinned_sessions.read().contains_key(id)
                && store.hidden_sessions.read().contains_key(id)
                && paths::sessions_root().join(id).exists()
        }));
    }

    #[test]
    fn orphan_transcript_does_not_consume_live_scheduled_retention_budget() {
        let (store, _g) = isolated_store();
        let mut live_ids = Vec::new();
        for index in 0..MAX_SESSIONS_PER_KIND {
            let session = store
                .create_scheduled_run(scheduled_profile(&format!("live-task-{index}")))
                .expect("create live scheduled conversation");
            live_ids.push(session.metadata.id);
        }

        let orphan_id = "sched-newer-orphan";
        let mut orphan = create_saved_session_with_id_and_mode(
            orphan_id.to_string(),
            &[],
            "/scheduled-model",
            store.scheduled_root.as_ref(),
            0,
            None,
            Some("yolo"),
        );
        orphan.metadata.updated_at = Utc::now() + chrono::Duration::minutes(1);
        store
            .save_session_atomic(&orphan)
            .expect("persist orphan transcript");
        store
            .enforce_session_retention_locked()
            .expect("enforce retention");

        assert_eq!(store.scheduled_profiles.read().len(), MAX_SESSIONS_PER_KIND);
        assert!(live_ids
            .iter()
            .all(|id| store.scheduled_profile(id).is_some() && store.load(id).is_ok()));
        assert!(store.load(orphan_id).is_ok(), "orphan must be preserved");
        assert!(store.scheduled_profile(orphan_id).is_none());
    }

    #[test]
    fn chat_retention_does_not_evict_scheduled_conversation() {
        let (store, _g) = isolated_store();
        let scheduled = store
            .create_scheduled_run(scheduled_profile("task-retained-across-chat-pruning"))
            .expect("scheduled conversation");

        for index in 0..51 {
            let mut chat = store
                .create_new(
                    "/chat-model".to_string(),
                    None,
                    std::env::temp_dir().join(format!("chat-{index}")),
                )
                .expect("create chat");
            chat.metadata.title = format!("chat {index}");
            store.save(&chat).expect("persist chat");
        }

        assert!(store.scheduled_session_exists(&scheduled.metadata.id));
        assert!(store.scheduled_profile(&scheduled.metadata.id).is_some());
        assert_eq!(store.list().expect("chat list").len(), 50);
    }

    #[test]
    fn boot_prunes_only_stale_scheduled_runtime_sidecars() {
        let (store, _g) = isolated_store();
        let live = store
            .create_scheduled_run(scheduled_profile("task-live-sidecars"))
            .expect("create live scheduled run")
            .metadata
            .id;
        let stale = store
            .create_scheduled_run(scheduled_profile("task-stale-sidecars"))
            .expect("create stale scheduled run")
            .metadata
            .id;
        for (id, suffix) in [(&live, "live"), (&stale, "stale")] {
            store.bind_skill(
                id,
                ActiveSkillBinding {
                    name: format!("{suffix}-scheduled-skill"),
                    pending_instruction: None,
                    phases: Vec::new(),
                    project_dir: None,
                },
            );
            store
                .session_models
                .write()
                .insert(id.clone(), format!("{suffix}-model"));
            store
                .pinned_sessions
                .write()
                .insert(id.clone(), format!("{suffix}-pin"));
            store
                .hidden_sessions
                .write()
                .insert(id.clone(), format!("{suffix}-hidden"));
        }
        store.save_skill_bindings();
        store.save_session_models();
        store.save_pinned_sessions();
        store.save_hidden_sessions();
        std::fs::remove_file(store.manager.sessions_dir().join(format!("{stale}.json")))
            .expect("simulate stale profile after session loss");
        let reloaded = reopen_store(&store).expect("reboot and prune sidecars");

        assert!(reloaded.mode_states.read().contains_key(&live));
        assert!(reloaded.session_models.read().contains_key(&live));
        assert!(reloaded.pinned_sessions.read().contains_key(&live));
        assert!(reloaded.hidden_sessions.read().contains_key(&live));
        assert!(!reloaded.mode_states.read().contains_key(&stale));
        assert!(!reloaded.session_models.read().contains_key(&stale));
        assert!(!reloaded.pinned_sessions.read().contains_key(&stale));
        assert!(!reloaded.hidden_sessions.read().contains_key(&stale));
        for sidecar in [
            "_skill_bindings.json",
            "_session_models.json",
            "_pinned_sessions.json",
            "_hidden_sessions.json",
        ] {
            let path = paths::sessions_root().join(sidecar);
            if let Ok(contents) = std::fs::read_to_string(path) {
                assert!(contents.contains(&live));
                assert!(!contents.contains(&stale));
            }
        }
    }

    #[test]
    fn boot_retains_orphan_transcript_left_before_profile_commit() {
        let (store, _g) = isolated_store();
        let id = "sched-orphan-before-profile";
        let orphan = create_saved_session_with_id_and_mode(
            id.to_string(),
            &[],
            "/scheduled-model",
            &std::env::temp_dir(),
            0,
            None,
            Some("yolo"),
        );
        store.manager.save_session(&orphan).expect("save orphan");
        let runtime_dir = paths::sessions_root().join(id);
        std::fs::create_dir_all(&runtime_dir).expect("runtime dir");

        let reloaded = reopen_store(&store).expect("reboot and reconcile");
        assert!(!reloaded.scheduled_session_exists(id));
        assert!(paths::sessions_root().join(format!("{id}.json")).exists());
        assert!(runtime_dir.exists());
        assert!(!reloaded
            .list()
            .expect("ordinary chat list")
            .iter()
            .any(|metadata| metadata.id == id));
    }

    #[test]
    fn concurrent_scheduled_creates_do_not_lose_registry_entries() {
        let (store, _g) = isolated_store();
        let handles: Vec<_> = (0..12)
            .map(|index| {
                let cloned = store.clone();
                std::thread::spawn(move || {
                    cloned
                        .create_scheduled_run(scheduled_profile(&format!(
                            "task-concurrent-{index}"
                        )))
                        .expect("concurrent create")
                        .metadata
                        .id
                })
            })
            .collect();
        let ids: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread"))
            .collect();

        let reloaded = reopen_store(&store).expect("reboot");
        assert_eq!(ids.len(), 12);
        assert!(ids
            .iter()
            .all(|id| reloaded.scheduled_profile(id).is_some()));
    }

    #[test]
    fn scheduled_runs_get_independent_conversations_and_share_the_task_workspace() {
        let (store, _g) = isolated_store();
        let first = store
            .create_scheduled_run(scheduled_profile("task-shared-workspace"))
            .expect("first run session");

        let mut edited = scheduled_profile("task-shared-workspace");
        edited.model = "edited-model".to_string();
        let second = store
            .create_scheduled_run(edited)
            .expect("second run session");

        assert_ne!(
            first.metadata.id, second.metadata.id,
            "every run of a task must create an independent conversation"
        );
        assert_eq!(
            store
                .scheduled_profile(&first.metadata.id)
                .expect("profile")
                .model,
            "/scheduled-model",
            "an earlier run keeps the profile captured for its conversation"
        );
        assert_eq!(
            store
                .scheduled_profile(&second.metadata.id)
                .expect("second profile")
                .model,
            "edited-model",
            "task edits apply to later run conversations"
        );
        assert_eq!(
            first.metadata.workspace, second.metadata.workspace,
            "conversations from one task must share its workspace"
        );
        assert_eq!(
            first.metadata.workspace,
            task_workspace(&store, "task-shared-workspace")
        );

        let other = store
            .create_scheduled_run(scheduled_profile("task-other"))
            .expect("other task session");
        assert_ne!(
            first.metadata.workspace, other.metadata.workspace,
            "different tasks must keep separate workspaces"
        );
    }

    #[test]
    fn corrupt_previous_run_does_not_block_a_new_conversation() {
        let (store, _g) = isolated_store();
        let first = store
            .create_scheduled_run(scheduled_profile("task-corrupt"))
            .expect("create scheduled conversation");
        std::fs::write(
            store
                .manager
                .sessions_dir()
                .join(format!("{}.json", first.metadata.id)),
            b"{not valid json",
        )
        .expect("corrupt transcript fixture");

        let second = store
            .create_scheduled_run(scheduled_profile("task-corrupt"))
            .expect("a new run must not load or reuse a corrupt older conversation");
        assert_ne!(first.metadata.id, second.metadata.id);
        let ids = store.scheduled_session_ids_for_task("task-corrupt");
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&first.metadata.id));
        assert!(ids.contains(&second.metadata.id));
    }

    #[test]
    fn set_title_updates_metadata() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        store
            .set_title(&s.metadata.id, "改个名字".into())
            .expect("rename");
        let loaded = store.load(&s.metadata.id).expect("load");
        assert_eq!(loaded.metadata.title, "改个名字");
    }

    #[test]
    fn touch_activity_updates_timestamp_without_mutating_conversation() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        std::thread::sleep(std::time::Duration::from_millis(2));

        store
            .touch_activity(&s.metadata.id)
            .expect("touch activity");

        let loaded = store.load(&s.metadata.id).expect("load");
        assert!(loaded.metadata.updated_at > s.metadata.updated_at);
        assert_eq!(loaded.metadata.title, s.metadata.title);
        assert_eq!(loaded.metadata.message_count, s.metadata.message_count);
        assert_eq!(loaded.messages, s.messages);
    }

    #[test]
    fn update_messages_rejects_unrelated_short_overwrite() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        store
            .update_messages(
                &s.metadata.id,
                vec![
                    user_text("old 1"),
                    assistant_text("old 2"),
                    user_text("old 3"),
                ],
            )
            .expect("seed messages");

        let result = store.update_messages(
            &s.metadata.id,
            vec![user_text("new unrelated"), assistant_text("new answer")],
        );

        assert!(result.is_err(), "short unrelated overwrite is rejected");
        let loaded = store.load(&s.metadata.id).expect("load");
        assert_eq!(loaded.messages.len(), 3);
    }

    #[test]
    fn transcript_cas_commits_and_returns_content_revision() {
        let (store, _g) = isolated_store();
        let session = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let expected = transcript_revision(&session.messages).expect("empty revision");
        let messages = vec![user_text("hello")];

        let committed = store
            .compare_and_swap_messages(&session.metadata.id, &expected, messages.clone())
            .expect("CAS commit");

        assert_eq!(committed, transcript_revision(&messages).expect("revision"));
        assert_eq!(
            store.load(&session.metadata.id).expect("load").messages,
            messages
        );
    }

    #[test]
    fn transcript_cas_rejects_stale_revision_without_overwrite() {
        let (store, _g) = isolated_store();
        let session = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let stale = transcript_revision(&session.messages).expect("empty revision");
        let winner = vec![user_text("winner")];
        store
            .compare_and_swap_messages(&session.metadata.id, &stale, winner.clone())
            .expect("first commit");

        let error = store
            .compare_and_swap_messages(
                &session.metadata.id,
                &stale,
                vec![user_text("stale overwrite")],
            )
            .expect_err("stale CAS must fail");

        assert!(format!("{error:#}").contains("session_revision_conflict"));
        assert_eq!(
            store.load(&session.metadata.id).expect("load").messages,
            winner
        );
    }

    #[test]
    fn metadata_and_artifacts_do_not_change_transcript_revision() {
        let (store, _g) = isolated_store();
        let session = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let messages = vec![user_text("stable transcript")];
        store
            .update_messages(&session.metadata.id, messages.clone())
            .expect("seed transcript");
        let before = transcript_revision(&store.load(&session.metadata.id).unwrap().messages)
            .expect("revision before metadata edits");

        store
            .set_title(&session.metadata.id, "renamed".to_string())
            .expect("rename");
        store
            .update_artifacts(
                &session.metadata.id,
                vec![std::env::temp_dir()
                    .join("transcript-revision-artifact.txt")
                    .to_string_lossy()
                    .into_owned()],
            )
            .expect("update artifacts");

        let after = transcript_revision(&store.load(&session.metadata.id).unwrap().messages)
            .expect("revision after metadata edits");
        assert_eq!(before, after);
        assert_eq!(
            store.load(&session.metadata.id).expect("load").messages,
            messages
        );
    }

    #[test]
    fn concurrent_stale_transcript_write_cannot_overwrite_winner() {
        let (store, _g) = isolated_store();
        let session = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let expected = transcript_revision(&session.messages).expect("empty revision");
        let barrier = Arc::new(std::sync::Barrier::new(2));

        let mut handles = Vec::new();
        for text in ["writer one", "writer two"] {
            let thread_store = store.clone();
            let thread_id = session.metadata.id.clone();
            let thread_expected = expected.clone();
            let thread_barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                thread_barrier.wait();
                thread_store.compare_and_swap_messages(
                    &thread_id,
                    &thread_expected,
                    vec![user_text(text)],
                )
            }));
        }

        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("writer thread"))
            .collect();
        assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(outcomes.iter().filter(|result| result.is_err()).count(), 1);

        let durable = store.load(&session.metadata.id).expect("load winner");
        let durable_revision = transcript_revision(&durable.messages).expect("durable revision");
        assert!(outcomes
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .any(|revision| revision == &durable_revision));
    }

    /// 开关持久化的真实行为回归：落盘 → 新 store 恢复 → 删除/清理同步。
    /// （复核指出旧测试只 grep 源码有没有调用，不覆盖真实重启与清理路径。）
    #[test]
    fn multi_agent_flags_survive_restart_and_follow_deletion() {
        let (store, _guard) = isolated_store();
        let chat = store
            .create_new("m".into(), None, std::env::temp_dir())
            .expect("create chat");
        let id = chat.metadata.id.clone();

        store.set_multi_agent(&id, true).expect("persist flag");
        let file = paths::sessions_root().join("_multi_agent.json");
        assert!(file.is_file(), "开关必须落盘");
        assert!(
            std::fs::read_to_string(&file).unwrap().contains(&id),
            "落盘清单必须包含该会话"
        );

        // "重启"：同一磁盘上重建 store → 开关恢复
        let reloaded = SessionStore::boot_with_scheduled_root(paths::scheduled_tasks_root())
            .expect("reboot store");
        assert!(
            reloaded.mode_state(&id).multi_agent,
            "重启后开关必须恢复（Web 门禁与每轮注入都依据它）"
        );

        // 关闭 → 清单收敛为空 → 文件删除（不留空壳）
        store.set_multi_agent(&id, false).expect("persist off");
        assert!(!file.exists(), "空清单必须删除 sidecar 文件");

        // 再开 → 删除会话 → 清单同步移除
        store
            .set_multi_agent(&id, true)
            .expect("persist flag again");
        store.delete(&id).expect("delete session");
        assert!(
            !file.exists(),
            "删除会话必须同步清掉 _multi_agent.json 条目"
        );
    }

    /// 删除路径侧车更新失败留下的幽灵 id，必须在下次启动被对账剔除，
    /// 且清单当场重写（不再传染后续启动）。
    #[test]
    fn ghost_ids_are_reconciled_away_on_load() {
        let (store, _guard) = isolated_store();
        let chat = store
            .create_new("m".into(), None, std::env::temp_dir())
            .expect("create chat");
        let real = chat.metadata.id.clone();
        store.set_multi_agent(&real, true).expect("persist flag");

        // 伪造一条幽灵记录（会话 JSON 不存在）
        let file = paths::sessions_root().join("_multi_agent.json");
        let mut ids: Vec<String> =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        ids.push("ghost-session".into());
        std::fs::write(&file, serde_json::to_string_pretty(&ids).unwrap()).unwrap();

        let reloaded = SessionStore::boot_with_scheduled_root(paths::scheduled_tasks_root())
            .expect("reboot store");
        assert!(reloaded.mode_state(&real).multi_agent, "真实会话恢复");
        assert!(
            !reloaded.mode_state("ghost-session").multi_agent,
            "幽灵 id 不得恢复开关"
        );
        let rewritten = std::fs::read_to_string(&file).unwrap();
        assert!(
            !rewritten.contains("ghost-session"),
            "清单必须当场重写剔除幽灵 id: {rewritten}"
        );
    }

    /// 并发「开启/关闭」交错后，落盘结果必须收敛到最终内存状态——保存的
    /// 快照与写盘在同一临界区内，旧快照不可能覆盖新快照。
    #[test]
    fn concurrent_flag_saves_converge_to_final_memory_state() {
        let (store, _guard) = isolated_store();
        let a = store
            .create_new("m".into(), None, std::env::temp_dir())
            .expect("create a")
            .metadata
            .id
            .clone();
        let b = store
            .create_new("m".into(), None, std::env::temp_dir())
            .expect("create b")
            .metadata
            .id
            .clone();

        let threads: Vec<_> = [(a.clone(), true), (b.clone(), true)]
            .into_iter()
            .map(|(id, on)| {
                let store = store.clone();
                std::thread::spawn(move || store.set_multi_agent(&id, on).expect("persist"))
            })
            .collect();
        for t in threads {
            t.join().expect("join");
        }

        let file = paths::sessions_root().join("_multi_agent.json");
        let listed: Vec<String> =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(
            listed.contains(&a) && listed.contains(&b),
            "并发保存不得互相丢会话: {listed:?}"
        );
    }

    /// 保留策略的自动清理同样要移出开关清单：残留幽灵 id 会在重启后复活
    /// 开关状态，专家池变更联动还会给它重建工作区。
    #[test]
    fn retention_purge_also_updates_multi_agent_flags() {
        let (store, _guard) = isolated_store();
        let chat = store
            .create_new("m".into(), None, std::env::temp_dir())
            .expect("create chat");
        let id = chat.metadata.id.clone();
        store.set_multi_agent(&id, true).expect("persist flag");
        let file = paths::sessions_root().join("_multi_agent.json");
        assert!(file.is_file());

        store.purge_session_side_maps(&[id.clone()]);

        assert!(!store.mode_state(&id).multi_agent, "内存状态已清");
        assert!(
            !file.exists(),
            "自动清理后 _multi_agent.json 不得残留幽灵 id"
        );
    }

    #[test]
    fn delete_removes_session() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        store.delete(&s.metadata.id).expect("delete");
        assert!(store.load(&s.metadata.id).is_err(), "load after delete");
    }

    #[test]
    fn active_id_tracks_set_active() {
        let (store, _g) = isolated_store();
        assert!(store.active_id().is_none());
        store.set_active(Some("abc".into()));
        assert_eq!(store.active_id().as_deref(), Some("abc"));
        store.set_active(None);
        assert!(store.active_id().is_none());
    }

    #[test]
    fn delete_active_clears_active_id() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        store.set_active(Some(s.metadata.id.clone()));
        store.delete(&s.metadata.id).expect("delete");
        assert!(store.active_id().is_none(), "delete active clears tracker");
    }

    #[test]
    fn delete_missing_session_file_is_idempotent() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");
        let session_file = store
            .manager
            .sessions_dir()
            .join(format!("{}.json", s.metadata.id));
        let session_dir = store.manager.sessions_dir().join(&s.metadata.id);
        std::fs::create_dir_all(&session_dir).expect("session dir");
        std::fs::remove_file(&session_file).expect("remove session file");
        store.set_active(Some(s.metadata.id.clone()));
        store.set_pinned(&s.metadata.id, true);

        store.delete(&s.metadata.id).expect("delete missing file");

        assert!(!session_dir.exists(), "stale session dir removed");
        assert!(store.active_id().is_none(), "active tracker cleared");
        assert!(!store.is_pinned(&s.metadata.id), "pinned state cleared");

        store
            .delete(&s.metadata.id)
            .expect("repeated delete remains successful");
    }

    #[test]
    fn pinned_sessions_persist_and_delete_cleans() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");

        store.set_pinned(&s.metadata.id, true);
        assert!(store.is_pinned(&s.metadata.id));
        assert!(
            store.pinned_at(&s.metadata.id).is_some(),
            "pinning records pinned_at"
        );

        let reloaded = SessionStore::boot().expect("reboot");
        reloaded.load_pinned_sessions();
        assert!(reloaded.is_pinned(&s.metadata.id));
        assert!(
            reloaded.pinned_at(&s.metadata.id).is_some(),
            "pinned_at survives reload"
        );

        reloaded.delete(&s.metadata.id).expect("delete");
        assert!(!reloaded.is_pinned(&s.metadata.id));
        assert!(reloaded.pinned_at(&s.metadata.id).is_none());
    }

    #[test]
    fn pinned_sessions_loads_legacy_id_array() {
        let (_store, _g) = isolated_store();
        let file = crate::platform::paths::sessions_root().join("_pinned_sessions.json");
        std::fs::create_dir_all(crate::platform::paths::sessions_root()).expect("mkdir");
        std::fs::write(&file, r#"["legacy-session"]"#).expect("write legacy pins");

        let reloaded = SessionStore::boot().expect("reboot");
        reloaded.load_pinned_sessions();
        assert!(reloaded.is_pinned("legacy-session"));
        assert!(
            reloaded.pinned_at("legacy-session").is_some(),
            "legacy pins receive a migration timestamp"
        );
    }

    #[test]
    fn hidden_sessions_persist_restore_and_delete_cleans() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");

        store.set_hidden(&s.metadata.id, true);
        assert!(store.is_hidden(&s.metadata.id));
        assert!(
            store.hidden_at(&s.metadata.id).is_some(),
            "hiding records hidden_at"
        );

        let reloaded = SessionStore::boot().expect("reboot");
        reloaded.load_hidden_sessions();
        assert!(reloaded.is_hidden(&s.metadata.id));
        assert!(
            reloaded.hidden_at(&s.metadata.id).is_some(),
            "hidden_at survives reload"
        );

        reloaded.set_hidden(&s.metadata.id, false);
        assert!(!reloaded.is_hidden(&s.metadata.id));
        assert!(reloaded.hidden_at(&s.metadata.id).is_none());

        reloaded.set_hidden(&s.metadata.id, true);
        reloaded.delete(&s.metadata.id).expect("delete");
        assert!(!reloaded.is_hidden(&s.metadata.id));
        assert!(reloaded.hidden_at(&s.metadata.id).is_none());
    }

    #[test]
    fn hidden_sessions_loads_legacy_id_array() {
        let (_store, _g) = isolated_store();
        let file = crate::platform::paths::sessions_root().join("_hidden_sessions.json");
        std::fs::create_dir_all(crate::platform::paths::sessions_root()).expect("mkdir");
        std::fs::write(&file, r#"["legacy-hidden-session"]"#).expect("write legacy hidden");

        let reloaded = SessionStore::boot().expect("reboot");
        reloaded.load_hidden_sessions();
        assert!(reloaded.is_hidden("legacy-hidden-session"));
        assert!(
            reloaded.hidden_at("legacy-hidden-session").is_some(),
            "legacy hidden sessions receive a migration timestamp"
        );
    }

    #[test]
    fn hiding_session_clears_pinned_state() {
        let (store, _g) = isolated_store();
        let s = store
            .create_new("/model".into(), None, std::env::temp_dir())
            .expect("create");

        store.set_pinned(&s.metadata.id, true);
        assert!(store.is_pinned(&s.metadata.id));

        store.set_hidden(&s.metadata.id, true);
        assert!(store.is_hidden(&s.metadata.id));
        assert!(!store.is_pinned(&s.metadata.id));
        assert!(store.pinned_at(&s.metadata.id).is_none());

        let reloaded = SessionStore::boot().expect("reboot");
        reloaded.load_pinned_sessions();
        reloaded.load_hidden_sessions();
        assert!(reloaded.is_hidden(&s.metadata.id));
        assert!(!reloaded.is_pinned(&s.metadata.id));
    }

    #[test]
    fn generate_session_id_url_safe() {
        let id = generate_session_id();
        assert!(id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn pinvou_review_defaults_off() {
        let (store, _g) = isolated_store();
        assert!(!store.mode_state("s1").pinvou_review_enabled);
    }

    #[test]
    fn set_pinvou_review_persists() {
        let (store, _g) = isolated_store();
        store.set_pinvou_review("s1", true);
        assert!(store.mode_state("s1").pinvou_review_enabled);
        store.set_pinvou_review("s1", false);
        assert!(!store.mode_state("s1").pinvou_review_enabled);
    }

    #[test]
    fn set_mode_preserves_pinvou_review() {
        // 关键不变量:切 mode(set_mode)不能覆盖品悟开关。
        let (store, _g) = isolated_store();
        store.set_pinvou_review("s1", true);
        store
            .set_mode("s1", crate::core::mode_state::SerializableMode::Yolo)
            .expect("set chat mode");
        let state = store.mode_state("s1");
        assert!(state.pinvou_review_enabled);
        assert!(matches!(
            state.mode,
            crate::core::mode_state::SerializableMode::Yolo
        ));
    }

    #[test]
    fn pending_plan_ticket_is_compare_and_consumed_with_failure_restore() {
        let (store, _g) = isolated_store();
        let sid = "plan-ticket-session";
        store
            .set_mode(sid, SerializableMode::Plan)
            .expect("enter plan");
        let registered = store
            .register_pending_plan(sid, "plan-1".to_string())
            .expect("register plan");
        assert_eq!(registered.pending_plan_id.as_deref(), Some("plan-1"));
        assert!(store.claim_pending_plan(sid, "stale-plan").is_err());

        let claim = store
            .claim_pending_plan(sid, "plan-1")
            .expect("claim current plan");
        assert_eq!(claim.accepted_state().mode, SerializableMode::Yolo);
        assert!(claim.accepted_state().pending_plan_id.is_none());
        assert!(store.claim_pending_plan(sid, "plan-1").is_err());
        drop(claim);
        let restored = store.mode_state(sid);
        assert_eq!(restored.mode, SerializableMode::Plan);
        assert_eq!(restored.pending_plan_id.as_deref(), Some("plan-1"));

        store
            .claim_pending_plan(sid, "plan-1")
            .expect("reclaim current plan")
            .commit();
        let committed = store.mode_state(sid);
        assert_eq!(committed.mode, SerializableMode::Yolo);
        assert!(committed.pending_plan_id.is_none());
        assert!(store.claim_pending_plan(sid, "plan-1").is_err());

        store
            .set_mode(sid, SerializableMode::Plan)
            .expect("re-enter plan");
        store
            .register_pending_plan(sid, "plan-2".to_string())
            .expect("register newer plan");
        assert!(store.discard_pending_plan(sid, "plan-1").is_err());
        let discarded = store
            .discard_pending_plan(sid, "plan-2")
            .expect("discard current plan");
        assert_eq!(discarded.mode, SerializableMode::Plan);
        assert!(discarded.pending_plan_id.is_none());
        assert!(store.discard_pending_plan(sid, "plan-2").is_err());
    }

    /// 模式切换闭环(回归底座二态后的核心契约):流转命令 set_plan_mode_next(→Plan) /
    /// accept_plan / exit_plan_to_yolo(→Yolo) 实质都只调 set_mode,全程**只动 mode**——
    /// 品悟开关 / 挂载知识集 / 人格卡 / skill 绑定等正交状态必须原样保留。
    /// (discard_plan「算了」不在此列:放弃方案但留在当前 mode,不调 set_mode。)
    /// 防有人给流转命令加副作用,或把 set_mode 改成整体覆盖式写法时连带清掉这些字段。
    /// 比 set_mode_preserves_pinvou_review 更全(多步往返 + 四字段)。
    #[test]
    fn mode_switch_loop_preserves_orthogonal_state() {
        use crate::core::mode_state::SerializableMode;
        let (store, _g) = isolated_store();
        let sid = "s-loop";

        // 起始默认 Yolo,挂满正交状态
        assert_eq!(store.mode_state(sid).mode, SerializableMode::Yolo);
        store.set_pinvou_review(sid, true);
        store.set_mounted_collection(sid, Some(42));
        store.set_active_persona(sid, Some("expert-x".into()));
        store.bind_skill(
            sid,
            ActiveSkillBinding {
                name: "legacy-ppt-workflow".into(),
                pending_instruction: None,
                phases: vec![],
                project_dir: None,
            },
        );

        // 闭环往返两轮:Yolo →(set_plan_mode_next)→ Plan →(accept/exit)→ Yolo
        for _ in 0..2 {
            store
                .set_mode(sid, SerializableMode::Plan)
                .expect("set chat plan mode");
            assert_eq!(store.mode_state(sid).mode, SerializableMode::Plan);
            store
                .set_mode(sid, SerializableMode::Yolo)
                .expect("set chat yolo mode");
            assert_eq!(store.mode_state(sid).mode, SerializableMode::Yolo);
        }

        // 四个正交字段全保留
        let st = store.mode_state(sid);
        assert!(st.pinvou_review_enabled, "切 mode 清了品悟开关");
        assert_eq!(st.mounted_collection, Some(42), "切 mode 卸载了知识集");
        assert_eq!(
            st.active_persona.as_deref(),
            Some("expert-x"),
            "切 mode 清了人格"
        );
        assert_eq!(
            st.active_skill.map(|s| s.name),
            Some("legacy-ppt-workflow".to_string()),
            "切 mode 解绑了 skill"
        );
    }

    // ===================== code 会话权限模式（两层持久化 + 默认值解析）=====================

    /// 注入一个简易 code 会话判定：列表内的 id 视为品悟原生 code 会话。
    fn with_code_sessions(store: &SessionStore, ids: &[&str]) {
        let owned: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        store.set_code_session_predicate(Arc::new(move |id: &str| {
            owned.iter().any(|candidate| candidate == id)
        }));
    }

    #[test]
    fn code_session_first_use_defaults_to_plan() {
        let (store, _g) = isolated_store();
        with_code_sessions(&store, &["code-1"]);
        // 从未用过 code 模式（无 per-session 记录、全局 last_mode=None）→ Plan 只读。
        assert_eq!(store.mode_state("code-1").mode, SerializableMode::Plan);
        // plain 会话维持 Yolo 现状。
        assert_eq!(store.mode_state("plain-1").mode, SerializableMode::Yolo);
    }

    /// 谓词未注入时（启动早期/测试）全部按 plain 语义，不误判。拆成独立测试：
    /// `isolated_store` 持有进程级 `ENV_LOCK` 直到 guard drop，同一线程内二次调用
    /// 会自死锁（`std::sync::Mutex` 不可重入）。每测试只调一次 `isolated_store`。
    #[test]
    fn code_session_without_predicate_defaults_to_yolo() {
        let (no_predicate, _g) = isolated_store();
        assert_eq!(
            no_predicate.mode_state("code-1").mode,
            SerializableMode::Yolo
        );
    }

    #[test]
    fn code_session_default_follows_global_last_mode() {
        let (store, _g) = isolated_store();
        with_code_sessions(&store, &["code-1", "code-2"]);
        // 在 code-1 显式切 yolo → 全局 last_mode=yolo → 新 code 会话默认跟随。
        store
            .set_mode("code-1", SerializableMode::Yolo)
            .expect("switch yolo");
        assert_eq!(store.mode_state("code-2").mode, SerializableMode::Yolo);
        // 再切回 plan → 新 code 会话默认跟随 plan；code-1 保持自己的显式值。
        store
            .set_mode("code-1", SerializableMode::Plan)
            .expect("switch plan");
        assert_eq!(store.mode_state("code-2").mode, SerializableMode::Plan);
        assert_eq!(store.mode_state("code-1").mode, SerializableMode::Plan);
    }

    #[test]
    fn code_mode_persists_per_session_across_restart() {
        let (store, _g) = isolated_store();
        with_code_sessions(&store, &["code-1", "code-2", "code-3"]);
        store
            .set_mode("code-1", SerializableMode::Yolo)
            .expect("code-1 yolo");
        store
            .set_mode("code-2", SerializableMode::Plan)
            .expect("code-2 plan");
        // sidecar 只存 code 会话的显式 mode。
        let file = paths::sessions_root().join("_code_mode_states.json");
        let on_disk: HashMap<String, SerializableMode> =
            serde_json::from_str(&std::fs::read_to_string(&file).expect("read sidecar"))
                .expect("parse sidecar");
        assert_eq!(on_disk.len(), 2);
        assert_eq!(on_disk.get("code-1"), Some(&SerializableMode::Yolo));
        assert_eq!(on_disk.get("code-2"), Some(&SerializableMode::Plan));

        // 重启：per-session 恢复各自上次的 mode（code-1 的 yolo 不被全局
        // last_mode=plan 盖掉），新 code 会话回落全局默认。
        let reopened = reopen_store(&store).expect("reboot");
        with_code_sessions(&reopened, &["code-1", "code-2", "code-3"]);
        assert_eq!(reopened.mode_state("code-1").mode, SerializableMode::Yolo);
        assert_eq!(reopened.mode_state("code-2").mode, SerializableMode::Plan);
        assert_eq!(reopened.mode_state("code-3").mode, SerializableMode::Plan);

        // 删除会话清理 per-session 持久化条目。
        reopened.delete("code-1").expect("delete code-1");
        let on_disk: HashMap<String, SerializableMode> =
            serde_json::from_str(&std::fs::read_to_string(&file).expect("read sidecar"))
                .expect("parse sidecar");
        assert!(!on_disk.contains_key("code-1"));
        assert_eq!(on_disk.get("code-2"), Some(&SerializableMode::Plan));
    }

    #[test]
    fn plain_session_mode_is_not_persisted() {
        let (store, _g) = isolated_store();
        with_code_sessions(&store, &["code-1"]);
        store
            .set_mode("plain-1", SerializableMode::Plan)
            .expect("plain plan");
        assert_eq!(store.mode_state("plain-1").mode, SerializableMode::Plan);
        // plain 不写 sidecar、不动全局键。
        assert!(!paths::sessions_root()
            .join("_code_mode_states.json")
            .exists());
        assert!(store.code_permission_prefs().last_mode.is_none());
        // 重启后 plain 回 Yolo（现状：mode 仅驻内存、默认 Yolo）。
        let reopened = reopen_store(&store).expect("reboot");
        assert_eq!(reopened.mode_state("plain-1").mode, SerializableMode::Yolo);
    }

    #[test]
    fn confirm_code_yolo_persists_globally() {
        let (store, _g) = isolated_store();
        assert!(!store.code_permission_prefs().yolo_confirmed);
        let prefs = store.confirm_code_yolo().expect("confirm yolo");
        assert!(prefs.yolo_confirmed);
        assert!(store.code_permission_prefs().yolo_confirmed);
        // 落盘 settings.json；重启后内存镜像仍记得。
        assert!(UserPrefs::load().code_permission.yolo_confirmed);
        let reopened = reopen_store(&store).expect("reboot");
        assert!(reopened.code_permission_prefs().yolo_confirmed);
    }

    /// 重启回归：绑过 skill 但从未显式切 mode 的 code 会话，重启后必须回到 Plan
    /// 首启默认，不能被 `load_skill_bindings` 启动期物化的 `or_default()`(=Yolo)
    /// 条目盖掉。谓词在启动后才注入，`load_skill_bindings` 当时无法判 code 会话，
    /// 故在 `set_code_session_predicate` 注入后做一次 reconcile 修正这类残留。
    #[test]
    fn code_session_with_skill_binding_defaults_to_plan_after_reboot() {
        // 绑 skill 的 code 会话重启后回 Plan，不被 Yolo 残留盖掉。
        let (store, _g) = isolated_store();
        // code-1 绑定 skill（谓词未注入 → 等同 `load_skill_bindings` 启动期物化
        // 出 mode=Yolo 的条目），落盘后从不显式切 mode（无 per-session 记录）。
        store.bind_skill(
            "code-1",
            ActiveSkillBinding {
                name: "demo".into(),
                pending_instruction: None,
                phases: vec![],
                project_dir: None,
            },
        );
        store.save_skill_bindings();
        assert!(store.mode_state("code-1").active_skill.is_some());

        // 重启：load_skill_bindings 恢复出 mode=Yolo + active_skill，
        // load_code_mode_states 因无 code-1 条目不覆盖。
        let reopened = reopen_store(&store).expect("reboot");
        // 谓词注入触发 reconcile：无持久化记录的 code 会话 mode 修正为 Plan。
        with_code_sessions(&reopened, &["code-1"]);
        assert_eq!(
            reopened.mode_state("code-1").mode,
            SerializableMode::Plan,
            "绑 skill 的 code 会话重启后应回 Plan 首启默认，而非 Yolo 残留"
        );
        // active_skill 必须保留（reconcile 只修 mode，不动其他字段）。
        assert!(reopened.mode_state("code-1").active_skill.is_some());
    }

    /// reconcile 只修正无持久化记录的 code 会话；显式切过的 mode 必须原样保留。
    /// 拆成独立测试：`isolated_store` 持有进程级 ENV_LOCK 直到 guard drop，同一线程
    /// 内二次调用会自死锁（`std::sync::Mutex` 不可重入），每测试只调一次。
    #[test]
    fn reconcile_does_not_overwrite_explicitly_persisted_mode() {
        let (store, _g) = isolated_store();
        with_code_sessions(&store, &["code-2"]);
        store
            .set_mode("code-2", SerializableMode::Yolo)
            .expect("code-2 explicit yolo");
        let reopened = reopen_store(&store).expect("reboot");
        with_code_sessions(&reopened, &["code-2"]);
        assert_eq!(
            reopened.mode_state("code-2").mode,
            SerializableMode::Yolo,
            "显式切过的 mode 不应被 reconcile 改写"
        );
    }

    #[test]
    fn fresh_code_session_default_plan_registers_pending_plan() {
        let (store, _g) = isolated_store();
        with_code_sessions(&store, &["code-1"]);
        // 首次使用（默认值经解析得到 Plan、尚无内存条目）时出方案必须能登记，
        // 不能被 entry or_default 物化成 Yolo 而静默丢失 Plan 语义。
        let registered = store
            .register_pending_plan("code-1", "plan-1".to_string())
            .expect("register plan on fresh code session");
        assert_eq!(registered.mode, SerializableMode::Plan);
        assert_eq!(registered.pending_plan_id.as_deref(), Some("plan-1"));
    }

    #[test]
    fn mounted_collections_are_ordered_deduplicated_and_legacy_compatible() {
        let (store, _g) = isolated_store();
        let sid = "s-multi-kb";
        store.set_mounted_collections(
            sid,
            vec![
                MountedCollection {
                    collection_id: 7,
                    enabled: true,
                },
                MountedCollection {
                    collection_id: 7,
                    enabled: false,
                },
                MountedCollection {
                    collection_id: 8,
                    enabled: false,
                },
                MountedCollection {
                    collection_id: -1,
                    enabled: true,
                },
            ],
        );
        assert_eq!(
            store.mounted_collections(sid),
            vec![
                MountedCollection {
                    collection_id: 7,
                    enabled: true,
                },
                MountedCollection {
                    collection_id: 8,
                    enabled: false,
                },
            ]
        );
        assert_eq!(store.mounted_collection_ids(sid), vec![7]);
        assert_eq!(store.mounted_collection(sid), Some(7));

        store.set_mounted_collection(sid, Some(42));
        assert_eq!(
            store.mounted_collections(sid),
            vec![MountedCollection {
                collection_id: 42,
                enabled: true,
            }]
        );
    }

    #[test]
    fn mounted_collection_item_updates_merge_across_concurrent_clients() {
        let (store, _g) = isolated_store();
        let sid = "s-concurrent-multi-kb";
        store.set_mounted_collection(sid, Some(7));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));

        let add_store = store.clone();
        let add_barrier = barrier.clone();
        let add = std::thread::spawn(move || {
            add_barrier.wait();
            add_store.add_mounted_collection(sid, 8);
        });
        let disable_store = store.clone();
        let disable_barrier = barrier.clone();
        let disable = std::thread::spawn(move || {
            disable_barrier.wait();
            disable_store.set_mounted_collection_enabled(sid, 7, false);
        });
        barrier.wait();
        add.join().unwrap();
        disable.join().unwrap();

        assert_eq!(
            store.mounted_collections(sid),
            vec![
                MountedCollection {
                    collection_id: 7,
                    enabled: false,
                },
                MountedCollection {
                    collection_id: 8,
                    enabled: true,
                },
            ],
        );
        assert_eq!(store.mounted_collection(sid), Some(8));
    }

    #[test]
    fn deleting_collection_removes_mount_from_every_affected_session() {
        let (store, _g) = isolated_store();
        store.set_mounted_collections(
            "session-a",
            vec![
                MountedCollection {
                    collection_id: 7,
                    enabled: true,
                },
                MountedCollection {
                    collection_id: 8,
                    enabled: false,
                },
            ],
        );
        store.set_mounted_collections(
            "session-b",
            vec![
                MountedCollection {
                    collection_id: 9,
                    enabled: true,
                },
                MountedCollection {
                    collection_id: 7,
                    enabled: false,
                },
            ],
        );
        store.set_mounted_collection("session-legacy", Some(7));
        store.set_mounted_collection("session-unaffected", Some(9));
        let unaffected_revision = store
            .mounted_collections_snapshot("session-unaffected")
            .revision;

        let changed = store.remove_mounted_collection_from_all(7);

        assert_eq!(
            changed
                .iter()
                .map(|(session_id, _)| session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["session-a", "session-b", "session-legacy"]
        );
        assert_eq!(
            store.mounted_collections("session-a"),
            vec![MountedCollection {
                collection_id: 8,
                enabled: false,
            }]
        );
        assert_eq!(
            store.mounted_collections("session-b"),
            vec![MountedCollection {
                collection_id: 9,
                enabled: true,
            }]
        );
        assert!(store.mounted_collections("session-legacy").is_empty());
        assert_eq!(
            store
                .mounted_collections_snapshot("session-unaffected")
                .revision,
            unaffected_revision,
            "unaffected sessions must not receive a spurious revision"
        );
    }

    #[test]
    fn bind_skill_then_take_consumes_once_and_returns_binding() {
        let (store, _g) = isolated_store();
        store.bind_skill(
            "s1",
            ActiveSkillBinding {
                name: "legacy-ppt-workflow".into(),
                pending_instruction: Some("PREPEND".into()),
                phases: vec![],
                project_dir: None,
            },
        );
        let b = store.active_skill("s1").expect("bound");
        assert_eq!(b.name, "legacy-ppt-workflow");
        // pending_instruction 被 #[serde(skip)] 标记,active_skill 路径走的是
        // .clone() 不影响 pending_instruction(它仍存在原 entry 上),take 走另一路径
        assert_eq!(
            store.take_pending_skill_instruction("s1").as_deref(),
            Some("PREPEND")
        );
        assert!(store.take_pending_skill_instruction("s1").is_none());
        // 取走 instruction 后 active_skill 仍能返回 binding(name+phases),
        // 仅 pending_instruction 槽位被消费 — 关键:phases 不丢
        let b2 = store.active_skill("s1").expect("still bound");
        assert_eq!(b2.name, "legacy-ppt-workflow");
    }

    #[test]
    fn pending_turn_injections_restore_on_drop_and_commit_only_after_submission() {
        let (store, _g) = isolated_store();
        store.bind_skill(
            "s1",
            ActiveSkillBinding {
                name: "skill-a".into(),
                pending_instruction: Some("SKILL BODY".into()),
                phases: vec![],
                project_dir: None,
            },
        );
        store.set_active_persona("s1", Some("persona-a".into()));
        store.set_pending_persona_body("s1", Some("PERSONA BODY".into()));

        {
            let pending = store.take_pending_turn_injections("s1");
            assert_eq!(pending.skill_instruction(), Some("SKILL BODY"));
            assert_eq!(pending.persona_body(), Some("PERSONA BODY"));
            assert!(store.take_pending_skill_instruction("s1").is_none());
            assert!(store.take_pending_persona_body("s1").is_none());
            // Simulate attachment/build/Engine submission failure.
        }
        assert_eq!(
            store.take_pending_skill_instruction("s1").as_deref(),
            Some("SKILL BODY")
        );
        assert_eq!(
            store.take_pending_persona_body("s1").as_deref(),
            Some("PERSONA BODY")
        );

        store.bind_skill(
            "s1",
            ActiveSkillBinding {
                name: "skill-a".into(),
                pending_instruction: Some("SECOND SKILL".into()),
                phases: vec![],
                project_dir: None,
            },
        );
        store.set_pending_persona_body("s1", Some("SECOND PERSONA".into()));
        store.take_pending_turn_injections("s1").commit();
        assert!(store.take_pending_skill_instruction("s1").is_none());
        assert!(store.take_pending_persona_body("s1").is_none());
    }

    #[test]
    fn unbind_skill_clears_binding() {
        let (store, _g) = isolated_store();
        store.bind_skill(
            "s1",
            ActiveSkillBinding {
                name: "legacy-ppt-workflow".into(),
                pending_instruction: None,
                phases: vec![],
                project_dir: None,
            },
        );
        assert!(store.active_skill("s1").is_some());
        store.unbind_skill("s1");
        assert!(store.active_skill("s1").is_none());
    }

    #[test]
    fn bind_skill_preserves_mode() {
        // 绑定/解绑 skill 不能动 mode / pinvou_review_enabled。
        let (store, _g) = isolated_store();
        store.set_pinvou_review("s1", true);
        store
            .set_mode("s1", crate::core::mode_state::SerializableMode::Plan)
            .expect("set chat plan mode");
        store.bind_skill(
            "s1",
            ActiveSkillBinding {
                name: "legacy-ppt-workflow".into(),
                pending_instruction: None,
                phases: vec![],
                project_dir: None,
            },
        );
        let state = store.mode_state("s1");
        assert!(state.pinvou_review_enabled);
        assert!(matches!(
            state.mode,
            crate::core::mode_state::SerializableMode::Plan
        ));
        store.unbind_skill("s1");
        let state2 = store.mode_state("s1");
        assert!(state2.pinvou_review_enabled);
        assert!(matches!(
            state2.mode,
            crate::core::mode_state::SerializableMode::Plan
        ));
    }

    /// 多智能体对话进入会话列表，标题就是用户诉求。
    ///
    /// 这是「工作流运行」术语（docs/multiagent/glossary.md）在存储层的落点：
    /// 宿主会话是运行的入口与档案，历史、重开、删除全部复用会话列表交互。

    /// 工作流运行的工作区由 run id 派生，不落在 sessions/ 下。

    #[test]
    fn session_model_update_rolls_back_memory_when_sidecar_write_fails() {
        let (store, _guard) = isolated_store();
        store
            .set_session_model_id("wf-model-test", Some("old-model".to_string()))
            .expect("persist initial model");
        let sidecar = paths::sessions_root().join("_session_models.json");
        std::fs::remove_file(&sidecar).expect("remove initial sidecar");
        std::fs::create_dir(&sidecar).expect("block sidecar path with a directory");

        let error = store
            .set_session_model_id("wf-model-test", Some("new-model".to_string()))
            .expect_err("an unwritable sidecar must fail the model transaction");

        assert!(error
            .to_string()
            .contains("persist per-session model bindings"));
        assert_eq!(
            store.session_model_override("wf-model-test").as_deref(),
            Some("old-model"),
            "failed persistence must not leave a memory-only model choice"
        );
    }
}
