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
            // The lifecycle of an auxiliary conversation (aux-) is owned by the
            // main session's cascade/discard (same precedent as sched-): it is
            // never an eviction candidate and does not consume the retention
            // budget of visible sessions.
            if super::validators::is_sched_session_id(&metadata.id)
                || super::validators::is_aux_session_id(&metadata.id)
            {
                continue;
            }
            chat_count += 1;
            if chat_count > MAX_SESSIONS_PER_KIND {
                let id = metadata.id;
                // Evicting a main session cascade-evicts its aux session:
                // resolve the mapping first (purge_session_side_maps removes it
                // once the main record commits), then let the aux record and
                // the main record enter the same deleted_ids set so side-map
                // cleanup runs uniformly. This mirrors the existing eviction
                // semantics of main sessions: the store layer cannot reach the
                // pool (dependency direction), so deleting the aux record here
                // reclaims no engine and emits no session:deleted — a still
                // running aux engine is reclaimed by id as a fallback by the
                // pool's idle-eviction sweep.
                if let Some(aux_id) = self.aux_session_id(&id) {
                    let (aux_committed, aux_result) = self.delete_session_record(&aux_id);
                    if aux_committed {
                        deleted_ids.push(aux_id.clone());
                    }
                    if let Err(error) = aux_result {
                        if error.kind() != ErrorKind::NotFound && delete_error.is_none() {
                            // No raw ids in log-reachable chains (see the
                            // cleartext-logging stance in sidecars.rs): this
                            // context ends up in the boot eprintln through
                            // `{error:#}`, so the step identifies the failure
                            // and the id stays out.
                            delete_error = Some(
                                anyhow::anyhow!(error)
                                    .context("delete the evicted session's aux record"),
                            );
                        }
                    }
                }
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

        // Working-directory binding: clear the in-memory cache and best-effort
        // delete the per-session sidecar file. The normal session-deletion path
        // already removes it; this covers leftovers of a partially failed
        // deletion. For unbound ids it is a pure NotFound probe with negligible
        // cost.
        for id in ids {
            self.session_workspaces.write().remove(id.as_str());
            if validate_session_id(id).is_ok() {
                self.remove_workspace_sidecar_file(id);
            }
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

        let removed_aux = {
            let mut aux_sessions = self.aux_sessions.write();
            let before = aux_sessions.len();
            // Bidirectional cleanup: remove an entry when either the key (main
            // session) or the value (aux session) hits the deleted set — neither
            // deleting a main session nor deleting an aux session alone may
            // leave a ghost mapping behind.
            aux_sessions.retain(|main_id, aux_id| {
                !contains(main_id.as_str()) && !contains(aux_id.as_str())
            });
            aux_sessions.len() != before
        };
        if removed_aux {
            self.save_aux_sessions();
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

    /// Orphan reconciliation for auxiliary conversations (`aux-` prefix), with
    /// the same startup-reclamation semantics as
    /// `purge_all_scheduled_side_maps`: a crash between "persist the aux
    /// record" and "persist the `_aux_sessions.json` mapping", or a corrupted
    /// mapping sidecar, leaves an invisible orphan that never enters the
    /// session list and is unconditionally skipped by retention (`aux-` prefix,
    /// see enforce_session_retention_locked); a main session dying first
    /// (external cleanup / interrupted cascade delete) accumulates the same way
    /// without reconciliation.
    ///
    /// Two cases, two dispositions: when the mapping is missing but the record
    /// is on disk, **first rebuild the mapping from the record's backlink
    /// `parent_session_id`** (as long as the main session is still alive the
    /// user's Q&A content is not lost; a record left over from the crash window
    /// is empty, so resurrecting it is equivalent to creating a fresh one; this
    /// matches the sched- side's "preserve the transcript first" protective
    /// posture). Only when the mapping cannot be rebuilt (main is dead / record
    /// unreadable / the main is ambiguously claimed by another aux) or the
    /// mapping exists but the main session record is no longer on disk do we
    /// reclaim it as an orphan: delete the record + `purge_session_side_maps`
    /// bidirectional cleanup (which also strips the ghost mapping of a dead
    /// main with a surviving aux).
    ///
    /// Called only on the startup path (same as the sched- side table
    /// reconciliation): at that point no get-or-create is in flight, so the
    /// legitimate creation window of "record persisted, mapping not yet" cannot
    /// overlap with reconciliation — therefore this function **must not** be
    /// wired into enforce_session_retention_locked (the save inside
    /// create_aux_session would trigger enforce and misdelete the newborn as an
    /// orphan).
    pub(crate) fn reconcile_aux_sessions(&self) -> Result<()> {
        // Skip entirely when the sidecar was never successfully read this
        // boot: mapping-based decisions (missing mapping ⇒ rebuild/delete)
        // would run against an artificially empty map and delete live
        // transcripts. The next boot retries the reconcile.
        if !self
            .aux_sessions_loaded
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            eprintln!("[sessions] aux reconciliation skipped: bindings not loaded this boot");
            return Ok(());
        }
        let mut aux_ids: Vec<String> = self
            .list_sessions_cached()
            .context("list sessions for aux session reconciliation")?
            .iter()
            .filter(|metadata| super::validators::is_aux_session_id(&metadata.id))
            .map(|metadata| metadata.id.clone())
            .collect();
        // Deterministic order, and the *same* survivor rule the sidecar load
        // uses (the sorted-first claim wins). Two unmapped records whose
        // backlink names the same parent both want the rebuild, and the loser
        // is reclaimed as an ambiguous duplicate — if this pass picked by the
        // store's updated_at ordering instead, the winner could differ from
        // the load-time provisional owner and the ownership would flip between
        // boots (round-12 S3: the two layers disagreed on the tie-break).
        aux_ids.sort();
        // The listing is file-driven, so two files whose contents declare the
        // same metadata id (a case-variant alias file on a case-sensitive
        // filesystem, or any duplicated record) yield the same logical record
        // twice. Collapse them: the loop below adopts a record by binding its
        // parent, so a second pass over the same id would see a parent that is
        // now bound and reclaim the record it just repaired as an ambiguous
        // duplicate — deleting the transcript this pass exists to save.
        aux_ids.dedup();
        // Validate every binding against the record it names, *before* the
        // record loop. A value that cannot be loaded under its own name is
        // unusable: either the transcript was deleted out of band (the loop
        // below cannot see it — no record means no iteration) or it is a
        // case-variant alias that `load` rejects on identity. Clearing it first
        // lets the loop rebuild the real record from its backlink in the same
        // pass, instead of reclaiming a live transcript because a stale binding
        // was still occupying its parent (round-12 P3/S3).
        //
        // Membership in the listed ids is deliberately *not* the test: on a
        // case-insensitive filesystem the alias is listed as its own record, so
        // a string-set check would accept the alias, the loop would then fail to
        // load it (identity), keep the binding, and reclaim the canonical
        // record as an unbound orphan — losing the very transcript this pass
        // exists to repair.
        //
        // This runs *before* the empty-early-return on purpose: when every aux
        // record is gone, `aux_ids` is empty and the record loop has nothing to
        // iterate, which is exactly the state whose stale bindings must still
        // be stripped.
        let bindings = self.aux_sessions.read().clone();
        let invalid_mains: Vec<String> = bindings
            .iter()
            .filter(|(_, mapped)| binding_is_unusable(self, mapped))
            .map(|(main_id, _)| main_id.clone())
            .collect();
        if !invalid_mains.is_empty() {
            let mut stripped = false;
            {
                let mut aux_sessions = self.aux_sessions.write();
                for main_id in &invalid_mains {
                    // Re-check under the write lock: the snapshot above is only
                    // a read, and a concurrent set_aux_session may have
                    // re-bound the key since.
                    let still_invalid = aux_sessions
                        .get(main_id)
                        .is_some_and(|mapped| binding_is_unusable(self, mapped));
                    if still_invalid && aux_sessions.remove(main_id).is_some() {
                        stripped = true;
                    }
                }
            }
            if stripped {
                self.save_aux_sessions();
            }
        }
        if aux_ids.is_empty() {
            return Ok(());
        }
        let mappings = self.aux_sessions.read().clone();
        let mut orphan_ids = Vec::new();
        for aux_id in &aux_ids {
            match mappings.iter().find(|(_, mapped)| *mapped == aux_id) {
                // Mapping exists, but the main session record is no longer on
                // disk → dead main with a surviving aux.
                Some((main_id, _)) => {
                    let main_gone =
                        !chat_session_file(&self.manager, main_id).is_ok_and(|path| path.exists());
                    // A hand-edited sidecar can map mainA to an aux whose
                    // record backlink names mainB — without this check the
                    // mapping is trusted and mainA's panel would read mainB's
                    // transcript. On mismatch (main still alive) detach the
                    // false mapping so the record can be re-adopted by its true
                    // parent; when the main is gone the record is orphaned
                    // outright. A record read failure keeps the mapping (fail
                    // open to "unknown", matching the transient-fault stance of
                    // the rebuild side).
                    let backlink_mismatch = match self.load(aux_id) {
                        Ok(session) => {
                            session.metadata.parent_session_id.as_deref() != Some(main_id.as_str())
                        }
                        Err(_) => false,
                    };
                    if main_gone {
                        orphan_ids.push(aux_id.clone());
                    } else if backlink_mismatch {
                        // Detach, then rebuild for the true parent in this same
                        // pass. Deferring the rebuild to the next boot left a
                        // window where a panel opened in between minted a fresh
                        // aux under the true parent — the stranded transcript
                        // was then reclaimed as an ambiguous duplicate and the
                        // user's Q&A was lost for good (round-12 S3).
                        if let Err(error) = self.set_aux_session(main_id, None) {
                            // Identity-free: this surfaces through the boot log.
                            eprintln!("[sessions] detach mismatched aux mapping failed: {error:#}");
                        }
                        if !self.rebuild_aux_mapping_from_record(aux_id)? {
                            // The true parent is gone, already bound to another
                            // aux, or invalid: nothing can adopt this record,
                            // so reclaim it now instead of leaving a ghost that
                            // only a future boot would collect.
                            orphan_ids.push(aux_id.clone());
                        }
                    }
                }
                // Mapping missing: crash in the creation window / corrupted
                // sidecar → try to repair first, delete only if that fails.
                None => {
                    if !self.rebuild_aux_mapping_from_record(aux_id)? {
                        orphan_ids.push(aux_id.clone());
                    }
                }
            }
        }
        let mut deleted_ids = Vec::new();
        let mut delete_error = None;
        for id in &orphan_ids {
            let (committed, result) = self.delete_session_record(id);
            if committed {
                deleted_ids.push(id.clone());
            }
            if let Err(error) = result {
                if error.kind() != ErrorKind::NotFound && delete_error.is_none() {
                    delete_error =
                        Some(anyhow::anyhow!(error).context("delete the orphan aux record"));
                }
            }
        }
        if !deleted_ids.is_empty() {
            // Same wrap-up as a retention eviction: stale the list snapshot +
            // bidirectional side-table purge (which also strips and persists
            // any main→aux ghost mapping whose value was hit).
            self.invalidate_list_cache();
            self.purge_session_side_maps(&deleted_ids);
        }
        match delete_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// An aux record whose mapping is missing: rebuild the main→aux mapping
    /// from the record's backlink `parent_session_id` and persist it. The main
    /// session must genuinely exist on disk, must have no mapping of its own
    /// (being claimed by another aux = ambiguous duplicate, keep the existing
    /// binding), and must not carry an aux-/sched- prefix (aux-of-aux /
    /// scheduled records do not get auxiliary conversations). Returns
    /// Ok(true) = rebuilt; Ok(false) = cannot rebuild, caller reclaims it as
    /// an orphan.
    fn rebuild_aux_mapping_from_record(&self, aux_id: &str) -> Result<bool> {
        let parent_id = match self.load(aux_id) {
            Ok(session) => session.metadata.parent_session_id,
            // Fail closed on anything but a genuinely unusable name: a transient
            // boot-time read fault (EIO, a held file) must not classify a
            // live record as an orphan — the caller would delete it. The
            // error aborts the reconcile pass, which retries on next boot.
            // A NotFound or a case-variant alias (the record loads, but under a
            // different spelling) is not transient: nobody can ever use that
            // name, so the caller may reclaim it.
            Err(error) if super::store::is_not_found_error(&error) => return Ok(false),
            Err(error) if super::store::is_identity_mismatch_error(&error) => return Ok(false),
            Err(error) => {
                return Err(error).with_context(|| "load the aux record for reconciliation");
            }
        };
        let Some(parent_id) = parent_id else {
            return Ok(false);
        };
        if super::validators::is_aux_session_id(&parent_id)
            || super::validators::is_sched_session_id(&parent_id)
        {
            return Ok(false);
        }
        if self.aux_session_id(&parent_id).is_some()
            || !chat_session_file(&self.manager, &parent_id).is_ok_and(|path| path.exists())
        {
            return Ok(false);
        }
        self.set_aux_session(&parent_id, Some(aux_id.to_string()))
            .with_context(|| "rebuild the aux record's mapping from its backlink")?;
        Ok(true)
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

/// True when a main→aux binding names a record that can never be used: the
/// transcript is gone, or the name is a case-variant alias that [`SessionStore::load`]
/// rejects on identity.
///
/// Transient read faults deliberately return `false`: clearing the binding
/// there would hand the next boot a mapping-less live transcript, and on a
/// fail-closed path it would orphan it outright.
fn binding_is_unusable(store: &SessionStore, mapped: &str) -> bool {
    match store.load(mapped) {
        Ok(_) => false,
        Err(error) => {
            super::store::is_not_found_error(&error)
                || super::store::is_identity_mismatch_error(&error)
        }
    }
}
