use std::io::Write;

fn main() {
    // `std::env::args` panics on non-Unicode argv (a raw-bytes path from a
    // tool would exit 101 with a backtrace); lossy-convert instead so the
    // regular usage/exit-code contract handles the argument.
    let result =
        pinvou_cli::parse_args(std::env::args_os().map(|arg| arg.to_string_lossy().into_owned()))
            .and_then(pinvou_cli::execute);
    let code = match result {
        Ok(outcome) => pinvou_cli::support::emit_report(std::io::stdout(), &outcome),
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "pinvou: {error}");
            error.exit_code().as_i32()
        }
    };
    std::process::exit(code);
}
