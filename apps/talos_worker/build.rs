#[cfg(all(windows, feature = "windows-resource"))]
#[path = "../build/windows_resource.rs"]
mod windows_resource;

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let target = out_dir.join("manifest_public_key.der");
    if let Ok(path) = env::var("RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH") {
        if let Ok(bytes) = fs::read(path) {
            let _ = fs::write(&target, bytes);
        } else {
            let _ = fs::write(&target, []);
        }
    } else {
        let _ = fs::write(&target, []);
    }

    #[cfg(all(windows, feature = "windows-resource"))]
    {
        let mut res = winres::WindowsResource::new();
        windows_resource::configure(&mut res, "Talos Worker", "Talos Worker", "talos_worker.exe");
        res.compile().expect("compile Windows resources");
    }
}
