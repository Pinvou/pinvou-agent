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
