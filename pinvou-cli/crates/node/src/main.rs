fn main() {
    if let Err(error) = pinvou_node::run_from_env() {
        eprintln!("pinvou-node: {error}");
        std::process::exit(error.exit_code().as_i32());
    }
}
