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
    let result = pinvou_cli::parse_args(arguments).and_then(pinvou_cli::execute);
    let code = match result {
        Ok(outcome) => pinvou_cli::support::emit_report(std::io::stdout(), &outcome),
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "pinvou: {error}");
            error.exit_code().as_i32()
        }
    };
    std::process::exit(code);
}
