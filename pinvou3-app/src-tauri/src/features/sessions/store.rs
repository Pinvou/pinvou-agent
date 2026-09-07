//! Session store CRUD and lifecycle.
//!
//! [`SessionStore`] is the central facade value: every field is `Arc`-wrapped
//! so the whole store clones cheaply into Tauri State and is shared across
//! background tasks. The struct definition itself lives in [`super`] (the
//! facade), while this module owns the conversational CRUD and engine-state
//! persistence entry points. Retention, mode state, sidecars, and the
//! scheduled-profile registry are split into their own sibling modules.

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(test)]
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use deepseek_tui::artifacts::{ArtifactKind, ArtifactRecord};
use deepseek_tui::models::Message;
use deepseek_tui::session_manager::{
    SavedSession, SessionManager, SessionMetadata, create_saved_session_with_id_and_mode,
};
use parking_lot::{Mutex, RwLock};

use crate::platform::paths;

use super::scheduled::ChatEngineState;
use super::transcript::{looks_like_truncating_overwrite, transcript_revision};
use super::validators::{generate_session_id, persisted_system_prompt, validate_session_id};
use super::{
    CodeSessionPredicate, ExecutionRootResolver, SessionDeletedHook, SessionKind,
    SessionPurgedHook, SessionRoots, SessionStore, session_roots_for,
};
use crate::core::mode_state::SerializableMode;
use crate::platform::prefs::UserPrefs;

#[cfg(test)]
static POST_RECORD_DELETE_FAULTS: LazyLock<Mutex<HashMap<String, ErrorKind>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Cap on the number of ordinary chat sessions retained on disk before the
/// oldest is evicted by [`super::retention::SessionStore::enforce_session_retention_locked`].
pub(crate) const MAX_SESSIONS_PER_KIND: usize = 50;

impl SessionStore {
    /// Repair persisted tool histories only at process boot, before any
    /// session engine can own an in-flight tool call. Runtime reads use the
    /// snapshot API and must never infer a crash from a dangling `tool_use`.
    fn recover_interrupted_tool_histories_locked(&self) -> Result<usize> {
        let sessions = self
            .list_sessions_cached()
            .context("list sessions for tool history recovery")?
            .as_ref()
            .clone();
        let mut recovered = 0usize;
        for metadata in sessions {
            let recovery = match self.manager.recover_session_for_resume(&metadata.id) {
                Ok(recovery) => recovery,
                Err(error) => {
                    eprintln!(
                        "[sessions] skip tool history recovery for {}: {error}",
                        metadata.id
                    );
                    continue;
                }
            };
            if !recovery.changed {
                continue;
            }
            if let Err(error) = self.save_session_atomic(&recovery.session) {
                eprintln!(
                    "[sessions] persist tool history recovery for {} failed: {error:#}",
                    metadata.id
                );
                continue;
            }
            recovered = recovered.saturating_add(1);
            eprintln!(
                "[sessions] recovered interrupted tool history for {}: repaired={} duplicate={} orphan={}",
                metadata.id,
                recovery.repaired_call_count,
                recovery.duplicate_result_count,
                recovery.orphan_result_count,
            );
        }
        Ok(recovered)
    }

    /// Open `~/.pinvou3/sessions/` without inferring that a live tool call
    /// crashed. This constructor is safe for secondary stores opened while the
    /// application process is already running.
    pub fn boot() -> Result<Self> {
        Self::boot_inner(false)
    }

    /// Open the process-owned session store and recover tool histories left
    /// incomplete by a previous process, before any Engine is started.
    pub fn boot_for_process_startup() -> Result<Self> {
        Self::boot_inner(true)
    }

    fn boot_inner(recover_interrupted_tools: bool) -> Result<Self> {
        let store = Self::from_paths(
            paths::sessions_root(),
            paths::scheduled_run_profiles_path(),
            paths::scheduled_tasks_root(),
        )?;
        // Sidecars historically load later in the Tauri setup hook. Loading
        // them here too lets reconciliation discard scheduled-only runtime
        // state immediately instead of resurrecting it after stale profiles
        // have already been removed.
        store.load_multi_agent_flags();
        store.load_session_models();
        store.load_pinned_sessions();
        store.load_hidden_sessions();
        store.load_session_mode_states();
        store.migrate_legacy_session_workspaces();
        {
            let _mutation = store.scheduled_mutation.lock();
            if recover_interrupted_tools {
                store.recover_interrupted_tool_histories_locked()?;
            }
            store.enforce_session_retention_locked()?;
        }
        store.purge_all_scheduled_side_maps();
        Ok(store)
    }

    pub(crate) fn boot_at_test_dir(root: &std::path::Path) -> Result<Self> {
        Self::from_paths(
            root.join("sessions"),
            root.join("scheduled-run-profiles.json"),
            root.join("scheduled"),
        )
    }

    pub(crate) fn boot_with_scheduled_root(scheduled_root: PathBuf) -> Result<Self> {
        let store = Self::from_paths(
            paths::sessions_root(),
            paths::scheduled_run_profiles_path(),
            scheduled_root,
        )?;
        store.load_multi_agent_flags();
        store.load_session_models();
        store.load_pinned_sessions();
        store.load_hidden_sessions();
        store.load_session_mode_states();
        {
            let _mutation = store.scheduled_mutation.lock();
            store.enforce_session_retention_locked()?;
        }
        store.purge_all_scheduled_side_maps();
        Ok(store)
    }

    pub(crate) fn from_paths(
        sessions_dir: PathBuf,
        scheduled_profiles_path: PathBuf,
        scheduled_root: PathBuf,
    ) -> Result<Self> {
        let manager = SessionManager::new(sessions_dir.clone())
            .with_context(|| format!("SessionManager::new({}) failed", sessions_dir.display()))?;
        let prefs_snapshot = UserPrefs::load();
        let store = Self {
            manager: Arc::new(manager),
            scheduled_profiles: Arc::new(RwLock::new(HashMap::new())),
            scheduled_profiles_path: Arc::new(scheduled_profiles_path),
            scheduled_root: Arc::new(scheduled_root),
            scheduled_mutation: Arc::new(Mutex::new(())),
            active: Arc::new(RwLock::new(None)),
            mode_states: Arc::new(RwLock::new(HashMap::new())),
            multi_agent_flags_io: Arc::new(Mutex::new(())),
            list_cache: Arc::new(RwLock::new(None)),
            list_cache_generation: Arc::new(AtomicU64::new(0)),
            session_models: Arc::new(RwLock::new(HashMap::new())),
            pinned_sessions: Arc::new(RwLock::new(HashMap::new())),
            hidden_sessions: Arc::new(RwLock::new(HashMap::new())),
            execution_root_resolver: Arc::new(RwLock::new(None)),
            session_workspaces: Arc::new(RwLock::new(HashMap::new())),
            code_session_predicate: Arc::new(RwLock::new(None)),
            session_mode_states: Arc::new(RwLock::new(HashMap::new())),
            code_permission: Arc::new(RwLock::new(prefs_snapshot.code_permission)),
            mode_defaults: Arc::new(RwLock::new(prefs_snapshot.mode_defaults)),
            session_purged_hooks: Arc::new(RwLock::new(Vec::new())),
            session_deleted_hooks: Arc::new(RwLock::new(Vec::new())),
        };
        store.load_scheduled_profiles()?;
        store.reconcile_scheduled_profiles_locked()?;
        Ok(store)
    }

    /// 上游 `manager.list_sessions()` 的缓存读取:首访全目录扫描后缓存
    /// `Arc<Vec<SessionMetadata>>`,后续 list 共享同一快照。失效点在 App 侧
    /// 唯一写路径(`save_session_atomic`/`delete`),所以缓存与盘面一致的
    /// 前提是「所有会话 JSON 都经 SessionStore 写入」——当前属实(save/
    /// set_title/touch_activity/create_new 全走 save_session_atomic)。
    /// 返回 `Arc` 让调用方(如 AcpPool 启动扫描)零拷贝消费。
    ///
    /// 回填带代数守卫:miss 扫描期间若发生写(失效自增了代数),该扫描结果
    /// 丢弃重扫——否则写前启动的慢扫描会用旧目录视图覆盖写后快照,陈旧
    /// 列表(如重命名后的标题)会驻留到下一次任意写才恢复。并发 miss 的
    /// 重复扫描良性(幂等读),不值得再加装载互斥。
    pub(crate) fn list_sessions_cached(&self) -> std::io::Result<Arc<Vec<SessionMetadata>>> {
        let generation_now = self.list_cache_generation.load(Ordering::Acquire);
        loop {
            if let Some((generation, cached)) = self.list_cache.read().clone() {
                if generation == generation_now {
                    return Ok(cached);
                }
                // 过期代数条目:等待的写方尚未清槽或守卫失效时被落地,击穿
                // 重扫,不能把写前视图当有效快照返回。
            }
            let generation_at_scan = self.list_cache_generation.load(Ordering::Acquire);
            let fresh = Arc::new(self.manager.list_sessions()?);
            let mut slot = self.list_cache.write();
            if self.list_cache_generation.load(Ordering::Acquire) == generation_at_scan {
                // 扫描期间无写:安全回填。写锁保证只有一个 miss 竞争者落地,
                // 后到者走到顶部已能命中(或带着更新的代数再扫一轮)。
                *slot = Some((generation_at_scan, Arc::clone(&fresh)));
                return Ok(fresh);
            }
            // 扫描期间发生过写:丢弃本次结果,重扫。连续写活跃时最多重扫
            // 到写间歇,与无缓存时的每 list 现扫同阶,不会活锁。
        }
    }

    pub(crate) fn invalidate_list_cache(&self) {
        *self.list_cache.write() = None;
        self.list_cache_generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn list(&self) -> Result<Vec<SessionMetadata>> {
        let mut out = self
            .list_sessions_cached()
            .context("list_sessions failed")?
            .as_ref()
            .clone();
        // Scheduled conversations share the durable store so detail/history can
        // load them normally, but remain owned by the Scheduled Tasks surface.
        // 多智能体是普通会话的持久开关，不是独立会话类型；这里只隔离定时
        // 会话，其余历史统一进入普通列表。
        // benchmark 构建中,评测会话(eval_ 前缀,含 GAIA 私有题目)不进用户历史:
        // 正常路径由评测运行器清理,崩溃残留也不能把私密题目带进会话列表。
        // 默认桌面构建不保留这项前缀语义,避免 benchmark 未启用时改变普通会话列表。
        out.retain(|metadata| !metadata.id.starts_with("sched-"));
        #[cfg(feature = "benchmark-hooks")]
        out.retain(|metadata| !metadata.id.starts_with("eval_"));
        out.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
        Ok(out)
    }

    pub fn load(&self, id: &str) -> Result<SavedSession> {
        self.manager
            .load_session_snapshot(id)
            .with_context(|| format!("load_session({id})"))
    }

    pub(crate) fn persisted_size(&self, id: &str) -> Result<u64> {
        validate_session_id(id)?;
        let path = self.manager.sessions_dir().join(format!("{id}.json"));
        std::fs::metadata(&path)
            .with_context(|| format!("read Session metadata {}", path.display()))
            .map(|metadata| metadata.len())
    }

    pub fn save(&self, session: &SavedSession) -> Result<PathBuf> {
        let _mutation = self.scheduled_mutation.lock();
        if self.is_scheduled_session(&session.metadata.id)? {
            return self.persist_then_reconcile(session, "committed save");
        }
        self.persist_then_reconcile(session, "session save")
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        if self.is_scheduled_session(id)? {
            bail!("Scheduled-run sessions are deleted through their automation");
        }
        // 上游 delete_session 先删会话 JSON 再清目录:目录清理失败时 JSON 已
        // 不在盘上但错误会向上传播——按「已发起删除即可能变更盘面」失效快照,
        // 不能等走到 match 之后的统一失效(Err 提前 return 会跳过它)。
        self.invalidate_list_cache();
        let (committed, delete_result) = self.delete_session_record(id);
        if committed {
            // A later workspace/artifact cleanup error does not roll back the
            // durable record removal. Purge store/process side maps now so an
            // error return cannot strand an active id, model binding or turn
            // state forever when the caller never retries.
            self.purge_session_side_maps(&[id.to_string()]);
            // 回退备份 sidecar 同样随会话清理（best-effort，见其实现注释）。
            Self::purge_rewound_turns_backups(&[id.to_string()]);
        }
        match delete_result {
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
        Ok(())
    }

    /// Delete the durable session record and report whether that deletion
    /// committed, independently from later workspace/artifact cleanup.
    ///
    /// `SessionManager::delete_session` removes `<id>.json` before recursively
    /// deleting `<id>/`. It can therefore return an error after the durable
    /// record is already gone. In that partial-commit state we must publish the
    /// durable-deletion hook while preserving the original cleanup error for the
    /// caller. Validation plus a direct metadata lookup makes the check
    /// fail-closed: an invalid id or an unreadable path is never interpreted as
    /// a committed deletion.
    pub(crate) fn delete_session_record(&self, id: &str) -> (bool, std::io::Result<()>) {
        let result = self.invoke_session_manager_delete(id);
        let committed = result.is_ok() || self.durable_session_record_is_absent(id);
        if committed {
            self.notify_session_deleted(id);
        }
        (committed, result)
    }

    pub(super) fn durable_session_record_is_absent(&self, id: &str) -> bool {
        if validate_session_id(id).is_err() {
            return false;
        }
        let record = self.manager.sessions_dir().join(format!("{id}.json"));
        matches!(
            std::fs::metadata(record),
            Err(error) if error.kind() == ErrorKind::NotFound
        )
    }

    fn invoke_session_manager_delete(&self, id: &str) -> std::io::Result<()> {
        #[cfg(test)]
        if let Some(kind) = POST_RECORD_DELETE_FAULTS.lock().remove(id) {
            validate_session_id(id).map_err(|error| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string())
            })?;
            std::fs::remove_file(self.manager.sessions_dir().join(format!("{id}.json")))?;
            return Err(std::io::Error::new(
                kind,
                "injected cleanup failure after durable session deletion",
            ));
        }
        self.manager.delete_session(id)
    }

    #[cfg(test)]
    pub(crate) fn inject_post_record_delete_fault(&self, id: &str, kind: ErrorKind) -> Result<()> {
        validate_session_id(id)?;
        POST_RECORD_DELETE_FAULTS
            .lock()
            .insert(id.to_string(), kind);
        Ok(())
    }

    pub(crate) fn notify_session_deleted(&self, id: &str) {
        // Never execute extension code while holding the registry lock. A hook
        // may register another hook (or enter another lifecycle seam); cloning
        // the Arc list first prevents lock-order inversions and self-deadlock.
        let hooks = self.session_deleted_hooks.read().clone();
        for hook in hooks {
            hook(id);
        }
    }

    pub fn session_kind(&self, id: &str) -> Result<SessionKind> {
        if self.is_scheduled_session(id)? {
            Ok(SessionKind::ScheduledRun)
        } else {
            Ok(SessionKind::Chat)
        }
    }

    pub fn set_execution_root_resolver(&self, resolver: ExecutionRootResolver) {
        *self.execution_root_resolver.write() = Some(resolver);
    }

    pub fn set_code_session_predicate(&self, predicate: CodeSessionPredicate) {
        *self.code_session_predicate.write() = Some(predicate);
        self.reconcile_code_default_modes();
    }

    /// Register a session-purged hook (dependency inversion, see
    /// [`SessionPurgedHook`]). The app composition root registers the
    /// timing/pending_user_input cleanup once the pool is ready; store
    /// clones share the same Arc, so injection takes effect immediately.
    pub fn register_session_purged_hook(&self, hook: SessionPurgedHook) {
        self.session_purged_hooks.write().push(hook);
    }

    /// Register a durable-session-deleted hook. Runtime hooks are intentionally
    /// not backed by an ever-growing replay log: deletions that happen during
    /// store boot are recovered by the composition root's on-disk orphan
    /// reconciliation before normal producers start.
    pub fn register_session_deleted_hook(&self, hook: SessionDeletedHook) {
        self.session_deleted_hooks.write().push(hook);
    }

    /// Notifies all registered parties after a session is deleted from the
    /// store ([`SessionStore::delete`] and deep paths without an app handle
    /// such as retention policy/scheduled cleanup). Failures are silent
    /// (hook implementations own their idempotency) and must not block the
    /// deletion path; callers must fire this only after all store-side
    /// locks are released.
    pub(crate) fn notify_session_purged(&self, id: &str) {
        let hooks = self.session_purged_hooks.read().clone();
        for hook in hooks {
            hook(id);
        }
    }

    pub(crate) fn reconcile_code_default_modes(&self) {
        // 有显式 per-session 记录的 code 会话：交给 load_session_mode_states 覆盖，
        // 不在此处理。
        let persisted: HashSet<String> = self.session_mode_states.read().keys().cloned().collect();
        let mut m = self.mode_states.write();
        for (id, state) in m.iter_mut() {
            if state.mode == SerializableMode::Yolo
                && !persisted.contains(id)
                && (self.is_code_session(id) || self.session_workspace_binding(id).is_some())
            {
                state.mode = SerializableMode::Plan;
            }
        }
    }

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
        // resolver 只解析 codex_acp 原生代码会话的项目绑定;普通 chat 会话的
        // 用户工作目录绑定由 store 自持有的 sidecar 回退解析。
        let bound_project_root = self
            .execution_root_resolver
            .read()
            .as_ref()
            .and_then(|resolver| resolver(id))
            .or_else(|| self.session_workspace_binding(id));
        Ok(session_roots_for(id, bound_project_root))
    }

    pub fn ledger_root(&self, id: &str) -> Result<PathBuf> {
        Ok(self.session_roots(id)?.ledger)
    }

    pub fn set_title(&self, id: &str, title: String) -> Result<()> {
        // 标题和 transcript 存在同一个 JSON 中。定时会话生成期间 Engine 也会写这个
        // 文件，所以必须把 load / modify / save 放在同一把锁里；否则重命名可能把
        // Engine 刚落盘的新消息用旧快照覆盖掉。
        let _mutation = self.scheduled_mutation.lock();
        let mut session = self
            .manager
            .load_session_snapshot(id)
            .with_context(|| format!("load_session({id}) for title update"))?;
        session.metadata.title = title;
        self.persist_then_reconcile(&session, "title update")?;
        Ok(())
    }

    pub fn touch_activity(&self, id: &str) -> Result<()> {
        let _mutation = self.scheduled_mutation.lock();
        validate_session_id(id)?;
        let mut session = self
            .manager
            .load_session_snapshot(id)
            .with_context(|| format!("load_session({id}) for activity update"))?;
        session.metadata.updated_at = Utc::now();
        self.persist_then_reconcile(&session, "activity update")?;
        Ok(())
    }

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

    pub fn update_messages(&self, id: &str, messages: Vec<Message>) -> Result<()> {
        let _mutation = self.scheduled_mutation.lock();
        let mut session = self
            .manager
            .load_session_snapshot(id)
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
        self.persist_then_reconcile(&session, "transcript update")?;
        Ok(())
    }

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
            .load_session_snapshot(id)
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
        self.persist_then_reconcile(&session, "transcript CAS")?;
        Ok(next_revision)
    }

    pub fn update_artifacts(&self, id: &str, paths: Vec<String>) -> Result<()> {
        let _mutation = self.scheduled_mutation.lock();
        if self.is_scheduled_session(id)? {
            bail!("Cannot replace artifacts for scheduled-run session '{id}'");
        }
        let mut session = self
            .manager
            .load_session_snapshot(id)
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
        self.persist_then_reconcile(&session, "artifact update")?;
        Ok(())
    }

    pub fn active_id(&self) -> Option<String> {
        self.active.read().clone()
    }

    pub fn set_active(&self, id: Option<String>) {
        *self.active.write() = id;
    }

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
            .load_session_snapshot(id)
            .with_context(|| format!("load chat session {id} for engine persistence"))?;
        session.metadata.updated_at = Utc::now();
        session.metadata.message_count = state.messages.len();
        session.metadata.model = state.model;
        session.metadata.workspace = state.workspace;
        session.messages = state.messages;
        session.system_prompt = persisted_system_prompt(state.system_prompt.as_ref());

        self.persist_then_reconcile_with(
            &session,
            || format!("persist chat engine state for {id}"),
            "committed engine state save",
        )?;
        Ok(session)
    }

    /// 以调用方提供的 ID 创建空会话，供需要在启动前确定隔离 ID 的内部运行时使用。
    ///
    /// 普通 GUI 会话仍使用 [`Self::create_new`] 的随机 ID；这里不设置 active session。
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) fn create_empty_with_id(
        &self,
        id: String,
        model: String,
        model_id: Option<String>,
        workspace: PathBuf,
    ) -> Result<SavedSession> {
        let mut session = create_saved_session_with_id_and_mode(
            id.clone(),
            &[],
            &model,
            &workspace,
            0,
            None,
            None,
        );
        session.metadata.title = "临时评测".to_string();
        if let Some(model_id) = model_id {
            self.set_session_model_id(&id, Some(model_id))?;
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
            .load_session_snapshot(id)
            .with_context(|| format!("load chat session {id} for admitted display fallback"))?;
        if transcript_revision(&session.messages)? != expected_revision {
            return Ok(session);
        }
        if edit_last {
            // Use the engine's authoritative target selection. Unsupported
            // user content is a real boundary and must not fall through to an
            // older editable prompt.
            match deepseek_tui::edit_last_turn_target(&session.messages) {
                deepseek_tui::EditLastTurnTarget::Editable(index) => {
                    session.messages.truncate(index);
                }
                deepseek_tui::EditLastTurnTarget::Unsupported => {
                    anyhow::bail!(
                        "cannot persist edit fallback: latest user content is not editable"
                    );
                }
                deepseek_tui::EditLastTurnTarget::Missing => {
                    anyhow::bail!(
                        "cannot persist edit fallback: session has no user prompt to replace"
                    );
                }
            }
        }
        session.messages.push(display_message);
        session.metadata.message_count = session.messages.len();
        session.metadata.updated_at = Utc::now();
        self.persist_then_reconcile_with(
            &session,
            || format!("persist admitted chat display for {id}"),
            "admitted display save",
        )?;
        Ok(session)
    }
}
