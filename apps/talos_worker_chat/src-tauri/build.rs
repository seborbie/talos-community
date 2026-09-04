use std::{
    env,
    fs::File,
    path::PathBuf,
    process::{Command, Stdio},
};

fn bun_command() -> PathBuf {
    env::var_os("TALOS_FRONTEND_BUN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("bun"))
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let app_dir = manifest_dir.parent().expect("src-tauri parent dir");
    let workspace_dir = app_dir.parent().expect("JavaScript workspace parent dir");

    println!("cargo:rerun-if-changed={}", app_dir.join("src").display());
    println!(
        "cargo:rerun-if-changed={}",
        app_dir.join("package.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        app_dir.join("vite.config.ts").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        app_dir.join("svelte.config.js").display()
    );
    for workspace_file in ["package.json", "bun.lock", "bunfig.toml"] {
        let path = workspace_dir.join(workspace_file);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    println!("cargo:rerun-if-env-changed=TALOS_FRONTEND_BUN");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let stdout_log = out_dir.join("talos_worker_chat_frontend_build.log");
    let stderr_log = out_dir.join("talos_worker_chat_frontend_build.err.log");
    let bun = bun_command();

    let vite = app_dir
        .join("node_modules")
        .join("vite")
        .join("bin")
        .join("vite.js");
    let mut command = Command::new(&bun);
    command
        .arg("--bun")
        .arg(&vite)
        .arg("build")
        .current_dir(app_dir);
    command.env("CI", "1");
    if let Ok(file) = File::create(&stdout_log) {
        command.stdout(Stdio::from(file));
    }
    if let Ok(file) = File::create(&stderr_log) {
        command.stderr(Stdio::from(file));
    }
    let status = command
        .status()
        .expect("Vite is required to build talos_worker_chat (run `bun install` in apps/)");

    if !status.success() {
        panic!(
            "vite build failed for talos_worker_chat frontend; see {} and {}",
            stdout_log.display(),
            stderr_log.display()
        );
    }

    tauri_build::build();
}
