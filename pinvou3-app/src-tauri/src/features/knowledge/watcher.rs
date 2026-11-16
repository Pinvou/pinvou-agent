// architecture-guard: allow-target-cfg -- the stat-error-vs-delete test needs a
// chmod-000 parent fixture, and mode bits are a unix concept; the production
// branch under test (stat error != NotFound must not delete) is
// platform-independent, only the fixture cannot be expressed portably. The same
// class covers this file's other unix-only test fixtures (symlink creation and
// non-UTF-8 names via OsStrExt, likewise unexpressible portably).

//! Real-time incremental indexing: notify watches the roots already enrolled
//! in the knowledge feature, and file changes update the L0 metadata store
//! immediately.
//!
//! Event handling (aligned with file_watcher.rs's "judge by existence"
//! approach), for every path carried by an event:
//! - `symlink_metadata` succeeds and the type is a regular file or directory →
//!   **root-scoped** component-level exclusion + [`Excluder::is_skipped`] (the
//!   same policy source as the scanner, including the `is_dir` context, so
//!   dot-suffixed directories and extensionless files agree with scan results),
//!   then `upsert`
//! - `NotFound` (deleted / renamed away) → [`Store::delete_tree`]: a directory
//!   record is deleted together with its subtree (a dir rename/move-out only
//!   surfaces the parent-path event), and a file degrades to an exact delete
//! - stat succeeds but the type is not indexable (symlink/socket/fifo) → also
//!   takes the delete branch: when the entity was replaced in place by a
//!   symlink or similar (the common `rm f && ln -s` rhythm), the old record is
//!   already a ghost — clear it; when the store has no row this is a safe
//!   no-op (the scanner never builds rows for those types)
//! - other stat errors (permission tightened / mount jitter) → warn only, do
//!   not delete, to avoid removing records for entities that still exist
//!
//! Paths with non-UTF-8 spellings enter the store on neither side:
//! [`scanner::record_from_parts`]'s `to_str` is the shared final gate for both
//! paths (the scanner goes through `to_record`, the watcher through event
//! paths), so a "clean leaf" under a bad-name directory likewise never reaches
//! record assembly (the scanner, per its empty-name semantics, prunes the whole
//! subtree at filter_entry) — the stored outcome agrees. Delete events for such
//! paths match no row under the lossy spelling (unless some row's name happens
//! to contain a literal U+FFFD replacement character — legal UTF-8 but
//! extremely rare); the normal case is a safe no-op.
//!
//! Exclusion scope is aligned with the scanner: recursive watching does not
//! prune automatically, so paths deep inside node_modules/.ssh etc. still
//! surface in events; apply component-level exclusion over **the watched
//! root's visible scope** first (the root's own basename + components below
//! it; ancestor components above the root are never visible to the scanner
//! and must not be allowed to drop a whole root's events); delete events
//! apply the same, and surviving paths are then type-judged by `is_skipped`.
//!
//! `spawn` first canonicalizes every root (failure warns and skips that root;
//! duplicate spellings of one entity register once; non-UTF-8 roots are
//! likewise rejected — fsevent's registration internally does
//! `to_str().unwrap()` and a non-UTF-8 root would panic the whole watcher
//! thread, and under the `to_str` gate such roots can never produce a row, so
//! skipping loses nothing). Watch registration and event attribution both use
//! the canonical spelling — macOS fsevent delivers canonicalized event paths
//! and Linux inotify absolutizes relative roots, so attributing by the
//! caller's original spelling would be pierced by spelling drift (a whole
//! root's events falling to the out-of-root fallback, or one row per spelling
//! under two spellings). **The load-bearing half of the wiring contract is
//! the scanner side: the caller must give the scanner the same canonical
//! spelling (hard requirement, not a suggestion).** `spawn` canonicalizes
//! unconditionally, so whatever spelling reaches `spawn` is tolerated. On
//! Windows "canonical spelling" means the verbatim `\\?\C:\…` form
//! `fs::canonicalize` returns — rows will carry that prefix, and a
//! non-verbatim decision (strip it at wiring time) must be made consciously
//! and applied to **both** sides of the contract. The
//! scanner, however, walks the raw spelling verbatim (walkdir follows root
//! scanner, however, walks the raw spelling verbatim (walkdir follows root
//! symlinks by default — `follow_root_links`), so a raw-spelled symlink root
//! splits the policy by itself: rows land under the raw spelling while the
//! watcher attributes events under the entity path — when the entity's name
//! is on the exclusion list (`node_modules` etc.) the scanner keeps producing
//! rows whose every event is dropped wholesale, and otherwise two spellings
//! each grow a row while delete/update events only ever hit one of them —
//! exclusion and incremental maintenance split at once.
//!
//! Known limits (the wiring side must know):
//! - Recursively watching `$HOME`-scale roots on Linux consumes a large
//!   number of inotify watches and may hit `fs.inotify.max_user_watches`.
//!   Hitting the limit during first registration: notify returns the first
//!   registration error directly, `watch()` reports Err, this side warns and
//!   does **not** count that root as watched — but watches registered before
//!   the failure are not rolled back, leaving the root partially watched.
//!   Truly silent partial watching has another source: entries that
//!   stat-jitter or are unreadable during the registration walk are swallowed
//!   by the backend (watch() still returns Ok and the log still reports
//!   watched). Events from already-registered sub-watches of a partially
//!   watched root still arrive while the root is not in the watched set:
//!   those events fall to the out-of-root fallback and are judged over the
//!   **full path** — if that root sits under an excluded-name ancestor
//!   (`build`/…), its incremental events are silently dropped wholesale (scan
//!   rows remain; rescan covers). A mid-run registration failure for a new
//!   directory arrives as a channel `Err` (MaxFilesWatch) and warns only
//!   once; afterwards no new directory is watched until restart — and
//!   mid-run `add_watch` failures *other* than the watch limit (EACCES on a
//!   mode-000 directory, ENOENT after instant recreation) are swallowed
//!   inside notify with no channel `Err`, leaving that branch unwatched for
//!   the run; the lazy rescan covers both. Large root
//!   sets fall back to the lazy-triggered rescan.
//! - A root dropped before registration stays unwatched for the watcher
//!   thread's entire lifetime: `prepare_roots` skips roots whose
//!   canonicalize fails or whose spelling is non-UTF-8 (warn + skip), and
//!   while the thread is alive any later `spawn` is ignored by the
//!   single-instance guard — there is no way to add a root to a running
//!   watcher. Re-arming requires one of the thread's exit paths (backend
//!   death, all-roots-failed, …) followed by a fresh `spawn`; until then the
//!   lazy rescan is the only coverage for such roots.
//! - The fallback rescan is **lazily triggered** (`kb_start_scan` runs when
//!   the file-management page is entered); there is no periodic rescan.
//!   Overflow / dropped watches can only warn, and the index stays stale
//!   until the next rescan — turning the `need_rescan` warning into an
//!   actual rescan trigger requires a machine-readable surface (a flag or
//!   channel the scan entry can poll) **added to this module first**; today
//!   the signal exists only as this thread's stderr warn, so #869 owns both
//!   the surface and the wiring. Linux inotify's Q_OVERFLOW event
//!   carries **no paths** (the kernel queue is shared by all roots), so only
//!   a full all-roots rescan is possible; macOS fsevent's dropped-event
//!   signal names the affected paths; **Windows has no dropped-events signal
//!   at all** — backend buffer overflow is logged only to its internal `log`
//!   (visible only when a logger is installed) and root deletion is
//!   completely silent; both silently stop the watch (no events and no
//!   channel Err). On Windows the lazy-triggered rescan is the only backstop.
//!   Also not watchable at all: **non-local volumes** — fsevent ignores
//!   `FSEventStreamStart`'s return on network mounts (FSEvents does not
//!   support them), Linux inotify sees only same-host changes on NFS, and
//!   Windows RDCW on SMB shares is best-effort — every case reports
//!   "watching N/N" and silently delivers nothing, so the lazy rescan is
//!   the only coverage there too.
//! - macOS fsevent's backend-thread death is equally unobservable: an event
//!   callback panic at the extern "C" boundary aborts the whole process
//!   (e.g. a delivered path containing non-UTF-8 — possible on exFAT/SMB
//!   mounts, notify's internal `to_str().expect()`), rather than unwinding
//!   the thread; even when the backend thread dies without panicking, the
//!   user-side tx is retained by the `FsEventWatcher` struct and never drops,
//!   so `rx` hangs forever (the channel-closed warning never fires, the
//!   guard never unlocks). On macOS the lazy-triggered rescan is the only
//!   backstop; the wiring side must not assume macOS is observable or
//!   retryable. Two further macOS-specific infidelities: fsevent's `watch()`
//!   still returns Ok when the runloop thread spawn fails or
//!   `FSEventStreamStart` fails (notify ignores both return values), so the
//!   startup line reports "watched" while the backend is already dead — the
//!   success log is not trustworthy on macOS; and multi-root registration
//!   restarts the whole stream per root (stop+run, sinceWhen=SinceNow), so
//!   events for earlier-registered roots inside the registration window are
//!   lost (startup only, covered by the first scan). The opposite direction
//!   also happens: fsevent can **replay writes made before `spawn`** when
//!   registration lands after them (observed live: rows for pre-spawn files
//!   appearing at startup in some processes) — benign for indexing
//!   (idempotent upserts), but the wiring side must not assume
//!   scan-then-spawn ordering means pre-spawn writes never arrive as
//!   events. A volume mounted under
//!   a root that is later ejected/yanked delivers one `Remove(Other)` on the
//!   mount-point path while the underlying directory still stats — the
//!   existence judgment treats it as that directory's (content-identical)
//!   upsert, and the rows under the mount point ghost until the next rescan.
//! - Symlinked directories inside the tree are not descended: the watcher is
//!   constructed with `with_follow_symlinks(false)`, the same direction as
//!   the scanner's `follow_links(false)` (notify defaults to following, so
//!   this must be turned off explicitly at construction). The setting is
//!   honored by the inotify/kqueue/poll backends; fsevent/Windows ignore it,
//!   but those two backends do not descend symlinked directories anyway
//!   (Windows' traversal behavior for junctions is unverified — when wiring,
//!   test store parity between an event-driven and a scan-driven run over a
//!   junction fixture, not merely that the watch registers). A symlinked
//!   **root** itself: after canonicalization it is watched and attributed
//!   under the entity path, with no more "empty watch / partial watch"
//!   ambiguity.
//! - A directory rename surfaces events only for the directory's own old and
//!   new paths; **the files under the new path produce no events at all** —
//!   the same shape on all three platforms (Linux inotify's watch follows
//!   the inode and untouched children do not fire; Windows reports only the
//!   directory's OLD_NAME/NEW_NAME; macOS FSEvents reports the directory,
//!   not the contents): the old subtree is cleared by
//!   [`Store::delete_tree`], and child rows are restored only by the next
//!   rescan or a later rewrite.
//! - The watched root itself: on Linux inotify, deleting it kills the watch
//!   permanently (no parent watch to re-hang), and renaming it makes events
//!   carry the old prefix forever (equivalently dead) — both rely on rescan;
//!   macOS fsevent's stream survives by path and events resume after the
//!   root is recreated; **Windows is completely silent**: the watch survives
//!   by handle, and after a root rename events keep arriving under the old
//!   prefix (the existence judgment then clears the old rows), while events
//!   for the new location never come, with no log and no channel Err — the
//!   lazy-triggered rescan is the only backstop.
//! - macOS fsevent renames arrive as `RenameMode::Any` single-path events;
//!   the old and new sides arrive independently and cannot be correlated;
//!   judging by existence, the old-side event clears the old record as soon
//!   as it arrives, and when it never arrives the old record waits for
//!   rescan. But on case-insensitive filesystems (APFS default, Windows
//!   NTFS) a **case-only rename** (`mv notes.md NOTES.MD`) pierces that
//!   judgment: the old spelling's stat still resolves to the renamed file →
//!   the old side is upserted as alive, old and new spellings coexist as two
//!   rows, one of them permanently stale until rescan (a delete event for
//!   the old spelling will never come). Cleaning up such old rows is a
//!   wiring-side decision (#869).
//! - No debouncing: consecutive events for the same file are each processed;
//!   the write side resolves them via [`UPSERT_IF_CHANGED_SQL`] — repeat
//!   events whose content matches the store skip the whole row (indexed_at
//!   untouched), changed content rewrites as usual (idempotent). This is a
//!   recorded decision, not an oversight: the `notify-debouncer-full` /
//!   `-mini` companion crates were weighed against it and deliberately not
//!   added — they would pull extra dependencies for ordering/rename tracking
//!   this design does not need (existence judgment is order-insensitive),
//!   while the SQL-level elision already collapses the repeat-event cost.
//!   Re-evaluate only if the wiring side needs cross-event coalescing for
//!   bulk storms; the event
//!   channel is unbounded; when the consumer is stalled by a long
//!   transaction or slow disk the queue accumulates (100k-scale events ≈
//!   tens of MB); bulk-ingestion scenarios are for the wiring side to
//!   evaluate coalescing on (file_watcher.rs's debounce covers only
//!   sched-*.json — borrow its window idea, not its scope). One class the
//!   elision cannot cover: **directory rows on child churn** — a
//!   directory's mtime genuinely changes when entries are created/removed
//!   in it (Linux fires IN_ATTRIB on the parent for every child change), so
//!   each child event also rewrites the parent's row (its FTS trigrams are
//!   skipped by the WHEN clause, but the files-table row rewrite remains);
//!   per-event transactions also mean an `rm -rf` with D directories takes
//!   the write lock D times — both are amortization targets for the
//!   wiring-side drain window.
//! - Concurrency windows between scan and events, and between events: a file
//!   deleted mid-rescan may be written back by the scan batch (the batched
//!   commit lands after the event's delete), and the row stays until the
//!   next rescan. Likewise, when a path is recreated in place while its
//!   directory-deletion event is queued (the common `rm -rf && mkdir`
//!   rhythm), the existence judgment turns that deletion into the new
//!   entity's upsert, and the old directory's child rows (old inode, no more
//!   events) remain until rescan. Wiring that needs strong consistency must
//!   wait for a pause surface (a gate the event loop checks) **added to this
//!   module first** (#869) — today event application cannot be paused or
//!   stopped once `spawn` succeeds. Also: after a directory is renamed into an
//!   excluded name (`proj`→`node_modules`), subsequent events for its files
//!   all carry the excluded component and are dropped; the old rows stored
//!   before the rename are cleared by the old-path delete event (only
//!   waiting for rescan when the old-side event never arrives).
//!
//! Currently unwired: `spawn` has no production caller yet (the same
//! landed-but-unwired state as the closed-source side); the follow-up wiring
//! owner calls it per root at the startup scan. The watcher is an app-level
//! singleton by design: `spawn` has a built-in re-entry guard and duplicate
//! calls are ignored; the thread's **reachable** exit paths (init failure,
//! all-roots registration failure, event-loop end, panic contained by
//! catch_unwind) all release the guard, after which a fresh `spawn` re-arms.
//! Two disclosed exceptions (see known limits): macOS fsevent backend death
//! neither unwinds this thread nor closes the channel — the guard stays
//! locked with no retry; Linux inotify's registration-window death is
//! contained by [`run_watcher`]'s ManuallyDrop + catch_unwind, degrading to
//! "failure warn + retryable" (the pre-containment code amplified the panic
//! into a process abort via panic-during-unwind). Diagnostic semantics are
//! therefore not equivalent across platforms — some macOS/Windows scenarios
//! are unobservable, and the wiring side must not assume a uniform failure
//! signal. To change the root set, the thread still needs a stop interface
//! first (#869); do not stack `spawn` calls.

#![allow(dead_code)] // library-first: spawn has no production caller before wiring (see the note above)

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::exclude::Excluder;
use super::scanner::{ext_of, record_from_parts};
use super::store::{FileRecord, Store};

/// Non-panicking stderr write: `eprintln!` panics when stderr is closed
/// (Rust ignores SIGPIPE, so a write failure panics), and the watcher thread
/// dying on such a log would silently stop incremental indexing — a write
/// failure is ignored in place (the same inline guard as mod.rs's scan
/// thread, see its comment). Expanded as a block expression (rather than a
/// bare statement) so it remains usable inside let-else branch bodies
/// alongside other statements.
macro_rules! warn {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stderr(), "[kb_watcher] {}", format_args!($($arg)*));
    }};
}

/// Single-instance guard: the watcher thread is designed to live for the
/// app's lifetime (there is no stop interface yet, #869), and a duplicate
/// `spawn` would stack a second thread double-consuming events for
/// the same roots (and double-occupy inotify watches). The wiring side must
/// stop the thread first when changing the root set. RAII: every exit path
/// (init failure, all-roots registration failure, panic unwind, natural
/// thread end) releases the guard, after which a fresh `spawn` can re-arm —
/// i.e. "lives as long as the app" is the intended steady state, not a
/// guarantee (see the known-limits exit paths above).
static ACTIVE: AtomicBool = AtomicBool::new(false);

struct ActiveGuard;

impl ActiveGuard {
    /// Returns None when a watcher is already active (duplicate `spawn` is
    /// ignored).
    fn acquire() -> Option<Self> {
        ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            .then_some(Self)
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::SeqCst);
    }
}

/// Starts the background watcher thread watching the given roots. Returns
/// immediately after spawning; the thread is designed to live for the app's
/// lifetime (its reachable exit paths are the known-limits list, and each
/// releases the guard); when a
/// watcher is already active this call is ignored (see [`ACTIVE`]). An empty
/// root list returns immediately without arming anything (no log either).
/// Pass a clone of the service's `Store` — do not `open()` a second store on
/// the same file: that is safe by design (WAL + busy_timeout, and the
/// two-process pair is supported usage), but it buys extra lock contention
/// for nothing.
pub fn spawn(store: Store, roots: Vec<PathBuf>) {
    if roots.is_empty() {
        return;
    }
    let Some(active) = ActiveGuard::acquire() else {
        warn!("spawn ignored: a watcher thread is already active");
        return;
    };
    thread::spawn(move || {
        // The guard lives with the thread body: every exit path — including a
        // panic contained by catch_unwind — reaches the closure's end and
        // drops the guard there, so spawn is never latch-locked for good.
        let _active = active;
        // The same containment as this feature's scan thread (see mod.rs): a
        // thread panic leaves one traceable log line instead of a silent
        // death.
        if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_watcher(&store, &roots);
        })) {
            warn!(
                "watcher thread panicked (contained; index stays stale until \
                 the next scan): {panic:?}"
            );
        }
    });
}

/// Pre-registration preparation for the root set: canonicalize (failure warns
/// and skips), dedupe duplicate spellings of one entity, reject non-UTF-8.
/// Factored out of `run_watcher` as a pure, unit-testable function.
///
/// Non-UTF-8 roots must be rejected: fsevent (the macOS default backend)
/// does `to_str().unwrap()` inside its registration path, so a non-UTF-8
/// root would panic the whole watcher thread (after catch_unwind contains
/// it, the remaining roots die with it). The rejection loses no coverage on
/// any platform — `record_from_parts`'s `to_str` gate means event paths
/// under such a root can never assemble a row, so watching it would only
/// ever produce no-ops.
fn prepare_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut prepared: Vec<PathBuf> = Vec::new();
    for root in roots {
        let Ok(canonical) = root.canonicalize() else {
            warn!("root({}) canonicalize failed; skipping it", root.display());
            continue;
        };
        if prepared.contains(&canonical) {
            continue; // duplicate spellings of one entity register once
        }
        if canonical.to_str().is_none() {
            warn!(
                "root({}) is not valid UTF-8; skipping it",
                canonical.display()
            );
            continue;
        }
        prepared.push(canonical);
    }
    prepared
}

/// Thread body (factored out of `spawn` so catch_unwind can contain it):
/// registers all roots, then consumes events.
fn run_watcher(store: &Store, roots: &[PathBuf]) {
    let ex = Excluder::default();
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    // follow_symlinks(false): notify follows symlinks by default (inotify's
    // registration descends into symlinked directories too), the opposite of
    // the scanner's follow_links(false) — must be turned off explicitly at
    // construction. The setting is honored by the inotify/kqueue/poll
    // backends; fsevent/Windows ignore it, but those two backends do not
    // descend symlinked directories anyway.
    // ManuallyDrop is one half of the registration-window death containment
    // (see the catch_unwind comment below): notify's panic-prone Drop must
    // not fire during unwind — inotify's `INotifyWatcher::drop` unconditionally
    // unwraps a send(Shutdown)/wake against a dead loop, and a Drop panicking
    // during unwind is panic-inside-panic: the process aborts outright.
    let mut watcher = std::mem::ManuallyDrop::new(
        match RecommendedWatcher::new(tx, notify::Config::default().with_follow_symlinks(false)) {
            Ok(w) => w,
            Err(e) => {
                warn!("init failed: {e}");
                return; // ActiveGuard::drop releases the guard
            }
        },
    );
    let mut watched_roots: Vec<PathBuf> = Vec::new();
    // Roots go through prepare_roots first (canonicalize / dedup / non-UTF-8
    // rejection, see its comment): watch registration and event attribution
    // both use the canonical spelling. Prepared once and shared by the
    // registration and the startup line, so the "watching X/Y" denominator
    // counts registrable roots — a deduped entity no longer reads as a
    // partial failure.
    let prepared = prepare_roots(roots);
    // Registration-window death containment: if the inotify backend thread
    // dies before/during registration (notify swallows thread-spawn failure
    // with `let _ =`; poll failure panics directly), `watch()` does not
    // return Err but panics (the send/wake unwraps land on a dead event
    // loop). After containment, forget the watcher — the backend is dead, so
    // Drop would panic again and has nothing to clean up anyway (the fd is
    // held by the dead event loop and was closed during its unwind); the
    // thread exits as an ordinary failure, the guard is released at closure
    // end, and a later spawn can re-arm. This path cannot be unit-tested
    // (it needs a real backend death); the notify 8.2.0 sources are the
    // evidence (inotify.rs's watch_inner and Drop unwraps).
    let registration = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for canonical in &prepared {
            match watcher.watch(canonical, RecursiveMode::Recursive) {
                Ok(()) => watched_roots.push(canonical.clone()),
                // Usually the inotify limit or permissions; warn and let the
                // lazy-triggered rescan cover it.
                Err(e) => warn!("watch({}) failed: {e}", canonical.display()),
            }
        }
    }));
    if registration.is_err() {
        // SAFETY: `take` moves the watcher out of the ManuallyDrop (leaving
        // `watcher` logically uninitialized; this function returns
        // immediately and never touches it again), and the outer `forget`
        // suppresses its panic-prone Drop (see the comment above) — which is
        // exactly the purpose of this containment.
        std::mem::forget(unsafe { std::mem::ManuallyDrop::take(&mut watcher) });
        warn!("watcher backend died during registration; inactive until a later spawn retries");
        return; // ActiveGuard::drop releases the guard
    }
    // Registration finished (including the all-roots-Err failure state):
    // back to ordinary ownership, so later early exits take the normal Drop —
    // a live backend needs Drop to send Shutdown for a clean exit (close the
    // fd, stop the thread). The event loop never touches the watcher again;
    // its only remaining use is the forget at loop end (see below).
    let watcher = std::mem::ManuallyDrop::into_inner(watcher);
    if watched_roots.is_empty() {
        warn!("no root could be watched; inactive until a later spawn retries");
        return; // ActiveGuard::drop releases the guard; the watcher's normal Drop winds down the live backend
    }
    warn!(
        "watching {}/{} root(s)",
        watched_roots.len(),
        prepared.len()
    );

    let mut max_files_warned = false;
    for res in rx {
        match res {
            Ok(ev) => {
                // The backend's dropped-events signal (inotify Q_OVERFLOW /
                // fsevent KERNEL_DROPPED etc., arriving as EventKind::Other +
                // the Rescan flag): this is the only proactive notice that
                // "incremental indexing has become unreliable" — it must be
                // shouted so the wiring side can schedule a rescan.
                if ev.need_rescan() {
                    warn!(
                        "backend dropped events (rescan requested); \
                             index stays stale until the next scan"
                    );
                }
                // Event admission (Create/Modify/Remove) is decided uniformly
                // by `apply` (the test contract lives there); no duplicate
                // front check here.
                apply(&ex, store, &watched_roots, &ev);
            }
            // An Err on the channel: read-loop IO error, mid-run
            // MaxFilesWatch (notify stops extending watches afterwards) etc.
            // must not be swallowed silently either. MaxFilesWatch re-arrives
            // for every new directory; warn only once against a log storm.
            Err(e) => {
                if matches!(e.kind, notify::ErrorKind::MaxFilesWatch) {
                    if !max_files_warned {
                        max_files_warned = true;
                        warn!(
                            "event error: {e} (watch limit hit; new dirs stay \
                             unwatched until restart, rescan covers the gap)"
                        );
                    }
                } else {
                    warn!("event error: {e}");
                }
            }
        }
    }
    // Reaching here means the loop ended = every user-side tx clone has been
    // dropped = the backend is gone (the backend holds the only tx). Forget
    // the local watcher before warning: inotify's Drop unconditionally
    // unwraps its send/wake against a dead loop, so a normal drop would be
    // contained by the outer catch_unwind as a second "thread panicked" log —
    // one backend death reported as two faults, misleading triage; the forget
    // avoids it (the backend is dead and Drop has nothing to clean up: on
    // Windows the graceful Drop against a dead server is already a no-op, and
    // on inotify the fd is held by the dead event loop). Today's reachable
    // scenario for this warning: a Linux backend thread dying mid-run (the
    // event loop unwinds, dropping the tx with it). Backend thread-spawn
    // failures never reach loop end — on Linux they surface through the
    // registration-window containment above (the first watch() panics against
    // the dead loop) or the no-roots early return, and on Windows through the
    // all-roots-failed return. A Windows server thread dying mid-run has no
    // reachable panic path today; if upstream introduces one, the in-flight
    // request's handler Arc would keep the tx alive and rx would never close —
    // the warning would then not fire either (same shape as macOS; the lazy
    // rescan is the backstop). macOS fsevent has always been this way: a
    // callback panic aborts the process at the extern "C" boundary, and the
    // handler Arc is held by the FsEventWatcher struct itself, so the tx does
    // not drop even when the backend thread dies — rx hangs forever, this
    // warning never fires, and the guard stays locked. On macOS the only
    // backstop is the lazy rescan (see the module doc's known limits). The
    // guard is released at the calling closure's end (one step after this
    // log), after which a spawn can re-arm.
    std::mem::forget(watcher);
    warn!(
        "event channel closed; the watcher backend is gone (index stays stale until \
         the next scan, a later spawn re-arms)"
    );
}

/// Event admission: only Create/Modify/Remove change index state. Access
/// (read/open/close) and Other (miscellaneous without the Rescan flag) must
/// not enter the existence judgment — otherwise an Access event for an
/// "indexed but just deleted" path becomes a spurious judge outside the
/// delete signal (reading a nonexistent path does not mean the index should
/// drop it).
fn apply(ex: &Excluder, store: &Store, roots: &[PathBuf], ev: &Event) {
    if !matches!(
        ev.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) {
        return;
    }
    handle_event(ex, store, roots, ev);
}

/// Component-level exclusion for event paths, taken over the **relative
/// scope** of the watched roots: consistent with the components visible to
/// the scanner (walkdir's filter_entry sees the root itself first; ancestor
/// components above it are never visible to the scanner). Roots are
/// canonicalized in spawn and match the spelling the backends deliver, so
/// "no owning root found" normally cannot happen — the full-path judgment is
/// kept only defensively (races between canonicalization and registration
/// etc.).
fn excluded(ex: &Excluder, roots: &[PathBuf], path: &Path) -> bool {
    match roots
        .iter()
        .filter(|r| path.starts_with(r))
        .max_by_key(|r| r.components().count())
        .map(|r| r.as_path())
    {
        Some(root) => ex.is_excluded_under_root(path, root),
        None => ex.is_excluded_path(path),
    }
}

/// Handles a single event: buckets by existence (alive → upsert, gone →
/// delete), at most two batched writes per event (one delete batch, one
/// upsert batch). Factored out of the thread closure for unit testing.
fn handle_event(ex: &Excluder, store: &Store, roots: &[PathBuf], ev: &Event) {
    let mut ups: Vec<FileRecord> = Vec::new();
    let mut dels: Vec<String> = Vec::new();
    for path in &ev.paths {
        // Component-level exclusion (the root itself + names/secrets below
        // it): churn directories are skipped wholesale, without a stat.
        if excluded(ex, roots, path) {
            continue;
        }
        match std::fs::symlink_metadata(path) {
            Ok(md) => {
                let ft = md.file_type();
                if !ft.is_file() && !ft.is_dir() {
                    // The symlink/socket/fifo body does not enter the store
                    // (same as the scanner). But when the entity was replaced
                    // in place by a symlink or similar, the old path's record
                    // is already a ghost → take the delete branch to clear it
                    // (no row in the store: safe no-op; a directory row:
                    // cleared together with its subtree).
                    dels.push(path.to_string_lossy().into_owned());
                    continue;
                }
                let is_dir = ft.is_dir();
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                let ext = if is_dir { None } else { ext_of(path) };
                if ex.is_skipped(name, is_dir, ext.as_deref()) {
                    continue;
                }
                if let Some(rec) = record_from_parts(path, name, is_dir, Some(&md)) {
                    ups.push(rec);
                }
            }
            // Deleted / renamed away: delete the record; a directory takes its
            // subtree along (inotify will not deliver further events for the
            // moved subtree).
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                dels.push(path.to_string_lossy().into_owned());
            }
            // Permission tightening / ESTALE jitter: do not delete — wrongly
            // deleting a record that still exists hurts more than missing one
            // update; the lazy rescan covers it.
            Err(e) => warn!("stat({}) failed: {e}", path.display()),
        }
    }
    // A failed store write is the same class as stat jitter: both are
    // "incremental indexing silently missed a beat". It must be shouted —
    // the index keeps missing this update until the next rescan, and
    // swallowing it silently leaves the "needs rescan" signal nowhere to be
    // seen. Deletes run before writes: if one event ever carried both a
    // "directory delete" and a "new file under it" (no current backend
    // synthesizes this shape — a defensive guarantee), delete-first lets the
    // new row survive, while the other order would let the prefix delete eat
    // it.
    if !dels.is_empty() {
        if let Err(e) = store.delete_tree(&dels) {
            warn!("delete({}) failed: {e}", dels.len());
        }
    }
    if !ups.is_empty() {
        // The incremental path uses the resolving upsert (repeat events with
        // identical content do not rewrite the row); the scan's full-confirm
        // path keeps upsert_many's touch-on-write semantics (the restart's
        // last_scan_finished_at fallback reads MAX(indexed_at)); see
        // UPSERT_IF_CHANGED_SQL.
        if let Err(e) = store.upsert_many_if_changed(&ups) {
            warn!("upsert({}) failed: {e}", ups.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind, RemoveKind, RenameMode};
    use std::fs;
    use std::path::Path;

    fn tmp(tag: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("pinvou3_kb_watch_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn ev(kind: EventKind, paths: &[&Path]) -> Event {
        let mut e = Event::new(kind);
        for p in paths {
            e = e.add_path(p.to_path_buf());
        }
        e
    }

    fn indexed(store: &Store) -> Vec<String> {
        let mut v: Vec<String> = store.load_index().unwrap().into_keys().collect();
        v.sort();
        v
    }

    /// Create → upsert (fields from a real stat), Remove → delete; Access
    /// events do not enter the existence judgment (an Access for an "indexed
    /// but already gone" path must not trigger a delete).
    #[test]
    fn create_and_remove_events_track_the_store() {
        let base = tmp("cr");
        fs::write(base.join("notes.md"), b"# note").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&base.join("notes.md")]),
        );
        assert_eq!(
            indexed(&store),
            vec![base.join("notes.md").to_string_lossy().into_owned()],
            "create event upserts exactly the touched file"
        );

        // Access (read/close) does not change index state: the path has
        // vanished from disk but the event is only Access — if admission
        // leaked, this event would walk the NotFound branch into the delete
        // branch.
        fs::remove_file(base.join("notes.md")).unwrap();
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Access(AccessKind::Close(AccessMode::Write)),
                &[&base.join("notes.md")],
            ),
        );
        assert_eq!(
            indexed(&store).len(),
            1,
            "access events must not reach the delete branch"
        );

        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Remove(RemoveKind::File),
                &[&base.join("notes.md")],
            ),
        );
        assert!(
            indexed(&store).is_empty(),
            "removed path must leave the index"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// A directory Remove event: the directory record is deleted together
    /// with its subtree (inotify delivers no further events for the moved
    /// subtree); sibling paths sharing only a prefix of characters must not
    /// be wrongly deleted.
    #[test]
    fn dir_remove_deletes_the_subtree() {
        let base = tmp("dirsub");
        fs::create_dir_all(base.join("项目/sub")).unwrap();
        fs::write(base.join("项目/报告.pdf"), b"p").unwrap();
        fs::write(base.join("项目/sub/a.md"), b"a").unwrap();
        fs::write(base.join("项目备份.md"), b"s").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(EventKind::Create(CreateKind::Folder), &[&base.join("项目")]),
        );
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Create(CreateKind::File),
                &[
                    &base.join("项目/报告.pdf"),
                    &base.join("项目/sub/a.md"),
                    &base.join("项目备份.md"),
                ],
            ),
        );
        assert_eq!(indexed(&store).len(), 4, "dir + 2 children + sibling");

        fs::remove_dir_all(base.join("项目")).unwrap();
        apply(
            &ex,
            &store,
            &roots,
            &ev(EventKind::Remove(RemoveKind::Folder), &[&base.join("项目")]),
        );

        let idx = indexed(&store);
        assert_eq!(
            idx,
            vec![base.join("项目备份.md").to_string_lossy().into_owned()],
            "subtree gone, prefix-overlapping sibling intact"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// A rename (inotify's Both event carries [from, to]): the old path is
    /// deleted and the new path enters the store.
    #[test]
    fn rename_event_deletes_old_and_upserts_new() {
        let base = tmp("ren");
        fs::write(base.join("old.md"), b"# old").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&base.join("old.md")]),
        );
        fs::rename(base.join("old.md"), base.join("新文档.md")).unwrap();
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                &[&base.join("old.md"), &base.join("新文档.md")],
            ),
        );

        let idx = indexed(&store);
        assert_eq!(
            idx,
            vec![base.join("新文档.md").to_string_lossy().into_owned()]
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// Separate From/To events are equivalent to Both: the Windows backend
    /// only delivers separate events and never Both, and macOS delivers the
    /// old and new sides independently (uncorrelatable) — a refactor of the
    /// "skip From/To, finish with Both" style that breaks the separate shape
    /// would silently break Windows rename handling; this test bites it.
    #[test]
    fn separate_from_to_rename_events_match_the_both_shape() {
        let base = tmp("renft");
        fs::write(base.join("旧.md"), b"# old").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&base.join("旧.md")]),
        );
        fs::rename(base.join("旧.md"), base.join("新.md")).unwrap();
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Modify(ModifyKind::Name(RenameMode::From)),
                &[&base.join("旧.md")],
            ),
        );
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Modify(ModifyKind::Name(RenameMode::To)),
                &[&base.join("新.md")],
            ),
        );

        assert_eq!(
            indexed(&store),
            vec![base.join("新.md").to_string_lossy().into_owned()],
            "From deletes the old path, To upserts the new one"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// Exclusion scope stops at the watched root: ancestor components
    /// **above** the root no longer participate — the scanner indexes roots
    /// under excluded-name ancestors like `build`/`snap` in full (walkdir
    /// starts at the root itself and cannot see the ancestors), so a watcher
    /// judging components over the full path would silently drop a whole
    /// root's events; and when the root's own basename hits the list, the
    /// scanner's filter_entry prunes the whole tree at the first entry, so
    /// the watcher must agree.
    #[test]
    fn exclusion_scope_stops_at_the_watched_root() {
        let base = tmp("rootscope");
        let root = base.join("build/proj"); // the ancestor "build" ∈ SKIP_NAMES
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("报告.pdf"), b"r").unwrap();
        let excluded_root = base.join("node_modules"); // the root's own basename hits the list
        fs::create_dir_all(excluded_root.join("pkg")).unwrap();
        fs::write(excluded_root.join("pkg/x.pdf"), b"x").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![root.clone(), excluded_root.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&root.join("报告.pdf")]),
        );
        apply(
            &ex,
            &store,
            &roots,
            &ev(EventKind::Create(CreateKind::Folder), &[&excluded_root]),
        );
        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&excluded_root.join("pkg/x.pdf")]),
        );

        assert_eq!(
            indexed(&store),
            vec![root.join("报告.pdf").to_string_lossy().into_owned()],
            "ancestors above the watched root must not exclude events; a root whose \
             own basename is excluded stays pruned like the scanner"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// Exclusion policy shares the scanner's source: churn ancestors are
    /// skipped wholesale (even when the last component is a whitelisted
    /// type), extensionless sensitive files never enter the store,
    /// dot-suffixed directories pass as directories, source extensions are
    /// excluded, whitelisted types enter the store.
    #[test]
    fn exclusion_matches_the_scanner_policy() {
        let base = tmp("excl");
        fs::create_dir_all(base.join("node_modules/pkg")).unwrap();
        fs::create_dir_all(base.join("backup.old")).unwrap();
        fs::write(base.join("node_modules/pkg/保险报价单.pdf"), b"p").unwrap();
        fs::write(base.join(".bash_history"), b"hist").unwrap();
        fs::write(base.join("main.c"), b"int main").unwrap();
        fs::write(base.join("报告.pdf"), b"r").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        let creates = EventKind::Create(CreateKind::File);
        for p in [
            "node_modules/pkg/保险报价单.pdf",
            ".bash_history",
            "main.c",
            "报告.pdf",
        ] {
            apply(&ex, &store, &roots, &ev(creates, &[&base.join(p)]));
        }
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Create(CreateKind::Folder),
                &[&base.join("backup.old")],
            ),
        );

        let idx = indexed(&store);
        assert_eq!(
            idx,
            vec![
                base.join("backup.old").to_string_lossy().into_owned(),
                base.join("报告.pdf").to_string_lossy().into_owned(),
            ],
            "allowed file + dot-suffixed dir indexed; churn ancestor, extensionless \
             secret and source ext are not"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// A dot-suffixed directory's delete event must also run through
    /// (component-level exclusion must not swallow it): the directory record
    /// and its subtree are both cleared, instead of — like the old version —
    /// being blocked by the last-component extension judgment outside the
    /// delete and leaving stale rows for rescan.
    #[test]
    fn dot_suffixed_dir_delete_event_cleans_the_record() {
        let base = tmp("dotdir");
        fs::create_dir_all(base.join("backup.old")).unwrap();
        fs::write(base.join("backup.old/报告.pdf"), b"p").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Create(CreateKind::Folder),
                &[&base.join("backup.old")],
            ),
        );
        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&base.join("backup.old/报告.pdf")]),
        );
        assert_eq!(indexed(&store).len(), 2);

        fs::remove_dir_all(base.join("backup.old")).unwrap();
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Remove(RemoveKind::Folder),
                &[&base.join("backup.old")],
            ),
        );

        assert!(
            indexed(&store).is_empty(),
            "dot-suffixed dir rows must be removable by events, not wait for a rescan"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// The symlink body does not enter the store (consistent with the
    /// scanner's to_record rejection and follow_links(false)).
    #[test]
    #[cfg(unix)]
    fn symlink_events_do_not_index_the_link() {
        let base = tmp("symlink");
        fs::write(base.join("报告.pdf"), b"r").unwrap();
        std::os::unix::fs::symlink(base.join("报告.pdf"), base.join("link.pdf")).unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&base.join("link.pdf")]),
        );

        assert!(
            indexed(&store).is_empty(),
            "the symlink body must not enter the index"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// An entity replaced in place by a symlink (the common `rm f && ln -s`
    /// rhythm): the replacement event's stat succeeds but hits a
    /// non-indexable type; if the old record is still there it is a ghost →
    /// it must take the delete branch and be cleared. With no record (the
    /// shape of the symlink test above) this is a safe no-op; together the
    /// two tests pin both directions of "non-indexable type: clear the old
    /// row, never create a row".
    #[test]
    #[cfg(unix)]
    fn replacement_by_symlink_removes_the_stale_row() {
        let base = tmp("replsymlink");
        fs::write(base.join("报告.pdf"), b"r").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&base.join("报告.pdf")]),
        );
        assert_eq!(indexed(&store).len(), 1);

        // rm followed by an in-place ln -s: the entity at that path has been
        // swapped for a symlink to another name.
        fs::remove_file(base.join("报告.pdf")).unwrap();
        std::os::unix::fs::symlink(base.join("其他.pdf"), base.join("报告.pdf")).unwrap();
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
                &[&base.join("报告.pdf")],
            ),
        );

        assert!(
            indexed(&store).is_empty(),
            "a file replaced by a symlink must drop its stale row"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// Stat jitter (a parent directory's permission tightened) must not
    /// treat a still-existing record as deleted — `Path::exists()` folds
    /// every error into false, which is exactly how the old version wrongly
    /// deleted live records.
    #[test]
    #[cfg(unix)]
    fn transient_stat_errors_do_not_delete_records() {
        use notify::event::DataChange;
        use std::os::unix::fs::PermissionsExt as _;

        let base = tmp("staterr");
        fs::create_dir_all(base.join("sub")).unwrap();
        fs::write(base.join("sub/notes.md"), b"# n").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&base.join("sub/notes.md")]),
        );
        assert_eq!(store.stats().unwrap().total_files, 1);

        fs::set_permissions(base.join("sub"), fs::Permissions::from_mode(0o000)).unwrap();
        let readable = base.join("sub").read_dir().is_ok();
        fs::set_permissions(base.join("sub"), fs::Permissions::from_mode(0o755)).unwrap();
        if readable {
            // root (or a filesystem ignoring mode bits) bypasses the
            // permission check and no stat error can be manufactured; skip.
            let _ = fs::remove_dir_all(&base);
            return;
        }

        fs::set_permissions(base.join("sub"), fs::Permissions::from_mode(0o000)).unwrap();
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Modify(ModifyKind::Data(DataChange::Any)),
                &[&base.join("sub/notes.md")],
            ),
        );
        fs::set_permissions(base.join("sub"), fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(
            store.stats().unwrap().total_files,
            1,
            "an unstat-able path must not take the delete branch"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// With multiple nested roots, events are attributed to the **deepest**
    /// watched root (`max_by_key`): once the inner root is chosen, the outer
    /// root's basename (even when it hits the exclusion list) does not
    /// participate — the scanner indexes the inner root in full as usual
    /// (walkdir starts at the inner root itself), and a watcher wrongly
    /// choosing the outer root would drop the whole root's events.
    #[test]
    fn nested_root_events_use_the_deepest_root() {
        let base = tmp("nested");
        let outer = base.join("node_modules"); // the outer root's own basename hits the list
        let inner = outer.join("proj");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("报告.pdf"), b"r").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![outer.clone(), inner.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&inner.join("报告.pdf")]),
        );

        assert_eq!(
            indexed(&store),
            vec![inner.join("报告.pdf").to_string_lossy().into_owned()],
            "the deepest containing root decides; the outer root's excluded \
             basename must not drop the event"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// A root that does NOT contain the path must never grab attribution
    /// just by having more components: `excluded` filters roots by
    /// containment before the deepest-root `max_by_key`. Removing that
    /// filter survives every fixture where the fallback's full-path judgment
    /// coincides — this fixture makes them diverge (a non-containing root
    /// whose own basename is on the exclusion list), so a filter removal now
    /// flips this test red: the event would be judged against the deeper
    /// non-containing root, its excluded basename would trip the full-path
    /// fallback, and the whole event would be dropped.
    #[test]
    fn a_deeper_non_containing_root_never_grabs_attribution() {
        let base = tmp("grab");
        let root = base.join("proj");
        let decoy = root.join("deeper").join("build"); // basename ∈ SKIP_NAMES
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("报告.pdf"), b"r").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![root.clone(), decoy];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&root.join("报告.pdf")]),
        );

        assert_eq!(
            indexed(&store),
            vec![root.join("报告.pdf").to_string_lossy().into_owned()],
            "the containing root decides; a deeper non-containing root must \
             not pull the event into the exclusion judgment"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// Pre-registration preparation (pure function): existing roots are
    /// canonicalized, one entity (including both spellings via a symlink)
    /// keeps a single entry, missing roots are skipped, non-UTF-8 roots are
    /// rejected. The non-UTF-8 fixture cannot be created on APFS
    /// (probe-skip); the assertion really executes on the ext4 CI (ubuntu
    /// full leg) — the same probe convention as
    /// `non_utf8_spellings_never_enter_the_index`.
    #[test]
    fn prepare_roots_canonicalizes_dedups_and_skips_unusable_roots() {
        let base = tmp("prep");
        let root = base.join("proj");
        fs::create_dir_all(&root).unwrap();

        // mut is used only by the cfg(unix) fixture injection; the Windows
        // leg compiles with -D unused-mut (round-6 review B1: without the
        // guard the cfg block is compiled away entirely and the mut dangles
        // red on required-gate).
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut inputs = vec![root.clone(), base.join("missing"), root.clone()];
        // The symlink spelling and the entity path canonicalize to the same
        // path → deduped to one.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&root, base.join("link")).unwrap();
            inputs.insert(1, base.join("link"));
        }
        // Non-UTF-8 root: assert rejection only when it can be created.
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt as _;
            let bad = base.join(OsStr::from_bytes(b"\xff.bad"));
            if fs::create_dir_all(&bad).is_ok() {
                inputs.insert(0, bad);
            }
        }

        let out = prepare_roots(&inputs);
        assert_eq!(
            out,
            vec![root.canonicalize().unwrap()],
            "one canonical entry per usable entity; missing and non-UTF-8 roots are dropped"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// Out-of-root paths take the defensive full-path judgment (normally
    /// unreachable after canonicalization): those hitting excluded
    /// components are dropped as before, clean paths enter the store by
    /// existence as usual — they must not be silently dropped just for being
    /// "under no root".
    #[test]
    fn out_of_root_paths_fall_back_to_full_path_judgment() {
        let base = tmp("fallback");
        fs::create_dir_all(base.join("root")).unwrap();
        let outside =
            std::env::temp_dir().join(format!("pinvou3_kb_watch_out_{}", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("报告.pdf"), b"r").unwrap();
        // The excluded-component fixture must pass the type whitelist (use
        // 报告.pdf rather than id_rsa), otherwise is_skipped would intercept
        // it again after the component gate and mask the component gate's own
        // regression.
        fs::create_dir_all(outside.join(".ssh")).unwrap();
        fs::write(outside.join(".ssh/报告.pdf"), b"r").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.join("root")];

        // Clean out-of-root path: full-path judgment passes → stat alive →
        // enters the store.
        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&outside.join("报告.pdf")]),
        );
        // Out-of-root path hitting an excluded component: full-path judgment
        // drops it (caught at the component level, no stat needed).
        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&outside.join(".ssh").join("报告.pdf")]),
        );

        assert_eq!(
            indexed(&store),
            vec![outside.join("报告.pdf").to_string_lossy().into_owned()],
            "out-of-root clean paths must survive the defensive fallback; \
             excluded components must still drop"
        );

        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&outside);
    }

    /// Non-UTF-8 spellings enter the store on neither side
    /// (`record_from_parts`'s `to_str` is the shared final gate for scanner
    /// and watcher; the scanner additionally prunes the whole subtree at
    /// filter_entry per its empty-name semantics) — the stored outcome
    /// agrees. This pins the "clean leaves also stay out" semantics,
    /// preventing a future change of record assembly to lossy spellings from
    /// splitting the two sides.
    #[test]
    #[cfg(unix)]
    fn non_utf8_spellings_never_enter_the_index() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let base = tmp("nonutf8");
        let bad_dir = base.join(OsStr::from_bytes(b"\xff\xfe.dir"));
        if fs::create_dir_all(&bad_dir).is_err() {
            // A hostile filesystem (APFS rejects non-UTF-8 names) cannot
            // create the fixture; skip.
            let _ = fs::remove_dir_all(&base);
            return;
        }
        let leaf = bad_dir.join("报告.pdf");
        if fs::write(&leaf, b"r").is_err() {
            let _ = fs::remove_dir_all(&base);
            return;
        }
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(&ex, &store, &roots, &ev(creates_file(), &[&leaf]));
        apply(
            &ex,
            &store,
            &roots,
            &ev(EventKind::Create(CreateKind::Folder), &[&bad_dir]),
        );
        // Delete event: the lossy spelling matches no row — safe no-op, no
        // wrong delete, no error.
        fs::remove_dir_all(&bad_dir).unwrap();
        apply(
            &ex,
            &store,
            &roots,
            &ev(EventKind::Remove(RemoveKind::Folder), &[&bad_dir]),
        );

        assert!(
            indexed(&store).is_empty(),
            "non-UTF-8 spellings must stay out of the index on both sides"
        );

        let _ = fs::remove_dir_all(&base);
    }

    fn creates_file() -> EventKind {
        EventKind::Create(CreateKind::File)
    }
}
