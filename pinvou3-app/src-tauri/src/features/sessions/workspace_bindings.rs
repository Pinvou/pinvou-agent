//! Per-session sidecar for the user-selected working directory of plain chat
//! sessions (lives and dies with the session directory).
//!
//! When a plain (assistant/engine) session is created, the frontend may let the
//! user pick a working directory. Once bound, [`SessionStore::session_roots`]
//! resolves the session's execution root to the bound directory (the engine cwd)
//! while the ledger root stays session-private — sharing the dual-root semantics
//! of `session_roots_for` with native code sessions' project-directory bindings.
//! The `execution_root_resolver` closures injected into bridge/SessionStore by
//! the app composition root fall back to this store when the native code session
//! lookup misses, so the bridge side (prompt environment section, AGENTS.md
//! injection, connector scope, audit root) behaves identically for both kinds of
//! bound sessions — a bound directory is equally a prompt-injection surface, and
//! the safety posture follows the binding, not the mode.
//!
//! Storage shape (consolidated binding storage): the binding record is a
//! per-session sidecar inside the session-private directory,
//! `<sessions>/<id>/workspace-binding.json`, using the same mechanism as native
//! code sessions' `code-session.json` — the binding lives and dies with the
//! session directory, so boot-time ghost cleanup of a global table is no longer
//! needed. The in-memory `session_workspaces` map degenerates into a read
//! cache: written on bind, backfilled from the sidecar on a read miss (bindings
//! created by other processes are visible too), and cleared on deletion /
//! retention cleanup.
//!
//! The legacy global table `_session_workspaces.json` was an intermediate
//! development format that never shipped with `main`; boot-time
//! [`SessionStore::migrate_legacy_session_workspaces`] exists only to converge
//! homes of intermediate dev builds: live-session entries are rewritten as
//! sidecars one by one and the old file is then removed; entries whose write
//! failed stay untouched in the old file (the in-memory table takes over
//! resolution for this run) and the next boot retries, without blocking startup.
//! Once all legacy entries are migrated, the migration becomes a permanent
//! no-op (missing file returns immediately).

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use super::{SessionStore, validate_session_id};

/// Round-8 review M4: test-only crash seam between the legacy-table rewrite
/// (phase 2) and the sidecar pass (phase 3) of `rebind_workspace_bindings`.
/// Armed by `inject_rebind_crash_after_legacy_rewrite`; production never
/// touches it.
#[cfg(test)]
static REBIND_CRASH_AFTER_LEGACY_REWRITE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Outcome of rebind_workspace_bindings: translated entries plus per-entry
/// failure isolation (review #464 MAJOR 4: an invalid id or a single write
/// failure no longer aborts the whole round; failures go into the failure
/// list and the command layer merges them into the report).
#[derive(Debug, Default)]
pub struct RebindBindingsOutcome {
    pub rebound: Vec<(String, PathBuf)>,
    pub failed_session_ids: Vec<String>,
}

/// Plan of a plain-lane rebind (review #463 round-13 M1): the candidate
/// translations plus the ids rejected before any write. Produced by
/// [`SessionStore::plan_rebind_workspace_bindings`] — which also syncs the
/// legacy global table, so a returned plan means the on-disk table already
/// carries every translation — and consumed by
/// [`SessionStore::apply_rebind_workspace_bindings`].
#[derive(Debug, Default)]
pub struct RebindBindingsPlan {
    /// (session id, translated path, sidecar file) triples to move.
    entries: Vec<(String, PathBuf, PathBuf)>,
    /// Candidates rejected in the planning half (invalid session id); carried
    /// into the outcome so the report treats them like write failures.
    failed_session_ids: Vec<String>,
}

/// Why the legacy-table sync refused a rebind run (review #463 round-13 M2):
/// the two causes have different remedies, so the planning half maps them to
/// different typed markers — an unwritable table is fixed with permissions,
/// a corrupt one only by repairing or removing the file.
#[derive(Debug)]
enum LegacyTableSyncFailure {
    /// The table is on disk but THIS run's read/parse attempt failed. It must
    /// not be rewritten or removed (the "repair it and retry" door, #464
    /// round-3 minor 6), so a run with a non-empty plan cannot publish its
    /// translations and aborts.
    Corrupt,
    /// The table parsed but the merged rewrite (or the empty-table removal)
    /// could not be persisted.
    Unwritable,
}

/// Schema version of the binding sidecar; used for migration if fields evolve.
const SESSION_WORKSPACE_SIDECAR_VERSION: u32 = 1;
/// Legacy global binding table of the intermediate format (never shipped with
/// `main`; removed once the boot-time migration succeeds).
const LEGACY_SESSION_WORKSPACES_FILE: &str = "_session_workspaces.json";
/// Per-session binding sidecar file name (inside the session-private directory).
const SESSION_WORKSPACE_SIDECAR_FILE: &str = "workspace-binding.json";

/// Binding sidecar contents. `path` is the absolute directory after
/// `validate_user_workspace_path` canonicalization; `bound_at` is metadata only
/// and plays no part in restore semantics.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionWorkspaceSidecar {
    version: u32,
    path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bound_at: Option<i64>,
}

fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

/// Folded-key prefix match for rebind candidate selection (same domain rule as
/// the codex lane): bindings are stored canonicalized at bind time and the
/// command entry normalizes `from` the same way, so a case-only rename on
/// Windows still matches while `/a/bc` never matches `/a/b`. The component
/// predicate is the shared `platform::os::path_identity_is_same_or_nested`
/// (review #463 round-8 elegance); the component cut stays at the call site.
fn folded_path_is_same_or_nested(path: &Path, base: &Path) -> bool {
    let path_key = crate::platform::os::filesystem_path_identity_key(&path.to_string_lossy());
    let base_key = crate::platform::os::filesystem_path_identity_key(&base.to_string_lossy());
    crate::platform::os::path_identity_is_same_or_nested(
        path_key.trim_end_matches('/'),
        base_key.trim_end_matches('/'),
    )
}

/// A future-version format must never be silently parsed as the current version:
/// refuse to read it and treat it as missing (bind rewrites it in the current
/// version, which self-heals); all parse errors are logged.
fn read_workspace_sidecar(path: &Path) -> Option<SessionWorkspaceSidecar> {
    // Present-but-unreadable must not masquerade as absent (review #463
    // round-11 minor 2): both the scan and the post-pass fence consume this
    // accessor, so a silent skip reports success while the session's binding
    // was never examined. Same disclosure treatment as the codex lane.
    let payload = match std::fs::read(path) {
        Ok(payload) => payload,
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(error) => {
            // Log hygiene (review #463 round-13 M3): the path embeds
            // sessions/<id>/, so only the error kind is logged — the same
            // rule the rebind write-path logs below follow (8fc7f7201).
            eprintln!(
                "[sessions] read workspace binding sidecar failed: {:?}",
                error.kind()
            );
            return None;
        }
    };
    match serde_json::from_slice::<SessionWorkspaceSidecar>(&payload) {
        Ok(sidecar) if sidecar.version <= SESSION_WORKSPACE_SIDECAR_VERSION => Some(sidecar),
        Ok(sidecar) => {
            eprintln!(
                "[sessions] workspace binding sidecar version {} above supported {} ({}), ignored",
                sidecar.version,
                SESSION_WORKSPACE_SIDECAR_VERSION,
                path.display()
            );
            None
        }
        Err(error) => {
            eprintln!(
                "[sessions] parse workspace binding sidecar failed ({}): {error}",
                path.display()
            );
            None
        }
    }
}

/// Cache backfill for [`SessionStore::session_workspace_binding`]
/// (insert-conditional, review #463 F4): the sidecar was read OUTSIDE the
/// cache lock, so a concurrent `rebind_workspace_binding` may have moved the
/// binding (sidecar first, then cache) while the read was in flight. Blindly
/// inserting the read result afterwards would resurrect the OLD path in the
/// cache — and the cache wins resolution until restart, silently undoing the
/// rebind for this process. Under the write lock, an entry that appeared
/// meanwhile was written sidecar-then-cache and is therefore at least as
/// fresh as the value read off disk, so it wins; only a still-vacant slot is
/// backfilled. Residual: none for the rebind race — the rebind's own cache
/// write either landed before this lock (occupied, kept) or lands after
/// (overwrites); a failed rebind leaves the cache untouched, matching the
/// all-or-nothing convention of `bind_session_workspace`.
pub(super) fn backfill_workspace_binding_cache(
    cache: &RwLock<HashMap<String, PathBuf>>,
    id: &str,
    read: PathBuf,
) -> PathBuf {
    let mut cache = cache.write();
    match cache.entry(id.to_string()) {
        std::collections::hash_map::Entry::Occupied(entry) => entry.get().clone(),
        std::collections::hash_map::Entry::Vacant(entry) => entry.insert(read).clone(),
    }
}

impl SessionStore {
    fn session_workspace_sidecar_path(&self, id: &str) -> PathBuf {
        self.manager
            .sessions_dir()
            .join(id)
            .join(SESSION_WORKSPACE_SIDECAR_FILE)
    }

    /// Binds the session working directory and atomically persists it to the
    /// sidecar inside the session-private directory. Requires the session's
    /// persistent record (`<id>.json`) to already exist — a binding is subordinate
    /// session data, so no directory is fabricated for unknown ids; the caller
    /// (the create_session command) creates the session first, then binds.
    /// On persist failure returns Err and leaves the in-memory cache untouched —
    /// the caller (the create_session command) uses that to delete the just-created
    /// empty session, leaving no session that merely "looked bound" and lost the
    /// binding after restart.
    pub fn bind_session_workspace(&self, id: &str, path: PathBuf) -> Result<()> {
        validate_session_id(id)?;
        let record = self.manager.sessions_dir().join(format!("{id}.json"));
        if !record.is_file() {
            anyhow::bail!("cannot bind workspace: session record {id} does not exist");
        }
        let sidecar = SessionWorkspaceSidecar {
            version: SESSION_WORKSPACE_SIDECAR_VERSION,
            path,
            bound_at: Some(now_unix_secs()),
        };
        let file = self.session_workspace_sidecar_path(id);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create session dir {}", parent.display()))?;
        }
        let payload =
            serde_json::to_vec_pretty(&sidecar).context("serialize session workspace binding")?;
        crate::platform::filesystem::atomic_write(&file, &payload)
            .with_context(|| format!("persist session workspace binding to {}", file.display()))?;
        self.session_workspaces
            .write()
            .insert(id.to_string(), sidecar.path);
        Ok(())
    }

    /// Reads the session's user working-directory binding (None when unbound; the
    /// execution root falls back to the session-private directory). On an in-memory
    /// cache miss, re-reads the sidecar; a leftover sidecar whose session record no
    /// longer exists (directory left behind by a partially failed deletion) is
    /// treated as None, matching the old ghost-cleanup semantics.
    pub fn session_workspace_binding(&self, id: &str) -> Option<PathBuf> {
        if let Some(path) = self.session_workspaces.read().get(id).cloned() {
            return Some(path);
        }
        if validate_session_id(id).is_err()
            || !self
                .manager
                .sessions_dir()
                .join(format!("{id}.json"))
                .is_file()
        {
            return None;
        }
        let sidecar = read_workspace_sidecar(&self.session_workspace_sidecar_path(id))?;
        Some(backfill_workspace_binding_cache(
            &self.session_workspaces,
            id,
            sidecar.path,
        ))
    }

    /// Best-effort deletion of the binding sidecar file; NotFound counts as
    /// deleted. Session-deletion directory cleanup usually removes it already;
    /// this covers the "session still present, only unbinding" case and the
    /// leftover-directory fallback.
    pub(crate) fn remove_workspace_sidecar_file(&self, id: &str) {
        let file = self.session_workspace_sidecar_path(id);
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "[sessions] remove workspace binding sidecar failed ({}): {error:#}",
                file.display()
            ),
        }
    }

    /// Whether the session still owns a durable record. A binding without one
    /// is inert: [`SessionStore::session_workspace_binding`] refuses to read a
    /// sidecar whose `<id>.json` is gone, so rebind must not claim to have
    /// moved such a ghost.
    fn workspace_binding_owner_exists(&self, id: &str) -> bool {
        validate_session_id(id).is_ok()
            && self
                .manager
                .sessions_dir()
                .join(format!("{id}.json"))
                .is_file()
    }

    /// Every plain-chat working-directory binding currently under the `from`
    /// prefix — the rebind candidate set for this lane (review #463 round-8
    /// B1). A chat created the ordinary way (`create_session` with a
    /// `workspace_path`, which then calls [`Self::bind_session_workspace`])
    /// carries only `sessions/<id>/workspace-binding.json` plus the in-memory
    /// cache: it has no codex index record and no `code-session.json`, so the
    /// code-session scan is structurally blind to it while
    /// [`SessionStore::session_roots`] resolves its execution directory from
    /// exactly this binding. Leaving it behind would keep the next turn in the
    /// vanished folder and recreate it via `create_dir_all`.
    ///
    /// The in-memory cache is scanned as well: entries whose sidecar write has
    /// not succeeded yet (legacy migration leftovers) live only there and are
    /// still what resolution reads. Session ids without a durable record are
    /// skipped — their binding is inert (see
    /// [`Self::workspace_binding_owner_exists`]).
    pub(crate) fn workspace_bindings_under(&self, from: &Path) -> Vec<(String, PathBuf)> {
        let sessions_dir = self.manager.sessions_dir();
        let mut matched: Vec<(String, PathBuf)> = Vec::new();
        {
            // Snapshot the cache, then drop the lock: the directory scan below
            // must not run under it.
            let cache: Vec<(String, PathBuf)> = self
                .session_workspaces
                .read()
                .iter()
                .map(|(id, path)| (id.clone(), path.clone()))
                .filter(|(_, path)| folded_path_is_same_or_nested(path, from))
                .collect();
            for (id, path) in cache {
                if self.workspace_binding_owner_exists(&id) {
                    matched.push((id, path));
                }
            }
        }
        let entries = match std::fs::read_dir(&sessions_dir) {
            Ok(entries) => entries,
            // No sessions directory yet is normal (no session ever created).
            Err(error) if error.kind() == ErrorKind::NotFound => return matched,
            Err(error) => {
                eprintln!(
                    "[sessions] sessions root unreadable during rebind workspace-binding scan: {} ({error})",
                    sessions_dir.display()
                );
                return matched;
            }
        };
        // An entry that cannot be stat-ed is disclosed, not silently skipped
        // (review #463 round-12 minor 2): the scan and the post-pass fence
        // both consume this iterator, so a dropped entry reads as absence and
        // the run could report success without ever examining that session.
        for entry in entries.filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(error) => {
                eprintln!("[sessions] rebind workspace-binding scan dropped an entry ({error})");
                None
            }
        }) {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let Some(id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if matched.iter().any(|(sid, _)| *sid == id) {
                continue;
            }
            if !self.workspace_binding_owner_exists(&id) {
                continue;
            }
            let Some(sidecar) = read_workspace_sidecar(&self.session_workspace_sidecar_path(&id))
            else {
                continue;
            };
            if folded_path_is_same_or_nested(&sidecar.path, from) {
                matched.push((id, sidecar.path));
            }
        }
        matched
    }

    /// Rebinds a plain-chat working-directory binding onto `next` (review #463
    /// round-8 B1): the sidecar is the durable binding and survives restart,
    /// while the in-memory cache is what resolution reads for the rest of this
    /// run — a stale cache entry would keep the old directory in use even
    /// after the sidecar moved. `bound_at` is metadata only and is preserved.
    ///
    /// Returns whether the durable sidecar is fresh. `false` means the old
    /// path is still on disk, so a restart would resurrect it; the cache is
    /// then deliberately left untouched — the same all-or-nothing convention
    /// as [`Self::bind_session_workspace`] — and the caller must report the
    /// session as failed. The retry converges: the sidecar still matches the
    /// `from` prefix, so the next scan finds it again.
    /// Whether ANY durable plain-lane binding artifact still references the
    /// session: a cache entry or the binding sidecar on disk (review #463
    /// round-10 minor 4). Session deletion clears the cache and removes the
    /// session directory (sidecar included), so `false` means the session
    /// died mid-rebind — the report and the event stream must not count a
    /// dead id as rebound.
    pub(crate) fn workspace_binding_artifacts_exist(&self, id: &str) -> bool {
        self.session_workspaces.read().contains_key(id)
            || self.session_workspace_sidecar_path(id).exists()
    }

    pub(crate) fn rebind_workspace_binding(&self, id: &str, next: PathBuf) -> bool {
        if validate_session_id(id).is_err() {
            return false;
        }
        let previous = read_workspace_sidecar(&self.session_workspace_sidecar_path(id));
        let sidecar = SessionWorkspaceSidecar {
            version: SESSION_WORKSPACE_SIDECAR_VERSION,
            path: next.clone(),
            bound_at: previous.and_then(|sidecar| sidecar.bound_at),
        };
        let payload = match serde_json::to_vec_pretty(&sidecar) {
            Ok(payload) => payload,
            Err(error) => {
                eprintln!("[sessions] serialize rebound workspace binding failed: {error:#}");
                return false;
            }
        };
        let file = self.session_workspace_sidecar_path(id);
        if let Some(parent) = file.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                // Only the error kind is logged: the io message embeds the
                // sessions/<id>/ path (CodeQL cleartext-logging, review #463
                // round 7).
                eprintln!(
                    "[sessions] create session dir during rebind failed: {:?}",
                    error.kind()
                );
                return false;
            }
        }
        if let Err(error) = crate::platform::filesystem::atomic_write(&file, &payload) {
            // Same CodeQL constraint: log the kind, never the path-bearing
            // io message. The session reaches the user through the report's
            // failed list instead.
            eprintln!(
                "[sessions] rebind workspace binding sidecar failed: {:?}",
                error.kind()
            );
            return false;
        }
        self.session_workspaces.write().insert(id.to_string(), next);
        true
    }

    /// Directory rebinding (the broken-link repair channel), split into a
    /// planning half and a mutation half (review #463 round-13 M1) so the
    /// command layer can run the legacy-table sync BEFORE any rebind lane
    /// mutates: [`Self::plan_rebind_workspace_bindings`] scans the candidates,
    /// rejects invalid ids and syncs the legacy global table;
    /// [`Self::apply_rebind_workspace_bindings`] then moves the sidecars and
    /// the in-memory cache. [`Self::rebind_workspace_bindings`] runs the two
    /// halves back to back. Same semantics and idempotency as
    /// SessionAgentStore::rebind_workspace_prefix — a from→to rerun with no
    /// matches is a no-op, and failures can be retried as a whole. Session
    /// metadata (the metadata.workspace display field) is rewritten uniformly
    /// by the command layer via set_workspace.
    ///
    /// Per-entry isolation (#464 MAJOR 4): an invalid id (boot migration
    /// leaves unvalidated failed entries in the in-memory legacy table) or a
    /// single write failure no longer aborts the whole round with `?` — by
    /// then the agent index has already been translated, so aborting would
    /// make the same entry fail again on every retry; failures go into
    /// `failed_session_ids` and the command layer merges them into the report.
    ///
    /// The candidate scan is the tolerant one shared with the command layer's
    /// fence (`workspace_bindings_under`): an unreadable sessions root
    /// is logged and yields the entries already found, so a transient
    /// `read_dir` failure cannot abort an otherwise healthy rebind. The `?` on
    /// the planning signature is reserved for the legacy-table sync.
    #[cfg(test)]
    pub(crate) fn inject_rebind_crash_after_legacy_rewrite(&self) {
        REBIND_CRASH_AFTER_LEGACY_REWRITE.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Phase 1 + phase 2 of the plain-lane rebind: build the plan and sync the
    /// legacy global table. No binding artifact has moved when this returns,
    /// so a sync failure aborts with the run truly untouched (table@from +
    /// sidecars@from consistent) and the retry redoes the whole run once the
    /// cause is fixed.
    pub fn plan_rebind_workspace_bindings(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<RebindBindingsPlan> {
        // Phase 1 — plan. Candidates = sidecar scan ∪ in-memory legacy table,
        // via workspace_bindings_under (already the union — review #464 round-5
        // nit: a second in-memory union here duplicated it exactly). Nothing is
        // written yet: the plan is what the legacy-table rewrite must publish
        // BEFORE the sidecars move, and an invalid id is rejected here instead
        // of half-way through the write phase.
        let candidates: Vec<(String, PathBuf)> = self.workspace_bindings_under(from);
        let mut plan = RebindBindingsPlan::default();
        for (id, path) in candidates {
            // Shared containment + suffix cut (round-8 review should-fix 9):
            // one platform predicate serves all three lanes.
            let Some(suffix) = crate::platform::os::path_relative_suffix_under(&path, from) else {
                continue;
            };
            // The id is validated before joining the path (same gate as
            // bind_session_workspace): a traversal-shaped id would write
            // outside sessions_dir.
            if validate_session_id(&id).is_err() {
                // Log hygiene (round-7 should-fix): the rejected id (and the
                // validator's message, which echoes it) stays out of the log;
                // the report's failure list is the disclosure channel.
                eprintln!("[sessions] rebind skipped a candidate with an invalid session id");
                plan.failed_session_ids.push(id);
                continue;
            }
            let next = if suffix.as_os_str().is_empty() {
                to.to_path_buf()
            } else {
                to.join(suffix)
            };
            let sidecar_path = self
                .manager
                .sessions_dir()
                .join(&id)
                .join(SESSION_WORKSPACE_SIDECAR_FILE);
            plan.entries.push((id, next, sidecar_path));
        }
        // Phase 2 — the legacy global table, before any sidecar moves (review
        // #464 round-6 finding 5) and, at the command layer, before the codex
        // lane runs at all (review #463 round-13 M1). The old order (sidecars
        // first, table last) left a crash window that heals in the DANGEROUS
        // direction: fresh sidecars on disk plus a stale table, with no report
        // possible, so the next boot re-binds the deleted directory. Writing
        // the translated table first means every crash window heals forward —
        // a boot sees either the old table with old sidecars (no rebind
        // happened), or the new table with old/new sidecars, where the boot
        // migration rewrites the stragglers to `to`. The table holds
        // translated values only, so a partial sidecar failure is finished by
        // the boot migration rather than undone by it.
        //
        // A failed sync aborts the run (review #463 round-12 B1): a surviving
        // stale table (table@from) over fresh sidecars (sidecar@to) is the
        // resurrection state — every later boot re-binds the vanished `from`
        // over the moved sidecar, silently undoing a reported success. The
        // command layer calls this before the codex lane mutates, so the
        // abort copy "nothing was moved" is literally true, and the retry
        // redoes the whole run once the cause is fixed. This does NOT
        // conflict with the crash-heal pin: that window is process death
        // between the sync and the sidecar pass (table@to over sidecars@from),
        // which this Err path never produces.
        self.sync_legacy_session_workspaces(&plan.entries)
            .map_err(|failure| match failure {
                LegacyTableSyncFailure::Corrupt => anyhow::anyhow!(
                    "REBIND_LEGACY_TABLE_CORRUPT: the legacy workspace table could not be parsed, so nothing was moved — repair or remove the corrupt table and retry"
                ),
                LegacyTableSyncFailure::Unwritable => anyhow::anyhow!(
                    "REBIND_LEGACY_TABLE_UNWRITABLE: the legacy workspace table could not be synced, so nothing was moved — make the sessions directory writable and retry"
                ),
            })?;
        Ok(plan)
    }

    /// Phase 3 — move the in-memory cache and the sidecars of a planned
    /// rebind. The legacy table already carries the plan's translations (the
    /// planning half synced it), so a fault anywhere in this pass leaves
    /// table@to over sidecars@from and the next boot heals forward. Per-entry
    /// isolation: one failed write does not abort the round (review #464
    /// MAJOR 4).
    pub fn apply_rebind_workspace_bindings(
        &self,
        plan: RebindBindingsPlan,
    ) -> RebindBindingsOutcome {
        let mut outcome = RebindBindingsOutcome {
            rebound: Vec::new(),
            failed_session_ids: plan.failed_session_ids,
        };
        // Test-only crash seam between the table sync and the sidecar pass
        // (round-8 review M4): it simulates a process death exactly where the
        // phase order matters — the translated table is on disk while every
        // sidecar is still at `from` — and returns "successfully crashed"
        // instead of erroring, like a killed process would. Production never
        // arms it.
        #[cfg(test)]
        if REBIND_CRASH_AFTER_LEGACY_REWRITE.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return outcome;
        }
        for (id, next, sidecar_path) in plan.entries {
            // In-memory legacy-table entries may have no session directory
            // (never written as a sidecar); atomic_write does not create
            // parent directories, so create it first (same as
            // bind_session_workspace).
            let write = (|| -> Result<()> {
                if let Some(parent) = sidecar_path.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("create session dir {}", parent.display()))?;
                }
                // bound_at is metadata only: keep it as-is, same convention as
                // the codex store's rebind; it is no longer reset to None
                // (review #452 finding 3).
                let bound_at = read_workspace_sidecar(&sidecar_path).and_then(|s| s.bound_at);
                let updated = SessionWorkspaceSidecar {
                    version: SESSION_WORKSPACE_SIDECAR_VERSION,
                    path: next.clone(),
                    bound_at,
                };
                let payload = serde_json::to_vec_pretty(&updated)
                    .context("serialize session workspace binding")?;
                crate::platform::filesystem::atomic_write(&sidecar_path, &payload).with_context(
                    || {
                        format!(
                            "rebind session workspace binding {}",
                            sidecar_path.display()
                        )
                    },
                )
            })();
            if let Err(error) = write {
                // Log hygiene (round-7 should-fix): the failure list in the
                // report is the only channel that names sessions; logs stay
                // free of ids and host paths, matching the singular-path
                // arm's `error.kind()` style.
                let io_kind = error
                    .chain()
                    .rev()
                    .find_map(|cause| cause.downcast_ref::<std::io::Error>())
                    .map(|io_error| io_error.kind());
                eprintln!(
                    "[sessions] rebind workspace binding write failed (io kind: {io_kind:?})"
                );
                outcome.failed_session_ids.push(id);
                continue;
            }
            self.session_workspaces
                .write()
                .insert(id.clone(), next.clone());
            outcome.rebound.push((id, next));
        }
        outcome
    }

    /// The two halves back to back (see the doc above
    /// [`Self::plan_rebind_workspace_bindings`]): the form for callers with no
    /// other lane to interleave between the sync and the sidecar pass.
    pub fn rebind_workspace_bindings(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<RebindBindingsOutcome> {
        let plan = self.plan_rebind_workspace_bindings(from, to)?;
        Ok(self.apply_rebind_workspace_bindings(plan))
    }

    /// Legacy-table sync of the rebind (phase 2, run inside
    /// [`Self::plan_rebind_workspace_bindings`] so it lands before any lane
    /// mutates). #445 round-2 removed the generic persistence (no other call
    /// surface); all that remains here: if the table is on disk, atomically
    /// rewrite it wholesale with the merged view (the in-memory table ∪ this
    /// run's translations) — and if the merged view is empty, delete the file
    /// (no entries left to resurrect).
    ///
    /// The parse is re-attempted PER RUN (review #463 round-13 M2): the gate
    /// must describe the file NOW, not the boot — a table the user repaired
    /// or removed out-of-band converges without an app restart. Only a file
    /// this run successfully parsed may be rewritten or removed (#464 round-3
    /// minor 6, the "repair it and retry" door): a table this run cannot read
    /// or parse is left untouched, and the run aborts with
    /// [`LegacyTableSyncFailure::Corrupt`] only when the plan is non-empty —
    /// an empty plan moves nothing, and the unparseable table is inert (no
    /// boot can parse it either), so it is no reason to refuse the run.
    fn sync_legacy_session_workspaces(
        &self,
        plan: &[(String, PathBuf, PathBuf)],
    ) -> std::result::Result<(), LegacyTableSyncFailure> {
        let legacy = self
            .manager
            .sessions_dir()
            .join(LEGACY_SESSION_WORKSPACES_FILE);
        let content = match std::fs::read_to_string(&legacy) {
            Ok(content) => content,
            // Absent is the converged case (the boot migration retired the
            // file), and a table deleted out-of-band heals the same way.
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                // Unreadable is not absent, and it is certainly not parsed:
                // invalid UTF-8 fails here rather than in the JSON pass below
                // (#464 round-6 finding 4). Log hygiene: the path stays out
                // of the log.
                eprintln!(
                    "[sessions] read legacy session workspaces failed: {}",
                    error.kind()
                );
                return if plan.is_empty() {
                    Ok(())
                } else {
                    Err(LegacyTableSyncFailure::Corrupt)
                };
            }
        };
        if let Err(error) = serde_json::from_str::<HashMap<String, PathBuf>>(&content) {
            eprintln!("[sessions] parse legacy session workspaces failed: {error}");
            return if plan.is_empty() {
                Ok(())
            } else {
                Err(LegacyTableSyncFailure::Corrupt)
            };
        }
        // Merged view = the live table ∪ this run's translations. The cache
        // entries for those sessions are not applied until phase 3, so writing
        // the bare cache here would publish the pre-rebind paths — the exact
        // resurrection this rewrite exists to prevent.
        let translations: HashMap<&str, &Path> = plan
            .iter()
            .map(|(id, next, _)| (id.as_str(), next.as_path()))
            .collect();
        let bindings = self.session_workspaces.read();
        let mut merged: HashMap<String, PathBuf> = bindings
            .iter()
            .map(|(id, path)| match translations.get(id.as_str()) {
                Some(next) => (id.clone(), (*next).to_path_buf()),
                None => (id.clone(), path.clone()),
            })
            .collect();
        // A candidate absent from the live table (an unsynced legacy-memory
        // entry, or a sidecar-scanned session this process has not cached yet)
        // is part of the translated set too: publishing it keeps the table and
        // the sidecars in one domain.
        for (id, next, _) in plan {
            merged.entry(id.clone()).or_insert_with(|| next.clone());
        }
        drop(bindings);
        if merged.is_empty() {
            return match std::fs::remove_file(&legacy) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => {
                    eprintln!(
                        "[sessions] remove legacy session workspaces failed: {}",
                        error.kind()
                    );
                    Err(LegacyTableSyncFailure::Unwritable)
                }
            };
        }
        match serde_json::to_vec_pretty(&merged) {
            Ok(payload) => match crate::platform::filesystem::atomic_write(&legacy, &payload) {
                Ok(()) => Ok(()),
                Err(error) => {
                    eprintln!(
                        "[sessions] rewrite legacy session workspaces failed: {}",
                        error.kind()
                    );
                    Err(LegacyTableSyncFailure::Unwritable)
                }
            },
            Err(error) => {
                eprintln!("[sessions] serialize legacy session workspaces failed: {error}");
                Err(LegacyTableSyncFailure::Unwritable)
            }
        }
    }

    /// Boot-time legacy migration: global table `_session_workspaces.json` →
    /// per-session sidecar (serving only homes of intermediate dev builds, see the
    /// module docs). Live-session entries are rewritten as sidecars one by one
    /// (rewriting identical values is idempotent); once all migrate, the old file
    /// is removed. If any entry write fails, the old file is kept as-is, unmigrated
    /// entries are taken over by the in-memory table so they still resolve, and the
    /// next boot retries without blocking startup. Ghost entries (whose `<id>.json`
    /// no longer exists — leftovers of sessions deleted out of process) are dropped
    /// without migration.
    pub fn migrate_legacy_session_workspaces(&self) {
        let legacy = self
            .manager
            .sessions_dir()
            .join(LEGACY_SESSION_WORKSPACES_FILE);
        let content = match std::fs::read_to_string(&legacy) {
            Ok(content) => content,
            Err(error) if error.kind() == ErrorKind::NotFound => return,
            Err(error) => {
                // Unreadable is not absent: invalid UTF-8 fails here rather
                // than in the JSON pass below (#464 round-6 finding 4). Keep
                // the file untouched — only a successfully parsed table may
                // be rewritten or removed, an invariant the rebind's sync
                // re-checks per run (review #463 round-13 M2). Log hygiene
                // (round-8 should-fix): the path stays out of the log.
                eprintln!(
                    "[sessions] read legacy session workspaces failed: {}",
                    error.kind()
                );
                return;
            }
        };
        let bindings: HashMap<String, PathBuf> = match serde_json::from_str(&content) {
            Ok(bindings) => bindings,
            Err(error) => {
                eprintln!("[sessions] parse legacy session workspaces failed: {error}");
                // Corrupt-but-recoverable: keep the file; the rebind's sync
                // re-attempts the parse per run and refuses to touch a table
                // it cannot parse (#464 round-3 minor 6 / round-4 minor 4).
                return;
            }
        };
        let mut unmigrated = HashMap::new();
        for (id, path) in bindings {
            if !self
                .manager
                .sessions_dir()
                .join(format!("{id}.json"))
                .is_file()
            {
                continue;
            }
            // Disagreement handling (review #463 round-11 B1a disposition):
            // a sidecar that disagrees with a legacy entry is NOT skipped in
            // favor of the entry, unlike the pre-absorption convergence arm —
            // the absorbed (#464) phase order rewrites the table BEFORE the
            // sidecars move, so the crash window between the two phases leaves
            // table@to over sidecar@from, and this boot pass is what heals it
            // forward (rebind_crash_between_phases_heals_forward_on_boot). A
            // successful rebind can never leave the table BEHIND a moved
            // sidecar: the sync publishes every plan translation, cache-only
            // entries included (rebind_publishes_cache_only_legacy_entry_
            // translation). The stale-table state (table@from, sidecar@to) is
            // unreachable: the sync runs before any lane mutates and a failed
            // sync aborts the whole run (review #463 round-12 B1 / round-13
            // M1), so a surviving table always matches unmoved sidecars.
            if let Err(error) = self.bind_session_workspace(&id, path.clone()) {
                // Log hygiene (round-8 should-fix): the unmigrated id reaches
                // the in-memory table, not the log; the failure list of a
                // subsequent rebind is the disclosure channel.
                eprintln!(
                    "[sessions] migrate workspace binding failed: {}",
                    error.root_cause()
                );
                unmigrated.insert(id, path);
            }
        }
        if unmigrated.is_empty() {
            match std::fs::remove_file(&legacy) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => eprintln!(
                    "[sessions] remove legacy session workspaces failed: {}",
                    error.kind()
                ),
            }
        } else {
            // Unmigrated entries are taken over by the cache so they still resolve
            // (retried on the next boot). extend instead of replacing the whole
            // map: a wholesale replace would drop entries bound earlier in this boot.
            self.session_workspaces.write().extend(unmigrated);
        }
    }
}
