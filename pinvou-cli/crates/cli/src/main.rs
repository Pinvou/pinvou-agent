use std::io::Write;

fn main() {
    // First statement: installs the SIGINT/SIGTERM forward/kill wiring for
    // vendor children spawned in their own process groups (see
    // `support/supervise.rs`). Runs before any supervised child exists; the
    // install is Once-guarded. The one wiring call this main.rs gets.
    pinvou_cli::support::supervise::install_signal_cleanup();
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
    // Non-Unicode argv must fail loudly at the door: the old
    // `to_string_lossy` silently turned an undecodable session id or path
    // into a look-alike and answered "not found" for input the user never
    // typed. The refusal is argv-decidable, so it follows the family
    // convention: exit 2, `support::decode_arguments` states the full
    // contract and unit-pins it (the program slot is exempt; a byte path
    // there must not lock the user out of every command).
    let raw: Vec<std::ffi::OsString> = std::env::args_os().collect();
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
    if std::env::var_os("PINVOU_CLI_TEST_FORCE_PANIC").is_some() {
        // Test seam (contract-pinned through the real binary): forces the
        // panic path so the pin can hold the catch + park + 101 wiring
        // together. No CLI behavior reads this variable.
        panic!("forced by PINVOU_CLI_TEST_FORCE_PANIC");
    }
    let code = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> i32 {
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
    std::process::exit(code);
}
