fn main() {
    let result = pinvou_cli::parse_args(std::env::args()).and_then(pinvou_cli::execute);
    match result {
        Ok(outcome) => {
            let mut stdout = std::io::stdout().lock();
            let mut stderr = std::io::stderr().lock();
            if pinvou_cli::write_outcome(&outcome, &mut stdout, &mut stderr).is_err() {
                std::process::exit(1);
            }
            std::process::exit(outcome.exit_code.as_i32());
        }
        Err(error) => {
            eprintln!("pinvou: {error}");
            std::process::exit(error.exit_code().as_i32());
        }
    }
}
