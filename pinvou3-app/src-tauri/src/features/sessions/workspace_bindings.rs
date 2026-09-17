//! User working-directory binding sidecar for plain chat sessions
//! (per-session, lives and dies with the session directory).
//!
//! When a plain (assistant/engine) session is created, the frontend may let
//! the user pick a working directory; once bound,
//! [`SessionStore::session_roots`] resolves the session's execution root
//! (engine cwd) to the bound directory while the ledger root stays the
//! session-private directory — sharing `session_roots_for`'s dual-root
//! semantics with native code sessions' project-directory binding. The
//! `execution_root_resolver` closure injected into bridge/SessionStore at the
//! app composition root falls back to this store when a native code session
//! misses, so the bridge side (prompt environment section, AGENTS.md
//! injection, connector scope, audit root) behaves identically for both
//! binding kinds — a bound directory is equally a prompt-injection surface,
//! and the safety posture follows the binding, not the mode.
//!
//! Storage shape (binding-store convergence): the binding record is a
//! per-session sidecar `<sessions>/<id>/workspace-binding.json` inside the
//! session-private directory — the same mechanism as native code sessions'
//! `code-session.json`. The binding lives and dies with the session
//! directory; deleting the directory removes it, and there is no more
//! boot-time ghost cleanup of a global table. The in-memory
//! `session_workspaces` degrades to a read cache: written on bind, refilled
//! from the sidecar on a read miss (bindings made by another process are
//! equally visible), and cleared on delete/retention cleanup.
//!
//! The legacy global table `_session_workspaces.json` was an intermediate
//! format from this PR's development and never shipped with `main`
//! (review #445 P2); the boot-time
//! [`SessionStore::migrate_legacy_session_workspaces`] only converges homes
//! of intermediate dev builds: live-session entries are written out as
//! sidecars one by one, then the old file is deleted; entries that fail to
//! write stay in the old file untouched (resolved by the in-memory table for
//! this run) and are retried on the next boot without blocking startup. Once
//! intermediate-version stock is fully migrated, the migration becomes a
//! permanent no-op (a missing file returns immediately).

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Outcome of rebind_workspace_bindings: translated + per-entry failure
/// isolation (review #464 MAJOR 4: an invalid id or a single failed write no
/// longer aborts the whole round; failures go into the failed list and the
/// command layer folds them into the report).
#[derive(Debug, Default)]
pub struct RebindBindingsOutcome {
    pub rebound: Vec<(String, PathBuf)>,
    pub failed_session_ids: Vec<String>,
}

/// "Equal to or nested under" prefix check under folded identity keys:
/// Windows folds separators and case, same convention as the store's
/// key_is_same_or_nested; an empty from matches everything (shared by the two
/// candidate scans in this file — review #464 MINOR 8: the same closure used
/// to be byte-duplicated and had already drifted from the project layer's
/// semantics).
fn folded_covers(from: &Path, path: &Path) -> bool {
    let from_key = crate::platform::os::filesystem_path_identity_key(&from.to_string_lossy());
    let from_trim = from_key.trim_end_matches('/');
    let key = crate::platform::os::filesystem_path_identity_key(&path.to_string_lossy());
    let trim = key.trim_end_matches('/');
    from_trim.is_empty() || trim == from_trim || trim.starts_with(&format!("{from_trim}/"))
}

use super::{SessionStore, validate_session_id};

/// Schema version of the binding sidecar; used for migration when fields
/// evolve in the future.
const SESSION_WORKSPACE_SIDECAR_VERSION: u32 = 1;
/// This PR's intermediate-version global binding table (never shipped with
/// `main`; deleted after a successful boot-time migration).
const LEGACY_SESSION_WORKSPACES_FILE: &str = "_session_workspaces.json";
/// Per-session binding sidecar filename (inside the session-private
/// directory).
const SESSION_WORKSPACE_SIDECAR_FILE: &str = "workspace-binding.json";

/// Binding sidecar content. `path` is the absolute directory canonicalized by
/// `validate_user_workspace_path`; `bound_at` is metadata only and plays no
/// part in restore semantics.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionWorkspaceSidecar {
    version: u32,
    path: PathBuf,
    /// Keychain snapshot locked at creation (§6): full set of accessible
    /// roots (including the primary root). Old sidecars missing the key /
    /// empty = single-root semantics (the `path` directory only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    workspace_roots: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bound_at: Option<i64>,
}

fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

/// A future higher-version format must not be silently parsed as the current
/// version: refuse to read and treat as missing (a bind rewrite at the
/// current version self-heals); parse anomalies are all logged.
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

    /// Bind a session's working directory and atomically persist it to the
    /// sidecar inside the session-private directory. Requires the session's
    /// durable record (`<id>.json`) to already exist — a binding is
    /// subordinate data of the session, and no directory is created out of
    /// thin air for an unknown id; the caller (the create_session command)
    /// creates the session first and binds second.
    /// A persist failure returns Err without touching the in-memory cache —
    /// the caller (the create_session command) deletes the just-created empty
    /// session accordingly, leaving no session that "looks bound but loses
    /// the binding on restart".
    pub fn bind_session_workspace(&self, id: &str, path: PathBuf) -> Result<()> {
        self.bind_session_workspace_with_roots(id, path, Vec::new())
    }

    /// Bind + keychain snapshot (§6): `workspace_roots` is the full set of
    /// accessible roots (empty = single-root semantics, the engine normalizes
    /// by cwd). Same persistence discipline as `bind_session_workspace`.
    pub fn bind_session_workspace_with_roots(
        &self,
        id: &str,
        path: PathBuf,
        workspace_roots: Vec<PathBuf>,
    ) -> Result<()> {
        validate_session_id(id)?;
        let record = self.manager.sessions_dir().join(format!("{id}.json"));
        if !record.is_file() {
            anyhow::bail!("cannot bind workspace: session record {id} does not exist");
        }
        let sidecar = SessionWorkspaceSidecar {
            version: SESSION_WORKSPACE_SIDECAR_VERSION,
            path,
            workspace_roots,
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

    /// Read the session's user working-directory binding (no binding → None,
    /// and the execution root falls back to the session-private directory).
    /// The sidecar is re-read on an in-memory cache miss; a leftover sidecar
    /// whose durable session record no longer exists (a directory left behind
    /// by a partially failed deletion) is treated as None, same semantics as
    /// the old ghost cleanup.
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

    /// Keychain snapshot locked at session creation (§6); no binding / old
    /// sidecar / leftover directory = empty (single-root semantics). The cold
    /// path reads the sidecar directly (only called on engine spawn/resume)
    /// and does not populate the `session_workspaces` path cache.
    pub fn session_workspace_roots(&self, id: &str) -> Vec<PathBuf> {
        if validate_session_id(id).is_err()
            || !self
                .manager
                .sessions_dir()
                .join(format!("{id}.json"))
                .is_file()
        {
            return Vec::new();
        }
        read_workspace_sidecar(&self.session_workspace_sidecar_path(id))
            .map(|sidecar| sidecar.workspace_roots)
            .unwrap_or_default()
    }

    /// Keychain replacement for "align to project" (§9.7): rewrites the
    /// sidecar snapshot wholesale (binding path and bound_at preserved).
    /// Returns Ok(false) when there is no binding — temporary sessions are
    /// rejected by the command layer first; this is a second line of defense.
    pub fn set_session_workspace_roots(
        &self,
        id: &str,
        workspace_roots: Vec<PathBuf>,
    ) -> Result<bool> {
        validate_session_id(id)?;
        let file = self.session_workspace_sidecar_path(id);
        let Some(existing) = read_workspace_sidecar(&file) else {
            return Ok(false);
        };
        let updated = SessionWorkspaceSidecar {
            version: SESSION_WORKSPACE_SIDECAR_VERSION,
            path: existing.path,
            workspace_roots,
            bound_at: existing.bound_at,
        };
        let payload =
            serde_json::to_vec_pretty(&updated).context("serialize session workspace binding")?;
        crate::platform::filesystem::atomic_write(&file, &payload)
            .with_context(|| format!("persist session workspace roots to {}", file.display()))?;
        Ok(true)
    }

    /// Best-effort removal of the binding sidecar file; NotFound counts as
    /// already removed. Directory cleanup on the session-delete path usually
    /// takes it away already; this covers "session still alive, unbind only"
    /// and the leftover-directory fallback.
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

    /// Scan all `workspace-binding.json` sidecars, merge them into the
    /// in-memory table, and list sessions bound under the `from` prefix (the
    /// fence candidate set for directory rebinding; `from` has usually
    /// vanished, so matching runs on the shared folded keys — Windows folds
    /// separators and case, same semantics as
    /// SessionAgentStore::sessions_under_workspace). `<id>.json` existence is
    /// deliberately not checked — leftover sidecars must be covered by the
    /// rebind too, otherwise the old directory revives. The in-memory table
    /// is merged because while the stock migration is incomplete, unmigrated
    /// entries exist only in memory/the old global table (review #464 nit:
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
        // Union of the in-memory table (deduplicated): covers entries not yet
        // persisted as sidecars under the migration fallback path.
        for (id, path) in self.session_workspaces.read().iter() {
            if covered(path) && !matched.iter().any(|(existing_id, _)| existing_id == id) {
                matched.push((id.clone(), path.clone()));
            }
        }
        matched
    }

    /// Directory rebind (broken-link repair channel): shift plain-session
    /// bindings under the `from` prefix wholesale to `to` (atomic sidecar
    /// rewrite + in-memory cache sync). Same semantics and idempotence as
    /// SessionAgentStore::rebind_workspace_prefix — a from→to rerun with no
    /// hits is a no-op, and a failure can be retried wholesale. Session
    /// metadata (the metadata.workspace display field) is rewritten by the
    /// command layer uniformly via set_workspace.
    ///
    /// Per-entry isolation (review #464 MAJOR 4): an invalid id (boot
    /// migration keeps failed unvalidated entries in the in-memory legacy
    /// table) or a single failed write no longer aborts the whole round with
    /// `?` — the project roots and the agent index have already shifted by
    /// then, and aborting would make the same entry fail again on every
    /// retry; failures go into `failed_session_ids` and the command layer
    /// folds them into the report.
    pub fn rebind_workspace_bindings(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<RebindBindingsOutcome> {
        let covered = |path: &Path| folded_covers(from, path);
        let skip = from.components().count();
        let mut outcome = RebindBindingsOutcome::default();
        // Candidates = sidecar scan ∪ in-memory legacy table: under the
        // fallback path of an incomplete stock migration, unmigrated entries
        // exist only in memory/the old global table, and missing them would
        // revive the old directory on the next boot's migration.
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
            // The id passes validation before any path is joined (same gate
            // as bind_session_workspace): a traversal-shaped id would write
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
            // An in-memory legacy-table entry may have no session directory
            // (never written as a sidecar); atomic_write does not create
            // parents, so create it first (same as bind_session_workspace).
            if let Some(parent) = sidecar_path.parent() {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    eprintln!("[sessions] rebind create session dir for {id} failed: {error:#}");
                    outcome.failed_session_ids.push(id);
                    continue;
                }
            }
            // bound_at is metadata only: preserved as-is, same convention as
            // the codex store's rebind, never reset to None (review #452
            // finding 3). The keychain snapshot shifts along: roots under the
            // from prefix move to `to`, the rest stay.
            let previous = read_workspace_sidecar(&sidecar_path);
            let bound_at = previous.as_ref().and_then(|s| s.bound_at);
            let rebound_roots: Vec<PathBuf> = previous
                .map(|s| s.workspace_roots)
                .unwrap_or_default()
                .into_iter()
                .map(|root| {
                    let suffix: PathBuf = root.components().skip(skip).collect();
                    if covered(&root) {
                        if suffix.as_os_str().is_empty() {
                            to.to_path_buf()
                        } else {
                            to.join(suffix)
                        }
                    } else {
                        root
                    }
                })
                .collect();
            let updated = SessionWorkspaceSidecar {
                version: SESSION_WORKSPACE_SIDECAR_VERSION,
                path: next.clone(),
                workspace_roots: rebound_roots,
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
        // Symmetric fallback path (review #452 finding 9): while the old
        // global table is still on disk (stock migration incomplete), rewrite
        // it in sync — otherwise the rebind would be revived from the old
        // table by the next boot's migration.
        self.rewrite_legacy_session_workspaces_if_present();
        Ok(outcome)
    }

    /// Minimal rewrite of the old global table (only used for the rebind
    /// fallback-path symmetry). #445 round-2 removed general persistence (no
    /// other call surface); only this remains: if the table is non-empty it
    /// is atomically rewritten wholesale with the in-memory table's content,
    /// and if empty the file is deleted (no revivable entries). Write
    /// failures are only logged — the in-memory table and the sidecars are
    /// already authoritative, and the old table is nothing but migration
    /// residue.
    fn rewrite_legacy_session_workspaces_if_present(&self) {
        let legacy = crate::platform::paths::sessions_root().join(LEGACY_SESSION_WORKSPACES_FILE);
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

    /// Boot-time stock migration: the global table `_session_workspaces.json`
    /// → per-session sidecars (only serves homes of this PR's
    /// intermediate-version dev builds; see the module docs). Live-session
    /// entries are written out as sidecars one by one (same-value rewrites
    /// are idempotent), and once every entry migrated successfully the old
    /// file is deleted; if any entry fails to write, the old file is kept
    /// as-is, the unmigrated entries are adopted into the in-memory table and
    /// stay resolvable, the next boot retries, and startup is not blocked.
    /// Ghost entries (whose `<id>.json` no longer exists — residue of a
    /// session deleted outside the process) are dropped, not migrated.
    pub fn migrate_legacy_session_workspaces(&self) {
        let legacy = crate::platform::paths::sessions_root().join(LEGACY_SESSION_WORKSPACES_FILE);
        let Ok(content) = std::fs::read_to_string(&legacy) else {
            return;
        };
        let bindings: HashMap<String, PathBuf> = match serde_json::from_str(&content) {
            Ok(bindings) => bindings,
            Err(error) => {
                eprintln!("[sessions] parse legacy session workspaces failed: {error}");
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
            *self.session_workspaces.write() = unmigrated;
        }
    }
}
