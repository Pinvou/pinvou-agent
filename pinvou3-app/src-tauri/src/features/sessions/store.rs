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
// Only the benchmark-gated helper below consults the record path, so the
// import must share its cfg to stay unused-warning-clean in plain builds.
#[cfg(any(feature = "benchmark-hooks", test))]
use super::validators::chat_session_file;
use super::{
    CodeSessionPredicate, ExecutionRootResolver, SessionDeletedHook, SessionKind,
    SessionPurgedHook, SessionRoots, SessionStore, session_roots_for,
};
use crate::core::mode_state::SerializableMode;
use crate::platform::prefs::UserPrefs;

#[cfg(test)]
static POST_RECORD_DELETE_FAULTS: LazyLock<Mutex<HashMap<String, ErrorKind>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(test)]
static PRE_RECORD_DELETE_FAULTS: LazyLock<Mutex<HashMap<String, ErrorKind>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Cap on the number of ordinary chat sessions retained on disk before the
/// oldest is evicted by [`super::retention::SessionStore::enforce_session_retention_locked`].
pub(crate) const MAX_SESSIONS_PER_KIND: usize = 50;

/// The aux conversation's internal default title (Chinese constant): aux
/// sessions never enter the regular session list, so this title is only used
/// in the detail view and the on-disk record, and does not switch with the UI
/// language.
pub(crate) const AUX_SESSION_TITLE: &str = "辅助对话";

/// Id prefix minted by the headless `agent run` path (`agentic_{pid}_{n}`).
///
/// Retention keys the headless budget on it, so the prefix is a durable
/// contract between the runner and the sweep rather than a formatting detail;
/// `features::assistant::product_runtime::agentic_task::fresh_session_id`
/// builds ids from it and a unit test pins the two together.
pub(crate) const HEADLESS_SESSION_PREFIX: &str = "agentic_";

/// Cap on retained headless `agent run` sessions, counted and evicted
/// independently of the chat budget.
///
/// A CLI invocation must never be a destructive operation on the desktop
/// user's conversations: sharing one budget meant each default-keep run
/// evicted the oldest GUI chat (with its workspace and checkpoints), and the
/// only notice went to a stderr the desktop user never reads. Sized like the
/// chat budget — a headless batch is exactly the workload that benefits from
/// keeping recent runs inspectable.
pub(crate) const MAX_HEADLESS_SESSIONS: usize = 50;

/// Placeholder title for a fresh chat session. One of the trilingual
/// sentinels in the frontend's `DEFAULT_CHAT_TITLES`: the sidebar localizes
/// it per UI language and the first send triggers the auto-rename. Headless
/// sessions persist by default and surface in the GUI history, so they carry
/// the same sentinel — an eval-internal label would leak untranslated into
/// every UI language, and because the command layer's auto-rename triggers on
/// exactly this value it would also freeze an adopted session's title forever.
///
/// It doubles as the agentic runner's adoption marker: a session still wearing
/// the sentinel is untouched factory state that a failed run may delete, while
/// any other title means a GUI user renamed it (or their first send triggered
/// the auto-rename) and now owns it.
pub(crate) const NEW_CHAT_TITLE: &str = "新对话";

/// Marker file the code-session feature writes inside a session's directory
/// (`sessions/<id>/code-session.json`). Named here because the probe below
/// lives here; the writer and the format stay owned by that feature.
const CODE_SESSION_MARKER_FILE: &str = "code-session.json";

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
        // Order constraint (review #455): this boot creates sessions/
        // directory entries (a first-boot self-write trace), so the
        // disabled_bundles migration verdict must complete before it —
        // lib.rs `startup_order_contract` pins the order via source-position
        // assertions.
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
        // Legacy of the pre-redesign aux mapping (round-30 B8): the main→aux
        // association is now derived from the id itself (`aux-{parent_id}`),
        // so the sidecar is dead state — remove it instead of leaving it
        // behind forever. Best-effort; a leftover file is inert either way.
        store.remove_legacy_aux_sessions_sidecar();
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

    /// One-time boot housekeeping for the pre-redesign aux mapping sidecar
    /// (round-30 B8): `_aux_sessions.json` held the persisted main→aux table
    /// when aux ids were random; with derived ids (`aux-{parent_id}`) the
    /// association needs no persisted state, so the file is removed. Aux
    /// records minted under the old random scheme (`aux-<random>`) name no
    /// existing main session and are reclaimed by the retention sweep's
    /// orphan pass — the feature was never released, so no migration of
    /// those transcripts is provided.
    fn remove_legacy_aux_sessions_sidecar(&self) {
        let file = crate::platform::paths::sessions_root().join("_aux_sessions.json");
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                eprintln!("[sessions] remove legacy aux session sidecar failed: {error}")
            }
        }
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
            pinned_sessions_io: Arc::new(Mutex::new(())),
            hidden_sessions_io: Arc::new(Mutex::new(())),
            session_models_io: Arc::new(Mutex::new(())),
            session_mode_states_io: Arc::new(Mutex::new(())),
            list_cache: Arc::new(RwLock::new(None)),
            list_cache_generation: Arc::new(AtomicU64::new(0)),
            session_models: Arc::new(RwLock::new(HashMap::new())),
            pinned_sessions: Arc::new(RwLock::new(HashMap::new())),
            pinned_sessions_loaded: Arc::new(AtomicBool::new(false)),
            hidden_sessions: Arc::new(RwLock::new(HashMap::new())),
            execution_root_resolver: Arc::new(RwLock::new(None)),
            session_workspaces: Arc::new(RwLock::new(HashMap::new())),
            code_session_predicate: Arc::new(RwLock::new(None)),
            session_mode_states: Arc::new(RwLock::new(HashMap::new())),
            code_permission: Arc::new(RwLock::new(prefs_snapshot.code_permission)),
            mode_defaults: Arc::new(RwLock::new(mode_defaults_snapshot)),
            session_purged_hooks: Arc::new(RwLock::new(Vec::new())),
            session_deleted_hooks: Arc::new(RwLock::new(Vec::new())),
            #[cfg(feature = "benchmark-hooks")]
            retention_eviction_observer: Arc::new(Mutex::new(None)),
            #[cfg(feature = "benchmark-hooks")]
            pending_retention_evictions: Arc::new(Mutex::new(
                super::RetentionEvictionRecord::default(),
            )),
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
        let foreign_now = self.sessions_dir_change_token();
        loop {
            if let Some((generation, token, cached)) = self.list_cache.read().clone() {
                if generation == generation_now && token == foreign_now {
                    return Ok(cached);
                }
                // Stale-generation entry: it can be persisted while the waiting writer has
                // not yet cleared the slot or the guard has gone stale, punching through
                // the rescan — the pre-write view must not be returned as a valid snapshot.
            }
            let generation_at_scan = self.list_cache_generation.load(Ordering::Acquire);
            let token_at_scan = self.sessions_dir_change_token();
            let fresh = Arc::new(self.manager.list_sessions()?);
            let mut slot = self.list_cache.write();
            if self.list_cache_generation.load(Ordering::Acquire) == generation_at_scan {
                // No writes during the scan: safe to backfill. The write lock
                // guarantees only one miss contender lands the entry; latecomers
                // reaching the top already hit the cache (or rescan with the
                // newer generation).
                *slot = Some((generation_at_scan, token_at_scan, Arc::clone(&fresh)));
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

    /// Cheap staleness token for session records written by ANOTHER process.
    ///
    /// The generation counter above only moves on this process's own writes,
    /// which was sound while `SessionStore` was the single writer. It is not
    /// any more: a headless `agent run` sharing `PINVOU3_HOME` creates and
    /// evicts records under a live GUI, and with a generation-only guard the
    /// GUI keeps serving its boot-time snapshot indefinitely — evicted
    /// sessions stay listed and reject the click that opens them, and the
    /// run's own session never appears. Pairing the generation with the
    /// sessions directory's entry-name set makes a foreign create or delete
    /// invalidate the cache the same way a local write does.
    ///
    /// The token is the sorted set of directory entry names, not the
    /// directory's mtime: a create or delete changes the name set on every
    /// filesystem, while an mtime can stay identical when two mutations land
    /// inside one timestamp tick (CI runners and coarse-granularity mounts
    /// turned exactly that into a missed foreign delete), and delayed mtime
    /// visibility on network mounts would miss it outright. A foreign
    /// in-place rewrite of one record (an atomic temp+rename) leaves the
    /// steady-state name set unchanged and still needs the owning process's
    /// own invalidation. The cost is one getdents per call — the same order
    /// as the stat it replaces at retention-bounded directory sizes.
    /// `None` on a read failure compares equal to itself, so an unreadable
    /// directory degrades to the previous generation-only behaviour rather
    /// than rescanning on every call.
    fn sessions_dir_change_token(&self) -> Option<Vec<std::ffi::OsString>> {
        let mut names: Vec<std::ffi::OsString> = std::fs::read_dir(self.manager.sessions_dir())
            .ok()?
            .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
            .collect();
        names.sort_unstable();
        Some(names)
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
        // Aux conversations (aux- prefix) share the durable store too, so the
        // detail view and history load normally, but they are attached to
        // their main session, opened only through the aux chat panel, and
        // never enter the regular session list.
        // In benchmark builds, evaluation sessions (eval_ prefix, including GAIA
        // private problems) do not enter user history: the normal path is cleaned
        // up by the evaluation runner, and crash leftovers must not leak private
        // problems into the session list.
        // Non-benchmark desktop builds do not keep this prefix semantics, avoiding
        // changes to the ordinary session list when the benchmark feature is absent.
        out.retain(|metadata| {
            !super::validators::is_sched_session_id(&metadata.id)
                && !super::validators::is_aux_session_id(&metadata.id)
        });
        #[cfg(feature = "benchmark-hooks")]
        out.retain(|metadata| !metadata.id.starts_with("eval_"));
        out.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
        Ok(out)
    }

    pub fn load(&self, id: &str) -> Result<SavedSession> {
        self.load_with_context(id, || format!("load_session({id})"))
    }

    fn load_with_context(
        &self,
        id: &str,
        context: impl FnOnce() -> String,
    ) -> Result<SavedSession> {
        let session = self
            .manager
            .load_session_snapshot(id)
            .with_context(context)?;
        // Fail closed on case-variant aliases of AUX records: on
        // case-insensitive filesystems `AUX-<suffix>.json` resolves to the
        // real `aux-…` record while case-sensitive identity tests miss the
        // mismatch, so a caller holding an alias would operate on a
        // different session than the id claims. Requiring the loaded
        // metadata id to equal the requested id makes every downstream
        // prefix/identity decision trustworthy regardless of filesystem
        // case semantics. The check is scoped to aux-prefixed ids (round-31
        // M10): the case-alias concern it covers is aux-only, and a global
        // check would change behavior for any non-aux session whose on-disk
        // metadata.id differs from its filename — previously loadable, now
        // a hard error.
        if super::validators::is_aux_session_id(id) && session.metadata.id != id {
            bail!(
                "session id mismatch: requested '{}' but record holds '{}'",
                id,
                session.metadata.id
            );
        }
        Ok(session)
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
        // write_session_archive takes the session-store ROOT since the
        // engine-side confinement rework: it derives, validates and confines
        // the artifacts dir itself (invalid ids and links under the store are
        // loud errors), and `include_artifacts` alone gates collection.
        Ok(deepseek_tui::session_export::write_session_archive(
            &session,
            Some(self.manager.sessions_dir()),
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

    /// Deleting races with the read-modify-write persist paths
    /// (`set_title`/`update_messages`/`save`): without the same
    /// `scheduled_mutation` guard, a persist that loaded its snapshot before
    /// the delete would rename a stale transcript back over the deletion and
    /// resurrect the session (sidecar entries already purged). Take the same
    /// guard the persist paths hold so the load→persist pair cannot straddle
    /// a delete. Cross-process delete races remain unguarded (no flock here,
    /// consistent with the other sidecar writers).
    pub fn delete(&self, id: &str) -> Result<()> {
        // An invalid id must fail as itself: the derived-id probe below is
        // fail-closed (an unreadable record counts as "present"), so garbage
        // input would otherwise surface as a fabricated "delete aux session"
        // cascade error naming an aux id that was never a session
        // (round-34 minor 5).
        super::validators::validate_session_id(id)?;
        let _mutation = self.scheduled_mutation.lock();
        self.delete_locked(id)
    }

    /// Locking contract of [`Self::delete`]: the caller holds
    /// `scheduled_mutation`. The internal create-rollback paths call the
    /// public [`Self::delete`], which acquires the guard — neither rollback
    /// site runs while a persist's guard is still held. The one exception is
    /// the aux cascade below: it runs inside the guard, so it calls
    /// [`Self::delete_locked`] directly.
    fn delete_locked(&self, id: &str) -> Result<()> {
        // An aux session is never a scheduled session, so this refusal guard
        // applies to cascade targets naturally and the auxiliary-conversation
        // path cannot bypass it.
        // The prefix leg is alias-defeating (round-36 minor 2): on a
        // case-insensitive filesystem a hand-copied `SCHED-<id>.json` IS the
        // automation's record file, and the exact registry check alone
        // would let `delete("SCHED-<id>")` remove it without the
        // automation-owned path.
        if super::validators::is_sched_session_id(id) || self.is_scheduled_session(id)? {
            bail!("Scheduled-run sessions are deleted through their automation");
        }
        // Auxiliary-conversation cascade: deleting a main session first
        // deletes its aux session, whose id is derived (`aux-{id}`) — see
        // [`Self::aux_session_id`], which is fail-closed: only a genuine
        // NotFound counts as "no aux", so a transient stat fault can never
        // skip the cascade. The aux id never owns an aux itself, so the
        // recursion depth is bounded at 1 by the prefix check inside
        // `aux_session_id`. A failed aux delete aborts the main delete — but
        // "abort" is not all-or-nothing once the aux record itself committed:
        // a post-record cleanup fault on the aux leg leaves the aux durably
        // gone (its deletion hook fired) while the main record survives
        // (retention.rs documents the same window for the eviction leg). The
        // state converges: on retry the derived-id probe reports no aux, the
        // cascade is skipped, and the main delete completes.
        if let Some(aux_id) = self.aux_session_id(id) {
            // The mutation guard is already held here, so the cascade leg
            // must re-enter `delete_locked`, not the public `delete` —
            // `scheduled_mutation` is not reentrant (parking_lot), and the
            // public call would deadlock on every main-with-aux delete.
            // Depth stays bounded at 1: an aux id never owns an aux.
            self.delete_locked(&aux_id)
                .with_context(|| format!("delete aux session {aux_id} of {id}"))?;
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
        if let Some(kind) = PRE_RECORD_DELETE_FAULTS.lock().remove(id) {
            return Err(std::io::Error::new(
                kind,
                "injected failure before durable session deletion",
            ));
        }
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
    pub(crate) fn inject_pre_record_delete_fault(&self, id: &str, kind: ErrorKind) -> Result<()> {
        validate_session_id(id)?;
        PRE_RECORD_DELETE_FAULTS.lock().insert(id.to_string(), kind);
        Ok(())
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

    /// Whether the session directory carries the native code-session marker.
    ///
    /// Durable, host-independent counterpart of the registered code-session
    /// predicate: that predicate answers from an index the host has loaded,
    /// so a host which never registers one (the headless runner) reads every
    /// code session as an ordinary plain chat — and then resolves it to the
    /// wrong consent scope and the wrong instruction layer. The marker file
    /// is written beside the transcript precisely for index-less recovery,
    /// and both live under this store's session directory, so the probe
    /// belongs here rather than reaching across into the code-session
    /// feature (which would close a dependency cycle).
    pub fn has_code_session_marker(&self, id: &str) -> bool {
        // Validate before the join, like every other path out of this store:
        // an id such as `../outside` must not turn the probe into an escaping
        // path read.
        validate_session_id(id).is_ok()
            && self
                .manager
                .sessions_dir()
                .join(id)
                .join(CODE_SESSION_MARKER_FILE)
                .is_file()
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
    /// deletion path.
    ///
    /// Locking contract, stated precisely because it is narrower than "all
    /// store-side locks are released": every sidecar io mutex IS released
    /// before this fires (the purge scopes each guard for exactly that
    /// reason), but `delete` holds `scheduled_mutation` across the whole
    /// path, so a hook runs with that one held. `parking_lot::Mutex` is not
    /// reentrant, so a hook that calls back into `delete`, or into any other
    /// entry point that takes `scheduled_mutation`, self-deadlocks. Hooks
    /// must treat the store as read-only.
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
        // In-place workspace rewrite: no session created or removed, so the
        // post-persist retention rescan is skipped (round-18 MAJOR-4).
        self.persist_in_place(&session, "workspace rebind")?;
        Ok(())
    }

    /// Artifact-path rebase for directory rebind (review #463 round-10
    /// Major 2): `SavedSession.artifacts[].storage_path` persists absolute
    /// workspace paths for deliverables, and without this pass every
    /// pre-rebind deliverable keeps rendering with the vanished root — fails
    /// to open, never healed by the frontend reconcile (its relative→absolute
    /// escape hatch is spent on an already-absolute stale entry), and dropped
    /// from the cross-session deliverables index. Same load→patch→persist
    /// pattern as [`Self::set_workspace`]; only the `storage_path` fields the
    /// caller's `translate` closure maps are rewritten, so record ids,
    /// timestamps and byte sizes survive intact. The path math lives with the
    /// caller (the command layer's `rebind_target_path`, single-sourced with
    /// the binding lanes) rather than in a sessions→codex_acp dependency.
    /// Returns the number of rebased entries; 0 persists nothing.
    ///
    /// The load context deliberately does not embed the session id (same
    /// CodeQL cleartext-logging constraint as `set_workspace`).
    pub fn rebase_workspace_artifact_paths(
        &self,
        id: &str,
        translate: &dyn Fn(&Path) -> Option<PathBuf>,
    ) -> Result<usize> {
        let _mutation = self.scheduled_mutation.lock();
        let mut session = self
            .manager
            .load_session_snapshot(id)
            .with_context(|| "load_session for artifact-path rebase".to_string())?;
        let mut rebased = 0;
        for artifact in &mut session.artifacts {
            if let Some(next) = translate(&artifact.storage_path) {
                // The translate closure's `to`-side arm returns candidates
                // already under the target unchanged (retry semantics); skip
                // those so an already-converged session persists nothing.
                if next != artifact.storage_path {
                    artifact.storage_path = next;
                    rebased += 1;
                }
            }
        }
        if rebased > 0 {
            // In-place artifact-path rewrite: see persist_in_place (round-18 MAJOR-4).
            self.persist_in_place(&session, "artifact-path rebase")?;
        }
        Ok(rebased)
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
        session.metadata.title = NEW_CHAT_TITLE.to_string();
        // Per-session model: persist the sidecar first, then publish the
        // Session JSON, to avoid a write failure leaving a session that
        // appears created successfully yet falls back to another model after
        // restart.
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

    /// The aux session id of a main session, derived as `aux-{parent_id}`
    /// (round-30 B8): the main↔aux association is a pure function of the id,
    /// so "one aux per main" holds by construction and no persisted mapping
    /// exists. Parent ids are 13-char lowercase base36 and
    /// [`validate_session_id`] accepts `[A-Za-z0-9_-]`, so the derived id is
    /// always a valid session id and always carries the `aux-` prefix the
    /// zero-tools gates and list filters key on.
    pub(crate) fn aux_session_id_for(parent_id: &str) -> String {
        format!("aux-{parent_id}")
    }

    /// The forward query `main_id → Option<aux_id>` (round-30 B8): compute the
    /// derived id, then probe whether its record is on disk. Fail-closed —
    /// `durable_session_record_is_absent` counts only a genuine NotFound as
    /// absent, so a transient stat fault reads as "aux present" and no
    /// destructive path (delete / discard / retention eviction) can mistake
    /// "unknown" for "no aux". An aux id itself has no aux (the prefix check
    /// keeps the delete cascade's recursion depth at 1).
    pub fn aux_session_id(&self, main_id: &str) -> Option<String> {
        if super::validators::is_aux_session_id(main_id) {
            return None;
        }
        let aux_id = Self::aux_session_id_for(main_id);
        if self.durable_session_record_is_absent(&aux_id) {
            None
        } else {
            Some(aux_id)
        }
    }

    /// Creates the auxiliary conversation of `parent_id` under the derived id
    /// `aux-{parent_id}` (same prefixed-creation pattern as the `sched-`
    /// precedent): a fixed internal default title, model and workspace
    /// inherited from the main session (including the per-session model
    /// binding in `_session_models.json`). Any failed step rolls back the
    /// persisted session JSON so no orphan aux session survives.
    /// Reuse semantics (return the existing record when it loads) live in
    /// [`Self::get_or_create_aux_session`], not here. Concurrent creators
    /// converge on the same derived id and write the same content, so no
    /// creation lock is needed.
    ///
    /// Crate-visible for tests and the command layer only — outside callers
    /// must go through [`Self::get_or_create_aux_session`].
    pub(crate) fn create_aux_session(&self, parent_id: &str) -> Result<SessionMetadata> {
        // Reject aux-of-aux in the creation path itself (not only via the
        // get-or-create wrapper): an auxiliary conversation must not own
        // another one, and an aux session is itself a Chat kind, so the
        // command layer's `ensure_chat_session` cannot catch it.
        if super::validators::is_aux_session_id(parent_id) {
            bail!("Auxiliary session '{parent_id}' cannot own an aux session");
        }
        if super::validators::is_sched_session_id(parent_id) {
            bail!("Scheduled-run session '{parent_id}' cannot own an aux session");
        }
        let parent = match self.load(parent_id) {
            Ok(parent) => parent,
            // A genuinely missing parent (deleted, or evicted between the
            // panel's last action and this ensure) is a different failure
            // class than a transient read fault: name it so the panel's
            // ensureFailed is recognizable instead of a generic load error.
            Err(error) if is_not_found_error(&error) => {
                bail!("the parent session no longer exists (deleted or evicted)")
            }
            Err(error) => {
                return Err(error).with_context(|| "load the parent session for aux creation");
            }
        };
        let id = Self::aux_session_id_for(parent_id);
        // Note: the copied `metadata.workspace` is record-keeping only — no
        // production reader resolves an aux session's roots from it
        // (`SessionStore::session_roots` never reads `metadata.workspace`, and
        // no workspace-binding sidecar is written for aux), so the aux
        // execution root always resolves to the private
        // `sessions/aux-<id>/workspace`. Keep it truthful for debugging, but
        // do not read it as a binding.
        let mut session = create_saved_session_with_id_and_mode(
            id.clone(),
            &[],
            &parent.metadata.model,
            &parent.metadata.workspace,
            0,
            None,
            None,
        );
        session.metadata.title = AUX_SESSION_TITLE.to_string();
        // The per-session model binding lives in the `_session_models.json`
        // sidecar, not in `metadata.model`: same order as `create_new` — write
        // the sidecar before publishing the session JSON; when a later step
        // fails and the session is rolled back, `purge_session_side_maps`
        // removes this binding along with it.
        // Concurrent-ensure narrowing (round-34 minor 1, hoisted above the
        // model-sidecar write per round-36 minor 1): a lagging ensure that
        // observed NotFound at its own entry load must not overwrite a
        // record a concurrent ensure (or a turn on it) just published at
        // the same derived path — a healthy record is reused, and a record
        // that loads with any other error fails closed exactly like the
        // entry probe (never overwrite what cannot be read). The hoist also
        // NARROWS the losing-creator window on the winner's
        // `_session_models.json` binding (round-37 MAJOR-3: a winner
        // publishing between this re-check and the sidecar write still gets
        // its binding overwritten with the parent's current choice — the
        // re-check narrows, it does not stop, so the earlier reviewer
        // wording said too much). This re-check is not an atomic create:
        // a save landing after another creator's first transcript write can
        // still clobber, and closing that residual window needs a
        // foundation-level exclusive-create — disclosed.
        match self.load(&id) {
            Ok(existing) => return Ok(existing.metadata),
            Err(error) if !is_not_found_error(&error) => {
                return Err(error).with_context(|| "re-check the aux record before create");
            }
            Err(_) => {}
        }
        if let Some(model_id) = self.session_model_override(parent_id) {
            self.set_session_model_id(&id, Some(model_id))?;
        }
        if let Err(error) = self.save(&session) {
            let rollback = self.delete(&id);
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback_error) => {
                    anyhow::anyhow!(
                        "{error:#}; rollback of the aux record failed: {rollback_error:#}"
                    )
                }
            });
        }
        // The save above can trigger the retention sweep; the pair-liveness
        // ordering protects a parent whose aux record is fresh, but a
        // concurrent out-of-band delete can still commit the parent between
        // the load at entry and now. Re-check and roll the newborn record
        // back instead of leaving a dead-parent/live-aux orphan.
        // NotFound-only (round-20 minor-9, the taxonomy used everywhere else
        // in this module): only a genuinely gone parent rolls the create
        // back. A transient load fault is not an eviction — rolling back
        // there would destroy a healthy aux record and misreport the cause,
        // so it propagates as an error instead.
        let parent_evicted = match self.load(parent_id) {
            Ok(_) => false,
            Err(error) if is_not_found_error(&error) => true,
            Err(error) => {
                return Err(error)
                    .with_context(|| "re-check the parent session after aux creation");
            }
        };
        if parent_evicted {
            let rollback = self.delete(&id);
            // Ids stay out of these messages too: the command layer surfaces
            // them to the caller and the boot log prints the chain.
            return Err(match rollback {
                Ok(()) => {
                    anyhow::anyhow!("the parent session was evicted while creating its aux session")
                }
                Err(rollback_error) => anyhow::anyhow!(
                    "the parent session was evicted while creating its aux session; rollback of the aux record failed: {rollback_error:#}"
                ),
            });
        }
        Ok(session.metadata)
    }

    /// Get-or-create for the derived aux id: the record exists and loads →
    /// return it; a genuine NotFound → create it. Every other load failure
    /// propagates — with a derived id, "recreate" would overwrite the record
    /// at the same path, so an unreadable record (transient fault OR
    /// permanent corruption) must never fall into the creation leg
    /// (round-30 B4/B8). The panel surfaces the error as ensureFailed; the
    /// user's recovery for a truly dead record is an explicit discard
    /// (`discard_aux_session` deletes by id without parsing the record).
    /// An auxiliary conversation must not own an aux session (aux-of-aux): an
    /// aux session is itself a Chat kind, so the command layer's
    /// `ensure_chat_session` cannot catch it — the single creation entry point
    /// must reject it explicitly.
    pub fn get_or_create_aux_session(&self, parent_id: &str) -> Result<SessionMetadata> {
        if super::validators::is_aux_session_id(parent_id) {
            bail!("Auxiliary session '{parent_id}' cannot own an aux session");
        }
        if super::validators::is_sched_session_id(parent_id) {
            bail!("Scheduled-run session '{parent_id}' cannot own an aux session");
        }
        let aux_id = Self::aux_session_id_for(parent_id);
        match self.load(&aux_id) {
            Ok(aux) => Ok(aux.metadata),
            Err(error) if is_not_found_error(&error) => self.create_aux_session(parent_id),
            Err(error) => Err(error).with_context(|| "load the aux session of this task"),
        }
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

    /// Whether a durable chat record already exists for `id`. The headless
    /// runner checks this before creating a fresh session, so a recycled pid
    /// replaying the same fresh-id counter cannot silently overwrite a kept
    /// session's record.
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) fn chat_session_record_exists(&self, id: &str) -> bool {
        validate_session_id(id).is_ok()
            && chat_session_file(&self.manager, id)
                .map(|path| path.exists())
                .unwrap_or(false)
    }

    /// Whether the durable chat record for `id` carries any messages. The
    /// headless runner uses this to tell a zero-message stub (safe to clean
    /// up) from a ran-and-errored transcript (the only copy — keep it
    /// inspectable). An unloadable record reports `Err`: callers must treat
    /// unknown state as "keep".
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) fn chat_session_has_messages(&self, id: &str) -> Result<bool> {
        validate_session_id(id)?;
        let session = self
            .manager
            .load_session_snapshot(id)
            .with_context(|| format!("load chat session {id} for stub classification"))?;
        Ok(!session.messages.is_empty())
    }

    /// Whether the session still wears the factory title ([`NEW_CHAT_TITLE`]).
    /// A rename away from the placeholder is ownership — the adoption marker
    /// the agentic teardown decisions consult before deleting a run's own
    /// session. An unreadable record reports `Err` and is NOT proven
    /// factory-titled: callers must treat unknown state as "keep".
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub(crate) fn chat_session_factory_titled(&self, id: &str) -> Result<bool> {
        validate_session_id(id)?;
        let session = self
            .manager
            .load_session_snapshot(id)
            .with_context(|| format!("load chat session {id} for adoption classification"))?;
        Ok(session.metadata.title == NEW_CHAT_TITLE)
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
        // The id is caller-chosen and headless sessions persist by default,
        // so an existing record must fail loud instead of being silently
        // replaced: a recycled pid replaying the same fresh-id counter (or an
        // eval rerun against a kept session) would otherwise destroy the kept
        // transcript and inherit its pin onto the new stub.
        if self.chat_session_record_exists(&id) {
            bail!("session record {id} already exists; refusing to overwrite it");
        }
        let mut session = create_saved_session_with_id_and_mode(
            id.clone(),
            &[],
            &model,
            &workspace,
            0,
            None,
            None,
        );
        // Headless sessions persist by default and surface in the GUI history,
        // so they carry the same new-chat placeholder sentinel as GUI-created
        // sessions (an eval-internal label would leak into every UI language).
        // The stored value is the fixed zh sentinel, not a localized string:
        // localization happens at render time, where the GUI maps any of the
        // three per-language sentinels to the current UI language.
        session.metadata.title = NEW_CHAT_TITLE.to_string();
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

/// True when `error` carries an `io::Error` of kind `NotFound` anywhere in its
/// chain — the "record is not on disk" case. Any other failure kind must be
/// treated as "unknown" rather than "absent", so transient IO errors are never
/// conflated with a missing record.
pub(super) fn is_not_found_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == ErrorKind::NotFound)
    })
}

/// True when `error` carries an `io::Error` of kind `InvalidData` anywhere in
/// its chain — the "record is permanently unreadable" case (truncated body, a
/// newer `schema_version` than this build supports, malformed receipts).
/// Tests use it to prove a fixture really produces the permanent-corruption
/// class (as opposed to a transient fault, which the same paths treat very
/// differently).
#[cfg(test)]
pub(super) fn is_invalid_data_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == ErrorKind::InvalidData)
    })
}
