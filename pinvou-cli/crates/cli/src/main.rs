use std::io::Write;

fn finish_worker_exit(
    code: i32,
    is_worker: bool,
    finish: impl FnOnce(i32) -> Result<i32, &'static str>,
) -> i32 {
    if !is_worker {
        return code;
    }
    match finish(code) {
        Ok(code) => code,
        Err(error) => {
            let _ = writeln!(
                std::io::stderr(),
                "pinvou: CLI output finish failed: {error}"
            );
            1
        }
    }
}

fn admission_error_message(code: &str) -> String {
    if code == "HOST_CLI_STREAMS_UNSUPPORTED" {
        "pinvou: managed CLI currently requires three redirected, non-terminal streams; stdout and stderr must not share the same output (ordinary terminals and 2>&1 are not supported)".to_owned()
    } else {
        format!("pinvou: upgrade host admission failed: {code}")
    }
}

#[cfg(any(feature = "upgrade-host-lab", feature = "upgrade-host-production"))]
fn is_host_profile_query(arguments: &[std::ffi::OsString]) -> bool {
    arguments.len() == 2 && arguments[1] == "--host-profile"
}

fn main() {
    // The packaging wrapper verifies this executable's embedded public profile
    // before installation. Only this exact invocation bypasses writer startup;
    // it reads no settings and cannot run a command.
    let raw: Vec<std::ffi::OsString> = std::env::args_os().collect();
    #[cfg(any(feature = "upgrade-host-lab", feature = "upgrade-host-production"))]
    if is_host_profile_query(&raw) {
        match pinvou3_lib::compiled_cli_profile() {
            Ok(bytes) => {
                if std::io::stdout().lock().write_all(&bytes).is_err() {
                    std::process::exit(1);
                }
                return;
            }
            Err(code) => {
                let _ = writeln!(
                    std::io::stderr(),
                    "pinvou: host profile unavailable: {code}"
                );
                std::process::exit(1);
            }
        }
    }
    // Before any normal command: install SIGINT/SIGTERM forward/kill wiring for
    // vendor children spawned in their own process groups (see
    // `support/supervise.rs`). Runs before any supervised child exists; the
    // install is Once-guarded. The one wiring call this main.rs gets.
    pinvou_cli::support::supervise::install_signal_cleanup();
    // Validate arguments before requesting a worker, preserving the existing
    // non-Unicode error instead of sending a lossy or ambiguous request.
    let arguments = match pinvou_cli::support::decode_arguments(raw) {
        Ok(arguments) => arguments,
        Err(index) => {
            let _ = writeln!(
                std::io::stderr(),
                "pinvou: argument {index} is not valid UTF-8 and cannot be handled losslessly; \
                 re-run with a UTF-8 value (a lossy conversion would silently turn it into a \
                 different id, path or flag)"
            );
            std::process::exit(2);
        }
    };
    // Only the native service can return Worker for this actual process. The
    // frontend returns the worker's original exit code without executing CLI
    // work itself. Keep the worker's lease through supervised-child cleanup.
    let _writer_admission = match pinvou3_lib::enter_current_cli() {
        Ok(pinvou3_lib::CliMainEntry::Worker(admission)) => Some(admission),
        Ok(pinvou3_lib::CliMainEntry::DisabledProfile) => None,
        Ok(pinvou3_lib::CliMainEntry::FrontendExit(code)) => std::process::exit(code),
        Err(code) => {
            let _ = writeln!(std::io::stderr(), "{}", admission_error_message(code));
            std::process::exit(1);
        }
    };
    // A panic that escapes every layer above exits 101 — outside the
    // documented 0/1/2 contract — so it must be unmistakably an internal
    // bug, not a diagnosable command failure. One clean stderr line
    // replaces the default multi-line panic dump; `RUST_BACKTRACE=1` still
    // captures the trace for a report. (The windowless product host
    // contains its own bootstrap panics into ordinary failures — see
    // `run_windowless_host` — so this hook is the last line of defense,
    // not the usual path.)
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".to_owned());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_owned());
        let _ = writeln!(
            std::io::stderr(),
            "pinvou: internal error (panic at {location}: {payload}); this is a bug, \
             please report it"
        );
        if std::env::var_os("RUST_BACKTRACE").is_some_and(|value| value != "0") {
            let _ = writeln!(
                std::io::stderr(),
                "{:?}",
                std::backtrace::Backtrace::force_capture()
            );
        }
    }));
    // Round-50 review: an internal bug must not bypass the supervisor on its
    // way out. A plain unwinding panic would kill the interrupt watcher at
    // whatever phase it is in — a vendor child parked in its own process
    // group (a minutes-long `connectors connect` login) would keep running
    // with no one left to TERM it, and a cleanup already past its SIGTERM
    // grace would never reach the phase-3 re-raise. `catch_unwind` keeps the
    // exit contract (the hook above already printed the one-line internal
    // error; the code stays 101, unmistakably outside 0/1/2) while the park
    // below still runs on the panic path exactly as it does on every other
    // exit. `AssertUnwindSafe` is sound here: nothing observed after the
    // catch reads CLI state — the process exits.
    let code = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> i32 {
        // Test seam (contract-pinned through the real binary): forces the
        // panic path from INSIDE the closure so the pin actually holds the
        // catch + park + 101 wiring together — a seam placed before
        // `catch_unwind` (the round-50 shape) let the panic escape `main`
        // directly, where std's own hook + 101-exit kept the pin green even
        // with the whole catch/park wiring deleted (round-51 review). No
        // CLI behavior reads this variable.
        if std::env::var_os("PINVOU_CLI_TEST_FORCE_PANIC").is_some() {
            panic!("forced by PINVOU_CLI_TEST_FORCE_PANIC");
        }
        match pinvou_cli::parse_args(arguments).and_then(pinvou_cli::execute) {
            Ok(outcome) => pinvou_cli::support::emit_report(std::io::stdout(), &outcome),
            Err(error) => {
                let _ = writeln!(std::io::stderr(), "pinvou: {error}");
                error.exit_code().as_i32()
            }
        }
    }))
    .unwrap_or(101);
    // A started interrupt cleanup owns the exit status: the watcher's phase-3
    // re-raise gives scripts the conventional 128+N, and main must not win
    // the race to `exit` with the family's own code. No-op unless cleanup
    // started (returns immediately on the normal path) — and on the panic
    // path it is the one chance to let a started cleanup conclude.
    pinvou_cli::support::supervise::park_while_interrupt_cleanup_concludes();
    let code = finish_worker_exit(
        code,
        _writer_admission.is_some(),
        pinvou3_lib::finish_current_cli,
    );
    std::process::exit(code);
}

#[cfg(test)]
mod finish_tests {
    use super::{admission_error_message, finish_worker_exit};

    #[test]
    fn worker_returns_exit_code_only_after_successful_finish() {
        let mut called = false;
        let code = finish_worker_exit(23, true, |original| {
            called = true;
            assert_eq!(original, 23);
            Ok(original)
        });
        assert!(called);
        assert_eq!(code, 23);
    }

    #[test]
    fn worker_finish_failure_is_an_error() {
        assert_eq!(finish_worker_exit(0, true, |_| Err("FINISH_FAILED")), 1);
    }

    #[test]
    fn disabled_profile_preserves_exit_without_finishing() {
        assert_eq!(
            finish_worker_exit(23, false, |_| panic!("unexpected finish")),
            23
        );
    }

    #[test]
    fn unsupported_streams_are_explained_without_changing_other_errors() {
        let message = admission_error_message("HOST_CLI_STREAMS_UNSUPPORTED");
        assert!(message.contains("three redirected"));
        assert!(message.contains("2>&1"));
        assert_eq!(
            admission_error_message("OTHER"),
            "pinvou: upgrade host admission failed: OTHER"
        );
    }
}

#[cfg(all(
    test,
    any(feature = "upgrade-host-lab", feature = "upgrade-host-production")
))]
mod tests {
    use super::is_host_profile_query;
    use std::ffi::OsString;

    #[test]
    fn only_the_exact_profile_query_bypasses_writer_startup() {
        let args = |values: &[&str]| {
            values
                .iter()
                .map(|value| OsString::from(*value))
                .collect::<Vec<_>>()
        };
        assert!(is_host_profile_query(&args(&["pinvou", "--host-profile"])));
        assert!(!is_host_profile_query(&args(&["pinvou"])));
        assert!(!is_host_profile_query(&args(&[
            "pinvou",
            "--host-profile",
            "--help"
        ])));
        assert!(!is_host_profile_query(&args(&[
            "pinvou",
            "--host-profile=1"
        ])));
    }
}
