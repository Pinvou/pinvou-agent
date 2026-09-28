use std::io::Write;

fn main() {
    // First statement: installs the SIGINT/SIGTERM forward/kill wiring for
    // vendor children spawned in their own process groups (see
    // `support/supervise.rs`). Runs before any supervised child exists; the
    // install is Once-guarded. The one wiring call this main.rs gets.
    pinvou_cli::support::supervise::install_signal_cleanup();
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
