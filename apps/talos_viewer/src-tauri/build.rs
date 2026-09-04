use std::{env, fs, path::PathBuf, process::Command};

mod build_manifest_public_key;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));

    println!("cargo:rerun-if-env-changed=RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH");
    let manifest_key_output =
        PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("manifest_public_key.der");
    if let Some(path) = env::var_os("RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH") {
        let path = PathBuf::from(path);
        println!("cargo:rerun-if-changed={}", path.display());
        build_manifest_public_key::persist_validated_pkcs1_rsa_public_key(
            &path,
            &manifest_key_output,
        )
        .unwrap_or_else(|error| panic!("manifest signing public key cannot be embedded: {error}"));
    } else {
        fs::write(&manifest_key_output, []).expect("write empty embedded manifest public key DER");
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "macos" {
        if let Some(swift_runtime_dir) = swift_runtime_dir() {
            println!(
                "cargo:rustc-link-search=native={}",
                swift_runtime_dir.display()
            );
        }
    }

    if target_os == "windows" {
        println!("cargo:rerun-if-changed=native/win_key_block.cpp");
        cc::Build::new()
            .file(manifest_dir.join("native").join("win_key_block.cpp"))
            .cpp(true)
            .compile("win_key_block");
        println!("cargo:rustc-link-lib=user32");
    }

    tauri_build::build();
}

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
