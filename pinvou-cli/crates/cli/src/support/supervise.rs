//! Ctrl-C supervision for the vendor CLI children spawned in their own
//! process groups.
//!
//! [`crate::support::set_process_group`] puts every long-running vendor child
//! in a dedicated group so a *timeout* can kill its npm/shell descendants
//! with it ([`crate::support::kill_process_tree`]). The same isolation is
//! what orphans those children when the *terminal* is interrupted: nothing
//! installs a SIGINT handler, so the default disposition terminates the CLI
//! alone, and the group children — kimi logins that can run to 1800s, codex,
//! a wedged `connectors connect` — keep running with no parent left to reap
//! or kill them.
//!
//! [`install_signal_cleanup`] is the missing half, installed once as the
//! first statement of `main`. It wires a self-pipe: the SIGINT/SIGTERM
//! handler does the one thing it may do — a `write(2)` of the signal number,
//! async-signal-safe — while a watcher thread owns every non-signal-safe
//! step: it raises the flag [`sigint_seen`] reports, forwards SIGTERM to
//! every registered group, waits a bounded grace, escalates to SIGKILL for
//! survivors, then restores the default disposition and re-raises the
//! original signal so the process exits with the conventional 128+N status.
//!
//! Spawn sites supervise a child by bracketing its lifetime through the
//! registry — nothing in this module is per-call-site, so later waves wire
//! spawn sites purely through this public surface:
//!
//! ```text
//! let mut command = …;
//! support::set_process_group(&mut command);        // existing call, unchanged
//! let child = command.spawn()?;                    // pgid == child pid
//! support::supervise::register_child_group(child.id());
//! …                                                // bounded wait; on timeout
//!                                                  // support::kill_process_tree
//! support::supervise::forget_child_group(child.id());
//! ```
//!
//! The CLI targets macOS/Linux, so the implementation is UNIX-only; every
//! entry point collapses to a no-op elsewhere, the same split
//! [`crate::support::set_process_group`] uses.

/// Installs the SIGINT/SIGTERM cleanup wiring. Idempotent. Must run before
/// any supervised child exists, which is why `main` calls it as its first
/// statement; nothing else may need to call it.
pub fn install_signal_cleanup() {
    #[cfg(unix)]
    {
        imp::install();
    }
    #[cfg(not(unix))]
    {
        // Not a CLI target; the unsupervised status quo is unchanged there.
    }
}

/// Registers a live child process group so an interrupt later forwards to it.
/// Call right after a successful `spawn` of a `set_process_group` child — the
/// pgid is the child's pid — and pair every exit from the lifetime (normal
/// completion, timeout kill) with [`forget_child_group`].
pub fn register_child_group(pgid: u32) {
    #[cfg(unix)]
    {
        imp::register(pgid);
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
    }
}

/// Removes a process group from supervision. Call when the site has waited
/// for the child (or killed it), so a later interrupt does not signal a pgid
/// the OS may already have recycled for an unrelated process.
pub fn forget_child_group(pgid: u32) {
    #[cfg(unix)]
    {
        imp::forget(pgid);
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
    }
}

/// True once an interrupt-family signal (SIGINT or SIGTERM) has been
/// observed by the watcher. Spawn sites and timeout code can poll it to
/// unwind early instead of soldiering on in a process that is about to
/// terminate.
pub fn sigint_seen() -> bool {
    #[cfg(unix)]
    {
        imp::seen()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// UNIX implementation. The unit tests drive everything except
/// `install`/`watcher` (real signals and pipes in the test process would be
/// a flake farm, not coverage).
#[cfg(unix)]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    use std::sync::{Mutex, Once, OnceLock};
    use std::time::Instant;

    /// Bounded grace between SIGTERM and SIGKILL: a vendor child that traps
    /// SIGTERM to flush state (a login writing its tokens) gets this long.
    /// `pub(super)` so the grace unit tests pin the same constant the
    /// watcher's loop runs with.
    pub(super) const GRACE_MS: u64 = 5_000;
    /// Poll cadence of the grace loop; also the longest the loop ever sleeps.
    const POLL_MS: libc::c_int = 100;

    /// Flag behind [`super::sigint_seen`]; set by the watcher, read by spawn
    /// sites, the unit tests (through `note_seen`) included.
    static SIGNAL_SEEN: AtomicBool = AtomicBool::new(false);

    /// Write end of the self-pipe as seen inside the handler, `-1` until
    /// installed. The handler must not touch locks, so this is the only
    /// handler-visible state — and it is written once, before any signal can
    /// be handled, then only read.
    static PIPE_WRITE: AtomicI32 = AtomicI32::new(-1);

    /// Registered child groups in registration order. `Mutex<Vec>` rather
    /// than a lock-free structure: the registry is cold (two calls per child
    /// lifetime) and tiny, and the codebase's caches use the same
    /// `OnceLock<Mutex<…>>` shape (voice.rs, models.rs).
    static CHILD_GROUPS: OnceLock<Mutex<Vec<libc::pid_t>>> = OnceLock::new();

    static INSTALL: Once = Once::new();

    /// Bounded grace in a test-free zone: `Once` guards double installs from
    /// repeated `main` calls in tests; every install failure degrades to the
    /// unsupervised status quo instead of panicking inside `main`'s first
    /// statement.
    pub(super) fn install() {
        INSTALL.call_once(real_install);
    }

    fn real_install() {
        // Declared outside the `unsafe` block: the watcher spawn below and
        // the `PIPE_WRITE.store` are ordinary code, and binding inside the
        // block would end `read_end`'s scope at its closing brace.
        let read_end;
        // SAFETY: raw-fd and signal-syscall wrappers below. Every failure
        // path leaves the process exactly as it was before this module: no
        // handler installed means default SIGINT termination (the old
        // behavior), not a broken one.
        unsafe {
            let mut fds = [0 as libc::c_int; 2];
            if libc::pipe(fds.as_mut_ptr()) != 0 {
                return;
            }
            // CLOEXEC on both ends: vendor children `exec` through
            // `std::process`, and inherited ends would let a child hold this
            // CLI's watcher open (and would hand the write end to every
            // child, where a stray close is harmless but a stray write is
            // not — a child writing the byte could raise this CLI's flag).
            for fd in fds {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags >= 0 {
                    libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
                }
            }
            read_end = fds[0];
            PIPE_WRITE.store(fds[1], Ordering::SeqCst);
            // BSD-semantic `signal(2)`: SA_RESTART for the restarted calls.
            // The handler itself never depends on that — see `on_signal`.
            // The double cast (fn item → raw pointer → sighandler_t) is the
            // lint-approved spelling of "handler address"; a direct
            // fn-to-integer cast is the shape the lint exists for.
            libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
            libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        }
        // The watcher owns every non-signal-safe step; a failed spawn is the
        // same degrade as a failed pipe — the handler's bytes just sit
        // unread, and no cleanup happens.
        let _ = std::thread::Builder::new()
            .name("pinvou-child-supervisor".to_owned())
            .spawn(move || watcher(read_end));
    }

    /// The handler. Async-signal-safe by construction: one `write(2)` of the
    /// signal number — a 1-byte pipe write is atomic under PIPE_BUF — and
    /// nothing else: no locks, no allocation, and errno saved around the
    /// write ([`write_signal_byte`]) so a failed one cannot leave its error
    /// code with the thread the handler interrupted.
    extern "C" fn on_signal(sig: libc::c_int) {
        let fd = PIPE_WRITE.load(Ordering::Relaxed);
        if fd < 0 {
            return;
        }
        let byte = sig as u8;
        write_signal_byte(fd, byte);
    }

    /// The handler's one syscall, split out so the unit tests can pin its
    /// errno hygiene without real signals: a failing `write(2)` inside a
    /// handler sets errno, and the interrupted thread classifies its EINTR
    /// off errno (`read_terminated`) — a foreign value left behind would
    /// misread the interrupted call's cause. Saved before, restored after:
    /// the standard self-pipe discipline. `pub(super)` for the tests, like
    /// the other pinned internals.
    pub(super) fn write_signal_byte(fd: libc::c_int, byte: u8) {
        // SAFETY: write(2) is reentrant; the buffer outlives the call. The
        // errno location is dereferenced only on this thread, around the
        // write itself.
        unsafe {
            let errno = errno_location();
            let saved = *errno;
            libc::write(fd, &byte as *const u8 as *const libc::c_void, 1);
            *errno = saved;
        }
    }

    /// Thread-local errno location for [`write_signal_byte`]. macOS spells
    /// the libc helper `__error`; Linux — the other CLI target — spells it
    /// `__errno_location`, so the difference stays behind this seam instead
    /// of inside the handler.
    #[cfg(target_os = "macos")]
    pub(super) fn errno_location() -> *mut libc::c_int {
        // SAFETY: the location call has no preconditions and no effect
        // beyond returning the thread-local address.
        unsafe { libc::__error() }
    }

    /// Non-macos spelling of [`errno_location`].
    #[cfg(not(target_os = "macos"))]
    pub(super) fn errno_location() -> *mut libc::c_int {
        // SAFETY: as above.
        unsafe { libc::__errno_location() }
    }

    /// The watcher thread: converts the handler's byte into the slow,
    /// signal-unsafe cleanup sequence, then the conventional exit.
    fn watcher(read_fd: libc::c_int) {
        // The first byte is the signal that started cleanup; it is also the
        // signal re-raised at the end, so the 128+N status names the right
        // one when both arrive.
        let Some(first) = read_terminated(read_fd) else {
            return;
        };
        note_seen();

        // Phase 1 — ask every registered group to stop.
        for pgid in snapshot() {
            // SAFETY: kill(2) to a group we registered; ESRCH (already gone)
            // is fine to ignore.
            unsafe {
                libc::kill(-pgid, libc::SIGTERM);
            }
        }

        // Phase 2 — bounded grace. The policy is `grace_decision`, a pure
        // function the unit tests pin; this loop is only its effect.
        let started = Instant::now();
        loop {
            let survivors: Vec<libc::pid_t> = snapshot()
                .into_iter()
                .filter(|pgid| group_alive(*pgid))
                .collect();
            match grace_decision(
                started.elapsed().as_millis() as u64,
                GRACE_MS,
                survivors.len(),
                second_signal_waiting(read_fd),
            ) {
                GraceAction::Finish => break,
                GraceAction::Continue => continue,
                GraceAction::Escalate => {
                    for pgid in survivors {
                        // SAFETY: as above; SIGKILL to a survivor that just
                        // ignored SIGTERM (or outlived the user's patience).
                        unsafe {
                            libc::kill(-pgid, libc::SIGKILL);
                        }
                    }
                    break;
                }
            }
        }

        // Phase 3 — conventional exit, 128+N: restore the default disposition
        // for the original signal and re-raise it, so parent scripts observe
        // the death they expect from an interrupted process (`wait` macros,
        // `trap` handlers, `128+$?` arithmetic).
        // SAFETY: raise(2) re-delivers to this process; with SIG_DFL
        // installed it does not return.
        unsafe {
            libc::signal(first as libc::c_int, libc::SIG_DFL);
            libc::raise(first as libc::c_int);
        }
    }

    /// Blocking read of the cleanup-start byte; `None` on a dead pipe (never
    /// installed, or the write end closed — cleanup has nothing to drive it).
    fn read_terminated(read_fd: libc::c_int) -> Option<u8> {
        let mut buf = [0u8; 1];
        loop {
            // SAFETY: read(2) into a 1-byte buffer that outlives the call.
            let count = unsafe { libc::read(read_fd, buf.as_mut_ptr() as *mut libc::c_void, 1) };
            if count == 1 {
                return Some(buf[0]);
            }
            // EINTR: re-read — poll-style calls are not restarted even under
            // SA_RESTART. Anything else (EOF, real error): no supervisor left.
            if count < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return None;
        }
    }

    /// True if another signal byte is already queued. Sleeps up to POLL_MS in
    /// poll(2) — the grace loop's pace — and reports readiness only on
    /// POLLIN, so an errored or spurious poll cannot fake a second Ctrl-C.
    fn second_signal_waiting(read_fd: libc::c_int) -> bool {
        let mut fds = [libc::pollfd {
            fd: read_fd,
            events: libc::POLLIN,
            revents: 0,
        }];
        // SAFETY: fixed-size array, fixed nfds.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, POLL_MS) };
        ready > 0 && (fds[0].revents & libc::POLLIN) != 0
    }

    /// Probes a group with signal 0 (kill(2) existence check): ESRCH means
    /// every member exited or was reaped, which ends the wait early.
    fn group_alive(pgid: libc::pid_t) -> bool {
        let Some(signed) = signalable_group(pgid as u32) else {
            return false;
        };
        // SAFETY: probe with signal 0; sends nothing.
        unsafe { libc::kill(-signed, 0) == 0 }
    }

    /// What the grace loop does next. Split from the effect so the unit
    /// tests pin the policy, not the timing.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum GraceAction {
        /// Survivors remain and grace remains: keep waiting at POLL_MS pace.
        Continue,
        /// No group survived SIGTERM: the wait is over, nothing to kill.
        Finish,
        /// Grace exhausted, or a second signal demanded it: SIGKILL time.
        Escalate,
    }

    /// Pure policy behind the grace loop. Precedence, pinned by the unit
    /// tests: no survivors first (nothing left to wait for); then a second
    /// signal (the user's "stop waiting"); then the deadline; otherwise wait.
    pub(super) fn grace_decision(
        elapsed_ms: u64,
        grace_ms: u64,
        survivors: usize,
        escalate_now: bool,
    ) -> GraceAction {
        if survivors == 0 {
            GraceAction::Finish
        } else if escalate_now || elapsed_ms >= grace_ms {
            GraceAction::Escalate
        } else {
            GraceAction::Continue
        }
    }

    /// kill(2) group-name guard. pid 0 names the caller's own group, pid 1
    /// belongs to init, and -1 names *every* process the user may signal,
    /// while a pgid above `i32::MAX` wraps negative under a plain `as`, so
    /// negating it targets an unrelated process. Same guards, same reasons,
    /// as [`crate::support::kill_process_tree`]; kept at every kill(2) site
    /// so a bad registry value cannot reach the syscall.
    pub(super) fn signalable_group(pgid: u32) -> Option<libc::pid_t> {
        i32::try_from(pgid).ok().filter(|signed| *signed > 1)
    }

    /// Flag access for [`super::sigint_seen`].
    pub(super) fn seen() -> bool {
        SIGNAL_SEEN.load(Ordering::SeqCst)
    }

    /// Internal setter: the watcher's first step, and the path the unit
    /// tests use to toggle the flag without raising real signals.
    pub(super) fn note_seen() {
        SIGNAL_SEEN.store(true, Ordering::SeqCst);
    }

    /// Test-only: leaves the flag down so later tests in this binary start
    /// from the documented false. `pub(super)` for the sibling tests module
    /// at file level, like every other imp item it drives.
    #[cfg(test)]
    pub(super) fn reset_seen_for_tests() {
        SIGNAL_SEEN.store(false, Ordering::SeqCst);
    }

    /// Registry slot; lock poisoning cannot cancel an interrupt cleanup, so
    /// a poisoned lock is recovered, the same convention the contract tests
    /// use for ENV_LOCK.
    fn child_groups() -> &'static Mutex<Vec<libc::pid_t>> {
        CHILD_GROUPS.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// Registers a group; double registration (retry loops at a site) must
    /// not double-signal the group later, hence the contains-check.
    pub(super) fn register(pgid: u32) {
        let Some(signed) = signalable_group(pgid) else {
            return;
        };
        let mut groups = child_groups().lock().unwrap_or_else(|p| p.into_inner());
        if !groups.contains(&signed) {
            groups.push(signed);
        }
    }

    /// Drops a group from supervision, wherever it sits in the list.
    pub(super) fn forget(pgid: u32) {
        let Some(signed) = signalable_group(pgid) else {
            return;
        };
        child_groups()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|group| *group != signed);
    }

    /// Registration-order snapshot for the watcher's forward phase and the
    /// registry unit tests' lookups.
    pub(super) fn snapshot() -> Vec<libc::pid_t> {
        child_groups()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::imp;

    /// Decision precedence: no survivors ends the wait no matter how much
    /// grace is left — even at t=0, even with a second signal pending.
    #[test]
    fn grace_decision_finishes_as_soon_as_no_group_survives() {
        assert_eq!(
            imp::grace_decision(0, imp::GRACE_MS, 0, false),
            imp::GraceAction::Finish
        );
        assert_eq!(
            imp::grace_decision(4_999, imp::GRACE_MS, 0, true),
            imp::GraceAction::Finish
        );
        // A test double-check that Finish is reported even past the deadline.
        assert_eq!(
            imp::grace_decision(60_000, imp::GRACE_MS, 0, false),
            imp::GraceAction::Finish
        );
    }

    /// A second signal means "stop waiting": escalation outranks both the
    /// deadline and the remaining grace.
    #[test]
    fn grace_decision_escalates_immediately_on_a_second_signal() {
        assert_eq!(
            imp::grace_decision(10, imp::GRACE_MS, 3, true),
            imp::GraceAction::Escalate
        );
    }

    /// The deadline: at or past the grace with survivors, escalate; one tick
    /// before it, keep waiting.
    #[test]
    fn grace_decision_escalates_at_the_deadline_and_waits_before_it() {
        assert_eq!(
            imp::grace_decision(4_999, imp::GRACE_MS, 2, false),
            imp::GraceAction::Continue
        );
        assert_eq!(
            imp::grace_decision(imp::GRACE_MS, imp::GRACE_MS, 2, false),
            imp::GraceAction::Escalate
        );
        assert_eq!(
            imp::grace_decision(60_000, imp::GRACE_MS, 2, false),
            imp::GraceAction::Escalate
        );
        // Zero grace must not spin: with survivors it escalates at t=0.
        assert_eq!(
            imp::grace_decision(0, 0, 2, false),
            imp::GraceAction::Escalate
        );
    }

    /// The kill(2) guard: pgid 0 is the CLI's own group and would suicide the
    /// supervisor's forward phase; 1 belongs to init; `u32::MAX` wraps
    /// negative under `as i32` and would target an unrelated process. Only
    /// ordinary child pgids pass.
    #[test]
    fn signalable_group_refuses_the_kill2_special_arguments() {
        assert_eq!(imp::signalable_group(0), None);
        assert_eq!(imp::signalable_group(1), None);
        assert_eq!(imp::signalable_group(u32::MAX), None);
        assert_eq!(imp::signalable_group(2), Some(2));
        assert_eq!(imp::signalable_group(42), Some(42));
        assert_eq!(imp::signalable_group(i32::MAX as u32), Some(i32::MAX));
    }

    /// Registry add/remove/lookup: registration order is preserved, double
    /// registration does not duplicate, and `forget` removes exactly the
    /// dropped group. The fake pgids (2_000_000+) are below `i32::MAX`, above
    /// every plausible real pid, and used by no other test, so parallel
    /// tests cannot mix their registrations in.
    #[test]
    fn registry_adds_looks_up_and_forgets_groups() {
        const A: u32 = 2_000_001;
        const B: u32 = 2_000_002;
        // Start and end from a clean slate: forget first (a previous run of
        // this very test may have left them behind via an early return).
        imp::forget(A);
        imp::forget(B);

        // Guarded values must not register at all.
        imp::register(0);
        imp::register(1);
        imp::register(u32::MAX);
        let registered = imp::snapshot();
        assert!(
            !registered.contains(&0) && !registered.contains(&1),
            "the kill(2) special arguments must never sit in the registry"
        );

        imp::register(A);
        imp::register(A); // duplicate is dropped, not stored twice
        imp::register(B);
        let registered = imp::snapshot();
        let a = registered.iter().filter(|pgid| **pgid == A as i32).count();
        let b = registered.iter().filter(|pgid| **pgid == B as i32).count();
        assert_eq!(
            a, 1,
            "double registration must not duplicate: {registered:?}"
        );
        assert_eq!(b, 1, "the second group must be present: {registered:?}");
        assert!(
            registered.iter().position(|pgid| *pgid == A as i32)
                < registered.iter().position(|pgid| *pgid == B as i32),
            "registration order must be preserved for the forward phase: {registered:?}"
        );

        imp::forget(A);
        let registered = imp::snapshot();
        assert!(
            !registered.contains(&(A as i32)),
            "a forgotten group must not be signaled later: {registered:?}"
        );
        assert!(
            registered.contains(&(B as i32)),
            "forgetting one group must not drop the others: {registered:?}"
        );

        imp::forget(B);
        assert!(
            !imp::snapshot().contains(&(B as i32)),
            "the last group must leave the registry too"
        );
    }

    /// `sigint_seen` toggling through the internal setter path — the same
    /// `note_seen` the watcher runs as its first step, not some test-only
    /// shortcut with different semantics.
    #[test]
    fn sigint_seen_toggles_through_the_internal_setter_path() {
        assert!(!imp::seen(), "the documented start state is false");
        assert!(!super::sigint_seen());
        imp::note_seen();
        assert!(
            imp::seen(),
            "note_seen (the watcher's path) raises the flag"
        );
        assert!(super::sigint_seen());
        // Leave the flag down for whatever runs later in this binary.
        imp::reset_seen_for_tests();
        assert!(!imp::seen());
    }

    /// The handler's errno hygiene, pinned without real signals: a failing
    /// self-pipe write (fd -1 is always EBADF) must leave the interrupted
    /// thread's errno exactly as it was — `read_terminated` classifies EINTR
    /// off errno, so a leaked handler error would misread the interrupted
    /// call's cause.
    #[test]
    fn handler_write_preserves_errno_of_the_interrupted_thread() {
        let errno = imp::errno_location();
        // SAFETY: thread-local errno; the sentinel (EAGAIN) only has to be
        // a value the failed write below does not decide, and the assert is
        // about the restore, not the sentinel.
        unsafe { *errno = libc::EAGAIN };
        imp::write_signal_byte(-1, libc::SIGINT as u8);
        // SAFETY: the same thread-local location as above.
        assert_eq!(
            unsafe { *errno },
            libc::EAGAIN,
            "a failed handler write must not leak its errno"
        );
    }
}
