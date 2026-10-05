//! Marketplace state files' cross-process read-modify-write serialization
//! (#521, generalizing #517's `with_scope_file_lock` for
//! `disabled_bundles.json`).
//!
//! The GUI and headless hosts share one `~/.pinvou3` home and both
//! install/uninstall/toggle packs, so an in-process mutex alone cannot stop
//! cross-process races: two concurrent load→save sections on the same file
//! silently drop each other's writes (the #515 lost-update class). Every
//! critical section of the three marketplace state files goes through a
//! combined lock — the per-path in-process mutex plus an OS-level file lock
//! (flock / LockFileEx via `fd-lock`, the same crate+primitive as the
//! remote-control process-ownership lock) on `<data file>.lock`:
//!
//! - `marketplace/bundles.json` → `marketplace/bundles.lock` (`store.rs`)
//! - `marketplace/recycle-bin.json` → `marketplace/recycle-bin.lock` (`recycle_bin.rs`)
//! - `mcp.json` → `mcp.lock` (`connectors.rs` writers)
//!
//! Each acquisition opens a fresh lock file, so the OS lock actually excludes
//! other threads of this process too; the in-process mutex stays in front of
//! it so the read path's `try_write` can only ever be beaten by a *peer*
//! process, and so a write's load→modify→save is exclusive before the OS lock
//! is even attempted.
//!
//! # Failure handling (the #517 disposition, applied uniformly)
//!
//! Writes are fail-closed: [`with_file_lock`] returns `Err` when
//! cross-process serialization cannot be established (the lock file cannot be
//! opened, tightened, or locked — e.g. a filesystem without lock support), and
//! callers must refuse the read-modify-write instead of running it
//! unsynchronized — an unlocked RMW is exactly the lost update this module
//! guards against. An `Ok` means the closure ran fully serialized. The wait is
//! unbounded by design (fd-lock v4 has no timeout API); the OS releases the
//! lock when the peer process exits or crashes, but a frozen peer (SIGSTOP /
//! debugger) makes this process wait. Locking is same-host by construction
//! (flock provides no cross-host exclusion on network filesystems), so hosts
//! sharing a network-mounted home are outside the threat model. The critical
//! sections are local JSON read-modify-writes plus directory moves, so that
//! fail-stop hang is accepted over a fail-open lost update.
//!
//! Reads are bounded by construction against *both* contention dimensions:
//! [`try_file_lock_for_read`] only *tries* the in-process mutex and the OS
//! lock. When either is unavailable the read degrades to a bounded,
//! never-persisting unlocked snapshot — writes replace the data files
//! atomically (tmp + rename), so the snapshot is always a complete (possibly
//! just-superseded) state, and the next uncontended read converges. An
//! unexpected lock error (not plain contention) is logged once per failure
//! mode and degrades the same way; hot readers must never couple to a peer's
//! critical section, so no read may hang behind a frozen peer. Note that
//! acquiring either lock materializes `<file>.lock` (and creates its parent
//! directory) even on the pure read path of a pristine home; on a read-only
//! home every read pays a failed create and then degrades as above.
//!
//! # Global lock order (acyclic; extends the #517 analysis)
//!
//! Within every acquisition the order is uniform: in-process mutex → OS file
//! lock. Across locks, the three file locks are **leaves**: their critical
//! sections only load/rewrite their own data file (plus directory moves and,
//! for `mcp.json`, platform credential-store resolutions) and never take
//! another marketplace lock — so there is no ordering edge among
//! `bundles.lock` / `recycle-bin.lock` / `mcp.lock` themselves. Everything
//! else nests INTO them, never out:
//!
//! - `MARKETPLACE_TRANSACTION_LOCK` → file locks: install/uninstall hold the
//!   transaction lock and enter every one of the three inside it.
//! - Per-id `import_lock` → file locks: the unified import path
//!   (`import_lock → bundles.lock`), restore (`import_lock → recycle-bin.lock`,
//!   `import_lock → TRANSACTION → mcp.lock`, `import_lock → bundles.lock`).
//! - The scope lock (#517 — PR still open, landing separately from the
//!   branch carrying this module — `disabled_bundles.lock`) → `bundles.lock`:
//!   the scope critical sections' store legs (legacy-migration id
//!   normalization, DenyAll installed-ids enumeration) re-enter the store
//!   lock; no store method enters the scope's — that ordering is never
//!   reversed.
//!
//! None of these edges is ever taken in reverse (a file-lock section takes no
//! marketplace lock; the scope/transaction/import locks are never acquired
//! from inside a file-lock section), so no deadlock class exists. The
//! transaction lock's own doc in `mod.rs` records the same order from the
//! transaction side, including the pre-existing unordered
//! TRANSACTION↔import_lock pairing this module does not change.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, TryLockError};

/// The cross-process lock file for a marketplace data file: same directory,
/// same stem, `.lock` suffix (`bundles.json` → `bundles.lock` — the same shape
/// as #517's `disabled_bundles.json` → `disabled_bundles.lock`). Holds no user
/// data.
pub(super) fn lock_path_for(data_path: &Path) -> PathBuf {
    data_path.with_extension("lock")
}

/// Per-lock-path in-process mutexes. The registry mutex is only held to
/// get-or-insert the entry and is released before the returned mutex is
/// acquired, so its (trivial) internal order cannot interact with the data
/// locks. Keyed by the derived lock path: same path spelling → same mutex,
/// and the OS lock on the same file still excludes any differently-spelled
/// duplicates.
static PROCESS_MUTEXES: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The in-process mutex serializing `lock_path`'s acquisitions in this
/// process. Exposed for the cross-process regression tests' handshake (the
/// worker must be observed past the mutex acquisition before the absence
/// assertion runs).
pub(super) fn process_mutex_for(lock_path: &Path) -> Arc<Mutex<()>> {
    let mut registry = PROCESS_MUTEXES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registry.entry(lock_path.to_path_buf()).or_default().clone()
}

/// Opens (creating if missing) the cross-process lock file. Shared by the
/// blocking write path and the try-lock read path.
fn open_lock_file(lock_path: &Path) -> Result<std::fs::File, String> {
    let open = || crate::platform::filesystem::open_private_lock_file(lock_path);
    // Open first: once the home exists — the steady state, and reads are the
    // common case — this skips the per-read create_dir_all probe; only a
    // missing file/directory pays for it.
    match open() {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = lock_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create {}: {e}", parent.display()))?;
            }
            open().map_err(|error| format!("open {}: {error}", lock_path.display()))
        }
        Err(error) => Err(format!("open {}: {error}", lock_path.display())),
    }
}

/// Runs a write critical section `f` while holding the combined in-process
/// mutex + OS file lock that serializes every `data_path` load→save against
/// the peer process (GUI / headless sharing the home) (#515, #521). Fallible,
/// not non-blocking: the wait is unbounded by design. Readers are immune to
/// that hang via [`try_file_lock_for_read`]'s degradation.
///
/// Returns `Err` when cross-process serialization cannot be established (the
/// lock file cannot be opened or locked, e.g. a filesystem without lock
/// support). Callers must then refuse the read-modify-write instead of running
/// it unsynchronized.
///
/// Blocking has no timeout (fd-lock v4 has no timeout API): the OS releases
/// the lock when the peer process exits or crashes (flock / LockFileEx die
/// with the fd), but a frozen peer (SIGSTOP / debugger) makes this process
/// wait indefinitely — accepted over a fail-open lost update (see the module
/// docs for the full accounting and the global lock order).
pub(super) fn with_file_lock<F, R>(data_path: &Path, f: F) -> Result<R, String>
where
    F: FnOnce() -> Result<R, String>,
{
    let lock_path = lock_path_for(data_path);
    let process_mutex = process_mutex_for(&lock_path);
    let _process_guard = process_mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let file = open_lock_file(&lock_path)?;
    let mut lock = fd_lock::RwLock::new(file);
    // The in-process mutex is already held while the OS lock is taken, and no
    // lock reachable inside `f` (see the module's global-order note) is held
    // by another thread waiting on this one, so deadlock is impossible. A
    // signal-interrupted flock retries instead of surfacing as a spurious
    // write refusal.
    let _os_guard = loop {
        match lock.write() {
            Ok(guard) => break guard,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(format!("lock {}: {error}", lock_path.display()));
            }
        }
    };
    f()
}

/// Once-per-mode logging for lock-read failures. Reads must stay bounded even
/// when the lock is persistently unavailable (a broken home must not print a
/// line per list refresh) — the first occurrence per failure mode is enough
/// to make the degradation diagnosable.
static READ_FAILURE_LOGGED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

const LOG_LOCK_OPEN: u8 = 1 << 0;
const LOG_LOCK_PROBE: u8 = 1 << 1;

fn log_read_failure(mode: u8, detail: &str) {
    use std::sync::atomic::Ordering;
    if READ_FAILURE_LOGGED.fetch_or(mode, Ordering::Relaxed) & mode == 0 {
        // log (not stderr): packaged Windows GUIs never see eprintln output.
        log::warn!("[marketplace] {detail}");
    }
}

/// Runs a pure read `f` of `data_path`, bounded on *both* lock dimensions:
/// the in-process mutex is only tried, and the OS lock is only tried. When
/// either is contended (or the lock file is unexpectedly unavailable) the
/// read degrades to an unlocked snapshot — the data files are written
/// atomically, so the snapshot is always complete, never persists, and the
/// next uncontended read converges. Contention on either lock is a normal,
/// silent degradation; an unexpected lock error is logged once per failure
/// mode and degrades the same way.
pub(super) fn try_file_lock_for_read<F, R>(data_path: &Path, f: F) -> Result<R, String>
where
    F: FnOnce() -> Result<R, String>,
{
    let lock_path = lock_path_for(data_path);
    // Try the in-process mutex first, so a degraded read below is only ever
    // beaten by a *peer* process's OS lock (never by a local writer's).
    let process_mutex = process_mutex_for(&lock_path);
    let process_guard = match process_mutex.try_lock() {
        Ok(guard) => Some(guard),
        // A local writer is inside its critical section (possibly parked on a
        // frozen peer's OS lock): degrade exactly like peer contention.
        Err(TryLockError::WouldBlock) => None,
        Err(TryLockError::Poisoned(p)) => Some(p.into_inner()),
    };
    // The OS write guard must outlive `f` but cannot outlive its lock, so the
    // lock stays the match scrutinee and the read runs (and returns) from
    // inside the guard arm. Every other arm falls through to the degraded,
    // unlocked snapshot below — bounded, silent, never persisting.
    if process_guard.is_some() {
        match open_lock_file(&lock_path) {
            Ok(file) => match fd_lock::RwLock::new(file).try_write() {
                Ok(_os_guard) => {
                    let result = f();
                    // Every degradation mode has recovered, so a later
                    // persistent failure is diagnosable again.
                    READ_FAILURE_LOGGED.store(0, std::sync::atomic::Ordering::Relaxed);
                    return result;
                }
                // Peer contention is the designed, silent degradation.
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => log_read_failure(
                    LOG_LOCK_PROBE,
                    &format!(
                        "cross-process lock probe failed on {}; unlocked read without persist: {error}",
                        lock_path.display()
                    ),
                ),
            },
            Err(error) => log_read_failure(
                LOG_LOCK_OPEN,
                &format!(
                    "cross-process lock unavailable on {}; unlocked read without persist: {error}",
                    lock_path.display()
                ),
            ),
        }
    }
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `bundles.json` → `bundles.lock`, `recycle-bin.json` →
    /// `recycle-bin.lock`, `mcp.json` → `mcp.lock` — the #517 shape
    /// (`disabled_bundles.json` → `disabled_bundles.lock`) generalized.
    #[test]
    fn lock_path_replaces_the_data_extension() {
        for (data, expected) in [
            ("bundles.json", "bundles.lock"),
            ("recycle-bin.json", "recycle-bin.lock"),
            ("mcp.json", "mcp.lock"),
            ("disabled_bundles.json", "disabled_bundles.lock"),
        ] {
            assert_eq!(
                lock_path_for(&Path::new("/home/x").join(data)),
                Path::new("/home/x").join(expected),
                "{data}"
            );
        }
    }
}
