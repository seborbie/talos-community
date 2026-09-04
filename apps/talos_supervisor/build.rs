#[cfg(windows)]
#[path = "../build/windows_resource.rs"]
mod windows_resource;

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let target = out_dir.join("manifest_public_key.der");
    if let Ok(path) = env::var("RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH") {
        println!("cargo:rerun-if-changed={path}");
        let bytes = fs::read(&path).unwrap_or_else(|error| {
            panic!("failed to read manifest signing public key DER at {path}: {error}")
        });
        fs::write(&target, bytes).expect("write embedded manifest public key DER");
    } else {
        fs::write(&target, []).expect("write empty embedded manifest public key DER");
    }

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        windows_resource::configure(
            &mut res,
            "Talos Supervisor",
            "Talos Supervisor",
            "talos_supervisor.exe",
        );
        res.compile().expect("compile Windows resources");
    }
}
