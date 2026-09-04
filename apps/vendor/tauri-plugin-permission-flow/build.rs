const COMMANDS: &[&str] = &[
    "create",
    "suggested_host_app_path",
    "authorization_state",
    "start_flow",
    "stop_current_flow",
];

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }

    tauri_plugin::Builder::new(COMMANDS).build();
}
