#[cfg(windows)]
#[path = "../build/windows_resource.rs"]
mod windows_resource;

#[cfg(target_os = "macos")]
use std::{path::PathBuf, process::Command};

fn main() {
    #[cfg(target_os = "macos")]
    {
        if let Some(swift_runtime_dir) = swift_runtime_dir() {
            println!(
                "cargo:rustc-link-search=native={}",
                swift_runtime_dir.display()
            );
        }
    }

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        windows_resource::configure(
            &mut res,
            "Talos Worker Helper",
            "Talos Worker",
            "talos_worker_helper.exe",
        );
        res.set_manifest_file("talos_worker_helper.exe.manifest");
        res.compile().unwrap();
    }
}

#[cfg(target_os = "macos")]
fn swift_runtime_dir() -> Option<PathBuf> {
    let output = Command::new("xcrun")
        .args(["--find", "swiftc"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let swiftc = String::from_utf8(output.stdout).ok()?;
    let developer_dir = PathBuf::from(swiftc.trim())
        .parent()?
        .parent()?
        .parent()?
        .to_path_buf();
    let runtime_dir = developer_dir.join("usr/lib/swift/macosx");
    if runtime_dir.join("libswiftCompatibility56.a").exists() {
        Some(runtime_dir)
    } else {
        None
    }
}
