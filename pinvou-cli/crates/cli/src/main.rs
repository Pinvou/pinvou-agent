fn main() {
    let result = pinvou_cli::parse_args(std::env::args()).and_then(pinvou_cli::execute);
    match result {
        Ok(outcome) => {
            println!("{}", outcome.stdout);
            std::process::exit(outcome.exit_code.as_i32());
        }
        Err(error) => {
            eprintln!("pinvou: {error}");
            std::process::exit(error.exit_code().as_i32());
        }
    }
}
