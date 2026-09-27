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
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use chrono::Utc;
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
    ///
    /// This is the ONLY production boot path, and the one place the legacy
    /// binding-table migration belongs: the rebind crash-window contract
    /// ("the legacy table is rewritten before the sidecars, so the next boot
    /// heals forward") is only true if the boot migration actually runs here —
    /// with the convergence missing, the first rebind would silently drop
    /// legacy-table-only entries (round-8 review B1). Secondary stores opened
    /// later via [`Self::boot`] must not repeat it.
    pub fn boot_for_process_startup() -> Result<Self> {
        let store = Self::boot_inner(true)?;
        store.migrate_legacy_session_workspaces();
        Ok(store)
    }

    fn boot_inner(recover_interrupted_tools: bool) -> Result<Self> {
        Self::boot_inner_with(paths::scheduled_tasks_root(), recover_interrupted_tools)
    }

    /// Test-only boot over an isolated root; production boot paths are
    /// [`Self::boot`] / [`Self::boot_for_process_startup`].
    #[cfg(test)]
    pub(crate) fn boot_at_test_dir(root: &std::path::Path) -> Result<Self> {
        Self::from_paths(
            root.join("sessions"),
            root.join("scheduled-run-profiles.json"),
            root.join("scheduled"),
        )
    }

    /// Test-only boot over an isolated scheduled root (all callers are
    /// `cfg(test)`). Mirrors [`Self::boot_for_process_startup`] by converging
    /// the legacy binding table explicitly after the shared boot sequence.
    #[cfg(test)]
    pub(crate) fn boot_with_scheduled_root(scheduled_root: PathBuf) -> Result<Self> {
        let store = Self::boot_inner_with(scheduled_root, false)?;
        store.migrate_legacy_session_workspaces();
        Ok(store)
    }

    /// Shared boot sequence: open the store over the ordinary sessions root
    /// with the given scheduled root, load the five sidecar maps, then —
    /// optionally after repairing interrupted tool histories — enforce
    /// retention and purge scheduled side maps.
    ///
    /// The legacy binding-table migration is NOT part of this sequence: it is
    /// the "next boot" half of the rebind crash-window contract (the legacy
    /// table is rewritten before the sidecars move, so a boot heals forward —
    /// review #464 round-6 finding 5) and is owned by the explicit boot
    /// callers ([`Self::boot_for_process_startup`], and the test-only
    /// [`Self::boot_with_scheduled_root`]). Plain [`Self::boot`] must not
    /// repeat the migration.
    fn boot_inner_with(scheduled_root: PathBuf, recover_interrupted_tools: bool) -> Result<Self> {
        let store = Self::from_paths(
            paths::sessions_root(),
            paths::scheduled_run_profiles_path(),
            scheduled_root,
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

    pub(crate) fn from_paths(
        sessions_dir: PathBuf,
        scheduled_profiles_path: PathBuf,
        scheduled_root: PathBuf,
    ) -> Result<Self> {
        let manager = SessionManager::new(sessions_dir.clone())
            .with_context(|| format!("SessionManager::new({}) failed", sessions_dir.display()))?;
        let prefs_snapshot = UserPrefs::load();
        // The design lane has been merged into the work lane: when legacy
        // settings.json has `mode_defaults.design` set and work is empty,
        // backfill the work in-memory mirror from the design value (a
        // one-time read fold, never written back to disk; explicit switches
        // afterwards land on work only). When work already has a value,
        // design does not override it.
        // The design field itself must round-trip verbatim through
        // whole-preferences writes: until the user explicitly writes work,
        // it is the only default value on disk — if an unrelated preferences
        // write evaporated it, a restart could never fold it back (see the
        // `ModeDefaultPrefs::design` comment).
        let mut mode_defaults_snapshot = prefs_snapshot.mode_defaults;
        if mode_defaults_snapshot.work.is_none() {
            mode_defaults_snapshot.work = mode_defaults_snapshot.design;
        }
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
            legacy_session_workspaces_parse_failed: Arc::new(AtomicBool::new(false)),
            code_session_predicate: Arc::new(RwLock::new(None)),
            session_mode_states: Arc::new(RwLock::new(HashMap::new())),
            code_permission: Arc::new(RwLock::new(prefs_snapshot.code_permission)),
            mode_defaults: Arc::new(RwLock::new(mode_defaults_snapshot)),
            session_purged_hooks: Arc::new(RwLock::new(Vec::new())),
            session_deleted_hooks: Arc::new(RwLock::new(Vec::new())),
        };
        store.load_scheduled_profiles()?;
        store.reconcile_scheduled_profiles_locked()?;
        Ok(store)
    }

    /// Cached read of the upstream `manager.list_sessions()`: after a full directory scan on first access, it caches
    /// `Arc<Vec<SessionMetadata>>`, and subsequent lists share the same snapshot. The invalidation point is the App-side
    /// single write path (`save_session_atomic`/`delete`), so cache-on-disk consistency presupposes that
    /// "all session JSON goes through SessionStore writes" — currently true (save/
    /// set_title/touch_activity/create_new all go through save_session_atomic).
    /// Returning an `Arc` lets callers (such as the AcpPool startup scan) consume it zero-copy.
    ///
    /// Backfill carries a generation guard: if a write occurs while a miss is scanning (the invalidation bumps the generation), that scan result is
    /// discarded and rescanned — otherwise a slow scan started before the write would overwrite the post-write snapshot with the old directory view, and a stale
    /// list (e.g. a renamed title) would persist until the next arbitrary write. Duplicate scans from concurrent misses are
    /// benign (idempotent reads), not worth adding a loading mutex.
    pub(crate) fn list_sessions_cached(&self) -> std::io::Result<Arc<Vec<SessionMetadata>>> {
        let generation_now = self.list_cache_generation.load(Ordering::Acquire);
        loop {
            if let Some((generation, cached)) = self.list_cache.read().clone() {
                if generation == generation_now {
                    return Ok(cached);
                }
                // Stale-generation entry: it can be persisted while the waiting writer has
                // not yet cleared the slot or the guard has gone stale, punching through
                // the rescan — the pre-write view must not be returned as a valid snapshot.
            }
            let generation_at_scan = self.list_cache_generation.load(Ordering::Acquire);
            let fresh = Arc::new(self.manager.list_sessions()?);
            let mut slot = self.list_cache.write();
            if self.list_cache_generation.load(Ordering::Acquire) == generation_at_scan {
                // No writes during the scan: safe to backfill. The write lock guarantees only one miss contender persists;
                // latecomers reaching the top already hit the cache (or rescan with the newer generation).
                *slot = Some((generation_at_scan, Arc::clone(&fresh)));
                return Ok(fresh);
            }
            // A write occurred during the scan: discard this result and rescan. Under sustained write activity it rescans at most
            // until the next write gap — same order as the per-list live scan without a cache, so no livelock.
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
        // Multi-agent is a persistent switch on ordinary sessions, not a separate
        // session type; only scheduled sessions are isolated here — all other
        // history goes into the ordinary list.
        // In benchmark builds, evaluation sessions (eval_ prefix, including GAIA
        // private problems) do not enter user history: the normal path is cleaned
        // up by the evaluation runner, and crash leftovers must not leak private
        // problems into the session list.
        // Non-benchmark desktop builds do not keep this prefix semantics, avoiding
        // changes to the ordinary session list when the benchmark feature is absent.
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

    /// Pack one session into a full-fidelity `.tar.xz` archive, reusing the
    /// base `deepseek_tui::session_export`. The archive contains the full
    /// context (system prompt, all turn messages, tool calls and results)
    /// plus the portable container JSON; the artifacts directory is packed
    /// by default, and `include_artifacts=false` exports the record only.
    ///
    /// Boundary: what gets packed is the bytes of the
    /// `sessions/<id>/artifacts` directory; ledger/workspace files that the
    /// artifacts panel also lists are not part of the archive — their
    /// "record" travels with `session.json`, and the file bytes themselves
    /// are not distributed with the archive.
    pub(crate) fn export_archive(
        &self,
        id: &str,
        output: &Path,
        include_artifacts: bool,
    ) -> Result<deepseek_tui::session_export::SessionArchiveSummary> {
        validate_session_id(id)?;
        let session = self.load(id)?;
        let artifacts_dir = if include_artifacts {
            deepseek_tui::session_export::session_artifacts_dir(
                self.manager.sessions_dir(),
                &session.metadata.id,
            )
        } else {
            None
        };
        Ok(deepseek_tui::session_export::write_session_archive(
            &session,
            artifacts_dir.as_deref(),
            output,
            deepseek_tui::session_export::SessionArchiveOptions {
                include_artifacts,
                ..deepseek_tui::session_export::SessionArchiveOptions::default()
            },
        )?)
    }

    pub(crate) fn persisted_size(&self, id: &str) -> Result<u64> {
        validate_session_id(id)?;
        let path = self.manager.sessions_dir().join(format!("{id}.json"));
        std::fs::metadata(&path)
            .with_context(|| format!("read Session metadata {}", path.display()))
            .map(|metadata| metadata.len())
    }

    /// Persist a whole session snapshot. Crate-internal: every durable write
    /// goes through [`Self::update_messages`] / [`Self::update_artifacts`] /
    /// the persist helpers above; direct whole-snapshot saves are reserved
    /// for the store's own create/recovery paths.
    pub(crate) fn save(&self, session: &SavedSession) -> Result<PathBuf> {
        let _mutation = self.scheduled_mutation.lock();
        self.persist_then_reconcile(session, "session save")
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        if self.is_scheduled_session(id)? {
            bail!("Scheduled-run sessions are deleted through their automation");
        }
        // Upstream delete_session removes the session JSON before cleaning the
        // directory: when directory cleanup fails, the JSON is already gone
        // from disk and the error propagates upward — invalidate the snapshot
        // as "a delete was attempted and disk may have changed", without
        // waiting for the unified invalidation after the match (an early Err
        // return would skip it).
        self.invalidate_list_cache();
        let (committed, delete_result) = self.delete_session_record(id);
        if committed {
            // A later workspace/artifact cleanup error does not roll back the
            // durable record removal. Purge store/process side maps now so an
            // error return cannot strand an active id, model binding or turn
            // state forever when the caller never retries.
            self.purge_session_side_maps(&[id.to_string()]);
            // The rewind-backup sidecar is likewise cleaned up with the session (best-effort; see its implementation comment).
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

    /// Whether the session JSON is no longer on disk (an invalid id is always
    /// treated as "present", fail-closed). Besides the delete path, the
    /// rebind orphan classification also uses it: only NotFound counts — a
    /// corrupt JSON is not an orphan (review #463: a parse failure must enter
    /// the failed list as retryable, never silently skipped).
    pub(crate) fn durable_session_record_is_absent(&self, id: &str) -> bool {
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
        // Code sessions with an explicit per-session record: left for load_session_mode_states to override,
        // not handled here.
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
                bound: false,
            });
        }
        if self.is_scheduled_session(id)? {
            bail!("Scheduled-run session '{id}' has no persisted execution profile");
        }
        // The production-injected resolver (lib.rs) already covers both binding
        // kinds: codex_acp native code sessions' project bindings plus plain chat
        // sessions' user working-directory bindings (sidecar). The .or_else
        // fallback here is defensive and only applies when no resolver is
        // injected (tests / early startup) — bridge goes through the resolver
        // directly (bridge.rs) and never hits this fallback.
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
        // The title and transcript live in the same JSON. The Engine also writes this
        // file while a scheduled session is generating, so load / modify / save must sit under the same lock; otherwise a rename could overwrite
        // the new messages the Engine just persisted with a stale snapshot.
        let _mutation = self.scheduled_mutation.lock();
        let mut session = self
            .manager
            .load_session_snapshot(id)
            .with_context(|| format!("load_session({id}) for title update"))?;
        session.metadata.title = title;
        self.persist_then_reconcile(&session, "title update")?;
        Ok(())
    }

    /// Metadata write for directory rebind (same load→patch→persist pattern
    /// as set_title). Only the SavedSession metadata workspace field changes;
    /// messages/transcript are untouched — old paths referenced by historical
    /// turns are factual records and stay as-is. The caller (command layer)
    /// owns the active-turn fence; the lock here guards against Engine writes.
    /// The load context deliberately does not embed the session id: the
    /// command layer logs this error chain and rebind logs must not persist
    /// session ids (CodeQL cleartext-logging, review #463 round 7); the id is
    /// available to the caller at the failure site.
    pub fn set_workspace(&self, id: &str, workspace: PathBuf) -> Result<()> {
        let _mutation = self.scheduled_mutation.lock();
        let mut session = self
            .manager
            .load_session_snapshot(id)
            .with_context(|| "load_session for workspace rebind".to_string())?;
        session.metadata.workspace = workspace;
        self.persist_then_reconcile(&session, "workspace rebind")?;
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
        // Per-session model: persist the sidecar first, then publish the Session JSON, to avoid a write failure leaving
        // a session that appears created successfully yet falls back to another model after restart.
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

    /// Test-only seam: the production CAS consumer (the legacy web
    /// transcript-save command) was removed with the dead-code sweep. Kept
    /// gated because these tests pin the revision-conflict, truncation-guard,
    /// and write-race semantics shared with the live revision-checked writers.
    #[cfg(test)]
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
        session.artifacts = paths
            .into_iter()
            .enumerate()
            .map(|(idx, p)| {
                super::retention::fabricated_tool_output_record(&session_id, idx, PathBuf::from(&p))
            })
            .collect();
        session.metadata.updated_at = Utc::now();
        self.persist_then_reconcile(&session, "artifact update")?;
        Ok(())
    }

    pub fn active_id(&self) -> Option<String> {
        self.active.read().clone()
    }

    pub fn set_active(&self, id: Option<String>) {
        *self.active.write() = id;
    }

    /// Persist one authoritative engine snapshot for an ordinary chat session.
    ///
    /// Takes `state` by reference so event-forwarder callers can keep the
    /// snapshot alive in an `Arc` for the terminal path without a second deep
    /// copy; the transcript clone into the durable record happens here, once
    /// per persist.
    pub fn persist_chat_engine_state(
        &self,
        id: &str,
        state: &ChatEngineState,
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
        session.metadata.model = state.model.clone();
        session.metadata.workspace = state.workspace.clone();
        session.messages = state.messages.clone();
        session.system_prompt = persisted_system_prompt(state.system_prompt.as_ref());

        self.persist_then_reconcile_with(
            &session,
            || format!("persist chat engine state for {id}"),
            "committed engine state save",
        )?;
        Ok(session)
    }

    /// Create an empty session with a caller-provided ID, for internal runtimes that need the isolation ID determined before startup.
    ///
    /// Ordinary GUI sessions still use [`Self::create_new`]'s random ID; this does not set the active session.
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
