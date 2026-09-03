//! Retention policy and scheduled-profile reconciliation for the session store.
//!
//! The retention surface owns three responsibilities that must stay together:
//!
//! 1. The Wave 1 persist-then-reconcile helpers
//!    ([`SessionStore::persist_then_reconcile`] /
//!    [`SessionStore::persist_then_reconcile_with`] /
//!    [`SessionStore::reconcile_retention`]) that collapse the shared
//!    "atomic save followed by best-effort retention cleanup" tail previously
//!    inlined by 13 public methods.
//! 2. [`SessionStore::enforce_session_retention_locked`], which keeps ordinary
//!    and scheduled histories in one directory without letting one class
//!    consume the other's retention budget.
//! 3. The scheduled-profile registry load / save / reconcile machinery that
//!    retention depends on, plus the runtime-sidecar purges.

use std::io::ErrorKind;
#[cfg(test)]
use std::{collections::HashMap, sync::LazyLock};

use anyhow::{Context, Result};

use super::SessionStore;
use super::scheduled::{SCHEDULED_PROFILE_SCHEMA_VERSION, ScheduledProfileRegistry};
use super::scheduled::{ScheduledEngineState, ScheduledRunProfile, ScheduledTokenAccounting};
use super::store::MAX_SESSIONS_PER_KIND;
use super::validators::validate_session_id;
use super::validators::{
    chat_session_file, scheduled_session_file, validate_scheduled_session_id,
    validate_scheduled_task_id, validate_scheduled_workspace_path,
};
use super::validators::{generate_session_id, persisted_system_prompt};
use anyhow::bail;
use chrono::Utc;
use deepseek_tui::artifacts::{ArtifactKind, ArtifactRecord};
use deepseek_tui::session_manager::create_saved_session_with_id_and_mode;
use deepseek_tui::session_manager::{SavedSession, SessionMetadata};
use std::path::PathBuf;

#[cfg(test)]
static SCHEDULED_RUNTIME_DELETE_FAULTS: LazyLock<parking_lot::Mutex<HashMap<String, ErrorKind>>> =
    LazyLock::new(|| parking_lot::Mutex::new(HashMap::new()));

impl SessionStore {
    pub(crate) fn save_session_atomic(&self, session: &SavedSession) -> Result<PathBuf> {
        validate_session_id(&session.metadata.id)?;
        let path = self
            .manager
            .sessions_dir()
            .join(format!("{}.json", session.metadata.id));
        let payload = serde_json::to_vec_pretty(session).context("serialize saved session")?;
        deepseek_tui::utils::write_atomic(&path, &payload)
            .with_context(|| format!("write session {}", path.display()))?;
        // 会话 JSON 落盘后列表快照即过期(标题/更新时间/新会话都可能变)
        self.invalidate_list_cache();
        Ok(path)
    }

    pub(crate) fn enforce_session_retention_locked(&self) -> Result<()> {
        let sessions = self
            .list_sessions_cached()
            .context("list sessions for retention")?
            .as_ref()
            .clone();
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
                let id = metadata.id;
                let (committed, result) = self.delete_session_record(&id);
                if committed {
                    deleted_ids.push(id.clone());
                }
                if let Err(error) = result {
                    if error.kind() != ErrorKind::NotFound && delete_error.is_none() {
                        delete_error = Some(
                            anyhow::anyhow!(error).context(format!("delete retained session {id}")),
                        );
                    }
                }
            }
        }
        if !deleted_ids.is_empty() {
            // 保留策略删掉的会话使列表快照过期;部分失败(JSON 已删、Err 提前
            // 冒泡)同样过期——不能只认 Ok 分支,否则幽灵条目驻留到下一次任意写。
            self.invalidate_list_cache();
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

    pub(crate) fn persist_then_reconcile(
        &self,
        session: &SavedSession,
        event: &'static str,
    ) -> Result<PathBuf> {
        let path = self.save_session_atomic(session)?;
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!("[sessions] retention reconciliation failed after {event}: {error:#}");
        }
        Ok(path)
    }

    pub(crate) fn persist_then_reconcile_with(
        &self,
        session: &SavedSession,
        save_context: impl FnOnce() -> String,
        event: &'static str,
    ) -> Result<PathBuf> {
        let path = self
            .save_session_atomic(session)
            .with_context(save_context)?;
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!("[sessions] retention reconciliation failed after {event}: {error:#}");
        }
        Ok(path)
    }

    pub(crate) fn reconcile_retention(&self, event: &'static str) {
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!("[sessions] retention reconciliation failed after {event}: {error:#}");
        }
    }

    pub fn reconcile_scheduled_profiles(&self) -> Result<()> {
        let _mutation = self.scheduled_mutation.lock();
        self.reconcile_scheduled_profiles_locked()
    }

    pub(crate) fn reconcile_scheduled_profiles_locked(&self) -> Result<()> {
        let stale_ids: Vec<String> = self
            .scheduled_profiles
            .read()
            .keys()
            .filter(|id| scheduled_session_file(&self.manager, id).is_ok_and(|path| !path.exists()))
            .cloned()
            .collect();

        let mut removed = Vec::new();
        for id in stale_ids {
            // The transcript is already absent. Notify before fallible
            // sidecar cleanup so downstream process-local resources cannot be
            // stranded if that cleanup needs a later reconciliation retry.
            self.notify_session_deleted(&id);
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

    pub(crate) fn purge_session_side_maps(&self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        let contains = |candidate: &str| ids.iter().any(|id| id == candidate);

        let removed_multi_agent = {
            let mut modes = self.mode_states.write();
            let mut removed_multi_agent = false;
            modes.retain(|id, state| {
                let keep = !contains(id.as_str());
                if !keep && state.multi_agent {
                    removed_multi_agent = true;
                }
                keep
            });
            removed_multi_agent
        };
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
            let mut modes = self.session_mode_states.write();
            let before = modes.len();
            modes.retain(|id, _| !contains(id.as_str()));
            modes.len() != before
        };
        if removed_code_modes {
            self.save_session_mode_states();
        }

        let removed_workspaces = {
            let mut workspaces = self.session_workspaces.write();
            let before = workspaces.len();
            workspaces.retain(|id, _| !contains(id.as_str()));
            workspaces.len() != before
        };
        if removed_workspaces {
            self.save_session_workspaces();
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

        // Keys of process-level turn-state maps (timing/pending_user_input)
        // accumulate per session id and are cleaned by the purge hook
        // registered by the app composition root (see SessionPurgedHook;
        // this module must not depend on the assistant feature directly).
        // Otherwise residual unpaired turn queues grow unbounded over the
        // app lifetime, and a late finish would rebuild an orphan timing
        // sidecar outside the deleted session's directory.
        for id in ids {
            self.notify_session_purged(id);
        }
        // 回退备份 sidecar 同样随会话清理（best-effort，见其实现注释）。
        Self::purge_rewound_turns_backups(ids);
    }

    pub(crate) fn purge_all_scheduled_side_maps(&self) {
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

    pub(crate) fn is_scheduled_session(&self, id: &str) -> Result<bool> {
        if self.scheduled_profiles.read().contains_key(id) {
            return Ok(true);
        }
        if !id.starts_with("sched-") {
            return Ok(false);
        }
        Ok(scheduled_session_file(&self.manager, id)?.exists())
    }

    pub(crate) fn load_scheduled_profiles(&self) -> Result<()> {
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

    pub(crate) fn save_scheduled_profiles(&self) -> Result<()> {
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

    pub(crate) fn remove_scheduled_runtime_dir(&self, id: &str) -> Result<()> {
        validate_scheduled_session_id(id)?;
        if !self.scheduled_profiles.read().contains_key(id)
            && chat_session_file(&self.manager, id)?.exists()
        {
            bail!("Refusing to remove runtime data for ordinary chat session '{id}'");
        }
        let runtime_dir = self.manager.sessions_dir().join(id);
        #[cfg(test)]
        if let Some(kind) = SCHEDULED_RUNTIME_DELETE_FAULTS.lock().remove(id) {
            return Err(std::io::Error::new(
                kind,
                "injected scheduled runtime directory cleanup failure",
            ))
            .with_context(|| format!("remove scheduled runtime dir {}", runtime_dir.display()));
        }
        if runtime_dir.exists() {
            std::fs::remove_dir_all(&runtime_dir).with_context(|| {
                format!("remove scheduled runtime dir {}", runtime_dir.display())
            })?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn inject_scheduled_runtime_delete_fault(
        &self,
        id: &str,
        kind: ErrorKind,
    ) -> Result<()> {
        validate_scheduled_session_id(id)?;
        SCHEDULED_RUNTIME_DELETE_FAULTS
            .lock()
            .insert(id.to_string(), kind);
        Ok(())
    }

    pub(crate) fn scheduled_workspace_for_task(&self, task_id: &str) -> Result<PathBuf> {
        validate_scheduled_task_id(task_id)?;
        Ok(self.scheduled_root.join(task_id).join("workspace"))
    }

    pub fn list_scheduled(&self) -> Result<Vec<SessionMetadata>> {
        let mut out = self
            .list_sessions_cached()
            .context("list_sessions failed")?
            .as_ref()
            .clone();
        out.retain(|metadata| metadata.id.starts_with("sched-"));
        out.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
        Ok(out)
    }

    pub fn scheduled_profile(&self, id: &str) -> Option<ScheduledRunProfile> {
        self.scheduled_profiles.read().get(id).cloned()
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
        self.scheduled_profile(id).is_some() && self.manager.load_session_snapshot(id).is_ok()
    }

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
            .load_session_snapshot(id)
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
            .load_session_snapshot(id)
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
            // 回滚删除本身也是一次落盘变更:失效列表缓存,防止并发读者恰在
            // save 失效与回滚删除之间重扫到 sched-*.json 并以当时的代数回填,
            // 让已被回滚的幽灵会话滞留在缓存里。
            self.invalidate_list_cache();
            let (_, rollback_result) = self.delete_session_record(&id);
            if let Err(rollback_error) = rollback_result {
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
            // Idempotent retry: the durable profile may already be gone while
            // a process-local consumer still owns resources for this id. An
            // orphan `sched-*.json` without a profile is deliberately retained
            // (see reconcile_scheduled_profiles_locked), so only an actually
            // absent transcript is a durable deletion lifecycle event.
            let record_path = scheduled_session_file(&self.manager, id)?;
            match std::fs::metadata(&record_path) {
                Ok(_) => return Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "inspect scheduled session record before idempotent delete {}",
                            record_path.display()
                        )
                    });
                }
            }

            self.notify_session_deleted(id);
            self.scheduled_profiles.write().remove(id);
            self.purge_session_side_maps(&[id.to_string()]);
            self.invalidate_list_cache();

            let runtime_cleanup = self.remove_scheduled_runtime_dir(id);
            let profile_persist = self.save_scheduled_profiles();
            return match (runtime_cleanup, profile_persist) {
                (Ok(()), Ok(())) => Ok(()),
                (runtime_cleanup, profile_persist) => {
                    let mut failures = Vec::new();
                    if let Err(error) = runtime_cleanup {
                        failures.push(format!("runtime cleanup: {error:#}"));
                    }
                    if let Err(error) = profile_persist {
                        failures.push(format!("profile persistence: {error:#}"));
                    }
                    Err(anyhow::anyhow!(
                        "idempotent scheduled session delete {id} completed with cleanup errors: {}",
                        failures.join("; ")
                    ))
                }
            };
        };
        if profile.task_id != expected_task_id {
            bail!(
                "Scheduled session task ownership mismatch: expected {expected_task_id}, found {}",
                profile.task_id
            );
        }

        // As in store.delete, deleting the durable JSON can commit before a
        // later directory cleanup reports an error. Once committed, remove
        // every scheduled/store side map even while preserving that error for
        // the caller.
        self.invalidate_list_cache();
        let (committed, delete_result) = self.delete_session_record(id);
        let delete_error = match delete_result {
            Ok(()) => None,
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) if committed => Some(err),
            Err(err) => {
                return Err(err).with_context(|| format!("delete scheduled session {id}"));
            }
        };

        if committed {
            self.scheduled_profiles.write().remove(id);
            self.purge_session_side_maps(&[id.to_string()]);
        }
        self.invalidate_list_cache();
        let runtime_cleanup = self.remove_scheduled_runtime_dir(id);
        let profile_persist = if committed {
            match self.save_scheduled_profiles() {
                Ok(()) => Ok(()),
                Err(error) => {
                    // Keep ownership metadata in memory when its durable removal
                    // did not commit. A same-process retry must be able to persist
                    // the removal instead of treating the missing transcript as a
                    // fully completed idempotent delete.
                    self.scheduled_profiles
                        .write()
                        .insert(id.to_string(), profile.clone());
                    Err(error)
                }
            }
        } else {
            Ok(())
        };

        match (delete_error, runtime_cleanup, profile_persist) {
            (None, Ok(()), Ok(())) => Ok(()),
            (Some(error), Ok(()), Ok(())) => {
                Err(error).with_context(|| format!("delete scheduled session {id}"))
            }
            (delete_error, runtime_cleanup, profile_persist) => {
                let mut failures = Vec::new();
                if let Some(error) = delete_error {
                    failures.push(format!("durable record cleanup: {error}"));
                }
                if let Err(error) = runtime_cleanup {
                    failures.push(format!("runtime cleanup: {error:#}"));
                }
                if let Err(error) = profile_persist {
                    failures.push(format!("profile persistence: {error:#}"));
                }
                Err(anyhow::anyhow!(
                    "delete scheduled session {id} completed with cleanup errors: {}",
                    failures.join("; ")
                ))
            }
        }
    }

    pub(crate) fn append_scheduled_artifact_path(&self, id: &str, path: PathBuf) -> Result<()> {
        let _mutation = self.scheduled_mutation.lock();
        if !self.scheduled_profiles.read().contains_key(id) {
            bail!("Session '{id}' is not a scheduled-run session");
        }
        validate_scheduled_session_id(id)?;
        let mut session = self
            .manager
            .load_session_snapshot(id)
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
}
