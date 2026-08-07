//! Session store CRUD and lifecycle.
//!
//! [`SessionStore`] is the central facade value: every field is `Arc`-wrapped
//! so the whole store clones cheaply into Tauri State and is shared across
//! background tasks. The struct definition itself lives in [`super`] (the
//! facade), while this module owns the conversational CRUD and engine-state
//! persistence entry points. Retention, mode state, sidecars, and the
//! scheduled-profile registry are split into their own sibling modules.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use deepseek_tui::artifacts::{ArtifactKind, ArtifactRecord};
use deepseek_tui::models::Message;
use deepseek_tui::session_manager::{
    create_saved_session_with_id_and_mode, SavedSession, SessionManager, SessionMetadata,
};
use parking_lot::{Mutex, RwLock};

use crate::platform::paths;

use super::scheduled::{
    ChatEngineState, ScheduledEngineState, ScheduledRunProfile, ScheduledTokenAccounting,
};
use super::transcript::{looks_like_truncating_overwrite, transcript_revision};
use super::validators::{
    generate_session_id, persisted_system_prompt, validate_scheduled_session_id,
    validate_session_id,
};
use super::{
    session_roots_for, ExecutionRootResolver, MountedCollection, MountedCollectionsSnapshot,
    SessionKind, SessionRoots, SessionStore,
};

/// Cap on the number of ordinary chat sessions retained on disk before the
/// oldest is evicted by [`super::retention::SessionStore::enforce_session_retention_locked`].
pub(crate) const MAX_SESSIONS_PER_KIND: usize = 50;

impl SessionStore {
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
        store.load_session_models();
        store.load_pinned_sessions();
        store.load_hidden_sessions();
        {
            let _mutation = store.scheduled_mutation.lock();
            store.enforce_session_retention_locked()?;
        }
        store.purge_all_scheduled_side_maps();
        Ok(store)
    }

    #[cfg(test)]
    pub(crate) fn boot_with_scheduled_root(scheduled_root: PathBuf) -> Result<Self> {
        let store = Self::from_paths(
            paths::sessions_root(),
            paths::scheduled_run_profiles_path(),
            scheduled_root,
        )?;
        store.load_skill_bindings();
        store.load_session_models();
        store.load_pinned_sessions();
        store.load_hidden_sessions();
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
        let store = Self {
            manager: Arc::new(manager),
            scheduled_profiles: Arc::new(RwLock::new(HashMap::new())),
            scheduled_profiles_path: Arc::new(scheduled_profiles_path),
            scheduled_root: Arc::new(scheduled_root),
            scheduled_mutation: Arc::new(Mutex::new(())),
            active: Arc::new(RwLock::new(None)),
            mode_states: Arc::new(RwLock::new(HashMap::new())),
            session_models: Arc::new(RwLock::new(HashMap::new())),
            pinned_sessions: Arc::new(RwLock::new(HashMap::new())),
            hidden_sessions: Arc::new(RwLock::new(HashMap::new())),
            execution_root_resolver: Arc::new(RwLock::new(None)),
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
        let _mutation = self.scheduled_mutation.lock();
        if self.is_scheduled_session(&session.metadata.id)? {
            return self.persist_then_reconcile(session, "committed save");
        }
        self.persist_then_reconcile(session, "session save")
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
        self.mode_states.write().remove(id);
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

    /// 注入原生代码会话的执行根解析器;由 app 组合根在 AcpPool 就绪后调用一次。
    /// 与 Engine bridge 共用同一份 `SessionAgentStore` 闭包,两侧解析结果一致。
    pub fn set_execution_root_resolver(&self, resolver: ExecutionRootResolver) {
        *self.execution_root_resolver.write() = Some(resolver);
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

        self.persist_then_reconcile_with(
            &session,
            || format!("persist scheduled engine state for {id}"),
            "committed engine state save",
        )?;
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

        self.persist_then_reconcile_with(
            &session,
            || format!("persist chat engine state for {id}"),
            "committed engine state save",
        )?;
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
        self.persist_then_reconcile_with(
            &session,
            || format!("persist admitted chat display for {id}"),
            "admitted display save",
        )?;
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

        self.persist_then_reconcile_with(
            &session,
            || format!("persist scheduled token total for {id}"),
            "committed token save",
        )?;
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
        self.reconcile_retention("committed create");
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
        self.persist_then_reconcile(&session, "title update")?;
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
        self.persist_then_reconcile(&session, "activity update")?;
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
        self.save(&session)?;
        // per-session 模型:新建会话继承全局默认(active)模型 id,落盘记住。
        if let Some(mid) = model_id {
            self.set_session_model_id(&id, Some(mid))?;
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
        self.persist_then_reconcile(&session, "transcript update")?;
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
        self.persist_then_reconcile(&session, "transcript CAS")?;
        Ok(next_revision)
    }

    /// 替换 session 的产物列表。前端跟踪 write_file / append_file 工具调用积累的 paths,
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
        self.persist_then_reconcile(&session, "artifact update")?;
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
        self.persist_then_reconcile_with(
            &session,
            || format!("persist scheduled artifact for {id}"),
            "committed artifact append",
        )?;
        Ok(())
    }

    pub fn active_id(&self) -> Option<String> {
        self.active.read().clone()
    }

    pub fn set_active(&self, id: Option<String>) {
        *self.active.write() = id;
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
        let mut states = self.mode_states.write();
        let state = states.entry(id.to_string()).or_default();
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
}
