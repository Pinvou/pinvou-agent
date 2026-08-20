fn main() {
    if let Err(error) = pinvou_controller::run_from_env() {
        eprintln!("pinvou-controller: {error}");
        std::process::exit(error.exit_code().as_i32());
    }
}
