fn main() {
    if let Err(error) = corvid_lsp::run_stdio_server() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
