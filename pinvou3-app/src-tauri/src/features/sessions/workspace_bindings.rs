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
//!
//! A file the boot pass did not parse (unreadable or invalid JSON) is preserved
//! verbatim and `legacy_session_workspaces_parse_failed` stays set for the rest
//! of the process: only a file this process successfully parsed may be
//! rewritten or removed by [`SessionStore::rebind_workspace_bindings`], so the
//! user keeps a "repair the file and retry" door. A file that *was* parsed but
//! could not be rewritten or removed during a rebind is reported as
//! `legacy_sync_failed` instead of being silently declared done.

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
    /// 创建时锁定的钥匙串快照(§6):全量可访问根(含主根)。旧 sidecar 缺该键
    /// 或为空 = 单根语义(仅 `path` 目录)。
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
        self.bind_session_workspace_with_roots(id, path, Vec::new())
    }

    /// 绑定 + 钥匙串快照(§6):`workspace_roots` 是全量可访问根(空 = 单根
    /// 语义,底座按 cwd 归一)。落盘纪律与 `bind_session_workspace` 相同。
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

    /// 创建时锁定的钥匙串快照(§6);无绑定/旧 sidecar/残留目录 = 空(单根
    /// 语义)。冷路径直接读 sidecar(只在引擎 spawn/resume 调用),不填充
    /// `session_workspaces` 路径缓存。
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

    /// 「对齐到项目」(§9.7)的钥匙串替换:整体重写 sidecar 快照(绑定路径与
    /// bound_at 保留)。无绑定时返回 Ok(false)——临时会话已由命令层先行拒绝,
    /// 这里是第二道防线。
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
    /// Fails closed on an unreadable sessions dir (review #464 round-5 item 4,
    /// mirroring the codex lane): silently yielding an empty candidate set
    /// would shrink the rewrite with zero signal and resurrect old paths at
    /// the next boot. A missing dir (fresh home) is the empty set.
    pub fn workspace_bindings_under(
        &self,
        from: &Path,
    ) -> Result<Vec<(String, PathBuf)>, std::io::Error> {
        let covered = |path: &Path| folded_covers(from, path);
        let mut matched = Vec::new();
        match std::fs::read_dir(self.manager.sessions_dir()) {
            Ok(entries) => {
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
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                eprintln!(
                    "[sessions] scan workspace binding sidecars failed ({}): {error:#}",
                    self.manager.sessions_dir().display()
                );
                return Err(error);
            }
        }
        // Union with the in-memory table (deduplicated): entries that never
        // landed as sidecars on the overwrite-migration degraded path.
        for (id, path) in self.session_workspaces.read().iter() {
            if covered(path) && !matched.iter().any(|(existing_id, _)| existing_id == id) {
                matched.push((id.clone(), path.clone()));
            }
        }
        Ok(matched)
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
    /// merges them into the report. The reserved fallibility is real: the
    /// sidecar scan fails closed on an unreadable sessions dir.
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
        let candidates: Vec<(String, PathBuf)> = self
            .workspace_bindings_under(from)
            .context("scan workspace bindings")?;
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
        let diverged = self.legacy_diverged_bindings();
        if !self.rewrite_legacy_session_workspaces_if_present(&plan) {
            outcome.legacy_sync_failed = true;
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
                // (review #452 finding 3). The keychain snapshot shifts along:
                // roots under the `from` prefix move onto `to`, the rest stay.
                let previous = read_workspace_sidecar(&sidecar_path);
                let bound_at = previous.as_ref().and_then(|s| s.bound_at);
                let skip = from.components().count();
                let rebound_roots: Vec<PathBuf> = previous
                    .map(|s| s.workspace_roots)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|root| {
                        if !folded_covers(from, &root) {
                            return root;
                        }
                        let suffix: PathBuf = root.components().skip(skip).collect();
                        if suffix.as_os_str().is_empty() {
                            next.clone()
                        } else {
                            next.join(suffix)
                        }
                    })
                    .collect();
                let updated = SessionWorkspaceSidecar {
                    version: SESSION_WORKSPACE_SIDECAR_VERSION,
                    path: next.clone(),
                    workspace_roots: rebound_roots,
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
    fn legacy_diverged_bindings(&self) -> Vec<String> {
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
        let mut diverged: Vec<String> = entries
            .into_iter()
            .filter(|(id, path)| {
                if live.get(id).is_some_and(|current| current == path) {
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
    /// without migration. A file that cannot be read (invalid UTF-8 included) or
    /// parsed is preserved verbatim and bars the rebind's degraded-path rewrite for
    /// the rest of this process (see the module docs).
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
                // (review #464 round-6 finding 4). Leaving the flag clear let a
                // later rebind treat a never-parsed file as syncable — with an
                // empty cache it would delete a repairable table, closing the
                // same "fix it and retry" door round-3 minor 6 opened, and with
                // a populated cache it would overwrite it wholesale. The
                // module invariant is stricter than that: only a file this
                // process successfully parsed may be rewritten or removed.
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
