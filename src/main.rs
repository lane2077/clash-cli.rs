fn main() {
    if let Err(err) = clash_cli::run() {
        clash_cli::print_run_error(&err);
        std::process::exit(1);
    }
}
