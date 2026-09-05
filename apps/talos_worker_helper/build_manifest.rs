// Release helpers retain UIAccess for signed installation in a trusted location.
// Unsigned debug/test executables cannot meet that Windows launch requirement.
pub fn windows_manifest_for_profile(profile: &str) -> String {
    let manifest = include_str!("talos_worker_helper.exe.manifest");
    if profile == "debug" {
        manifest.replace("uiAccess=\"true\"", "uiAccess=\"false\"")
    } else {
        manifest.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_debug_helper_runs_with_invoker_privileges() {
        let manifest = windows_manifest_for_profile("debug");
        assert!(manifest.contains("level=\"asInvoker\" uiAccess=\"false\""));
        assert!(!manifest.contains("uiAccess=\"true\""));
    }

    #[test]
    fn release_and_unknown_profiles_preserve_the_release_manifest_exactly() {
        let expected = include_str!("talos_worker_helper.exe.manifest");
        assert!(expected.contains("uiAccess=\"true\""));
        for profile in ["release", "", "custom"] {
            assert_eq!(windows_manifest_for_profile(profile), expected);
        }
    }
}
