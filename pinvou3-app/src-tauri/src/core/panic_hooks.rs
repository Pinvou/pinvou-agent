//! Serialize application hook registration, preserving existing diagnostics.
//! Registrants return true only for the panic scope they actually own.
//!
//! # Contract for consumers
//!
//! - Registration is serialized under an internal mutex and the process hook
//!   is installed exactly once ([`Once`]), so a later registration never
//!   opens a take/set window over an active scope (the default-hook gap) and
//!   can never silently replace an earlier scope's handler.
//! - The process hook dispatches through an immutable snapshot of the
//!   registered handlers in REVERSE registration order: the first handler
//!   that returns `true` owns the panic, and if none does, the hook that was
//!   installed before the first [`chain`] call runs unchanged. A scope's
//!   priority therefore follows registration order — the most recently
//!   registered owning scope handles the panic first.
//! - No lock is held while handlers run, and the snapshot clone is an `Arc`
//!   bump, so the panic path allocates nothing.
//! - The registry is append-only by design: it expects a small, fixed set of
//!   once-per-process scopes, not per-session registration.
//! - Handlers run inside a panic hook. A handler that itself panics aborts
//!   the process (double panic), and a handler writing to stderr must
//!   tolerate write failure.
//!
//! What this does *not* fix: the very first [`chain`] call still installs
//! the dispatcher via one take/set pair, so that single window remains. In
//! this repository it runs on the owning worker thread before any request
//! (hence any redactable payload) exists, and the regression test below
//! pins every later registration as window-free.
//!
//! Placement note: `core/README.md` admits modules shared by two or more
//! feature modules, and this registry currently has one. It still belongs
//! here rather than in a feature: it mutates a process-global that any
//! future cross-feature panic scope must reach without feature-to-feature
//! imports (the closed fork's second scope,
//! `remote_control/controller/panic_scope.rs`, registers through this same
//! module).
use std::panic::PanicHookInfo;
use std::sync::{Arc, LazyLock, Mutex, Once, RwLock};

type Handler = dyn Fn(&PanicHookInfo<'_>) -> bool + Send + Sync;
type HandlerSnapshot = Arc<Vec<Arc<Handler>>>;

static REGISTRATION: Mutex<()> = Mutex::new(());
static INSTALL: Once = Once::new();
static HANDLERS: LazyLock<RwLock<HandlerSnapshot>> =
    LazyLock::new(|| RwLock::new(Arc::new(Vec::new())));

pub(crate) fn chain(handler: impl Fn(&PanicHookInfo<'_>) -> bool + Send + Sync + 'static) {
    let _registration = REGISTRATION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // No owned scope may become active until its registration returns. Only
    // the first install exchanges the process hook; later registrations must
    // not temporarily restore the default hook while another scope is active.
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Do not keep a registry lock while invoking arbitrary handlers.
            // The immutable snapshot needs no allocation in the panic hook.
            let snapshot = HANDLERS
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            for handler in snapshot.iter().rev() {
                if handler(info) {
                    return;
                }
            }
            previous(info);
        }));
    });
    #[cfg(test)]
    registration_checkpoint();
    let mut current = HANDLERS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut next = (**current).clone();
    next.push(Arc::new(handler));
    *current = Arc::new(next);
}

#[cfg(test)]
static CHECKPOINT: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>> =
    Mutex::new(None);

#[cfg(test)]
fn registration_checkpoint() {
    let checkpoint = CHECKPOINT.lock().unwrap().take();
    if let Some((entered, release)) = checkpoint {
        entered.send(()).unwrap();
        release
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the test binary as a child with a hard watchdog: `output()` alone
    /// blocks until exit, so a dispatch-deadlock regression would otherwise
    /// hang the suite until the CI job cap instead of failing within a
    /// minute. The filtered single-test output is a few kilobytes, far below
    /// the pipe buffer, so not draining while polling cannot stall the child.
    fn run_isolated_child(
        mut command: std::process::Command,
    ) -> (std::process::ExitStatus, String, String) {
        use std::io::Read;
        use std::time::{Duration, Instant};
        const BUDGET: Duration = Duration::from_secs(60);
        let mut child = command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("test binary can respawn itself");
        let deadline = Instant::now() + BUDGET;
        let status = loop {
            match child.try_wait().expect("child status pollable") {
                Some(status) => break status,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("isolated child did not exit within {BUDGET:?}");
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        let mut stdout = String::new();
        let mut stderr = String::new();
        child
            .stdout
            .take()
            .expect("stdout piped")
            .read_to_string(&mut stdout)
            .expect("stdout readable after exit");
        child
            .stderr
            .take()
            .expect("stderr piped")
            .read_to_string(&mut stderr)
            .expect("stderr readable after exit");
        (status, stdout, stderr)
    }

    #[test]
    fn late_registration_preserves_active_redaction_without_a_default_hook_gap() {
        const CHILD: &str = "PINVOU_PANIC_HOOK_GAP_CHILD";
        const SENTINEL: &str = "test-owned-panic-payload-sentinel";
        const SECOND_PAYLOAD: &str = "test-second-panic-payload-sentinel";
        const FORWARDED_PAYLOAD: &str = "test-unowned-panic-payload-sentinel";
        if std::env::var_os(CHILD).is_some() {
            let owner = std::thread::current().id();
            chain(move |_| {
                // Owner-scoped per the module contract: a panic raised
                // anywhere else must fall through, so a child-side failure
                // surfaces through the default hook instead of being
                // swallowed by the fixture.
                if std::thread::current().id() == owner {
                    eprintln!("owned_panic_redacted");
                    true
                } else {
                    false
                }
            });
            let (entered_tx, entered_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            *CHECKPOINT.lock().unwrap() = Some((entered_tx, release_rx));
            let registrant = std::thread::spawn(move || {
                chain(move |_| {
                    if std::thread::current().id() == owner {
                        eprintln!("late_scope_claimed");
                        true
                    } else {
                        false
                    }
                });
            });
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            // Phase 1: the panic fires while the late registration is
            // mid-flight — the in-flight scope is not in the snapshot yet,
            // so the owned scope handles it exactly once and the default
            // hook never sees the payload.
            assert!(std::panic::catch_unwind(|| panic!("{SENTINEL}")).is_err());
            release_tx.send(()).unwrap();
            registrant.join().unwrap();
            // Phase 2: with both scopes registered, the LAST-registered
            // owner wins — the late scope claims the panic and the earlier
            // owned scope must not run a second time.
            assert!(std::panic::catch_unwind(|| panic!("{SECOND_PAYLOAD}")).is_err());
            // Phase 3: a panic no scope owns must still reach the previous
            // (default) hook — that forwarding is what keeps unredacted
            // diagnostics elsewhere intact.
            let unowned = std::thread::spawn(|| {
                assert!(std::panic::catch_unwind(|| panic!("{FORWARDED_PAYLOAD}")).is_err());
            });
            unowned.join().unwrap();
            return;
        }
        let module = module_path!().split_once("::").unwrap().1;
        let filter = format!(
            "{module}::late_registration_preserves_active_redaction_without_a_default_hook_gap"
        );
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", &filter, "--nocapture"])
            .env(CHILD, "1");
        let (status, stdout, stderr) = run_isolated_child(command);
        assert!(
            status.success(),
            "isolated hook registration regression failed: {status}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
        );
        assert!(
            stdout.contains("test result: ok. 1 passed"),
            "isolated test matched zero cases\n--- stdout ---\n{stdout}"
        );
        assert!(
            !stderr.contains(SENTINEL),
            "owned panic reached default hook during registration\n--- stderr ---\n{stderr}"
        );
        assert!(
            !stderr.contains(SECOND_PAYLOAD),
            "second panic fell through to the default hook\n--- stderr ---\n{stderr}"
        );
        assert_eq!(
            stderr.matches("owned_panic_redacted").count(),
            1,
            "owned scope must redact exactly once — a dispatcher that runs an earlier scope despite a newer claim also fails here\n--- stderr ---\n{stderr}"
        );
        assert_eq!(
            stderr.matches("late_scope_claimed").count(),
            1,
            "the last-registered owner must win the second panic\n--- stderr ---\n{stderr}"
        );
        assert!(
            stderr.contains(FORWARDED_PAYLOAD),
            "a panic no scope owns must still reach the default hook\n--- stderr ---\n{stderr}"
        );
    }
}
