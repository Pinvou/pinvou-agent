use std::io::Write;

fn main() {
    let result = pinvou_cli::parse_args(std::env::args()).and_then(pinvou_cli::execute);
    match result {
        Ok(outcome) => {
            // Rust ignores SIGPIPE, so a closed pipe (`pinvou ... | head`)
            // turns into a write error, not a signal. Exit quietly with 0 —
            // the conventional pipe-closed handling — instead of panicking
            // with exit 101.
            let write_result = writeln!(std::io::stdout(), "{}", outcome.stdout);
            match write_result {
                Ok(()) => std::process::exit(outcome.exit_code.as_i32()),
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                    std::process::exit(0);
                }
                Err(error) => {
                    let _ = writeln!(std::io::stderr(), "pinvou: cannot write output: {error}");
                    std::process::exit(1);
                }
            }
        }
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "pinvou: {error}");
            std::process::exit(error.exit_code().as_i32());
        }
    }
}
