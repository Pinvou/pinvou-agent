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

use anyhow::{Context, Result};

use super::scheduled::{ScheduledProfileRegistry, SCHEDULED_PROFILE_SCHEMA_VERSION};
use super::store::MAX_SESSIONS_PER_KIND;
use super::validators::{
    chat_session_file, scheduled_session_file, validate_scheduled_session_id,
    validate_scheduled_task_id, validate_scheduled_workspace_path,
};
use super::SessionStore;

impl SessionStore {
    pub(crate) fn save_session_atomic(
        &self,
        session: &deepseek_tui::session_manager::SavedSession,
    ) -> Result<std::path::PathBuf> {
        super::validators::validate_session_id(&session.metadata.id)?;
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
    pub(crate) fn enforce_session_retention_locked(&self) -> Result<()> {
        let sessions = self
            .manager
            .list_sessions()
            .context("list sessions for retention")?;
        let mut chat_count = 0usize;
        let mut deleted_ids = Vec::new();
        let mut delete_error = None;
        for metadata in sessions {
            // Scheduled sessions are retained per automation together with
            // their Run and Task records. This generic chat cleanup must never
            // delete one side of that three-part history.
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

    /// Persist `session` atomically, then run retention reconciliation.
    ///
    /// This collapses the shared tail (`save_session_atomic` succeeded by an
    /// `enforce_session_retention_locked` whose failure is logged and swallowed)
    /// that 13 public methods previously inlined. Retention reconciliation never
    /// invalidates a committed save: it runs after the atomic write has landed,
    /// mirroring the historical "save first, best-effort cleanup" contract.
    ///
    /// `event` describes the mutating operation (e.g. "committed save",
    /// "title update") so the diagnostic uniquely identifies which call failed.
    /// The helper deliberately does NOT re-check `is_scheduled_session`: callers
    /// that branch on it (notably [`super::store::SessionStore::save`]) decide
    /// scheduling before delegating here.
    pub(crate) fn persist_then_reconcile(
        &self,
        session: &deepseek_tui::session_manager::SavedSession,
        event: &'static str,
    ) -> Result<std::path::PathBuf> {
        let path = self.save_session_atomic(session)?;
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!("[sessions] retention reconciliation failed after {event}: {error:#}");
        }
        Ok(path)
    }

    /// Atomically persist `session` (enriching any failure with `save_context`),
    /// then run best-effort retention reconciliation.
    ///
    /// Variant of [`Self::persist_then_reconcile`] for callers that attach a
    /// session-specific error context to the atomic save (engine-state, token,
    /// artifact persistence). Reconciliation keeps the same "log and swallow"
    /// contract as the other variant.
    pub(crate) fn persist_then_reconcile_with(
        &self,
        session: &deepseek_tui::session_manager::SavedSession,
        save_context: impl FnOnce() -> String,
        event: &'static str,
    ) -> Result<std::path::PathBuf> {
        let path = self
            .save_session_atomic(session)
            .with_context(save_context)?;
        if let Err(error) = self.enforce_session_retention_locked() {
            eprintln!("[sessions] retention reconciliation failed after {event}: {error:#}");
        }
        Ok(path)
    }

    /// Run best-effort retention reconciliation after a committed mutation.
    ///
    /// Used by methods (notably
    /// [`super::store::SessionStore::create_scheduled_run`]) where the atomic
    /// save is followed by side effects (profile insertion + rollback) before
    /// the retention tail, so [`Self::persist_then_reconcile`] cannot couple
    /// them. Same "log and swallow" contract as the persist variants.
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

        let removed_modes = {
            let mut modes = self.mode_states.write();
            let before = modes.len();
            modes.retain(|id, _| !contains(id.as_str()));
            modes.len() != before
        };
        if removed_modes {
            self.save_skill_bindings();
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
            anyhow::bail!(
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
            anyhow::bail!("Refusing to remove runtime data for ordinary chat session '{id}'");
        }
        let runtime_dir = self.manager.sessions_dir().join(id);
        if runtime_dir.exists() {
            std::fs::remove_dir_all(&runtime_dir).with_context(|| {
                format!("remove scheduled runtime dir {}", runtime_dir.display())
            })?;
        }
        Ok(())
    }

    pub(crate) fn scheduled_workspace_for_task(&self, task_id: &str) -> Result<std::path::PathBuf> {
        validate_scheduled_task_id(task_id)?;
        Ok(self.scheduled_root.join(task_id).join("workspace"))
    }
}
