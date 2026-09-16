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
use serde::{Deserialize, Serialize};

/// Outcome of rebind_workspace_bindings: translated entries plus per-entry
/// failure isolation (review #464 MAJOR 4: an invalid id or a single write
/// failure no longer aborts the whole round; failures go into the failure
/// list and the command layer merges them into the report).
#[derive(Debug, Default)]
pub struct RebindBindingsOutcome {
    pub rebound: Vec<(String, PathBuf)>,
    pub failed_session_ids: Vec<String>,
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

use super::{SessionStore, validate_session_id};

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
        let path = sidecar.path;
        self.session_workspaces
            .write()
            .insert(id.to_string(), path.clone());
        Some(path)
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

    /// Scans every `workspace-binding.json` sidecar and merges the in-memory
    /// table, listing sessions bound under the `from` prefix (the fence
    /// candidate set for directory rebinding; `from` has usually already
    /// vanished, so matching happens on the shared folded key — Windows folds
    /// separators and case, same semantics as
    /// SessionAgentStore::sessions_under_workspace). Does not check that
    /// `<id>.json` exists — leftover sidecars must also be covered by the
    /// rebind, otherwise the old directory resurrects. The in-memory table is
    /// merged because while the legacy-data migration is unfinished, unmigrated
    /// entries exist only in memory / the old global table (review #464 nit:
    /// the fence candidates must not miss this group).
    pub fn workspace_bindings_under(&self, from: &Path) -> Vec<(String, PathBuf)> {
        let covered = |path: &Path| folded_covers(from, path);
        let mut matched = Vec::new();
        if let Ok(entries) = std::fs::read_dir(self.manager.sessions_dir()) {
            for entry in entries.flatten() {
                if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    continue;
                }
                let Some(id) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                let Some(sidecar) =
                    read_workspace_sidecar(&entry.path().join(SESSION_WORKSPACE_SIDECAR_FILE))
                else {
                    continue;
                };
                if covered(&sidecar.path) {
                    matched.push((id, sidecar.path));
                }
            }
        }
        // Union with the in-memory table (deduplicated): entries that never
        // landed as sidecars on the overwrite-migration degraded path.
        for (id, path) in self.session_workspaces.read().iter() {
            if covered(path) && !matched.iter().any(|(existing_id, _)| existing_id == id) {
                matched.push((id.clone(), path.clone()));
            }
        }
        matched
    }

    /// Directory rebinding (the broken-link repair channel): translates every
    /// plain-session binding under the `from` prefix to `to` in one pass
    /// (atomic sidecar rewrite + in-memory cache sync). Same semantics and
    /// idempotency as SessionAgentStore::rebind_workspace_prefix — a from→to
    /// rerun with no matches is a no-op, and failures can be retried as a
    /// whole. Session metadata (the metadata.workspace display field) is
    /// rewritten uniformly by the command layer via set_workspace.
    ///
    /// Per-entry isolation (review #464 MAJOR 4): an invalid id (boot
    /// migration leaves unvalidated failed entries in the in-memory legacy
    /// table) or a single write failure no longer aborts the whole round with
    /// `?` — by then the project roots and agent index have already been
    /// translated, so aborting would make the same entry fail again on every
    /// retry; failures go into `failed_session_ids` and the command layer
    /// merges them into the report.
    pub fn rebind_workspace_bindings(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<RebindBindingsOutcome> {
        let covered = |path: &Path| folded_covers(from, path);
        let skip = from.components().count();
        let mut outcome = RebindBindingsOutcome::default();
        // Candidates = sidecar scan ∪ in-memory legacy table: on the degraded
        // path where the legacy-data migration is unfinished, unmigrated
        // entries exist only in memory / the old global table; missing them
        // would resurrect the old directory at the next boot migration.
        let mut candidates: Vec<(String, PathBuf)> = self.workspace_bindings_under(from);
        for (id, path) in self.session_workspaces.read().iter() {
            if !candidates.iter().any(|(existing_id, _)| existing_id == id) {
                candidates.push((id.clone(), path.clone()));
            }
        }
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
            // In-memory legacy-table entries may have no session directory
            // (never written as a sidecar); atomic_write does not create
            // parent directories, so create it first (same as
            // bind_session_workspace).
            if let Some(parent) = sidecar_path.parent() {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    eprintln!("[sessions] rebind create session dir for {id} failed: {error:#}");
                    outcome.failed_session_ids.push(id);
                    continue;
                }
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
            let write = serde_json::to_vec_pretty(&updated)
                .context("serialize session workspace binding")
                .and_then(|payload| {
                    crate::platform::filesystem::atomic_write(&sidecar_path, &payload).with_context(
                        || {
                            format!(
                                "rebind session workspace binding {}",
                                sidecar_path.display()
                            )
                        },
                    )
                });
            if let Err(error) = write {
                eprintln!("[sessions] rebind workspace binding {id} failed: {error:#}");
                outcome.failed_session_ids.push(id);
                continue;
            }
            self.session_workspaces
                .write()
                .insert(id.clone(), next.clone());
            outcome.rebound.push((id, next));
        }
        // Degraded-path symmetry (review #452 finding 9): while the old
        // global table is still on disk (legacy-data migration unfinished),
        // rewrite it in sync, otherwise the rebind would be resurrected by
        // the old table at the next boot migration.
        self.rewrite_legacy_session_workspaces_if_present();
        Ok(outcome)
    }

    /// Minimal rewrite of the old global table (used only for the rebind's
    /// degraded-path symmetry). #445 round-2 removed the generic persistence
    /// (no other call surface); all that remains here: if the table is
    /// non-empty, atomically rewrite it wholesale with the in-memory table
    /// contents; if empty, delete the file (no entries left to resurrect).
    /// Write failures are only logged — the in-memory table and sidecars are
    /// already authoritative, and the old table is nothing but migration
    /// residue. Exception: a file whose boot parse failed stays untouched, so
    /// a corrupt-but-repairable file keeps the "fix it and retry" door open.
    fn rewrite_legacy_session_workspaces_if_present(&self) {
        // A file this process never successfully parsed (corrupt but
        // repairable) must not be deleted or overwritten — otherwise the
        // first rebind closes the "repair the file and retry" door
        // (review #464 round-3 minor 6).
        if self
            .legacy_session_workspaces_parse_failed
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let legacy = self
            .manager
            .sessions_dir()
            .join(LEGACY_SESSION_WORKSPACES_FILE);
        if !legacy.is_file() {
            return;
        }
        let bindings = self.session_workspaces.read();
        if bindings.is_empty() {
            if let Err(error) = std::fs::remove_file(&legacy) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("[sessions] remove legacy session workspaces failed: {error:#}");
                }
            }
            return;
        }
        match serde_json::to_vec_pretty(&*bindings) {
            Ok(payload) => {
                if let Err(error) = crate::platform::filesystem::atomic_write(&legacy, &payload) {
                    eprintln!("[sessions] rewrite legacy session workspaces failed: {error:#}");
                }
            }
            Err(error) => {
                eprintln!("[sessions] serialize legacy session workspaces failed: {error:#}");
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
        let Ok(content) = std::fs::read_to_string(&legacy) else {
            return;
        };
        let bindings: HashMap<String, PathBuf> = match serde_json::from_str(&content) {
            Ok(bindings) => bindings,
            Err(error) => {
                eprintln!("[sessions] parse legacy session workspaces failed: {error}");
                // Corrupt-but-recoverable: keep the file and bar the rebind
                // degraded-path rewrite from deleting a file we never parsed.
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
