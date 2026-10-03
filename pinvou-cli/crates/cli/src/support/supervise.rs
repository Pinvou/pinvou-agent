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
//! first statement of `main`. It wires a self-pipe: the SIGINT/SIGTERM/SIGHUP
//! handler does the one thing it may do — a `write(2)` of the signal number,
//! async-signal-safe — while a watcher thread owns every non-signal-safe
//! step: it raises the flag [`sigint_seen`] reports, forwards SIGTERM to
//! every registered group, waits a bounded grace, escalates to SIGKILL for
//! survivors, then restores the default disposition and re-raises the
//! original signal so the process exits with the conventional 128+N status.
//! SIGHUP is in the set because closing the terminal delivers it to the
//! foreground group too — the most common real interruption of a long
//! login — and leaving it at its default disposition would orphan the group
//! children all the same.
//!
//! Spawn sites supervise a child by bracketing its lifetime through the
//! registry — nothing in this module is per-call-site, so later waves wire
//! spawn sites purely through this public surface:
//!
//! ```text
//! let mut command = …;
//! support::set_process_group(&mut command);        // existing call, unchanged
//! let child = support::supervise::spawn_supervised(&mut command)?;
//!                                                  // spawn + register happen
//!                                                  // with the interrupt
//!                                                  // signals blocked, so no
//!                                                  // interrupt can slip the
//!                                                  // window; bounded wait, on
//!                                                  // timeout
//!                                                  // support::kill_process_tree
//! support::supervise::forget_child_group(child.id());
//! ```
//!
//! The CLI targets macOS/Linux, so the implementation is UNIX-only; every
//! entry point collapses to a no-op elsewhere, the same split
//! [`crate::support::set_process_group`] uses.

/// Installs the SIGINT/SIGTERM/SIGHUP cleanup wiring. Idempotent. Must run
/// before any supervised child exists, which is why `main` calls it as its
/// first statement; nothing else may need to call it.
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

/// Spawns `command` and registers the child's process group in one step,
/// with the interrupt-family signals blocked on this thread for the
/// spawn→register window. A raw `spawn()` followed by
/// [`register_child_group`] leaves a window in which an interrupt forwards
/// to nothing and re-raises with the fresh group child absent from the
/// registry — the exact orphan this module exists to prevent. With the
/// signals blocked, a pending interrupt stays pending until the unblock
/// below and is delivered once, with the child already registered.
///
/// THREAD REQUIREMENT: this is the only form a non-main thread may use. The
/// spawn→register window below is excluded from the interrupt snapshot via
/// [`imp::spawn_window`], so a signal arriving while any thread is inside the
/// window is answered by a watcher snapshot that either already contains the
/// fresh group or waits for the window to close first — the orphan window a
/// raw `spawn()` + [`register_child_group`] pair would leave on a worker
/// thread (where the block/unblock here is per-thread and defers nothing) is
/// closed by that mutual exclusion on EVERY thread. On the main thread the
/// sigmask block additionally keeps the pending signal parked until the
/// unblock, so it is delivered once, with the child already registered.
///
/// The spawn→register window blocks the interrupt family on the spawning
/// thread, and the child would inherit that mask across fork. Std does NOT
/// guarantee an empty child mask at exec: on macOS (verified by probe,
/// rustc 1.98) even the posix-spawn path keeps the blocked INT/TERM/HUP
/// mask alive in the child, so a forwarded SIGTERM would pend forever and
/// every Ctrl-C cleanup would ride the 5 s SIGKILL escalation — exactly the
/// vendor-CLI-flushes-state case the grace window exists for. The child's
/// mask is therefore reset EXPLICITLY, via a `pre_exec` closure running
/// between fork and exec.
///
/// Every supervised spawn site goes through this instead of a bare `spawn`.
pub fn spawn_supervised(
    command: &mut std::process::Command,
) -> std::io::Result<std::process::Child> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: the closure runs in the forked child before exec; it only
        // calls sigprocmask with a zeroed-and-emptied set (no allocation, no
        // locks held) and propagates the raw errno via the io::Result
        // contract `pre_exec` requires.
        unsafe {
            command.pre_exec(|| {
                let mut empty: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut empty);
                if libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut()) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let saved = imp::block_interrupt_signals();
        // The window guard makes the spawn→register pair atomic against the
        // watcher's snapshots: on a worker thread the sigmask block above
        // defers nothing (an interrupt is delivered to main's handler while
        // this spawn is in flight), so the exclusion is what keeps the fresh
        // group inside the cleanup's phase-1 forward instead of orphaned.
        // The pre_exec closure still holds no lock IN THE CHILD: it runs
        // between fork and exec and only calls sigprocmask.
        let _window = imp::spawn_window();
        let spawned = command.spawn();
        if let Ok(child) = &spawned {
            imp::register(child.id());
        }
        drop(_window);
        imp::restore_interrupt_signals(saved);
        spawned
    }
    #[cfg(not(unix))]
    {
        command.spawn()
    }
}

/// Registers a live child process group so an interrupt later forwards to it.
/// Call right after a successful `spawn` of a `set_process_group` child — the
/// pgid is the child's pid — and pair every exit from the lifetime (normal
/// completion, timeout kill) with [`forget_child_group`]. Main-thread only:
/// the spawn→register exclusion lives inside [`spawn_supervised`], so a
/// split spawn + register pair is safe only where the sigmask window defers
/// the interrupt (see `spawn_supervised`'s thread requirement).
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

/// Main's exit path calls this right before `std::process::exit`: once an
/// interrupt cleanup has STARTED, the conventional 128+N exit belongs to the
/// watcher's phase-3 re-raise, not to main's own exit code. Without this
/// park, a vendor child that dies promptly on the forwarded SIGTERM lets
/// main render and exit sub-millisecond — the watcher's re-raise usually
/// loses that race and scripts observe the family's exit 1 instead of
/// 128+N. Once cleanup has started the watcher is GUARANTEED to terminate
/// the process (the grace loop is bounded and phase 3 reinstalls SIG_DFL
/// and kills), so parking here cannot hang. No cleanup started: returns
/// immediately, the normal path.
pub fn park_while_interrupt_cleanup_concludes() {
    #[cfg(unix)]
    imp::park_while_cleanup_concludes();
    #[cfg(not(unix))]
    {
        // No watcher on this platform; nothing to conclude.
    }
}

/// Removes a process group from supervision. Call when the site has waited
/// for the child (or killed it), so a later interrupt does not signal a pgid
/// the OS may already have recycled for an unrelated process.
///
/// DISCIPLINE: registration is automatic inside [`spawn_supervised`], but
/// the release half is per-site — a missed `forget` leaves a stale pgid the
/// watcher SIGTERMs without a liveness probe on the next Ctrl-C. Prefer the
/// RAII [`GroupGuard`] over a new manual pair: the manual pairs answer the
/// ordinary return paths only, and a panic between spawn and the paired
/// forget used to leave the group registered.
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

/// RAII handle over one registered child group: releases the registration on
/// drop, so every exit — ordinary return AND unwind — is paired, and a panic
/// between spawn and the explicit forget cannot leave the pgid registered
/// for the OS to recycle onto an unrelated process. Sites that must release
/// EARLY (before a drain grace, so an interrupt inside the grace window
/// cannot forward-signal a recycled pgid) call [`GroupGuard::release`]
/// explicitly; every other exit forgets at scope end, after whatever kill
/// the arm performed.
pub(crate) struct GroupGuard {
    pgid: u32,
    armed: bool,
}

impl GroupGuard {
    /// Wrap a pgid that `spawn_supervised` just registered.
    pub(crate) fn arm(pgid: u32) -> Self {
        Self { pgid, armed: true }
    }

    /// Immediate paired release — the reaped-or-dead case where the forget
    /// must happen NOW rather than at scope end.
    pub(crate) fn release(mut self) {
        self.armed = false;
        forget_child_group(self.pgid);
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if self.armed {
            forget_child_group(self.pgid);
        }
    }
}

/// True once an interrupt-family signal (SIGINT, SIGTERM or SIGHUP) has been
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

    /// Excludes a spawn→register window (held inside `spawn_supervised`)
    /// from the watcher's registry snapshots: a fresh group is either in the
    /// registry before a snapshot runs, or the snapshot waits for the window
    /// to close first. Contention is bounded by one fork+exec, never by a
    /// child's runtime.
    static SPAWN_WINDOW: Mutex<()> = Mutex::new(());

    static INSTALL: Once = Once::new();
    /// Set by the watcher the moment cleanup starts. Main's exit path parks
    /// forever once this is set: the watcher's phase 3 always terminates the
    /// process (bounded by the grace + escalation), so the park cannot hang —
    /// it only stops main from winning the race to `std::process::exit` and
    /// robbing the 128+N re-raise of its conventional exit status.
    static CLEANUP_STARTED: AtomicBool = AtomicBool::new(false);

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
        }
        // The watcher is spawned BEFORE the handlers go live. In the other
        // order a failed spawn would leave the handlers installed with no
        // reader: every later interrupt would write one byte into an unread
        // pipe and return — SIGINT/SIGTERM/SIGHUP permanently trapped,
        // registered children never signaled, the CLI stoppable only by
        // SIGQUIT/SIGKILL. That is strictly worse than the uninstalled
        // status quo, so a failed spawn here restores the pre-install state
        // (an inert pipe, default dispositions) instead.
        let watcher = std::thread::Builder::new()
            .name("pinvou-child-supervisor".to_owned())
            .spawn(move || watcher(read_end));
        if watcher.is_err() {
            // The failed spawn also never ran the closure that captured
            // `read_end` (a Copy fd: the binding stays valid). Close both
            // ends — resetting PIPE_WRITE alone leaks the descriptors for
            // the process lifetime — and return to the pre-install state.
            let write_end = PIPE_WRITE.swap(-1, Ordering::SeqCst);
            // SAFETY: both are the pipe(2) fds this function created; the
            // watcher thread owns neither (it never started).
            unsafe {
                libc::close(read_end);
                if write_end >= 0 {
                    libc::close(write_end);
                }
            }
            return;
        }
        // BSD-semantic `signal(2)`: SA_RESTART for the restarted calls.
        // The handler itself never depends on that — see `on_signal`.
        // The double cast (fn item → raw pointer → sighandler_t) is the
        // lint-approved spelling of "handler address"; a direct
        // fn-to-integer cast is the shape the lint exists for.
        // SIGINT/SIGTERM install unconditionally (the conventional choice:
        // an inherited SIG_IGN for them is rare and interactive shells do
        // not set it). SIGHUP is the one that must RESPECT an inherited
        // SIG_IGN: `nohup pinvou connectors connect …` in the foreground
        // runs with SIGHUP ignored by the caller's explicit choice, and
        // installing the handler here made terminal-close kill a job the
        // user deliberately detached (a round-27 review regression against
        // the pre-CLI status quo).
        // SAFETY: signal(2) with a plain handler address; no preconditions.
        // The disposition probe (null sigaction) reads without installing.
        unsafe {
            libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
            libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
            if inherited_sighup_is_ignored() {
                // Leave SIGHUP ignored, exactly as the caller arranged it.
            } else {
                libc::signal(libc::SIGHUP, on_signal as *const () as libc::sighandler_t);
            }
        }
    }

    /// Whether SIGHUP arrived at this process with SIG_IGN installed
    /// (`nohup`, a daemonizing shell, `setsid`). Read via `sigaction` with a
    /// null act — a pure probe, no disposition is changed. Split out so a
    /// unit test can pin both answers without real terminal state.
    pub(super) fn inherited_sighup_is_ignored() -> bool {
        // SAFETY: sigaction with a null act only WRITES the old disposition
        // into the out-param; no handler state is modified.
        unsafe {
            let mut old: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(libc::SIGHUP, std::ptr::null(), &mut old) != 0 {
                // The probe itself failed: install the handler (the
                // conventional behaviour) rather than guess.
                return false;
            }
            // SIG_IGN is 1 on every supported platform; compare the stored
            // handler address.
            old.sa_sigaction == libc::SIG_IGN as usize
        }
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
        // This thread blocks the interrupt family for its whole life: with
        // the signals blocked on the spawner inside [`super::
        // spawn_supervised`]'s window too, a signal arriving during a
        // spawn→register gap stays pending process-wide instead of being
        // delivered HERE (where it would run the cleanup with the fresh
        // child not yet registered — the window the blocking exists to
        // close). Delivery to the main thread after the unblock runs the
        // handler normally. The re-raise below accounts for this mask by
        // targeting the process, not this thread.
        let _saved = block_interrupt_signals();

        // The first byte is the signal that started cleanup; it is also the
        // signal re-raised at the end, so the 128+N status names the right
        // one when both arrive.
        let Some(first) = read_terminated(read_fd) else {
            return;
        };
        CLEANUP_STARTED.store(true, Ordering::Release);
        note_seen();

        // Phase 1 — ask every registered group to stop. The snapshot closes
        // the spawn window (see `snapshot_spawn_safe`): a group whose
        // spawning thread was mid-fork is either already registered here or
        // the snapshot waited for the register to land.
        for pgid in snapshot_spawn_safe() {
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
            // Same spawn-window discipline as phase 1: a spawn completing
            // during the grace is registered before this snapshot sees it.
            let survivors: Vec<libc::pid_t> = snapshot_spawn_safe()
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
        //
        // `kill(getpid())` rather than `raise()`: this thread keeps the
        // interrupt family blocked for life (see the top of this function),
        // and a blocked raise would pend here forever instead of killing the
        // process. Targeting the process lets the kernel deliver to a thread
        // with the signal unblocked — the main thread, whose mask
        // `spawn_supervised` always restores — so a death that races a
        // spawn window is merely deferred to the unblock, never lost.
        // SAFETY: kill(2) to this process with SIG_DFL installed; it does
        // not return.
        unsafe {
            libc::signal(first as libc::c_int, libc::SIG_DFL);
            libc::kill(libc::getpid(), first as libc::c_int);
        }
    }

    /// Blocks SIGINT/SIGTERM/SIGHUP on the calling thread and returns the
    /// previous mask for [`restore_interrupt_signals`]. Used in two places:
    /// `spawn_supervised` (the spawn→register window) and the watcher's
    /// first step (so a window-deferred signal cannot be delivered here and
    /// run the cleanup before the fresh child is registered).
    pub(super) fn block_interrupt_signals() -> libc::sigset_t {
        // SAFETY: sigset construction and pthread_sigmask have no
        // preconditions; the returned mask is a plain value.
        unsafe {
            let mut blocked: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut blocked);
            libc::sigaddset(&mut blocked, libc::SIGINT);
            libc::sigaddset(&mut blocked, libc::SIGTERM);
            libc::sigaddset(&mut blocked, libc::SIGHUP);
            let mut previous: libc::sigset_t = std::mem::zeroed();
            libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut previous);
            previous
        }
    }

    /// Restores a mask returned by [`block_interrupt_signals`], delivering
    /// anything that went pending while it was blocked.
    pub(super) fn restore_interrupt_signals(previous: libc::sigset_t) {
        // SAFETY: the mask came from block_interrupt_signals on this thread.
        unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
        }
    }

    /// See [`super::park_while_interrupt_cleanup_concludes`]. A plain
    /// sleep-loop: this runs at most once per process lifetime, on a path
    /// whose only exit is the watcher killing the process.
    pub(super) fn park_while_cleanup_concludes() {
        while CLEANUP_STARTED.load(Ordering::Acquire) {
            std::thread::sleep(std::time::Duration::from_millis(100));
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
    /// every member exited or was reaped, which ends the wait early. EPERM
    /// means a member exists that may not be signalled (a setuid/setgid
    /// descendant of the vendor CLI) — the app's documented convention
    /// (`platform/os/posix.rs`: success and EPERM both mean the process
    /// exists). Counting EPERM as dead would end the grace with the
    /// survivor un-KILLed, exactly the orphan this module exists to
    /// prevent.
    fn group_alive(pgid: libc::pid_t) -> bool {
        let Some(signed) = signalable_group(pgid as u32) else {
            return false;
        };
        // SAFETY: probe with signal 0; sends nothing.
        unsafe {
            let rc = libc::kill(-signed, 0);
            rc == 0 || (rc == -1 && *errno_location() == libc::EPERM)
        }
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

    /// Held across a supervised spawn→register pair (inside
    /// `spawn_supervised`), and briefly by the watcher around each snapshot
    /// via [`snapshot_spawn_safe`].
    pub(super) fn spawn_window() -> std::sync::MutexGuard<'static, ()> {
        SPAWN_WINDOW.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Registry snapshot with the spawn window closed: the returned list is
    /// the complete set of groups at an instant where no thread sits between
    /// spawn and register, so an interrupt cleanup cannot miss a fresh child.
    pub(super) fn snapshot_spawn_safe() -> Vec<libc::pid_t> {
        let _window = spawn_window();
        snapshot()
    }
}

#[cfg(all(test, unix))]
mod tests {

    /// The inherited-SIG_IGN probe: `nohup` (SIG_IGN) must be respected —
    /// the handler must NOT be installed over a caller's explicit detach —
    /// while the default disposition installs normally. The test saves and
    /// restores the process-wide SIGHUP disposition, and flips it through
    /// SIG_IGN and SIG_DFL to drive both probe answers without real
    /// terminal state.
    #[test]
    fn sighup_probe_reads_the_inherited_disposition() {
        use super::imp::inherited_sighup_is_ignored;
        // SAFETY: sigaction with a null act only reads the disposition;
        // the installs below are restored before returning.
        unsafe {
            let mut saved: libc::sigaction = std::mem::zeroed();
            assert_eq!(
                libc::sigaction(libc::SIGHUP, std::ptr::null(), &mut saved),
                0,
                "the disposition probe must work in-process"
            );
            // SIG_IGN inherited (what nohup arranges) → the caller sees it.
            // The handler field is the same sighandler_t the installer
            // writes (same double cast).
            let mut ign: libc::sigaction = std::mem::zeroed();
            ign.sa_sigaction = libc::SIG_IGN as *const () as libc::sighandler_t as usize;
            assert_eq!(libc::sigaction(libc::SIGHUP, &ign, std::ptr::null_mut()), 0);
            assert!(inherited_sighup_is_ignored());

            // Default disposition → install normally.
            let mut dfl: libc::sigaction = std::mem::zeroed();
            dfl.sa_sigaction = libc::SIG_DFL as *const () as libc::sighandler_t as usize;
            assert_eq!(libc::sigaction(libc::SIGHUP, &dfl, std::ptr::null_mut()), 0);
            assert!(!inherited_sighup_is_ignored());

            // Restore exactly what the process had (an installed handler
            // from a previous test's real_install must survive this test).
            assert_eq!(
                libc::sigaction(libc::SIGHUP, &saved, std::ptr::null_mut()),
                0
            );
        }
    }
    use super::imp;
    use super::{forget_child_group, spawn_supervised};

    /// The grace value itself is load-bearing (a vendor CLI that traps
    /// SIGTERM to flush state gets this long); the decision tests above pin
    /// precedence relative to whatever the constant is, so drift of the
    /// VALUE itself must fail loudly here.
    #[test]
    #[cfg(unix)]
    fn grace_window_value_is_pinned() {
        assert_eq!(imp::GRACE_MS, 5_000);
    }

    /// The `pre_exec` mask reset in `spawn_supervised` is load-bearing: std
    /// does not guarantee an empty child mask at exec, and on macOS the
    /// blocked INT/TERM/HUP mask verifiably survives into the child, which
    /// would make every forwarded SIGTERM undeliverable (all cleanups would
    /// ride the SIGKILL escalation, destroying the flush-on-TERM case the
    /// grace exists for). A child spawned while the spawner's mask blocks
    /// the family must therefore still die on a plain SIGTERM.
    #[test]
    #[cfg(unix)]
    fn spawned_children_receive_signals_the_spawner_blocks() {
        let saved = imp::block_interrupt_signals();
        let mut command = std::process::Command::new("sleep");
        command.arg("30");
        command.stdout(std::process::Stdio::null());
        command.stderr(std::process::Stdio::null());
        let mut child = spawn_supervised(&mut command).expect("spawn under a blocked mask");
        imp::restore_interrupt_signals(saved);
        forget_child_group(child.id());

        // SAFETY: kill to this test's own fresh child; sleep installs no
        // TERM handler, so delivery means immediate default-disposition death.
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let exited = loop {
            match child.try_wait().expect("waitable child") {
                Some(status) => break Some(status),
                None if std::time::Instant::now() >= deadline => break None,
                None => std::thread::sleep(std::time::Duration::from_millis(25)),
            }
        };
        // Reap regardless so the sleeper never outlives the test.
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            exited.is_some(),
            "SIGTERM must reach a child spawned under a blocked spawner mask;              the pre_exec reset regressed"
        );
    }

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
