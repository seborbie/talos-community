#![cfg_attr(windows, windows_subsystem = "windows")]

use std::{
    backtrace::Backtrace,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Once, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::fd::AsRawFd;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use talos_protocol::{
    MacosUpdateAccountIpcRequest, MacosUpdateAccountIpcResponse, MacosUpdateAccountStatus,
    MACOS_UPDATE_ACCOUNT_SOCKET_PATH,
};
use tauri::{Manager, UserAttentionType};
use tracing::{debug, error, info, trace, warn};

mod update_account_ipc;
use update_account_ipc::macos_update_account_ipc_with_retry;

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static PANIC_LOGGING_HOOK: Once = Once::new();

const WORKER_APP_PATH: &str = "/Library/Talos/Worker/Talos Worker.app";
const WORKER_HELPER_APP_PATH: &str = "/Library/Talos/Worker/Talos Worker Helper.app";
const WORKER_EXE_PATH: &str = "/Library/Talos/Worker/Talos Worker.app/Contents/MacOS/talos_worker";
const RESTART_REQUEST_PATH: &str = "/tmp/talos-worker-restart-request";
const PERMISSION_FLOW_RESOURCE_BUNDLE_NAME: &str = "PermissionFlow_PermissionFlow.bundle";
const PERMISSION_FLOW_LOG_PATH_ENV: &str = "TALOS_PERMISSION_HELPER_LOG_PATH";
const MACOS_UPDATE_ACCOUNT_STATUS_IPC_ATTEMPTS: usize = 1;
const MACOS_UPDATE_ACCOUNT_APPROVAL_IPC_ATTEMPTS: usize = 8;
const MACOS_UPDATE_ACCOUNT_IPC_RETRY_DELAY: Duration = Duration::from_millis(500);

#[cfg(unix)]
const STDOUT_FILENO: i32 = 1;
#[cfg(unix)]
const STDERR_FILENO: i32 = 2;

#[cfg(unix)]
unsafe extern "C" {
    fn dup2(oldfd: i32, newfd: i32) -> i32;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct WorkerFullDiskAccessCheck {
    permission: String,
    granted: bool,
    #[serde(rename = "probe_path")]
    probe_path: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct PermissionState {
    granted: bool,
    #[serde(rename = "probePath")]
    probe_path: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct WorkerHelperPermissionCheck {
    accessibility: bool,
    #[serde(rename = "screenRecording")]
    screen_recording: bool,
}

#[derive(Clone, Debug, Serialize)]
struct PermissionSnapshot {
    #[serde(rename = "fullDiskAccess")]
    full_disk_access: PermissionState,
    #[serde(rename = "screenRecording")]
    screen_recording: PermissionState,
    accessibility: PermissionState,
    #[serde(rename = "macosSoftwareUpdate")]
    macos_software_update: MacosUpdateAccountStatus,
    #[serde(rename = "workerAppPath")]
    worker_app_path: String,
    #[serde(rename = "workerHelperAppPath")]
    worker_helper_app_path: String,
    #[serde(rename = "checkedAtUnixMs")]
    checked_at_unix_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchContext {
    reason: String,
    full_disk_access_required: bool,
    screen_recording_required: bool,
    accessibility_required: bool,
    macos_software_update_required: bool,
    after_install: bool,
    login_check: bool,
}

fn launch_context_from_args() -> LaunchContext {
    let args = std::env::args().collect::<Vec<_>>();
    launch_context_from_args_slice(&args)
}

fn launch_context_from_args_slice(args: &[String]) -> LaunchContext {
    let remote_desktop_required = args.iter().any(|arg| arg == "--remote-desktop-required");
    let full_disk_access_required = args.iter().any(|arg| arg == "--full-disk-access-required");
    let macos_software_update_required = args
        .iter()
        .any(|arg| arg == "--macos-update-owner-required");
    let after_install = args.iter().any(|arg| arg == "--after-install");
    let login_check = args.iter().any(|arg| arg == "--login-check");
    let reason = if remote_desktop_required {
        "remote_desktop".to_string()
    } else if full_disk_access_required {
        "file_transfer".to_string()
    } else if macos_software_update_required {
        "macos_software_update".to_string()
    } else if after_install {
        "after_install".to_string()
    } else if login_check {
        "login_check".to_string()
    } else {
        "manual".to_string()
    };
    let full_disk_access_required = full_disk_access_required || after_install || login_check;
    let core_permissions_required = true;
    let context = LaunchContext {
        reason,
        full_disk_access_required: core_permissions_required || full_disk_access_required,
        screen_recording_required: core_permissions_required || remote_desktop_required,
        accessibility_required: core_permissions_required || remote_desktop_required,
        macos_software_update_required: macos_software_update_required || after_install,
        after_install,
        login_check,
    };
    trace!(args = ?args, context = ?context, "resolved launch context from args");
    context
}

fn core_permissions_ready_for_login_check() -> bool {
    let full_disk_access = run_worker_full_disk_access_check();
    if !full_disk_access.granted {
        return false;
    }
    match run_worker_helper_permission_check() {
        Ok(check) => check.screen_recording && check.accessibility,
        Err(err) => {
            warn!(error = %err, "Worker Helper permissions are not ready during login check");
            false
        }
    }
}

fn log_path_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let home = home.trim();
        if !home.is_empty() {
            paths.push(
                PathBuf::from(home)
                    .join("Library")
                    .join("Logs")
                    .join("Talos")
                    .join("talos_permissions_helper.log"),
            );
        }
    }
    paths.push(PathBuf::from(
        "/Library/Logs/Talos/talos_permissions_helper.log",
    ));
    paths.push(std::env::temp_dir().join("talos_permissions_helper.log"));
    paths
}

fn resolve_log_path() -> PathBuf {
    for path in log_path_candidates() {
        let parent_ok = path
            .parent()
            .map(|parent| fs::create_dir_all(parent).is_ok())
            .unwrap_or(true);
        if !parent_ok {
            continue;
        }
        if OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .is_ok()
        {
            return path;
        }
    }
    log_path_candidates()
        .into_iter()
        .next()
        .unwrap_or_else(|| std::env::temp_dir().join("talos_permissions_helper.log"))
}

fn helper_log_path() -> PathBuf {
    LOG_PATH.get_or_init(resolve_log_path).clone()
}

fn prepare_native_permission_flow_logging() {
    let log_path = helper_log_path();
    std::env::set_var(PERMISSION_FLOW_LOG_PATH_ENV, &log_path);
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "full");
    }
    install_panic_logging_hook();
}

fn install_panic_logging_hook() {
    let log_path = helper_log_path();
    PANIC_LOGGING_HOOK.call_once(move || {
        std::panic::set_hook(Box::new(move |panic_info| {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
                let _ = writeln!(
                    file,
                    "TRACE talos_permissions_helper_panic panic_info={} backtrace={:?}",
                    panic_info,
                    Backtrace::force_capture()
                );
                let _ = file.flush();
            }
            eprintln!("Talos Permissions Helper panic: {panic_info}");
        }));
    });
}

#[cfg(unix)]
fn redirect_stdout_stderr_to_log() {
    let log_path = helper_log_path();
    match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(mut file) => {
            let fd = file.as_raw_fd();
            let stdout_result = unsafe { dup2(fd, STDOUT_FILENO) };
            let stderr_result = unsafe { dup2(fd, STDERR_FILENO) };
            let _ = writeln!(
                file,
                "Talos Permissions Helper redirected stdout/stderr to {} stdout_result={} stderr_result={}",
                log_path.display(),
                stdout_result,
                stderr_result
            );
            let _ = file.flush();
            if stdout_result == -1 || stderr_result == -1 {
                warn!(
                    stdout_result,
                    stderr_result, "stdout/stderr redirection returned an error"
                );
            } else {
                info!(path = %log_path.display(), "stdout/stderr redirected to permissions helper log");
            }
        }
        Err(err) => {
            warn!(path = %log_path.display(), error = %err, "failed to redirect stdout/stderr")
        }
    }
}

#[cfg(not(unix))]
fn redirect_stdout_stderr_to_log() {}

fn permission_flow_resource_bundle_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos_dir = exe.parent()?;
    let contents_dir = macos_dir.parent()?;
    Some(
        contents_dir
            .join("Resources")
            .join(PERMISSION_FLOW_RESOURCE_BUNDLE_NAME),
    )
}

fn log_permission_flow_resource_bundle_status() {
    let Some(bundle_path) = permission_flow_resource_bundle_path() else {
        warn!("PermissionFlow resource bundle path could not be resolved");
        return;
    };
    let english_strings_path = bundle_path.join("en.lproj").join("Localizable.strings");
    info!(
        bundle_path = %bundle_path.display(),
        bundle_exists = bundle_path.is_dir(),
        english_strings_path = %english_strings_path.display(),
        english_strings_exists = english_strings_path.is_file(),
        "PermissionFlow resource bundle status"
    );
}

fn init_file_logging() -> Result<(), std::io::Error> {
    let log_path = helper_log_path();
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let writer = talos_log_util::DailyFileMakeWriter::try_new(log_path.clone())?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_timer(tracing_subscriber::fmt::time::SystemTime)
        .with_writer(writer)
        .with_ansi(false)
        .init();
    info!(path = %log_path.display(), "Talos Permissions Helper logging to file");
    Ok(())
}

fn truncate_for_log(value: &str) -> String {
    const MAX_LOG_CHARS: usize = 4000;
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(MAX_LOG_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}... <truncated>")
    } else {
        truncated
    }
}

fn user_facing_helper_permission_error(err: &anyhow::Error) -> String {
    let chain = err
        .chain()
        .map(|cause| cause.to_string())
        .collect::<Vec<_>>()
        .join(": ")
        .to_ascii_lowercase();
    if chain.contains("not found") || chain.contains("missing") {
        "Talos Worker Helper is not installed.".to_string()
    } else {
        "Talos could not check Screen Recording or Accessibility.".to_string()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn worker_executable_path() -> PathBuf {
    PathBuf::from(WORKER_EXE_PATH)
}

fn worker_helper_app_path() -> PathBuf {
    PathBuf::from(WORKER_HELPER_APP_PATH)
}

fn worker_helper_executable_path(helper_app: &Path) -> PathBuf {
    helper_app
        .join("Contents")
        .join("MacOS")
        .join("talos_worker_helper")
}

fn worker_app_path() -> PathBuf {
    PathBuf::from(WORKER_APP_PATH)
}

fn permission_check_output_path(prefix: &str) -> PathBuf {
    PathBuf::from(format!(
        "/tmp/{}-{}-{}.json",
        prefix,
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}

fn run_worker_full_disk_access_check() -> WorkerFullDiskAccessCheck {
    let worker_app = worker_app_path();
    let worker = worker_executable_path();
    debug!(
        worker_app = %worker_app.display(),
        worker_executable = %worker.display(),
        worker_app_exists = worker_app.exists(),
        worker_executable_exists = worker.exists(),
        "checking Worker Full Disk Access"
    );
    if !worker_app.exists() {
        warn!(worker_app = %worker_app.display(), "Worker app missing during Full Disk Access check");
        return WorkerFullDiskAccessCheck {
            permission: "full_disk_access".to_string(),
            granted: false,
            probe_path: Some(worker_app.to_string_lossy().to_string()),
            error: Some("Talos Worker is not installed.".to_string()),
        };
    }

    if !worker.exists() {
        warn!(
            worker_app = %worker_app.display(),
            worker_executable = %worker.display(),
            "Worker app executable missing during Full Disk Access check"
        );
        return WorkerFullDiskAccessCheck {
            permission: "full_disk_access".to_string(),
            granted: false,
            probe_path: Some(worker_app.to_string_lossy().to_string()),
            error: Some("Talos Worker is not installed.".to_string()),
        };
    }

    let output_path = permission_check_output_path("talos-worker-full-disk-access");
    match Command::new("/usr/bin/open")
        .arg("-n")
        .arg("-W")
        .arg(&worker_app)
        .arg("--args")
        .arg("--check-full-disk-access")
        .arg("--json")
        .arg("--json-output")
        .arg(&output_path)
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            trace!(
                status = ?output.status.code(),
                success = output.status.success(),
                stdout = %truncate_for_log(stdout.trim()),
                stderr = %truncate_for_log(stderr.trim()),
                output_path = %output_path.display(),
                "Worker app Full Disk Access check completed"
            );
            let bytes = fs::read(&output_path);
            let _ = fs::remove_file(&output_path);
            match bytes
                .with_context(|| {
                    format!("read worker permission output: {}", output_path.display())
                })
                .and_then(|bytes| {
                    trace!(
                        output_path = %output_path.display(),
                        bytes = bytes.len(),
                        json = %truncate_for_log(&String::from_utf8_lossy(&bytes)),
                        "read Worker Full Disk Access check output"
                    );
                    serde_json::from_slice::<WorkerFullDiskAccessCheck>(&bytes)
                        .context("parse Worker Full Disk Access check output")
                }) {
                Ok(check) => {
                    debug!(
                        granted = check.granted,
                        probe_path = ?check.probe_path,
                        error = ?check.error,
                        "Worker Full Disk Access check parsed"
                    );
                    check
                }
                Err(err) => {
                    warn!(
                        error = %err,
                        status = ?output.status.code(),
                        stderr = %truncate_for_log(stderr.trim()),
                        "unable to parse Worker Full Disk Access check output"
                    );
                    WorkerFullDiskAccessCheck {
                        permission: "full_disk_access".to_string(),
                        granted: false,
                        probe_path: Some(worker_app.to_string_lossy().to_string()),
                        error: Some("Talos could not check Full Disk Access.".to_string()),
                    }
                }
            }
        }
        Err(err) => {
            warn!(
                error = %err,
                worker_app = %worker_app.display(),
                "unable to run Worker app Full Disk Access check"
            );
            WorkerFullDiskAccessCheck {
                permission: "full_disk_access".to_string(),
                granted: false,
                probe_path: Some(worker_app.to_string_lossy().to_string()),
                error: Some("Talos could not check Full Disk Access.".to_string()),
            }
        }
    }
}

fn run_worker_helper_permission_check() -> Result<WorkerHelperPermissionCheck> {
    let helper_app = worker_helper_app_path();
    debug!(
        helper_app = %helper_app.display(),
        helper_app_exists = helper_app.exists(),
        "checking Worker Helper permissions"
    );
    if helper_app.exists() {
        return run_worker_helper_permission_check_via_app(&helper_app);
    }

    warn!(helper_app = %helper_app.display(), "Worker Helper app missing during permission check");
    Err(anyhow!("Talos Worker Helper is not installed."))
}

fn run_worker_helper_permission_check_via_app(
    helper_app: &Path,
) -> Result<WorkerHelperPermissionCheck> {
    let helper_executable = worker_helper_executable_path(helper_app);
    if !helper_executable.exists() {
        warn!(
            helper_app = %helper_app.display(),
            helper_executable = %helper_executable.display(),
            "Worker Helper app executable missing during permission check"
        );
        return Err(anyhow!("Talos Worker Helper is not installed."));
    }

    let output_path = permission_check_output_path("talos-worker-helper-permissions");
    debug!(
        helper_app = %helper_app.display(),
        helper_executable = %helper_executable.display(),
        output_path = %output_path.display(),
        "running Worker Helper app permission check"
    );
    let output = Command::new("/usr/bin/open")
        .arg("-n")
        .arg("-W")
        .arg(helper_app)
        .arg("--args")
        .arg("check-macos-permissions")
        .arg("--json")
        .arg("--json-output")
        .arg(&output_path)
        .output()
        .with_context(|| format!("open {}", helper_app.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    trace!(
        helper_app = %helper_app.display(),
        helper_executable = %helper_executable.display(),
        output_path = %output_path.display(),
        status = ?output.status.code(),
        success = output.status.success(),
        stdout = %truncate_for_log(stdout.trim()),
        stderr = %truncate_for_log(stderr.trim()),
        "Worker Helper app permission check completed"
    );
    let bytes = fs::read(&output_path).with_context(|| {
        format!(
            "read helper permission output: {}; status={:?}; stderr={}",
            output_path.display(),
            output.status.code(),
            stderr.trim()
        )
    })?;
    let _ = fs::remove_file(&output_path);
    trace!(
        output_path = %output_path.display(),
        bytes = bytes.len(),
        json = %truncate_for_log(&String::from_utf8_lossy(&bytes)),
        "read Worker Helper permission check output"
    );
    let check =
        serde_json::from_slice::<WorkerHelperPermissionCheck>(&bytes).with_context(|| {
            format!(
                "parse worker helper app permission check: status={:?}; stderr={}",
                output.status.code(),
                stderr.trim()
            )
        })?;
    debug!(
        screen_recording = check.screen_recording,
        accessibility = check.accessibility,
        "Worker Helper permission check parsed"
    );
    Ok(check)
}

fn run_macos_update_account_approval_prompt(username: &str, password: &str) -> Result<String> {
    if username.is_empty()
        || username.len() > 64
        || username.starts_with('-')
        || username.contains('/')
        || !username
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(anyhow!("invalid Talos update account username"));
    }
    let stdin_text = format!("{password}\n");
    let mut child = Command::new("/usr/sbin/sysadminctl")
        .args(["interactive", "-secureTokenOn", username, "-password", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("open macOS Software Updates approval prompt")?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .context("open sysadminctl approval stdin")?;
        stdin
            .write_all(stdin_text.as_bytes())
            .context("write Talos update account password to sysadminctl")?;
    }
    let output = child
        .wait_with_output()
        .context("wait for macOS Software Updates approval prompt")?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let redacted = redact_secret_values(&combined, &[password.to_string()]);
    if output.status.success() {
        Ok(redacted)
    } else {
        Err(anyhow!(
            "macOS Software Updates approval prompt failed with {:?}: {}",
            output.status.code(),
            truncate_for_log(&redacted)
        ))
    }
}

fn redact_secret_values(value: &str, secrets: &[String]) -> String {
    secrets.iter().fold(value.to_string(), |redacted, secret| {
        if secret.is_empty() {
            redacted
        } else {
            redacted.replace(secret, "[redacted]")
        }
    })
}

fn macos_update_account_status() -> MacosUpdateAccountStatus {
    match macos_update_account_ipc_with_retry(
        &MacosUpdateAccountIpcRequest::GetStatus,
        MACOS_UPDATE_ACCOUNT_STATUS_IPC_ATTEMPTS,
        MACOS_UPDATE_ACCOUNT_IPC_RETRY_DELAY,
    ) {
        Ok(response) if response.ok => response.status.unwrap_or_else(|| {
            macos_update_account_error_status(
                "empty_status",
                "Talos could not check Software Updates.",
            )
        }),
        Ok(response) => response.status.unwrap_or_else(|| {
            macos_update_account_error_status(
                response
                    .error_code
                    .as_deref()
                    .unwrap_or("worker_rejected_request"),
                response
                    .error_message
                    .as_deref()
                    .unwrap_or("Talos could not check Software Updates."),
            )
        }),
        Err(err) => {
            warn!(error = %err, "Talos Worker macOS update account IPC unavailable");
            macos_update_account_error_status(
                "worker_unavailable",
                &format_macos_update_account_ipc_error(&err),
            )
        }
    }
}

fn format_macos_update_account_ipc_error(err: &anyhow::Error) -> String {
    let chain = err
        .chain()
        .map(|cause| cause.to_string())
        .collect::<Vec<_>>()
        .join(": ");
    if chain.contains(MACOS_UPDATE_ACCOUNT_SOCKET_PATH)
        || chain.contains("Connection refused")
        || chain.contains("empty macOS update account IPC response")
    {
        return "Talos is still starting. Keep this window open.".to_string();
    }
    "Talos could not check Software Updates.".to_string()
}

fn macos_update_account_error_status(code: &str, message: &str) -> MacosUpdateAccountStatus {
    MacosUpdateAccountStatus {
        schema_version: 1,
        required: true,
        status: "error".to_string(),
        username: "talos".to_string(),
        is_apple_silicon: true,
        account_present: false,
        is_admin: false,
        is_volume_owner: false,
        secure_token_enabled: false,
        credential_available: false,
        credential_version: None,
        generated_uid: None,
        expected_generated_uid: None,
        discovered_volume_owners: Vec::new(),
        failure_code: Some(code.to_string()),
        failure_message: Some(message.to_string()),
        checked_at: now_ms().to_string(),
    }
}

fn permission_snapshot() -> PermissionSnapshot {
    debug!("building permission snapshot");
    let check = run_worker_full_disk_access_check();
    let helper_check = run_worker_helper_permission_check();
    let helper_app_path = worker_helper_app_path().to_string_lossy().to_string();
    let (screen_recording, accessibility) = match helper_check {
        Ok(check) => (
            PermissionState {
                granted: check.screen_recording,
                probe_path: Some(helper_app_path.clone()),
                error: None,
            },
            PermissionState {
                granted: check.accessibility,
                probe_path: Some(helper_app_path.clone()),
                error: None,
            },
        ),
        Err(err) => {
            let message = user_facing_helper_permission_error(&err);
            warn!(error = %err, "Worker Helper permission check failed");
            (
                PermissionState {
                    granted: false,
                    probe_path: Some(helper_app_path.clone()),
                    error: Some(message.clone()),
                },
                PermissionState {
                    granted: false,
                    probe_path: Some(helper_app_path),
                    error: Some(message),
                },
            )
        }
    };
    let snapshot = PermissionSnapshot {
        full_disk_access: PermissionState {
            granted: check.granted,
            probe_path: check.probe_path,
            error: check.error,
        },
        screen_recording,
        accessibility,
        macos_software_update: macos_update_account_status(),
        worker_app_path: worker_app_path().to_string_lossy().to_string(),
        worker_helper_app_path: worker_helper_app_path().to_string_lossy().to_string(),
        checked_at_unix_ms: now_ms(),
    };
    debug!(snapshot = ?snapshot, "permission snapshot built");
    snapshot
}

fn open_path(path: &Path) -> Result<()> {
    let status = Command::new("/usr/bin/open")
        .arg(path)
        .status()
        .with_context(|| format!("open {}", path.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "open {} failed with {:?}",
            path.display(),
            status.code()
        ))
    }
}

#[tauri::command]
fn get_permission_snapshot() -> PermissionSnapshot {
    trace!("get_permission_snapshot command invoked");
    let snapshot = permission_snapshot();
    trace!(snapshot = ?snapshot, "get_permission_snapshot command returning");
    snapshot
}

#[tauri::command]
fn get_launch_context() -> LaunchContext {
    trace!("get_launch_context command invoked");
    let context = launch_context_from_args();
    debug!(context = ?context, "get_launch_context command returning");
    context
}

#[tauri::command]
fn approve_macos_software_update_enrollment() -> Result<MacosUpdateAccountIpcResponse, String> {
    info!("approve_macos_software_update_enrollment command invoked");
    let mut begin = macos_update_account_ipc_with_retry(
        &MacosUpdateAccountIpcRequest::BeginInteractiveEnrollment,
        MACOS_UPDATE_ACCOUNT_APPROVAL_IPC_ATTEMPTS,
        MACOS_UPDATE_ACCOUNT_IPC_RETRY_DELAY,
    )
    .map_err(|err| format_macos_update_account_ipc_error(&err))?;
    if !begin.ok || begin.session_id.is_none() {
        begin.enrollment_account = None;
        return Ok(begin);
    }
    let session_id = begin
        .session_id
        .clone()
        .ok_or_else(|| "Talos could not start Software Updates approval.".to_string())?;
    let account = begin
        .enrollment_account
        .take()
        .ok_or_else(|| "Talos could not start Software Updates approval.".to_string())?;
    info!(
        username = %account.username,
        "opening macOS Software Updates approval prompt"
    );
    let prompt_result =
        run_macos_update_account_approval_prompt(&account.username, &account.password);
    let (sysadminctl_succeeded, sysadminctl_output) = match prompt_result {
        Ok(output) => {
            info!("macOS Software Updates approval prompt completed");
            (true, truncate_for_log(&output))
        }
        Err(error) => {
            warn!(error = %error, "macOS Software Updates approval prompt failed");
            (false, truncate_for_log(&format!("{error:#}")))
        }
    };
    macos_update_account_ipc_with_retry(
        &MacosUpdateAccountIpcRequest::CompleteInteractiveEnrollment {
            session_id,
            sysadminctl_succeeded,
            sysadminctl_output,
        },
        MACOS_UPDATE_ACCOUNT_APPROVAL_IPC_ATTEMPTS,
        MACOS_UPDATE_ACCOUNT_IPC_RETRY_DELAY,
    )
    .map(|mut response| {
        response.enrollment_account = None;
        response
    })
    .map_err(|err| format_macos_update_account_ipc_error(&err))
}

#[tauri::command]
fn log_frontend_event(level: String, event: String, detail: Option<Value>) {
    let detail = detail.unwrap_or_else(|| json!({}));
    match level.to_ascii_lowercase().as_str() {
        "trace" => {
            trace!(target: "talos_permissions_helper::frontend", event = %event, detail = %detail, "frontend event")
        }
        "debug" => {
            debug!(target: "talos_permissions_helper::frontend", event = %event, detail = %detail, "frontend event")
        }
        "info" => {
            info!(target: "talos_permissions_helper::frontend", event = %event, detail = %detail, "frontend event")
        }
        "warn" => {
            warn!(target: "talos_permissions_helper::frontend", event = %event, detail = %detail, "frontend event")
        }
        "error" => {
            error!(target: "talos_permissions_helper::frontend", event = %event, detail = %detail, "frontend event")
        }
        other => {
            warn!(target: "talos_permissions_helper::frontend", level = %other, event = %event, detail = %detail, "frontend event with unknown level")
        }
    }
}

#[tauri::command]
fn reveal_worker_app(target: Option<String>) -> Result<(), String> {
    let helper_app = PathBuf::from(WORKER_HELPER_APP_PATH);
    let app = match target.as_deref() {
        Some("worker") => worker_app_path(),
        Some("helper") => helper_app,
        _ if helper_app.exists() => helper_app,
        _ => worker_app_path(),
    };
    debug!(target = ?target, app = %app.display(), app_exists = app.exists(), "reveal_worker_app command invoked");
    if app.exists() {
        let status = Command::new("/usr/bin/open")
            .arg("-R")
            .arg(&app)
            .status()
            .with_context(|| format!("reveal {}", app.display()))
            .map_err(|err| {
                warn!(error = %err, app = %app.display(), "reveal_worker_app open command failed");
                "Could not show Talos in Finder.".to_string()
            })?;
        if status.success() {
            info!(app = %app.display(), "reveal_worker_app succeeded");
            return Ok(());
        }
        warn!(app = %app.display(), status = ?status.code(), "reveal_worker_app failed");
        return Err("Could not show Talos in Finder.".to_string());
    }
    warn!(
        target = ?target,
        app = %app.display(),
        "target app missing; opening Worker directory instead"
    );
    open_path(Path::new("/Library/Talos/Worker")).map_err(|err| {
        warn!(error = %err, "reveal_worker_app fallback open failed");
        "Could not show Talos in Finder.".to_string()
    })
}

#[tauri::command]
fn request_worker_restart() -> serde_json::Value {
    info!("request_worker_restart command invoked");
    let restart_marker = fs::write(RESTART_REQUEST_PATH, now_ms().to_string()).is_ok();
    let launchctl = Command::new("/bin/launchctl")
        .args(["kickstart", "-k", "system/com.talos.talos-worker"])
        .status()
        .ok()
        .map(|status| status.success())
        .unwrap_or(false);
    let response = json!({
        "markerWritten": restart_marker,
        "launchctlRestarted": launchctl,
    });
    info!(response = %response, "request_worker_restart command completed");
    response
}

fn surface_main_window(app: &tauri::App) {
    let Some(window) = app.get_webview_window("main") else {
        error!("permissions helper main window missing during setup");
        return;
    };
    if let Err(err) = window.set_theme(Some(tauri::utils::Theme::Dark)) {
        warn!(error = %err, "permissions helper set dark theme failed");
    }
    if let Err(err) = window.set_background_color(Some(tauri::utils::config::Color(4, 9, 26, 255)))
    {
        warn!(error = %err, "permissions helper set background color failed");
    }
    #[cfg(target_os = "macos")]
    if let Err(err) = window.set_title_bar_style(tauri::TitleBarStyle::Transparent) {
        warn!(error = %err, "permissions helper set title bar style failed");
    }
    if let Err(err) = window.unminimize() {
        warn!(error = %err, "permissions helper unminimize failed");
    }
    if let Err(err) = window.center() {
        warn!(error = %err, "permissions helper center failed");
    }
    if let Err(err) = window.show() {
        warn!(error = %err, "permissions helper show failed");
    }
    if let Err(err) = window.set_focus() {
        warn!(error = %err, "permissions helper focus failed");
    }
    if let Err(err) = window.request_user_attention(Some(UserAttentionType::Informational)) {
        warn!(error = %err, "permissions helper user attention failed");
    }
}

fn main() {
    prepare_native_permission_flow_logging();
    if let Err(err) = init_file_logging() {
        eprintln!("Talos Permissions Helper file logging init failed: {err}");
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .init();
    }
    redirect_stdout_stderr_to_log();

    let login_check = std::env::args().any(|arg| arg == "--login-check");
    let after_install = std::env::args().any(|arg| arg == "--after-install");
    let launch_context = launch_context_from_args();
    let login_core_permissions_ready = login_check.then(core_permissions_ready_for_login_check);
    if login_core_permissions_ready == Some(true) {
        let macos_update_status = macos_update_account_status();
        let macos_update_ready = !macos_update_status.required
            || matches!(macos_update_status.status.as_str(), "ready" | "notRequired");
        if macos_update_ready {
            info!("Core permissions and macOS update account already ready; login-check helper exiting");
            return;
        }
    }
    if login_check
        && login_core_permissions_ready == Some(true)
        && !launch_context.macos_software_update_required
    {
        let macos_update_status = macos_update_account_status();
        if macos_update_status.status == "error"
            && macos_update_status.failure_code.as_deref() == Some("worker_unavailable")
        {
            info!("Core permissions granted and worker unavailable for macOS update status; login-check helper exiting");
            return;
        }
    }
    info!(
        pid = std::process::id(),
        login_check,
        after_install,
        reason = %launch_context.reason,
        full_disk_access_required = launch_context.full_disk_access_required,
        screen_recording_required = launch_context.screen_recording_required,
        accessibility_required = launch_context.accessibility_required,
        macos_software_update_required = launch_context.macos_software_update_required,
        "Talos Permissions Helper starting"
    );
    log_permission_flow_resource_bundle_status();

    tauri::Builder::default()
        .plugin(tauri_plugin_permission_flow::init())
        .invoke_handler(tauri::generate_handler![
            get_launch_context,
            get_permission_snapshot,
            approve_macos_software_update_enrollment,
            log_frontend_event,
            reveal_worker_app,
            request_worker_restart
        ])
        .setup(|app| {
            surface_main_window(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Talos Permissions Helper");
}
