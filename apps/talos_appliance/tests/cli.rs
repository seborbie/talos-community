// Medium tests: execute the built launcher with disposable paths; no daemon or network.
use std::process::Command;

#[test]
fn no_argument_downloaded_launcher_explains_the_startup_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_talos-server"))
        .output()
        .expect("run built launcher");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("quickstart --config"));
}

#[cfg(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "x86_64")
))]
#[test]
fn missing_docker_has_a_readable_error_and_creates_no_state() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let root = temporary.path().join("new installation");
    let output = Command::new(env!("CARGO_BIN_EXE_talos-server"))
        .arg("--state-dir")
        .arg(&root)
        .arg("--docker")
        .arg(temporary.path().join("missing-docker"))
        .args(["quickstart", "--config"])
        .arg(temporary.path().join("request.json"))
        .output()
        .expect("run built launcher");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Install/start Docker"));
    assert!(!root.exists());
}
