#[cfg(windows)]
#[path = "../build/windows_resource.rs"]
mod windows_resource;

use std::{env, fs, path::PathBuf};

#[cfg(windows)]
const AS_INVOKER_MANIFEST: &str = r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#;

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

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        windows_resource::configure(
            &mut res,
            "Talos Viewer Updater",
            "Talos Viewer",
            "talos_viewer_updater.exe",
        );
        res.set_manifest(AS_INVOKER_MANIFEST);
        res.compile().expect("compile Windows resources");
    }
}
