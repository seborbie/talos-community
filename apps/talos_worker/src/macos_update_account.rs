use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::fd::AsRawFd,
    os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::Utc;
use core_foundation::base::TCFType;
use core_foundation::data::CFData;
use security_framework::os::macos::code_signing::{
    Flags as CodeSigningFlags, GuestAttributes, SecCode, SecRequirement,
};
use security_framework::os::macos::keychain::SecKeychain;
use security_framework::passwords as default_keychain_passwords;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use talos_protocol::{
    MacosUpdateAccountIpcRequest, MacosUpdateAccountIpcResponse, MacosUpdateAccountStatus,
    MacosUpdateAccountStatusPayload, MacosUpdateEnrollmentAccount, MacosVolumeOwnerUser,
    MACOS_UPDATE_ACCOUNT_SOCKET_PATH,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};
use uuid::Uuid;

const STATE_DIR: &str = "/Library/Application Support/Talos";
const STATE_PATH: &str = "/Library/Application Support/Talos/macos-update-account-state.json";
const STATE_LOCK_PATH: &str = "/Library/Application Support/Talos/macos-update-account-state.lock";
const SOCKET_DIR: &str = "/var/run/talos";
const KEYCHAIN_SERVICE: &str = "com.talos.macos-software-update";
const SYSTEM_KEYCHAIN_PATH: &str = "/Library/Keychains/System.keychain";
const DEFAULT_USERNAME: &str = "talos";
const FULL_NAME: &str = "Talos Software Update";
const USER_HOME: &str = "/var/empty";
const USER_SHELL: &str = "/usr/bin/false";
const HELPER_BUNDLE_IDENTIFIER: &str = "com.talos.permissions-helper";
const HELPER_TEAM_IDENTIFIER_ENV: &str = "TALOS_MACOS_UPDATE_ACCOUNT_HELPER_TEAM_ID";
const ENROLLMENT_SESSION_TTL: Duration = Duration::from_secs(10 * 60);
const SECURE_TOKEN_VERIFY_TIMEOUT: Duration = Duration::from_secs(5);
const VOLUME_OWNER_VERIFY_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone)]
pub struct MacosUpdateInstallCredential {
    pub username: String,
    pub password: String,
    pub status: MacosUpdateAccountStatus,
}

#[derive(Debug, Clone)]
pub struct MacosUpdateAccountFailure {
    pub code: String,
    pub message: String,
    pub status: MacosUpdateAccountStatus,
}

impl std::fmt::Display for MacosUpdateAccountFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for MacosUpdateAccountFailure {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountState {
    username: String,
    credential_version: i32,
    generated_uid: Option<String>,
    created_at: String,
    updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_failure_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_failure_message: Option<String>,
}

#[derive(Debug, Clone)]
struct EnrollmentSession {
    created_at: Instant,
}

#[derive(Clone)]
struct StatusReporter {
    agent_id: String,
    outbound_tx: mpsc::UnboundedSender<Message>,
}

#[derive(Serialize)]
struct OutgoingEnvelope<T>
where
    T: Serialize,
{
    #[serde(rename = "type")]
    message_type: &'static str,
    data: T,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CodeSignatureDetail {
    identifier: Option<String>,
    team_identifier: Option<String>,
    leaf_certificate_hash: Option<String>,
    authorities: Vec<String>,
}

static ENROLLMENT_SESSIONS: OnceLock<Mutex<HashMap<String, EnrollmentSession>>> = OnceLock::new();
static EXPECTED_WORKER_SIGNATURE_DETAIL: OnceLock<
    std::result::Result<CodeSignatureDetail, String>,
> = OnceLock::new();
static STATUS_REPORTER: OnceLock<Mutex<Option<StatusReporter>>> = OnceLock::new();

pub fn start_ipc_server() {
    tokio::spawn(async {
        if let Err(error) = run_ipc_server().await {
            warn!(%error, "macOS update account IPC server stopped");
        }
    });
}

pub fn configure_status_reporter(agent_id: String, outbound_tx: mpsc::UnboundedSender<Message>) {
    let reporter = StatusReporter {
        agent_id,
        outbound_tx,
    };
    let cell = STATUS_REPORTER.get_or_init(|| Mutex::new(None));
    match cell.lock() {
        Ok(mut guard) => *guard = Some(reporter),
        Err(_) => warn!("macOS update account status reporter mutex poisoned"),
    }
}

pub fn ensure_startup_status() -> MacosUpdateAccountStatus {
    match ensure_account_and_status() {
        Ok(status) => status,
        Err(error) => error_status("macos_update_account_error", &format!("{error:#}")),
    }
}

pub fn current_status() -> MacosUpdateAccountStatus {
    match build_status(false) {
        Ok(status) => status,
        Err(error) => error_status("macos_update_account_error", &format!("{error:#}")),
    }
}

pub fn credential_for_softwareupdate(
) -> std::result::Result<Option<MacosUpdateInstallCredential>, Box<MacosUpdateAccountFailure>> {
    if !is_apple_silicon() {
        return Ok(None);
    }
    let status = ensure_startup_status();
    if status.status != "ready" {
        let code = status
            .failure_code
            .clone()
            .unwrap_or_else(|| "macos_update_account_needs_enrollment".to_string());
        let message = status.failure_message.clone().unwrap_or_else(|| {
            "Talos macOS software update account is not ready. Open Talos Permissions Helper and complete Software Updates enrollment.".to_string()
        });
        return Err(Box::new(MacosUpdateAccountFailure {
            code,
            message,
            status,
        }));
    }
    let password = get_managed_password().map_err(|error| {
        let mut status = current_status();
        status.status = "error".to_string();
        status.failure_code = Some("macos_update_account_credential_missing".to_string());
        status.failure_message = Some(format!("Unable to read local Talos update credential: {error:#}"));
        Box::new(MacosUpdateAccountFailure {
            code: "macos_update_account_credential_missing".to_string(),
            message: "Unable to read the local Talos update credential. Open Talos Permissions Helper and recreate Software Updates enrollment.".to_string(),
            status,
        })
    })?;
    Ok(Some(MacosUpdateInstallCredential {
        username: DEFAULT_USERNAME.to_string(),
        password,
        status,
    }))
}

async fn run_ipc_server() -> Result<()> {
    prepare_socket_path()?;
    let listener = UnixListener::bind(MACOS_UPDATE_ACCOUNT_SOCKET_PATH)
        .with_context(|| format!("bind {MACOS_UPDATE_ACCOUNT_SOCKET_PATH}"))?;
    fs::set_permissions(
        MACOS_UPDATE_ACCOUNT_SOCKET_PATH,
        fs::Permissions::from_mode(0o666),
    )
    .ok();
    info!(
        path = MACOS_UPDATE_ACCOUNT_SOCKET_PATH,
        "macOS update account IPC server listening"
    );
    loop {
        let (stream, _) = listener.accept().await.context("accept IPC client")?;
        tokio::spawn(async move {
            if let Err(error) = handle_ipc_stream(stream).await {
                warn!(%error, "macOS update account IPC request failed");
            }
        });
    }
}

fn prepare_socket_path() -> Result<()> {
    fs::create_dir_all(SOCKET_DIR).with_context(|| format!("create {SOCKET_DIR}"))?;
    fs::set_permissions(SOCKET_DIR, fs::Permissions::from_mode(0o755)).ok();
    match fs::symlink_metadata(MACOS_UPDATE_ACCOUNT_SOCKET_PATH) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            fs::remove_file(MACOS_UPDATE_ACCOUNT_SOCKET_PATH)
                .with_context(|| format!("remove stale {MACOS_UPDATE_ACCOUNT_SOCKET_PATH}"))?;
        }
        Ok(_) => bail!("{MACOS_UPDATE_ACCOUNT_SOCKET_PATH} exists and is not a Unix socket"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect macOS update account socket path"),
    }
    Ok(())
}

async fn handle_ipc_stream(mut stream: UnixStream) -> Result<()> {
    if let Err(error) = validate_ipc_peer(&stream).context("validate macOS update account IPC peer")
    {
        let detail = truncate_for_detail(&format!("{error:#}"));
        let message = format!(
            "Talos Worker rejected the Permissions Helper IPC caller ({detail}). Reinstall the latest signed Talos Permissions Helper with the worker, then reopen it."
        );
        let _ = write_ipc_response(
            &mut stream,
            MacosUpdateAccountIpcResponse {
                ok: false,
                status: Some(error_status(
                    "macos_update_account_ipc_unauthorized",
                    &message,
                )),
                session_id: None,
                error_code: Some("macos_update_account_ipc_unauthorized".to_string()),
                error_message: Some(message),
                enrollment_account: None,
            },
        )
        .await;
        return Err(error);
    }
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .await
        .context("read macOS update account IPC request")?;
    if read == 0 {
        return Ok(());
    }
    if line.len() > 8192 {
        write_ipc_response(
            reader.get_mut(),
            MacosUpdateAccountIpcResponse {
                ok: false,
                status: None,
                session_id: None,
                error_code: Some("request_too_large".to_string()),
                error_message: Some("Request is too large".to_string()),
                enrollment_account: None,
            },
        )
        .await?;
        return Ok(());
    }
    let request: MacosUpdateAccountIpcRequest =
        serde_json::from_str(line.trim()).context("parse macOS update account IPC request")?;
    let response = match request {
        MacosUpdateAccountIpcRequest::GetStatus => MacosUpdateAccountIpcResponse {
            ok: true,
            status: Some(ensure_startup_status()),
            session_id: None,
            error_code: None,
            error_message: None,
            enrollment_account: None,
        },
        MacosUpdateAccountIpcRequest::BeginInteractiveEnrollment => {
            begin_interactive_enrollment_response()
        }
        MacosUpdateAccountIpcRequest::CompleteInteractiveEnrollment {
            session_id,
            sysadminctl_succeeded,
            sysadminctl_output,
        } => complete_interactive_enrollment_response(
            &session_id,
            sysadminctl_succeeded,
            &sysadminctl_output,
        ),
    };
    write_ipc_response(reader.get_mut(), response).await
}

async fn write_ipc_response(
    stream: &mut UnixStream,
    response: MacosUpdateAccountIpcResponse,
) -> Result<()> {
    let mut bytes = serde_json::to_vec(&response).context("serialize IPC response")?;
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .await
        .context("write macOS update account IPC response")
}

fn validate_ipc_peer(stream: &UnixStream) -> Result<()> {
    let fd = stream.as_raw_fd();
    let pid = peer_pid(fd)?;
    match peer_audit_token(fd) {
        Ok(token) => match validate_helper_audit_signature(&token) {
            Ok(()) => {
                debug!(
                    pid,
                    "validated macOS update account IPC peer by audit token code signature"
                );
                return Ok(());
            }
            Err(error) => {
                warn!(
                    pid,
                    error = %error,
                    "macOS update account IPC peer audit-token validation failed; falling back to pid validation"
                );
            }
        },
        Err(error) => {
            warn!(
                pid,
                error = %error,
                "unable to read macOS update account IPC peer audit token; falling back to pid validation"
            );
        }
    }
    match process_path(pid) {
        Ok(path) => {
            validate_helper_signature(&path).with_context(|| {
                format!(
                    "IPC peer pid {pid} at {} failed Permissions Helper validation",
                    path.display()
                )
            })?;
            debug!(pid, path = %path.display(), "validated macOS update account IPC peer");
        }
        Err(path_error) => {
            validate_helper_process_signature(pid).with_context(|| {
                format!(
                    "IPC peer pid {pid} failed dynamic Permissions Helper validation after process path lookup failed: {path_error:#}"
                )
            })?;
            debug!(
                pid,
                error = %path_error,
                "validated macOS update account IPC peer by dynamic code signature after process path lookup failed"
            );
        }
    }
    Ok(())
}

fn peer_pid(fd: i32) -> Result<i32> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            &mut pid as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error()).context("read IPC peer pid");
    }
    Ok(pid)
}

fn peer_audit_token(fd: i32) -> Result<CFData> {
    let mut token = [0u32; 8];
    let mut len = std::mem::size_of_val(&token) as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            token.as_mut_ptr() as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error()).context("read IPC peer audit token");
    }
    let expected_len = std::mem::size_of_val(&token) as libc::socklen_t;
    if len != expected_len {
        bail!("read IPC peer audit token returned {len} bytes, expected {expected_len}");
    }
    let bytes = unsafe {
        std::slice::from_raw_parts(token.as_ptr() as *const u8, std::mem::size_of_val(&token))
    };
    Ok(CFData::from_buffer(bytes))
}

fn process_path(pid: i32) -> Result<PathBuf> {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let len = unsafe {
        libc::proc_pidpath(
            pid,
            buffer.as_mut_ptr() as *mut libc::c_void,
            buffer.len() as u32,
        )
    };
    if len <= 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("resolve process path for pid {pid}"));
    }
    buffer.truncate(len as usize);
    Ok(PathBuf::from(String::from_utf8_lossy(&buffer).to_string()))
}

fn validate_helper_signature(path: &Path) -> Result<()> {
    if std::env::var("TALOS_MACOS_UPDATE_ACCOUNT_ALLOW_UNSIGNED_HELPER")
        .ok()
        .map(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
    {
        return Ok(());
    }
    if !codesign_signature_is_valid(path)? {
        bail!("code signature verification failed for {}", path.display());
    }
    let helper_detail = codesign_detail(path)?;
    let worker_detail = expected_worker_signature_detail()?;
    if helper_signature_detail_matches(&helper_detail, &worker_detail) {
        return Ok(());
    }

    let detail = helper_signature_mismatch_detail(path, &helper_detail, &worker_detail);
    warn!(detail = %detail, "Talos Permissions Helper signature did not match expected worker signing identity");
    bail!("signature mismatch: {detail}");
}

fn validate_helper_audit_signature(token: &CFData) -> Result<()> {
    if std::env::var("TALOS_MACOS_UPDATE_ACCOUNT_ALLOW_UNSIGNED_HELPER")
        .ok()
        .map(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
    {
        return Ok(());
    }
    let worker_detail = expected_worker_signature_detail()?;
    let requirement_text = helper_process_requirement(&worker_detail)?;
    let requirement: SecRequirement = requirement_text.parse().with_context(|| {
        format!("compile helper audit-token code requirement {requirement_text}")
    })?;
    let mut attrs = GuestAttributes::new();
    attrs.set_audit_token(token.as_concrete_TypeRef());
    let code = SecCode::copy_guest_with_attribues(None, &attrs, CodeSigningFlags::NONE)
        .context("copy helper audit-token code object")?;
    code.check_validity(CodeSigningFlags::STRICT_VALIDATE, &requirement)
        .context("validate helper audit-token code requirement")
}

fn validate_helper_process_signature(pid: i32) -> Result<()> {
    if std::env::var("TALOS_MACOS_UPDATE_ACCOUNT_ALLOW_UNSIGNED_HELPER")
        .ok()
        .map(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
    {
        return Ok(());
    }
    let worker_detail = expected_worker_signature_detail()?;
    let requirement_text = helper_process_requirement(&worker_detail)?;
    let requirement: SecRequirement = requirement_text
        .parse()
        .with_context(|| format!("compile helper process code requirement {requirement_text}"))?;
    let mut attrs = GuestAttributes::new();
    attrs.set_pid(pid);
    let code = SecCode::copy_guest_with_attribues(None, &attrs, CodeSigningFlags::NONE)
        .with_context(|| format!("copy helper process code object for pid {pid}"))?;
    code.check_validity(CodeSigningFlags::STRICT_VALIDATE, &requirement)
        .with_context(|| format!("validate helper process code requirement for pid {pid}"))
}

fn helper_process_requirement(worker_detail: &CodeSignatureDetail) -> Result<String> {
    let identifier = requirement_identifier(HELPER_BUNDLE_IDENTIFIER)?;
    if let Some(hash) = worker_detail
        .leaf_certificate_hash
        .as_deref()
        .map(str::trim)
        .filter(|hash| !hash.is_empty())
    {
        let hash = requirement_leaf_hash(hash)?;
        return Ok(format!(
            "identifier \"{identifier}\" and certificate leaf = H\"{hash}\""
        ));
    }
    if let Some(team) = worker_detail
        .team_identifier
        .as_deref()
        .map(str::trim)
        .filter(|team| !team.is_empty())
    {
        let team = requirement_team_identifier(team)?;
        return Ok(format!(
            "identifier \"{identifier}\" and certificate leaf[subject.OU] = \"{team}\""
        ));
    }
    bail!("worker signing identity has no TeamIdentifier or leaf certificate hash")
}

fn requirement_identifier(value: &str) -> Result<String> {
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
    {
        bail!("invalid helper code requirement identifier");
    }
    Ok(value.to_string())
}

fn requirement_leaf_hash(value: &str) -> Result<String> {
    let hash = value.to_ascii_lowercase();
    if hash.len() < 40 || !hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
        bail!("invalid helper code requirement certificate hash");
    }
    Ok(hash)
}

fn requirement_team_identifier(value: &str) -> Result<String> {
    if value.is_empty() || !value.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        bail!("invalid helper code requirement team identifier");
    }
    Ok(value.to_string())
}

fn codesign_signature_is_valid(path: &Path) -> Result<bool> {
    let output = Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", "--all-architectures", "--verbose=2"])
        .arg(path)
        .output()
        .with_context(|| format!("verify code signature for {}", path.display()))?;
    if output.status.success() {
        return Ok(true);
    }
    let detail = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    debug!(
        path = %path.display(),
        detail = %truncate_for_detail(&detail),
        "Talos Permissions Helper code signature verification failed"
    );
    Ok(false)
}

fn expected_worker_signature_detail() -> Result<CodeSignatureDetail> {
    match EXPECTED_WORKER_SIGNATURE_DETAIL.get_or_init(resolve_expected_worker_signature_detail) {
        Ok(detail) => Ok(detail.clone()),
        Err(error) => Err(anyhow!(error.clone())),
    }
}

fn resolve_expected_worker_signature_detail() -> std::result::Result<CodeSignatureDetail, String> {
    let configured_team = std::env::var(HELPER_TEAM_IDENTIFIER_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("not set"));
    let worker_path = std::env::current_exe()
        .map_err(|error| format!("resolve Talos Worker executable path: {error}"))?;
    let mut worker_detail = codesign_detail(&worker_path).map_err(|error| format!("{error:#}"))?;
    if let Some(team) = configured_team {
        worker_detail.team_identifier = Some(team);
    }
    Ok(worker_detail)
}

fn helper_signature_detail_matches(
    helper_detail: &CodeSignatureDetail,
    worker_detail: &CodeSignatureDetail,
) -> bool {
    if helper_detail.identifier.as_deref() != Some(HELPER_BUNDLE_IDENTIFIER) {
        return false;
    }
    if matches!(
        (
            helper_detail.leaf_certificate_hash.as_deref(),
            worker_detail.leaf_certificate_hash.as_deref()
        ),
        (Some(helper_hash), Some(worker_hash)) if helper_hash.eq_ignore_ascii_case(worker_hash)
    ) {
        return true;
    }
    if let Some(expected_team) = worker_detail.team_identifier.as_deref() {
        return helper_detail.team_identifier.as_deref() == Some(expected_team);
    }
    false
}

fn helper_signature_mismatch_detail(
    path: &Path,
    helper_detail: &CodeSignatureDetail,
    worker_detail: &CodeSignatureDetail,
) -> String {
    truncate_for_detail(&format!(
        "path={}, helper_identifier={}, helper_team={}, helper_leaf={}, helper_authorities={}, worker_team={}, worker_leaf={}, worker_authorities={}",
        path.display(),
        helper_detail.identifier.as_deref().unwrap_or("none"),
        helper_detail.team_identifier.as_deref().unwrap_or("none"),
        short_hash(helper_detail.leaf_certificate_hash.as_deref()),
        authorities_summary(&helper_detail.authorities),
        worker_detail.team_identifier.as_deref().unwrap_or("none"),
        short_hash(worker_detail.leaf_certificate_hash.as_deref()),
        authorities_summary(&worker_detail.authorities),
    ))
}

fn short_hash(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "none".to_string();
    };
    if value.len() <= 16 {
        return value.to_string();
    }
    format!("{}...{}", &value[..8], &value[value.len() - 8..])
}

fn authorities_summary(authorities: &[String]) -> String {
    if authorities.is_empty() {
        return "none".to_string();
    }
    truncate_for_detail(&authorities.join(" > "))
}

fn codesign_detail(path: &Path) -> Result<CodeSignatureDetail> {
    let output = Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=4"])
        .arg(path)
        .output()
        .with_context(|| format!("codesign {}", path.display()))?;
    let detail = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        bail!(
            "codesign {} failed: {}",
            path.display(),
            truncate_for_detail(&detail)
        );
    }
    let mut parsed = parse_codesign_detail(&detail);
    parsed.leaf_certificate_hash = codesign_leaf_certificate_hash(path)?;
    Ok(parsed)
}

fn codesign_leaf_certificate_hash(path: &Path) -> Result<Option<String>> {
    let output = Command::new("/usr/bin/codesign")
        .args(["-d", "-r-"])
        .arg(path)
        .output()
        .with_context(|| format!("codesign requirement {}", path.display()))?;
    let detail = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        bail!(
            "codesign requirement {} failed: {}",
            path.display(),
            truncate_for_detail(&detail)
        );
    }
    Ok(parse_leaf_certificate_hash(&detail))
}

fn parse_leaf_certificate_hash(requirement: &str) -> Option<String> {
    let marker = "certificate leaf = H\"";
    let start = requirement.find(marker)? + marker.len();
    let rest = &requirement[start..];
    let end = rest.find('"')?;
    let hash = rest[..end].trim();
    if hash.is_empty() {
        None
    } else {
        Some(hash.to_ascii_lowercase())
    }
}

fn parse_codesign_detail(detail: &str) -> CodeSignatureDetail {
    let mut parsed = CodeSignatureDetail::default();
    for line in detail.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("Identifier=") {
            parsed.identifier = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("TeamIdentifier=") {
            let value = value.trim();
            if !value.is_empty() && !value.eq_ignore_ascii_case("not set") {
                parsed.team_identifier = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Authority=") {
            let value = value.trim();
            if !value.is_empty() {
                parsed.authorities.push(value.to_string());
            }
        }
    }
    parsed
}

fn begin_interactive_enrollment_response() -> MacosUpdateAccountIpcResponse {
    let result = begin_interactive_enrollment();
    match result {
        Ok((status, session_id, enrollment_account)) => MacosUpdateAccountIpcResponse {
            ok: true,
            status: Some(status),
            session_id,
            error_code: None,
            error_message: None,
            enrollment_account,
        },
        Err(error) => MacosUpdateAccountIpcResponse {
            ok: false,
            status: Some(error_status(
                "macos_update_account_error",
                &format!("{error:#}"),
            )),
            session_id: None,
            error_code: Some("macos_update_account_error".to_string()),
            error_message: Some(format!("{error:#}")),
            enrollment_account: None,
        },
    }
}

fn complete_interactive_enrollment_response(
    session_id: &str,
    sysadminctl_succeeded: bool,
    sysadminctl_output: &str,
) -> MacosUpdateAccountIpcResponse {
    let result =
        complete_interactive_enrollment(session_id, sysadminctl_succeeded, sysadminctl_output);
    match result {
        Ok(status) => {
            queue_status_report(status.clone());
            MacosUpdateAccountIpcResponse {
                ok: status.status == "ready",
                status: Some(status.clone()),
                session_id: None,
                error_code: status.failure_code,
                error_message: status.failure_message,
                enrollment_account: None,
            }
        }
        Err(error) => {
            let message = format!("{error:#}");
            let _ = write_state_failure(
                Some("macos_update_account_enrollment_failed".to_string()),
                Some(message.clone()),
            );
            let status = current_status();
            queue_status_report(status.clone());
            MacosUpdateAccountIpcResponse {
                ok: false,
                status: Some(status),
                session_id: None,
                error_code: Some("macos_update_account_enrollment_failed".to_string()),
                error_message: Some(message),
                enrollment_account: None,
            }
        }
    }
}

fn queue_status_report(status: MacosUpdateAccountStatus) {
    let Some(reporter) = STATUS_REPORTER
        .get()
        .and_then(|cell| cell.lock().ok().and_then(|guard| guard.clone()))
    else {
        return;
    };
    let envelope = OutgoingEnvelope {
        message_type: "macos_update_account_status",
        data: MacosUpdateAccountStatusPayload {
            agent_id: reporter.agent_id,
            status,
        },
    };
    match serde_json::to_string(&envelope) {
        Ok(text) => {
            if reporter.outbound_tx.send(Message::Text(text)).is_err() {
                warn!("failed to queue macOS update account status after enrollment");
            }
        }
        Err(error) => warn!(%error, "failed to serialize macOS update account status"),
    }
}

fn begin_interactive_enrollment() -> Result<(
    MacosUpdateAccountStatus,
    Option<String>,
    Option<MacosUpdateEnrollmentAccount>,
)> {
    let _lock = acquire_state_lock()?;
    ensure_account_state_locked()?;
    let status = build_status_locked()?;
    if !status.required || status.status == "ready" {
        return Ok((status, None, None));
    }

    let previous_state = read_state()?;
    recreate_managed_account_locked(
        previous_state.as_ref(),
        "Talos recreated the local update account before opening the macOS Software Updates approval prompt.",
    )
    .context("prepare Talos update account for macOS interactive approval")?;
    let password = get_managed_password()
        .context("read generated Talos update password for approval prompt")?;
    let status = build_status_locked()?;
    let session_id = Uuid::new_v4().to_string();
    enrollment_sessions()
        .lock()
        .expect("enrollment session mutex poisoned")
        .insert(
            session_id.clone(),
            EnrollmentSession {
                created_at: Instant::now(),
            },
        );
    Ok((
        status,
        Some(session_id),
        Some(MacosUpdateEnrollmentAccount {
            username: DEFAULT_USERNAME.to_string(),
            password,
        }),
    ))
}

fn complete_interactive_enrollment(
    session_id: &str,
    sysadminctl_succeeded: bool,
    sysadminctl_output: &str,
) -> Result<MacosUpdateAccountStatus> {
    validate_enrollment_session(session_id)?;
    let _lock = acquire_state_lock()?;
    ensure_account_state_locked()?;
    if !sysadminctl_succeeded {
        bail!(
            "macOS Software Updates approval failed: {}",
            truncate_for_detail(sysadminctl_output)
        );
    }
    wait_for_secure_token(DEFAULT_USERNAME, sysadminctl_output)
        .context("verify Talos update account Secure Token")?;
    wait_for_volume_owner(DEFAULT_USERNAME).context("verify Talos update account volume owner")?;
    refresh_apfs_preboot_records();
    write_state_failure(None, None)?;
    build_status_locked()
}

fn validate_enrollment_session(session_id: &str) -> Result<()> {
    let mut sessions = enrollment_sessions()
        .lock()
        .expect("enrollment session mutex poisoned");
    sessions.retain(|_, session| session.created_at.elapsed() < ENROLLMENT_SESSION_TTL);
    match sessions.remove(session_id) {
        Some(session) if session.created_at.elapsed() < ENROLLMENT_SESSION_TTL => Ok(()),
        _ => {
            bail!("enrollment session expired; reopen Software Updates in Talos Permissions Helper")
        }
    }
}

fn enrollment_sessions() -> &'static Mutex<HashMap<String, EnrollmentSession>> {
    ENROLLMENT_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ensure_account_and_status() -> Result<MacosUpdateAccountStatus> {
    let _lock = acquire_state_lock()?;
    ensure_account_state_locked()?;
    build_status_locked()
}

fn ensure_account_state_locked() -> Result<()> {
    if !is_apple_silicon() {
        return Ok(());
    }
    let state = read_state()?;
    let user_exists = user_exists(DEFAULT_USERNAME)?;
    let needs_create = !user_exists;
    if state.is_none() && user_exists {
        let generated_uid = generated_uid(DEFAULT_USERNAME).ok().flatten();
        let state = AccountState {
            username: DEFAULT_USERNAME.to_string(),
            credential_version: 1,
            generated_uid,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
            last_failure_code: Some("macos_update_account_unmanaged_existing_user".to_string()),
            last_failure_message: Some(
                "A talos macOS account already exists but Talos has no local credential for it. Delete the account or recreate Software Updates enrollment.".to_string(),
            ),
        };
        write_state(&state)?;
        return Ok(());
    }
    if needs_create {
        create_managed_account_locked(
            state.as_ref(),
            "Talos created the local update account. Open Talos Permissions Helper and complete Software Updates enrollment.",
        )?;
    } else if state.is_some() && get_managed_password().is_err() {
        if migrate_default_keychain_password_to_system_keychain()?.is_none() {
            recreate_managed_account_locked(
                state.as_ref(),
                "Talos recreated the local update account because its local credential was missing. Complete Software Updates enrollment again.",
            )?;
        } else {
            harden_standard_user(DEFAULT_USERNAME)?;
        }
    } else {
        harden_standard_user(DEFAULT_USERNAME)?;
    }
    Ok(())
}

fn build_status(ensure: bool) -> Result<MacosUpdateAccountStatus> {
    let _lock = acquire_state_lock()?;
    if ensure {
        ensure_account_state_locked()?;
    }
    build_status_locked()
}

fn build_status_locked() -> Result<MacosUpdateAccountStatus> {
    let required = is_apple_silicon();
    let owners = discover_volume_owners().unwrap_or_default();
    let state = read_state()?;
    let account_present = user_exists(DEFAULT_USERNAME).unwrap_or(false);
    let generated_uid = generated_uid(DEFAULT_USERNAME).ok().flatten();
    let expected_generated_uid = state.as_ref().and_then(|state| state.generated_uid.clone());
    let credential_available = get_managed_password().is_ok();
    let secure_token_enabled = secure_token_enabled(DEFAULT_USERNAME).unwrap_or(false);
    let is_admin = user_is_admin(DEFAULT_USERNAME).unwrap_or(false);
    let is_volume_owner = if account_present {
        user_is_volume_owner(DEFAULT_USERNAME, &owners).unwrap_or(false)
    } else {
        false
    };
    let (status, failure_code, failure_message) = if !required {
        ("notRequired", None, None)
    } else if !account_present {
        (
            "missing",
            Some("macos_update_account_missing".to_string()),
            Some(
                "The Talos macOS update account is missing. Open Talos Permissions Helper to recreate Software Updates enrollment.".to_string(),
            ),
        )
    } else if expected_generated_uid.is_some()
        && generated_uid.is_some()
        && expected_generated_uid != generated_uid
    {
        (
            "missing",
            Some("macos_update_account_recreated".to_string()),
            Some(
                "The Talos macOS update account was recreated outside Talos. Open Talos Permissions Helper to recreate Software Updates enrollment.".to_string(),
            ),
        )
    } else if is_admin {
        (
            "error",
            Some("macos_update_account_admin".to_string()),
            Some(
                "The Talos macOS update account has admin rights. Talos requires it to remain a standard user.".to_string(),
            ),
        )
    } else if !credential_available {
        (
            "error",
            Some("macos_update_account_credential_missing".to_string()),
            Some(
                "The local Talos macOS update credential is missing. Open Talos Permissions Helper to recreate Software Updates enrollment.".to_string(),
            ),
        )
    } else if !secure_token_enabled || !is_volume_owner {
        let default_code = "macos_update_account_needs_enrollment".to_string();
        let default_message = "Talos needs a volume-owner approval before macOS software updates can be installed. Open Talos Permissions Helper and complete Software Updates enrollment.".to_string();
        (
            "needsEnrollment",
            state
                .as_ref()
                .and_then(|state| state.last_failure_code.clone())
                .or(Some(default_code)),
            state
                .as_ref()
                .and_then(|state| state.last_failure_message.clone())
                .or(Some(default_message)),
        )
    } else {
        ("ready", None, None)
    };

    Ok(MacosUpdateAccountStatus {
        schema_version: 1,
        required,
        status: status.to_string(),
        username: DEFAULT_USERNAME.to_string(),
        is_apple_silicon: required,
        account_present,
        is_admin,
        is_volume_owner,
        secure_token_enabled,
        credential_available,
        credential_version: state.as_ref().map(|state| state.credential_version),
        generated_uid,
        expected_generated_uid,
        discovered_volume_owners: owners,
        failure_code,
        failure_message,
        checked_at: Utc::now().to_rfc3339(),
    })
}

fn error_status(code: &str, message: &str) -> MacosUpdateAccountStatus {
    MacosUpdateAccountStatus {
        schema_version: 1,
        required: is_apple_silicon(),
        status: "error".to_string(),
        username: DEFAULT_USERNAME.to_string(),
        is_apple_silicon: is_apple_silicon(),
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
        checked_at: Utc::now().to_rfc3339(),
    }
}

fn read_state() -> Result<Option<AccountState>> {
    let path = Path::new(STATE_PATH);
    if !path.is_file() {
        return Ok(None);
    }
    let data = fs::read_to_string(path).with_context(|| format!("read {STATE_PATH}"))?;
    let state = serde_json::from_str(&data).with_context(|| format!("parse {STATE_PATH}"))?;
    Ok(Some(state))
}

fn write_state(state: &AccountState) -> Result<()> {
    fs::create_dir_all(STATE_DIR).with_context(|| format!("create {STATE_DIR}"))?;
    fs::set_permissions(STATE_DIR, fs::Permissions::from_mode(0o700)).ok();
    let tmp_path = format!("{STATE_PATH}.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&tmp_path)
        .with_context(|| format!("open {tmp_path}"))?;
    let data = serde_json::to_vec_pretty(state).context("serialize macOS update account state")?;
    file.write_all(&data)
        .with_context(|| format!("write {tmp_path}"))?;
    file.sync_all().ok();
    fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600)).ok();
    fs::rename(&tmp_path, STATE_PATH).with_context(|| format!("replace {STATE_PATH}"))?;
    Ok(())
}

fn write_state_failure(code: Option<String>, message: Option<String>) -> Result<()> {
    let Some(mut state) = read_state()? else {
        return Ok(());
    };
    state.last_failure_code = code;
    state.last_failure_message = message;
    state.updated_at = Utc::now().to_rfc3339();
    write_state(&state)
}

fn acquire_state_lock() -> Result<File> {
    fs::create_dir_all(STATE_DIR).with_context(|| format!("create {STATE_DIR}"))?;
    fs::set_permissions(STATE_DIR, fs::Permissions::from_mode(0o700)).ok();
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(STATE_LOCK_PATH)
        .with_context(|| format!("open {STATE_LOCK_PATH}"))?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("lock {STATE_LOCK_PATH}"));
    }
    Ok(file)
}

fn system_keychain() -> Result<SecKeychain> {
    SecKeychain::open(SYSTEM_KEYCHAIN_PATH)
        .with_context(|| format!("open System Keychain at {SYSTEM_KEYCHAIN_PATH}"))
}

fn store_managed_password(password: &str) -> Result<()> {
    system_keychain()?
        .set_generic_password(KEYCHAIN_SERVICE, DEFAULT_USERNAME, password.as_bytes())
        .context("store Talos macOS update password in System Keychain")
}

fn get_managed_password() -> Result<String> {
    let (password, _) = system_keychain()?
        .find_generic_password(KEYCHAIN_SERVICE, DEFAULT_USERNAME)
        .context("read Talos macOS update password from System Keychain")?;
    String::from_utf8(password.as_ref().to_vec()).context("decode Talos macOS update password")
}

fn migrate_default_keychain_password_to_system_keychain() -> Result<Option<String>> {
    let bytes = match default_keychain_passwords::get_generic_password(
        KEYCHAIN_SERVICE,
        DEFAULT_USERNAME,
    ) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
    let password = String::from_utf8(bytes).context("decode legacy Talos macOS update password")?;
    store_managed_password(&password)
        .context("migrate Talos macOS update password to System Keychain")?;
    Ok(Some(password))
}

fn generate_password() -> Result<String> {
    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")
        .context("open /dev/urandom")?
        .read_exact(&mut bytes)
        .context("read random password bytes")?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn is_apple_silicon() -> bool {
    std::env::consts::ARCH == "aarch64"
        || run_output("/usr/bin/uname", &["-m"])
            .ok()
            .map(|value| value.trim() == "arm64")
            .unwrap_or(false)
}

fn create_managed_account_locked(
    previous_state: Option<&AccountState>,
    failure_message: &str,
) -> Result<()> {
    let password = generate_password()?;
    create_standard_user(DEFAULT_USERNAME, &password)?;
    if let Err(error) = store_managed_password(&password) {
        let _ = delete_standard_user(DEFAULT_USERNAME);
        return Err(error);
    }
    write_managed_account_state(previous_state, failure_message)
}

fn recreate_managed_account_locked(
    previous_state: Option<&AccountState>,
    failure_message: &str,
) -> Result<()> {
    let _ = system_keychain()?;
    delete_standard_user(DEFAULT_USERNAME)?;
    create_managed_account_locked(previous_state, failure_message)
}

fn write_managed_account_state(
    previous_state: Option<&AccountState>,
    failure_message: &str,
) -> Result<()> {
    let generated_uid = generated_uid(DEFAULT_USERNAME).ok().flatten();
    let now = Utc::now().to_rfc3339();
    let version = next_credential_version(previous_state);
    write_state(&AccountState {
        username: DEFAULT_USERNAME.to_string(),
        credential_version: version,
        generated_uid,
        created_at: previous_state
            .map(|state| state.created_at.clone())
            .unwrap_or_else(|| now.clone()),
        updated_at: now,
        last_failure_code: Some("macos_update_account_needs_enrollment".to_string()),
        last_failure_message: Some(failure_message.to_string()),
    })
}

fn next_credential_version(previous_state: Option<&AccountState>) -> i32 {
    previous_state
        .map(|state| state.credential_version.saturating_add(1))
        .unwrap_or(1)
}

fn create_standard_user(username: &str, password: &str) -> Result<()> {
    validate_username(username)?;
    let args = [
        "-addUser",
        username,
        "-fullName",
        FULL_NAME,
        "-password",
        "-",
        "-home",
        USER_HOME,
        "-shell",
        USER_SHELL,
    ];
    run_command_with_stdin(
        "/usr/sbin/sysadminctl",
        &args,
        &format!("{password}\n"),
        "create Talos macOS update account",
    )?;
    if !user_exists(username)? {
        bail!("Talos macOS update account was not created");
    }
    harden_standard_user(username)
}

fn delete_standard_user(username: &str) -> Result<()> {
    validate_username(username)?;
    if !user_exists(username)? {
        return Ok(());
    }
    run_command_with_stdin(
        "/usr/sbin/sysadminctl",
        &["-deleteUser", username],
        "",
        "delete Talos macOS update account",
    )?;
    if user_exists(username)? {
        bail!("Talos macOS update account was not deleted");
    }
    Ok(())
}

fn harden_standard_user(username: &str) -> Result<()> {
    validate_username(username)?;
    let path = user_path(username);
    let _ = run_status(
        "/usr/bin/dscl",
        &[".", "-create", path.as_str(), "IsHidden", "1"],
    );
    let _ = run_status(
        "/usr/bin/dscl",
        &[".", "-create", path.as_str(), "UserShell", USER_SHELL],
    );
    let _ = run_status(
        "/usr/bin/dscl",
        &[".", "-create", path.as_str(), "NFSHomeDirectory", USER_HOME],
    );
    if user_is_admin(username).unwrap_or(false) {
        let _ = run_status(
            "/usr/sbin/dseditgroup",
            &["-o", "edit", "-d", username, "-t", "user", "admin"],
        );
    }
    Ok(())
}

fn wait_for_secure_token(username: &str, grant_output: &str) -> Result<()> {
    let started = Instant::now();
    loop {
        if secure_token_enabled(username).unwrap_or(false) {
            return Ok(());
        }
        if started.elapsed() > SECURE_TOKEN_VERIFY_TIMEOUT {
            bail!(
                "Talos macOS update account did not receive a Secure Token; sysadminctl output: {}",
                truncate_for_detail(grant_output)
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn wait_for_volume_owner(username: &str) -> Result<()> {
    let started = Instant::now();
    loop {
        let owners = discover_volume_owners().unwrap_or_default();
        let is_volume_owner = user_is_volume_owner(username, &owners).unwrap_or(false);
        let has_secure_token = secure_token_enabled(username).unwrap_or(false);
        if is_volume_owner && has_secure_token {
            return Ok(());
        }
        if started.elapsed() > VOLUME_OWNER_VERIFY_TIMEOUT {
            bail!(
                "Talos macOS update account did not become a volume owner (secureTokenEnabled={}, volumeOwner={}, discoveredVolumeOwners={})",
                has_secure_token,
                is_volume_owner,
                format_discovered_volume_owners(&owners)
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn refresh_apfs_preboot_records() {
    match run_checked_combined_output(
        "/usr/sbin/diskutil",
        &["apfs", "updatePreboot", "/"],
        "refresh APFS preboot records",
    ) {
        Ok(output) => debug!(
            detail = %truncate_for_detail(&output),
            "refreshed APFS preboot records after Talos update account enrollment"
        ),
        Err(error) => warn!(%error, "failed to refresh APFS preboot records after enrollment"),
    }
}

fn user_exists(username: &str) -> Result<bool> {
    validate_username(username)?;
    let path = user_path(username);
    Ok(Command::new("/usr/bin/dscl")
        .args([".", "-read", path.as_str()])
        .status()
        .with_context(|| format!("read macOS user {username}"))?
        .success())
}

fn generated_uid(username: &str) -> Result<Option<String>> {
    validate_username(username)?;
    let path = user_path(username);
    let output = run_output(
        "/usr/bin/dscl",
        &[".", "-read", path.as_str(), "GeneratedUID"],
    )?;
    Ok(parse_dscl_single_value(&output, "GeneratedUID"))
}

fn user_is_admin(username: &str) -> Result<bool> {
    validate_username(username)?;
    let output = run_combined_output(
        "/usr/sbin/dseditgroup",
        &["-o", "checkmember", "-m", username, "admin"],
    )?;
    Ok(output.to_ascii_lowercase().contains("yes"))
}

fn secure_token_enabled(username: &str) -> Result<bool> {
    validate_username(username)?;
    let output = run_combined_output("/usr/sbin/sysadminctl", &["-secureTokenStatus", username])?;
    Ok(output.to_ascii_lowercase().contains("enabled"))
}

fn user_is_volume_owner(username: &str, owners: &[MacosVolumeOwnerUser]) -> Result<bool> {
    let uid = generated_uid(username)?;
    Ok(owners.iter().any(|owner| {
        owner.volume_owner
            && (owner.username.as_deref() == Some(username)
                || (uid
                    .as_deref()
                    .zip(owner.generated_uid.as_deref())
                    .is_some_and(|(expected, actual)| expected.eq_ignore_ascii_case(actual))))
    }))
}

fn discover_volume_owners() -> Result<Vec<MacosVolumeOwnerUser>> {
    if !is_apple_silicon() {
        return Ok(Vec::new());
    }
    let plist = Command::new("/usr/sbin/diskutil")
        .args(["apfs", "listUsers", "/", "-plist"])
        .output()
        .context("run diskutil apfs listUsers")?;
    if !plist.status.success() {
        bail!(
            "diskutil apfs listUsers failed: {}",
            truncate_for_detail(&String::from_utf8_lossy(&plist.stderr))
        );
    }
    let json = plist_bytes_to_json(&plist.stdout)?;
    let local_users = local_users_by_generated_uid().unwrap_or_default();
    let mut owners = Vec::new();
    collect_volume_owner_objects(&json, &local_users, &mut owners);
    owners.sort_by(|a, b| {
        a.username
            .cmp(&b.username)
            .then(a.generated_uid.cmp(&b.generated_uid))
    });
    owners.dedup_by(|a, b| a.username == b.username && a.generated_uid == b.generated_uid);
    Ok(owners)
}

fn plist_bytes_to_json(plist: &[u8]) -> Result<Value> {
    let mut child = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn plutil")?;
    {
        let stdin = child.stdin.as_mut().context("open plutil stdin")?;
        stdin.write_all(plist).context("write plist to plutil")?;
    }
    let output = child.wait_with_output().context("wait for plutil")?;
    if !output.status.success() {
        bail!(
            "plutil failed: {}",
            truncate_for_detail(&String::from_utf8_lossy(&output.stderr))
        );
    }
    serde_json::from_slice(&output.stdout).context("parse diskutil plist json")
}

fn collect_volume_owner_objects(
    value: &Value,
    local_users: &HashMap<String, String>,
    owners: &mut Vec<MacosVolumeOwnerUser>,
) {
    match value {
        Value::Object(map) => {
            let volume_owner = map
                .get("VolumeOwner")
                .or_else(|| map.get("volumeOwner"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if volume_owner {
                let generated_uid = first_string(
                    map,
                    &[
                        "GeneratedUID",
                        "APFSCryptoUserUUID",
                        "CryptoUserUUID",
                        "UUID",
                        "uuid",
                    ],
                );
                let username = first_string(map, &["UserName", "Username", "Name", "userName"])
                    .or_else(|| {
                        generated_uid
                            .as_ref()
                            .and_then(|uid| local_users.get(&uid.to_ascii_uppercase()).cloned())
                    });
                owners.push(MacosVolumeOwnerUser {
                    full_name: username
                        .as_ref()
                        .and_then(|name| real_name(name).ok().flatten()),
                    username,
                    generated_uid,
                    volume_owner,
                });
            }
            for child in map.values() {
                collect_volume_owner_objects(child, local_users, owners);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_volume_owner_objects(child, local_users, owners);
            }
        }
        _ => {}
    }
}

fn first_string(map: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        map.get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

fn local_users_by_generated_uid() -> Result<HashMap<String, String>> {
    let output = run_output("/usr/bin/dscl", &[".", "-list", "/Users", "GeneratedUID"])?;
    let mut users = HashMap::new();
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        let Some(username) = parts.next() else {
            continue;
        };
        let Some(uid) = parts.next() else {
            continue;
        };
        users.insert(uid.to_ascii_uppercase(), username.to_string());
    }
    Ok(users)
}

fn real_name(username: &str) -> Result<Option<String>> {
    validate_username(username)?;
    let path = user_path(username);
    let output = run_output("/usr/bin/dscl", &[".", "-read", path.as_str(), "RealName"])?;
    Ok(parse_dscl_single_value(&output, "RealName"))
}

fn parse_dscl_single_value(output: &str, key: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix(&format!("{key}:")) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        } else if !trimmed.is_empty() && !trimmed.contains(':') {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn run_output(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("run {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} exited with {}: {}",
            output.status,
            truncate_for_detail(&String::from_utf8_lossy(&output.stderr))
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_combined_output(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("run {program}"))?;
    Ok(format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn run_checked_combined_output(program: &str, args: &[&str], description: &str) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("run {description}"))?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        Ok(combined)
    } else {
        bail!(
            "{description} exited with {}: {}",
            output.status,
            truncate_for_detail(&combined)
        )
    }
}

fn run_status(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("run {program}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{program} exited with {status}"))
    }
}

fn run_command_with_stdin(
    program: &str,
    args: &[&str],
    stdin_text: &str,
    description: &str,
) -> Result<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn {description}"))?;
    {
        let stdin = child.stdin.as_mut().context("open child stdin")?;
        stdin
            .write_all(stdin_text.as_bytes())
            .with_context(|| format!("write stdin for {description}"))?;
    }
    let output = child
        .wait_with_output()
        .with_context(|| format!("wait for {description}"))?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let secrets = stdin_secret_values(stdin_text);
    let combined = redact_secret_values(&combined, &secrets);
    if output.status.success() {
        Ok(combined)
    } else {
        bail!(
            "{description} exited with {}: {}",
            output.status,
            truncate_for_detail(&combined)
        )
    }
}

fn validate_username(username: &str) -> Result<()> {
    if username.is_empty()
        || username.len() > 64
        || username.starts_with('-')
        || username.contains('/')
        || !username
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        bail!("invalid macOS username");
    }
    Ok(())
}

fn stdin_secret_values(stdin_text: &str) -> Vec<String> {
    stdin_text
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
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

fn user_path(username: &str) -> String {
    // All callers validate first. dscl accepts the slash-prefixed record path as one argv.
    format!("/Users/{username}")
}

fn format_discovered_volume_owners(owners: &[MacosVolumeOwnerUser]) -> String {
    let mut names = owners
        .iter()
        .filter(|owner| owner.volume_owner)
        .map(|owner| {
            owner
                .username
                .clone()
                .or_else(|| owner.generated_uid.clone())
                .unwrap_or_else(|| "unknown".to_string())
        })
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(",")
    }
}

fn truncate_for_detail(value: &str) -> String {
    let value = value.trim();
    const LIMIT: usize = 1200;
    if value.len() <= LIMIT {
        value.to_string()
    } else {
        format!("{}...", truncate_at_char_boundary(value, LIMIT))
    }
}

fn truncate_at_char_boundary(value: &str, limit: usize) -> &str {
    if value.len() <= limit {
        return value;
    }
    let end = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= limit)
        .last()
        .unwrap_or(0);
    &value[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use serde_json::json;

    #[test]
    fn parses_dscl_single_line_value() {
        assert_eq!(
            parse_dscl_single_value("GeneratedUID: ABCD\n", "GeneratedUID").as_deref(),
            Some("ABCD")
        );
    }

    #[test]
    fn rejects_unsafe_usernames() {
        assert!(validate_username("owner.user-1").is_ok());
        assert!(validate_username("-bad").is_err());
        assert!(validate_username("../bad").is_err());
        assert!(validate_username("").is_err());
    }

    #[test]
    fn parses_codesign_identifier_and_team() {
        let detail = parse_codesign_detail(
            "Executable=/Applications/Talos Permissions Helper.app/Contents/MacOS/talos_permissions_helper\nIdentifier=com.talos.permissions-helper\nAuthority=Developer ID Application: Talos Ltd (ABCDE12345)\nAuthority=Developer ID Certification Authority\nTeamIdentifier=ABCDE12345\n",
        );

        assert_eq!(detail.identifier.as_deref(), Some(HELPER_BUNDLE_IDENTIFIER));
        assert_eq!(detail.team_identifier.as_deref(), Some("ABCDE12345"));
        assert_eq!(
            detail.authorities,
            vec![
                "Developer ID Application: Talos Ltd (ABCDE12345)".to_string(),
                "Developer ID Certification Authority".to_string()
            ]
        );
    }

    #[test]
    fn ignores_unset_codesign_team_identifier() {
        let detail = parse_codesign_detail(
            "Identifier=com.talos.permissions-helper\nTeamIdentifier=not set\n",
        );

        assert_eq!(detail.identifier.as_deref(), Some(HELPER_BUNDLE_IDENTIFIER));
        assert!(detail.team_identifier.is_none());
    }

    #[test]
    fn parses_codesign_leaf_certificate_hash() {
        let requirement = "Executable=/Applications/Talos Permissions Helper.app/Contents/MacOS/talos_permissions_helper\ndesignated => identifier \"com.talos.permissions-helper\" and certificate leaf = H\"5A9858D5B89B2DACFDD3AEB360564240042CA8F2\"\n";

        assert_eq!(
            parse_leaf_certificate_hash(requirement).as_deref(),
            Some("5a9858d5b89b2dacfdd3aeb360564240042ca8f2")
        );
        assert!(parse_leaf_certificate_hash("designated => identifier \"adhoc\"").is_none());
    }

    #[test]
    fn helper_signature_match_accepts_matching_team_identifier() {
        let helper = CodeSignatureDetail {
            identifier: Some(HELPER_BUNDLE_IDENTIFIER.to_string()),
            team_identifier: Some("ABCDE12345".to_string()),
            leaf_certificate_hash: None,
            authorities: vec!["Different local authority".to_string()],
        };
        let worker = CodeSignatureDetail {
            identifier: Some("com.talos.worker".to_string()),
            team_identifier: Some("ABCDE12345".to_string()),
            leaf_certificate_hash: None,
            authorities: vec!["Worker authority".to_string()],
        };

        assert!(helper_signature_detail_matches(&helper, &worker));
    }

    #[test]
    fn helper_signature_match_rejects_mismatched_team_identifier() {
        let helper = CodeSignatureDetail {
            identifier: Some(HELPER_BUNDLE_IDENTIFIER.to_string()),
            team_identifier: Some("WRONGTEAM".to_string()),
            leaf_certificate_hash: Some("helperhash".to_string()),
            authorities: vec!["Developer ID Application: Talos Ltd".to_string()],
        };
        let worker = CodeSignatureDetail {
            identifier: Some("com.talos.worker".to_string()),
            team_identifier: Some("ABCDE12345".to_string()),
            leaf_certificate_hash: Some("workerhash".to_string()),
            authorities: vec!["Developer ID Application: Talos Ltd".to_string()],
        };

        assert!(!helper_signature_detail_matches(&helper, &worker));
    }

    #[test]
    fn helper_signature_match_accepts_same_leaf_certificate_with_different_team_metadata() {
        let helper = CodeSignatureDetail {
            identifier: Some(HELPER_BUNDLE_IDENTIFIER.to_string()),
            team_identifier: None,
            leaf_certificate_hash: Some("5a9858d5b89b2dacfdd3aeb360564240042ca8f2".to_string()),
            authorities: vec!["Seb Test Code Signing".to_string()],
        };
        let worker = CodeSignatureDetail {
            identifier: Some("com.talos.worker".to_string()),
            team_identifier: Some("ABCDE12345".to_string()),
            leaf_certificate_hash: Some("5A9858D5B89B2DACFDD3AEB360564240042CA8F2".to_string()),
            authorities: vec!["Seb Test Code Signing".to_string()],
        };

        assert!(helper_signature_detail_matches(&helper, &worker));
    }

    #[test]
    fn helper_signature_match_accepts_local_same_leaf_certificate_without_team_identifier() {
        let helper = CodeSignatureDetail {
            identifier: Some(HELPER_BUNDLE_IDENTIFIER.to_string()),
            team_identifier: None,
            leaf_certificate_hash: Some("5a9858d5b89b2dacfdd3aeb360564240042ca8f2".to_string()),
            authorities: vec!["Seb Test Code Signing".to_string()],
        };
        let worker = CodeSignatureDetail {
            identifier: Some("com.talos.worker".to_string()),
            team_identifier: None,
            leaf_certificate_hash: Some("5A9858D5B89B2DACFDD3AEB360564240042CA8F2".to_string()),
            authorities: vec!["Seb Test Code Signing".to_string()],
        };

        assert!(helper_signature_detail_matches(&helper, &worker));
    }

    #[test]
    fn helper_signature_match_rejects_local_leaf_certificate_mismatch_or_adhoc() {
        let helper = CodeSignatureDetail {
            identifier: Some(HELPER_BUNDLE_IDENTIFIER.to_string()),
            team_identifier: None,
            leaf_certificate_hash: Some("5a9858d5b89b2dacfdd3aeb360564240042ca8f2".to_string()),
            authorities: vec!["Seb Test Code Signing".to_string()],
        };
        let wrong_worker = CodeSignatureDetail {
            identifier: Some("com.talos.worker".to_string()),
            team_identifier: None,
            leaf_certificate_hash: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()),
            authorities: vec!["Seb Test Code Signing".to_string()],
        };
        let adhoc_worker = CodeSignatureDetail {
            identifier: Some("com.talos.worker".to_string()),
            team_identifier: None,
            leaf_certificate_hash: None,
            authorities: Vec::new(),
        };

        assert!(!helper_signature_detail_matches(&helper, &wrong_worker));
        assert!(!helper_signature_detail_matches(&helper, &adhoc_worker));
    }

    #[test]
    fn helper_signature_match_rejects_wrong_bundle_identifier() {
        let helper = CodeSignatureDetail {
            identifier: Some("com.example.other".to_string()),
            team_identifier: Some("ABCDE12345".to_string()),
            leaf_certificate_hash: None,
            authorities: Vec::new(),
        };
        let worker = CodeSignatureDetail {
            identifier: Some("com.talos.worker".to_string()),
            team_identifier: Some("ABCDE12345".to_string()),
            leaf_certificate_hash: None,
            authorities: Vec::new(),
        };

        assert!(!helper_signature_detail_matches(&helper, &worker));
    }

    #[test]
    fn helper_process_requirement_prefers_leaf_certificate_hash() {
        let worker = CodeSignatureDetail {
            identifier: Some("com.talos.worker".to_string()),
            team_identifier: Some("TEAM123456".to_string()),
            leaf_certificate_hash: Some("5A9858D5B89B2DACFDD3AEB360564240042CA8F2".to_string()),
            authorities: Vec::new(),
        };

        let requirement = helper_process_requirement(&worker).expect("build requirement");
        let _: SecRequirement = requirement.parse().expect("parse requirement");

        assert_eq!(
            requirement,
            "identifier \"com.talos.permissions-helper\" and certificate leaf = H\"5a9858d5b89b2dacfdd3aeb360564240042ca8f2\""
        );
    }

    #[test]
    fn helper_process_requirement_falls_back_to_team_identifier() {
        let worker = CodeSignatureDetail {
            identifier: Some("com.talos.worker".to_string()),
            team_identifier: Some("TEAM123456".to_string()),
            leaf_certificate_hash: None,
            authorities: Vec::new(),
        };

        let requirement = helper_process_requirement(&worker).expect("build requirement");
        let _: SecRequirement = requirement.parse().expect("parse requirement");

        assert_eq!(
            requirement,
            "identifier \"com.talos.permissions-helper\" and certificate leaf[subject.OU] = \"TEAM123456\""
        );
    }

    #[test]
    fn helper_process_requirement_rejects_unsafe_identity_parts() {
        let worker = CodeSignatureDetail {
            identifier: Some("com.talos.worker".to_string()),
            team_identifier: Some("TEAM\" or true".to_string()),
            leaf_certificate_hash: None,
            authorities: Vec::new(),
        };

        assert!(helper_process_requirement(&worker).is_err());
        assert!(requirement_leaf_hash("not-a-hash").is_err());
    }

    #[test]
    fn next_credential_version_rotates_from_previous_state() {
        let state = AccountState {
            username: DEFAULT_USERNAME.to_string(),
            credential_version: 7,
            generated_uid: Some("generated-uid".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            last_failure_code: None,
            last_failure_message: None,
        };

        assert_eq!(next_credential_version(None), 1);
        assert_eq!(next_credential_version(Some(&state)), 8);
    }

    #[test]
    fn redacts_stdin_secrets_from_command_output() {
        let secrets = stdin_secret_values("talos-password\nowner-password\n");
        let output =
            redact_secret_values("failed with talos-password and owner-password", &secrets);

        assert_eq!(output, "failed with [redacted] and [redacted]");
        assert!(!output.contains("talos-password"));
        assert!(!output.contains("owner-password"));
    }

    #[test]
    fn truncates_detail_on_char_boundary() {
        let value = format!("{}é", "a".repeat(1199));
        let truncated = truncate_for_detail(&value);

        assert!(truncated.ends_with("..."));
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn collects_volume_owner_objects_recursively() {
        let value = json!({
            "Users": [{
                "APFSCryptoUserUUID": "A1",
                "VolumeOwner": true
            }, {
                "APFSCryptoUserUUID": "B2",
                "VolumeOwner": false
            }]
        });
        let mut local = HashMap::new();
        local.insert("A1".to_string(), "alice".to_string());
        let mut owners = Vec::new();
        collect_volume_owner_objects(&value, &local, &mut owners);
        assert_eq!(owners.len(), 1);
        assert_eq!(owners[0].username.as_deref(), Some("alice"));
        assert!(owners[0].volume_owner);
    }

    #[test]
    fn error_status_is_non_secret() {
        let status = error_status("code", "message");
        let json = serde_json::to_string(&status).expect("serialize status");
        assert!(!json.contains("password"));
    }

    #[test]
    fn deduplicates_volume_owners_by_status_caller() {
        let mut owners = vec![
            MacosVolumeOwnerUser {
                username: Some("alice".to_string()),
                full_name: None,
                generated_uid: Some("A".to_string()),
                volume_owner: true,
            },
            MacosVolumeOwnerUser {
                username: Some("alice".to_string()),
                full_name: None,
                generated_uid: Some("A".to_string()),
                volume_owner: true,
            },
        ];
        owners.sort_by(|a, b| {
            a.username
                .cmp(&b.username)
                .then(a.generated_uid.cmp(&b.generated_uid))
        });
        owners.dedup_by(|a, b| a.username == b.username && a.generated_uid == b.generated_uid);
        assert_eq!(owners.len(), 1);
    }

    #[test]
    fn formats_discovered_volume_owners_for_detail() {
        let owners = vec![
            MacosVolumeOwnerUser {
                username: Some("bob".to_string()),
                full_name: None,
                generated_uid: Some("B".to_string()),
                volume_owner: true,
            },
            MacosVolumeOwnerUser {
                username: Some("alice".to_string()),
                full_name: None,
                generated_uid: Some("A".to_string()),
                volume_owner: true,
            },
            MacosVolumeOwnerUser {
                username: Some("alice".to_string()),
                full_name: None,
                generated_uid: Some("A".to_string()),
                volume_owner: true,
            },
            MacosVolumeOwnerUser {
                username: Some("ignored".to_string()),
                full_name: None,
                generated_uid: Some("C".to_string()),
                volume_owner: false,
            },
        ];

        assert_eq!(format_discovered_volume_owners(&owners), "alice,bob");
        assert_eq!(format_discovered_volume_owners(&[]), "none");
    }

    #[test]
    fn status_strings_are_stable() {
        let statuses: HashSet<&str> = [
            "notRequired",
            "ready",
            "needsEnrollment",
            "missing",
            "error",
        ]
        .into_iter()
        .collect();
        assert!(statuses.contains("needsEnrollment"));
    }
}
