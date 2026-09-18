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

/// Outcome of rebind_workspace_bindings: translated entries plus per-entry
/// failure isolation (review #464 MAJOR 4: an invalid id or a single write
/// failure no longer aborts the whole round; failures go into the failure
/// list and the command layer merges them into the report).
#[derive(Debug, Default)]
pub struct RebindBindingsOutcome {
    pub rebound: Vec<(String, PathBuf)>,
    pub failed_session_ids: Vec<String>,
    /// True when the legacy global table is still on disk but this process
    /// failed to rewrite/remove it in sync: the next boot migration would
    /// re-bind the old paths over the fresh sidecars (silent resurrection),
    /// so the report must not claim success (review #464 round-5 blocker 1).
    /// A rerun converges — the rewrite is retried from the in-memory table.
    pub legacy_sync_failed: bool,
    /// Sessions the surviving legacy table would re-bind over their fresh
    /// sidecars at the next boot (see `legacy_diverged_bindings`): non-empty
    /// only together with `legacy_sync_failed`. The command layer merges these
    /// ids into the report's failure list **independently of this run's
    /// `rebound` set** — on a retry nothing is left to rewrite, so driving the
    /// merge off `rebound` reported full success while the stale table
    /// survived (review #464 round-6 blocking 1). Ids only: they are data for
    /// the report; the paths stay out of the logs.
    pub legacy_resurrection_ids: Vec<String>,
}

/// "Equal to or nested under" prefix check on folded identity keys: Windows
/// folds separators and case, same convention as the store's
/// key_is_same_or_nested; an empty from matches everything (shared by the two
/// candidate scans in this file — review #464 MINOR 8: the same closure was
/// once byte-for-byte duplicated and its semantics had drifted from the
/// project layer).
fn folded_covers(from: &Path, path: &Path) -> bool {
    let from_key = crate::platform::os::filesystem_path_identity_key(&from.to_string_lossy());
    let from_trim = from_key.trim_end_matches('/');
    let key = crate::platform::os::filesystem_path_identity_key(&path.to_string_lossy());
    let trim = key.trim_end_matches('/');
    from_trim.is_empty() || trim == from_trim || trim.starts_with(&format!("{from_trim}/"))
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
    let payload = std::fs::read(path).ok()?;
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
        for entry in entries.flatten() {
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

    /// Directory rebinding (the broken-link repair channel): translates every
    /// plain-session binding under the `from` prefix to `to` in one pass
    /// (atomic sidecar rewrite + in-memory cache sync). Same semantics and
    /// idempotency as SessionAgentStore::rebind_workspace_prefix — a from→to
    /// rerun with no matches is a no-op, and failures can be retried as a
    /// whole. Session metadata (the metadata.workspace display field) is
    /// rewritten uniformly by the command layer via set_workspace.
    ///
    /// Per-entry isolation (#464 MAJOR 4): an invalid id (boot migration
    /// leaves unvalidated failed entries in the in-memory legacy table) or a
    /// single write failure no longer aborts the whole round with `?` — by
    /// then the agent index has already been translated, so aborting would
    /// make the same entry fail again on every retry; failures go into
    /// `failed_session_ids` and the command layer merges them into the report.
    ///
    /// The candidate scan is the tolerant one shared with the command layer's
    /// fence ([`Self::workspace_bindings_under`]): an unreadable sessions root
    /// is logged and yields the entries already found, so a transient
    /// `read_dir` failure cannot abort an otherwise healthy rebind. The `?` on
    /// this signature is reserved for the sidecar write phase.
    ///
    /// Degraded-path honesty: when the legacy global table survives the write
    /// (see `rewrite_legacy_session_workspaces_if_present`), the outcome also
    /// names every session that table would resurrect, so the report can be
    /// honest on a retry too. See [`RebindBindingsOutcome::legacy_resurrection_ids`].
    pub fn rebind_workspace_bindings(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<RebindBindingsOutcome> {
        let covered = |path: &Path| folded_covers(from, path);
        let skip = from.components().count();
        let mut outcome = RebindBindingsOutcome::default();
        // Phase 1 — plan. Candidates = sidecar scan ∪ in-memory legacy table,
        // via workspace_bindings_under (already the union — review #464 round-5
        // nit: a second in-memory union here duplicated it exactly). Nothing is
        // written yet: the plan is what the legacy-table rewrite must publish
        // BEFORE the sidecars move, and an invalid id is rejected here instead
        // of half-way through the write phase.
        let candidates: Vec<(String, PathBuf)> = self.workspace_bindings_under(from);
        let mut plan: Vec<(String, PathBuf, PathBuf)> = Vec::new();
        for (id, path) in candidates {
            if !covered(&path) {
                continue;
            }
            // The id is validated before joining the path (same gate as
            // bind_session_workspace): a traversal-shaped id would write
            // outside sessions_dir.
            if let Err(error) = validate_session_id(&id) {
                eprintln!("[sessions] rebind skips invalid session id {id:?}: {error:#}");
                outcome.failed_session_ids.push(id);
                continue;
            }
            let suffix: PathBuf = path.components().skip(skip).collect();
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
            plan.push((id, next, sidecar_path));
        }
        // Phase 2 — the legacy global table, before any sidecar moves (review
        // #464 round-6 finding 5). The old order (sidecars first, table last)
        // left a crash window that heals in the DANGEROUS direction: fresh
        // sidecars on disk plus a stale table, with no report possible, so the
        // next boot re-binds the deleted directory. Writing the translated
        // table first means every crash window heals forward — a boot sees
        // either the old table with old sidecars (no rebind happened), or the
        // new table with old/new sidecars, where the boot migration rewrites
        // the stragglers to `to`. The table holds translated values only, so a
        // partial sidecar failure is finished by the boot migration rather
        // than undone by it.
        //
        // A failed write is NOT log-only: the next boot would re-bind the old
        // paths over the fresh sidecars, so the outcome must carry the failure
        // and the report cannot claim success (review #464 round-5 blocker 1).
        // Which sessions that concerns is read from the table that survives,
        // not from this run's write log: on a retry nothing is left to rewrite
        // and the rebound set is empty while the stale table is still there
        // (review #464 round-6 blocking 1).
        let diverged = self.legacy_diverged_bindings(&plan);
        if !self.rewrite_legacy_session_workspaces_if_present(&plan) {
            outcome.legacy_sync_failed = true;
            // Assigned only on the failure path: `diverged` describes the file
            // as it was before the rewrite, so a successful rewrite (which
            // already put the translated values on disk) must not report the
            // sessions as resurrectable.
            outcome.legacy_resurrection_ids = diverged;
        }
        // Phase 3 — move the in-memory cache and the sidecars. Per-entry
        // isolation: one failed write does not abort the round (review #464
        // MAJOR 4).
        for (id, next, sidecar_path) in plan {
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
                eprintln!("[sessions] rebind workspace binding {id} failed: {error:#}");
                outcome.failed_session_ids.push(id);
                continue;
            }
            // The cache is moved even when the legacy table could not be
            // rewritten: this run resolves the live process the same way on
            // either outcome, and the surviving table's divergent entries are
            // reported rather than masked.
            self.session_workspaces
                .write()
                .insert(id.clone(), next.clone());
            outcome.rebound.push((id, next));
        }
        Ok(outcome)
    }

    /// Sessions whose live binding in the legacy global table differs from the
    /// current in-memory binding — i.e. exactly the entries a next-boot
    /// migration would write back over the fresh sidecars, resurrecting the
    /// pre-rebind directory (review #464 round-6 blocking 1). The comparison
    /// mirrors the boot migration's own population rule: a session record must
    /// exist (`migrate_legacy_session_workspaces` skips ghost entries), and
    /// only a file this process successfully parsed is consulted — a file whose
    /// parse failed is deliberately preserved and can be neither trusted nor
    /// rewritten (round-3 minor 6).
    ///
    /// Returned sorted by id so a retry reports the same set in the same order.
    fn legacy_diverged_bindings(&self, plan: &[(String, PathBuf, PathBuf)]) -> Vec<String> {
        if self
            .legacy_session_workspaces_parse_failed
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Vec::new();
        }
        let legacy = self
            .manager
            .sessions_dir()
            .join(LEGACY_SESSION_WORKSPACES_FILE);
        let Ok(content) = std::fs::read_to_string(&legacy) else {
            return Vec::new();
        };
        let Ok(entries) = serde_json::from_str::<HashMap<String, PathBuf>>(&content) else {
            // Readable but unparseable (the boot pass owns flagging that case;
            // this call can also run on a long-lived process whose boot read a
            // file that has since been damaged): nothing may be rewritten, so
            // no session can be named as resurrectable.
            return Vec::new();
        };
        let live = self.session_workspaces.read();
        let sessions_dir = self.manager.sessions_dir();
        // This runs BEFORE phase 3 applies the plan to the cache, so the cache
        // still holds the pre-rebind values. The target must therefore be the
        // planned translation first, falling back to the cache: comparing the
        // stale table against the equally stale cache made every entry look in
        // sync and reported an empty resurrection set (review #464 round-6
        // blocking 1 regression).
        let translations: HashMap<&str, &Path> = plan
            .iter()
            .map(|(id, next, _)| (id.as_str(), next.as_path()))
            .collect();
        let mut diverged: Vec<String> = entries
            .into_iter()
            .filter(|(id, path)| {
                let target = translations
                    .get(id.as_str())
                    .copied()
                    .or_else(|| live.get(id).map(|current| current.as_path()));
                if target.is_some_and(|current| current == path) {
                    return false;
                }
                sessions_dir.join(format!("{id}.json")).is_file()
            })
            .map(|(id, _)| id)
            .collect();
        diverged.sort_unstable();
        diverged
    }

    /// Minimal rewrite of the old global table (used only for the rebind's
    /// degraded-path symmetry). #445 round-2 removed the generic persistence
    /// (no other call surface); all that remains here: if the table is
    /// non-empty, atomically rewrite it wholesale with the in-memory table
    /// contents — with this run's translations for the sessions it plans to
    /// move, whose cache entries are not written until phase 3 — and if the
    /// merged table is empty, delete the file (no entries left to resurrect).
    ///
    /// Returns false when the file is still on disk but the sync failed — the
    /// caller reports it (a silent success would resurrect old paths at the
    /// next boot).
    ///
    /// The guard is boot migration state, not a fresh parse: `true` means the
    /// file was not successfully parsed on the one attempt that owns the file
    /// (`migrate_legacy_session_workspaces`), and preservation of a
    /// possibly-repairable file wins over the symmetry — the round-3 "fix it
    /// and retry" door stays open. Files that process parsed are rewritten or
    /// removed here, which is the whole point of the degraded path: a table
    /// that still holds a stale `from` path re-binds it over the fresh
    /// sidecars at the next boot.
    fn rewrite_legacy_session_workspaces_if_present(
        &self,
        plan: &[(String, PathBuf, PathBuf)],
    ) -> bool {
        // A file this process never successfully parsed (corrupt but
        // repairable) must not be deleted or overwritten — otherwise the
        // first rebind closes the "repair the file and retry" door
        // (review #464 round-3 minor 6).
        if self
            .legacy_session_workspaces_parse_failed
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return true;
        }
        let legacy = self
            .manager
            .sessions_dir()
            .join(LEGACY_SESSION_WORKSPACES_FILE);
        if !legacy.is_file() {
            return true;
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
                Ok(()) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                Err(error) => {
                    eprintln!("[sessions] remove legacy session workspaces failed: {error:#}");
                    false
                }
            };
        }
        match serde_json::to_vec_pretty(&merged) {
            Ok(payload) => match crate::platform::filesystem::atomic_write(&legacy, &payload) {
                Ok(()) => true,
                Err(error) => {
                    eprintln!("[sessions] rewrite legacy session workspaces failed: {error:#}");
                    false
                }
            },
            Err(error) => {
                eprintln!("[sessions] serialize legacy session workspaces failed: {error:#}");
                false
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
                // Unreadable is not absent, and it is certainly not parsed:
                // invalid UTF-8 fails here rather than in the JSON pass below
                // (#464 round-6 finding 4). Leaving the flag clear let a later
                // rebind treat a never-parsed file as syncable — with an empty
                // cache it would delete a repairable table. The module
                // invariant is stricter: only a file this process successfully
                // parsed may be rewritten or removed.
                eprintln!(
                    "[sessions] read legacy session workspaces failed ({}): {error:#}",
                    legacy.display()
                );
                self.legacy_session_workspaces_parse_failed
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                return;
            }
        };
        let bindings: HashMap<String, PathBuf> = match serde_json::from_str(&content) {
            Ok(bindings) => bindings,
            Err(error) => {
                eprintln!("[sessions] parse legacy session workspaces failed: {error}");
                // Corrupt-but-recoverable: keep the file and bar the rebind
                // degraded-path rewrite from deleting a file we never parsed
                // (#464 round-3 minor 6 / round-4 minor 4).
                self.legacy_session_workspaces_parse_failed
                    .store(true, std::sync::atomic::Ordering::SeqCst);
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
            if let Err(error) = self.bind_session_workspace(&id, path.clone()) {
                eprintln!("[sessions] migrate workspace binding for {id} failed: {error:#}");
                unmigrated.insert(id, path);
            }
        }
        if unmigrated.is_empty() {
            match std::fs::remove_file(&legacy) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => eprintln!(
                    "[sessions] remove legacy session workspaces failed ({}): {error:#}",
                    legacy.display()
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
