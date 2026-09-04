fn main() {
    if let Err(error) = talos_appliance::run(std::env::args_os().skip(1)) {
        eprintln!("talos-server: {error:#}");
        std::process::exit(1);
    }
}
