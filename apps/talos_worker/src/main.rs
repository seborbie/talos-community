use std::{
    collections::{HashMap, HashSet, VecDeque},
    env,
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom},
    net::{Ipv4Addr, Ipv6Addr, ToSocketAddrs, UdpSocket},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::sync::atomic::AtomicU64;

mod feature_upgrade_preflight;
mod feature_upgrade_stage_iso;
mod feature_upgrade_start;
#[cfg(any(target_os = "linux", test))]
mod linux_telemetry;
#[cfg(target_os = "macos")]
mod macos_desktop;
#[cfg(target_os = "macos")]
mod macos_events;
#[cfg(target_os = "macos")]
mod macos_telemetry;
mod patching;
mod remediation;

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod chat;

use anyhow::{anyhow, ensure, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use chacha20poly1305::ChaCha20Poly1305;
use futures_util::{SinkExt, StreamExt};
use get_if_addrs::{get_if_addrs, IfAddr};
use local_ip_address::local_ip;
#[cfg(target_os = "windows")]
use quinn::Connection;
use quinn::Endpoint;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls_pemfile::{certs, pkcs8_private_keys};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use stunclient::StunClient;
use sysinfo::{Disks, Networks, ProcessesToUpdate, System};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use talos_worker::encode;
use talos_worker::file_transfer;
#[cfg(any(target_os = "windows", target_family = "unix"))]
use talos_worker::shell;
#[cfg(target_os = "windows")]
use talos_worker::{control, display, registry};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{broadcast, mpsc, RwLock},
    time::{sleep, timeout},
};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::{
    tungstenite::{client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};
use tracing::{debug, info, warn};

#[cfg(target_os = "windows")]
use talos_collector::event_stream::{
    monitors::{spawn_monitors, MonitorConfig},
    schema::EventInput,
};
use talos_protocol::relay_transport::{
    build_e2e_cipher, build_relay_client_tls_config, parse_relay_target, read_e2e_frame_from,
    read_http_response, write_e2e_frame, write_e2e_frame_flush,
};
#[cfg(not(target_os = "windows"))]
use talos_protocol::RemoteDesktopUnavailablePayload;
#[cfg(target_os = "macos")]
use talos_protocol::CONTROL_TYPE_STOP_CAPTURE;
use talos_protocol::{
    build_file_transfer_frame, parse_file_transfer_frame, AgentFeatureCapabilities, AgentHello,
    AgentPlatform, FileTransferRequest, FileTransferResponse, FullSnapshotUpdate, IncomingEnvelope,
    LinuxShellCredentialStoredPayload, LocalAddr, OutgoingEnvelope, PunchStartPayload,
    QuicReflexPayload, ReflexAddress, RelayPreparePayload, RemoteDesktopCapabilities,
    RemoteDesktopDisplayProfile, RequestFullSnapshotPayload, SessionCapabilitiesRequest,
    SessionCapabilitiesResponse, SessionTransportMode, ShellCommandPayload, ShellOutputPayload,
    TelemetryEventsUpdate, TunnelPreparePayload, FILE_TRANSFER_MSG_DATA, FILE_TRANSFER_MSG_FINISH,
    FILE_TRANSFER_MSG_JSON, HEARTBEAT_PAYLOAD, REMOTE_DESKTOP_CODEC_BGRA_ATLAS_COMMANDS,
    REMOTE_DESKTOP_CODEC_H264, REMOTE_DESKTOP_CODEC_SCREENSHOT_BGRA, REMOTE_DESKTOP_CODEC_VP8,
    REMOTE_DESKTOP_PROFILE_EXPERIMENTAL, REMOTE_DESKTOP_PROFILE_LEGACY,
    REMOTE_DESKTOP_PROFILE_MODERN_CPU, REMOTE_DESKTOP_PROFILE_MODERN_GPU,
    REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY,
};
#[cfg(target_os = "windows")]
use talos_protocol::{
    parse_control_frame, HELPER_PIPE_HANDSHAKE_MAGIC, HELPER_PIPE_MAX_AUTH_TOKEN_LEN,
    HELPER_PIPE_PROTOCOL_VERSION,
};
#[cfg(target_os = "macos")]
use talos_protocol::{MacosUpdateAccountStatus, MacosUpdateAccountStatusPayload};
#[cfg(target_os = "windows")]
use talos_protocol::{
    RegistryRequest, RegistryResponseEnvelope, CONTROL_PAYLOAD_SESSION_ID_LEN,
    CONTROL_PAYLOAD_TIMESTAMP_LEN, CONTROL_TYPE_CONNECTION_PING, CONTROL_TYPE_REGISTRY_REQUEST,
    CONTROL_TYPE_SESSION_LOGOFF, CONTROL_TYPE_SESSION_SWITCH, CONTROL_TYPE_STOP_CAPTURE,
    DISPLAY_RECORD_FRAME_BEGIN, DISPLAY_RECORD_FRAME_END, REGISTRY_META_MESSAGE_TYPE,
};
#[cfg(any(target_os = "windows", target_family = "unix"))]
use talos_protocol::{ShellOfferPayload, ShellStartPayload};

#[cfg(any(target_os = "windows", target_os = "macos"))]
const CAPTURE_BUFFER_MAX_CHUNKS: usize = 600;

#[cfg(target_os = "windows")]
static PIPE_INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(1);

// Monotonic counter for viewer connections handled by this agent process.
// Helps correlate "2nd connect" / "3rd connect" problems in production logs.
#[cfg(target_os = "windows")]
static VIEWER_SESSION_SEQ_COUNTER: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "windows")]
static VIEWER_SESSION_SEQ_BY_ID: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
#[cfg(target_os = "windows")]
static VIEWER_SESSION_START_MS_BY_ID: OnceLock<Mutex<HashMap<String, u128>>> = OnceLock::new();
#[cfg(target_os = "windows")]
static VIEWER_SESSION_PROFILE_BY_ID: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
#[cfg(target_os = "macos")]
static MACOS_CAPTURE_MODE_BY_SESSION: OnceLock<Mutex<HashMap<String, MacosDesktopCaptureMode>>> =
    OnceLock::new();
#[cfg(target_os = "macos")]
static MACOS_HIDE_CURSOR_BY_SESSION: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
#[cfg(target_os = "windows")]
static HELPER_STARTUP_FAULTED_SESSIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
#[cfg(target_os = "windows")]
static CAPTURE_PIPE_FAULTED_SESSIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static AGENT_LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

#[cfg(target_os = "macos")]
#[derive(Serialize)]
struct MacosFullDiskAccessCheck {
    permission: &'static str,
    granted: bool,
    probe_path: Option<String>,
    error: Option<String>,
}

#[cfg(target_os = "macos")]
const MACOS_WORKER_BUNDLE_ID: &str = "com.talos.worker";
#[cfg(target_os = "macos")]
const MACOS_STARTUP_PERMISSION_HELPER_MARKER_PATH: &str =
    "/tmp/talos-permissions-helper-startup-surfaced";
#[cfg(target_os = "macos")]
const MACOS_STARTUP_PERMISSION_HELPER_COOLDOWN: Duration = Duration::from_secs(5 * 60);

const FILE_TRANSFER_RESUME_TTL: Duration = Duration::from_secs(30 * 60);
static FILE_TRANSFER_UPLOAD_RESUMES: OnceLock<
    tokio::sync::Mutex<HashMap<String, ResumableUploadState>>,
> = OnceLock::new();
static FILE_TRANSFER_DOWNLOAD_RESUMES: OnceLock<
    tokio::sync::Mutex<HashMap<String, ResumableDownloadState>>,
> = OnceLock::new();

struct ResumableUploadState {
    upload: file_transfer::UploadContext,
    expected_size_bytes: u64,
    touched_at: Instant,
}

struct ResumableDownloadState {
    requested_paths: Vec<String>,
    prepared: file_transfer::PreparedDownload,
    touched_at: Instant,
}

fn file_transfer_upload_resumes(
) -> &'static tokio::sync::Mutex<HashMap<String, ResumableUploadState>> {
    FILE_TRANSFER_UPLOAD_RESUMES.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

fn file_transfer_download_resumes(
) -> &'static tokio::sync::Mutex<HashMap<String, ResumableDownloadState>> {
    FILE_TRANSFER_DOWNLOAD_RESUMES.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

fn file_transfer_resume_key(session_id: &str, transfer_id: &str) -> String {
    format!("{session_id}:{transfer_id}")
}

fn cleanup_upload_resume_state(state: ResumableUploadState) {
    let _ = fs::remove_file(state.upload.temp_input_path);
}

fn cleanup_download_resume_state(state: ResumableDownloadState) {
    if state.prepared.cleanup_source {
        let _ = fs::remove_file(state.prepared.source_path);
    }
}

fn file_transfer_error_response(
    code: talos_protocol::OperationErrorCode,
    message: impl Into<String>,
    retryable: bool,
) -> FileTransferResponse {
    FileTransferResponse::Error {
        code,
        message: message.into(),
        retryable,
    }
}

fn resumable_upload_matches(state: &ResumableUploadState, request: &FileTransferRequest) -> bool {
    let FileTransferRequest::Upload {
        destination_path,
        file_name,
        is_archive,
        extract_archive,
        conflict_mode,
        expected_size_bytes,
        ..
    } = request
    else {
        return false;
    };
    let normalized_destination_path =
        match file_transfer::normalize_upload_destination_path(destination_path) {
            Ok(path) => path,
            Err(_) => return false,
        };

    state.expected_size_bytes == *expected_size_bytes
        && state.upload.destination_path == normalized_destination_path
        && state.upload.file_name == *file_name
        && state.upload.is_archive == *is_archive
        && state.upload.extract_archive == *extract_archive
        && state.upload.conflict_mode == *conflict_mode
}

async fn remember_upload_resume_state(session_id: &str, state: ResumableUploadState) {
    let key = file_transfer_resume_key(session_id, &state.upload.transfer_id);
    file_transfer_upload_resumes()
        .lock()
        .await
        .insert(key, state);
}

async fn remember_download_resume_state(
    session_id: &str,
    transfer_id: &str,
    requested_paths: Vec<String>,
    prepared: file_transfer::PreparedDownload,
) {
    let key = file_transfer_resume_key(session_id, transfer_id);
    file_transfer_download_resumes().lock().await.insert(
        key,
        ResumableDownloadState {
            requested_paths,
            prepared,
            touched_at: Instant::now(),
        },
    );
}

async fn prune_file_transfer_resume_state() {
    let now = Instant::now();
    let mut stale_uploads = Vec::new();
    {
        let mut guard = file_transfer_upload_resumes().lock().await;
        guard.retain(|_, state| {
            if now.duration_since(state.touched_at) > FILE_TRANSFER_RESUME_TTL {
                stale_uploads.push(state.upload.temp_input_path.clone());
                false
            } else {
                true
            }
        });
    }
    for path in stale_uploads {
        let _ = fs::remove_file(path);
    }

    let mut stale_downloads = Vec::new();
    {
        let mut guard = file_transfer_download_resumes().lock().await;
        guard.retain(|_, state| {
            if now.duration_since(state.touched_at) > FILE_TRANSFER_RESUME_TTL {
                if state.prepared.cleanup_source {
                    stale_downloads.push(state.prepared.source_path.clone());
                }
                false
            } else {
                true
            }
        });
    }
    for path in stale_downloads {
        let _ = fs::remove_file(path);
    }
}

async fn clear_file_transfer_resume_state(session_id: &str, transfer_id: &str) {
    let key = file_transfer_resume_key(session_id, transfer_id);
    if let Some(state) = file_transfer_upload_resumes().lock().await.remove(&key) {
        cleanup_upload_resume_state(state);
    }
    if let Some(state) = file_transfer_download_resumes().lock().await.remove(&key) {
        cleanup_download_resume_state(state);
    }
}

#[allow(dead_code)]
async fn clear_all_file_transfer_resume_state_for_session(session_id: &str) {
    let prefix = format!("{session_id}:");
    let upload_keys = {
        let guard = file_transfer_upload_resumes().lock().await;
        guard
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect::<Vec<_>>()
    };
    for key in upload_keys {
        if let Some(state) = file_transfer_upload_resumes().lock().await.remove(&key) {
            cleanup_upload_resume_state(state);
        }
    }
    let download_keys = {
        let guard = file_transfer_download_resumes().lock().await;
        guard
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect::<Vec<_>>()
    };
    for key in download_keys {
        if let Some(state) = file_transfer_download_resumes().lock().await.remove(&key) {
            cleanup_download_resume_state(state);
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn now_unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn now_unix_ms_u64() -> u64 {
    now_unix_ms().min(u128::from(u64::MAX)) as u64
}

#[cfg(target_os = "windows")]
fn env_flag_enabled(name: &str) -> bool {
    env::var(name)
        .ok()
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            !matches!(normalized.as_str(), "" | "0" | "false" | "no" | "off")
        })
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn consume_fault_once(
    storage: &'static OnceLock<Mutex<HashSet<String>>>,
    session_id: &str,
) -> bool {
    let set = storage.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = match set.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.insert(session_id.to_string())
}

#[cfg(target_os = "windows")]
fn should_fault_inject_helper_startup_fail_once(session_id: &str) -> bool {
    env_flag_enabled("RMM_FAULT_INJECT_HELPER_STARTUP_FAIL_ONCE")
        && consume_fault_once(&HELPER_STARTUP_FAULTED_SESSIONS, session_id)
}

#[cfg(target_os = "windows")]
fn should_fault_inject_capture_pipe_fail_once(session_id: &str) -> bool {
    env_flag_enabled("RMM_FAULT_INJECT_CAPTURE_PIPE_FAIL_ONCE")
        && consume_fault_once(&CAPTURE_PIPE_FAULTED_SESSIONS, session_id)
}

#[cfg(target_os = "windows")]
fn get_or_assign_viewer_session_seq(session_id: &str) -> (u64, bool) {
    let map = VIEWER_SESSION_SEQ_BY_ID.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = match map.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(existing) = guard.get(session_id).copied() {
        return (existing, false);
    }
    let seq = VIEWER_SESSION_SEQ_COUNTER.fetch_add(1, Ordering::SeqCst) + 1;
    guard.insert(session_id.to_string(), seq);

    let start_map = VIEWER_SESSION_START_MS_BY_ID.get_or_init(|| Mutex::new(HashMap::new()));
    let mut start_guard = match start_map.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    start_guard.insert(session_id.to_string(), now_unix_ms());

    (seq, true)
}

#[cfg(target_os = "windows")]
fn viewer_session_seq(session_id: &str) -> Option<u64> {
    VIEWER_SESSION_SEQ_BY_ID
        .get()
        .and_then(|m| m.lock().ok().and_then(|g| g.get(session_id).copied()))
}

#[cfg(target_os = "windows")]
fn viewer_session_started_ms(session_id: &str) -> Option<u128> {
    VIEWER_SESSION_START_MS_BY_ID
        .get()
        .and_then(|m| m.lock().ok().and_then(|g| g.get(session_id).copied()))
}

#[cfg(target_os = "windows")]
fn set_viewer_session_profile(session_id: &str, requested_profile: Option<&str>) -> (String, bool) {
    let selected = encode::selected_display_profile_for_effective_processing_mode(
        "agent session profile selection",
        requested_profile,
    );
    let map = VIEWER_SESSION_PROFILE_BY_ID.get_or_init(|| Mutex::new(HashMap::new()));
    let changed = match map.lock() {
        Ok(mut guard) => guard
            .insert(session_id.to_string(), selected.clone())
            .map(|previous| previous != selected)
            .unwrap_or(true),
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            guard
                .insert(session_id.to_string(), selected.clone())
                .map(|previous| previous != selected)
                .unwrap_or(true)
        }
    };
    (selected, changed)
}

#[cfg(target_os = "windows")]
fn viewer_session_profile(session_id: &str) -> String {
    VIEWER_SESSION_PROFILE_BY_ID
        .get()
        .and_then(|m| m.lock().ok().and_then(|g| g.get(session_id).cloned()))
        .unwrap_or_else(|| {
            encode::selected_display_profile_for_effective_processing_mode(
                "agent session profile lookup",
                None,
            )
        })
}

#[cfg(target_os = "windows")]
fn clear_viewer_session_tracking(session_id: &str) {
    if let Some(map) = VIEWER_SESSION_SEQ_BY_ID.get() {
        if let Ok(mut guard) = map.lock() {
            guard.remove(session_id);
        }
    }
    if let Some(map) = VIEWER_SESSION_START_MS_BY_ID.get() {
        if let Ok(mut guard) = map.lock() {
            guard.remove(session_id);
        }
    }
    if let Some(map) = VIEWER_SESSION_PROFILE_BY_ID.get() {
        if let Ok(mut guard) = map.lock() {
            guard.remove(session_id);
        }
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MacosDesktopCaptureMode {
    H264,
    Legacy,
    Atx2,
    Screenshot,
}

#[cfg(target_os = "macos")]
fn macos_capture_mode_for_profile(requested_profile: Option<&str>) -> MacosDesktopCaptureMode {
    match requested_profile.map(|value| value.trim().to_ascii_lowercase()) {
        Some(profile) if profile == REMOTE_DESKTOP_PROFILE_LEGACY => {
            MacosDesktopCaptureMode::Legacy
        }
        Some(profile) if profile == REMOTE_DESKTOP_PROFILE_MODERN_CPU => {
            MacosDesktopCaptureMode::Legacy
        }
        Some(profile) if profile == REMOTE_DESKTOP_PROFILE_EXPERIMENTAL => {
            MacosDesktopCaptureMode::Atx2
        }
        Some(profile) if profile == REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY => {
            MacosDesktopCaptureMode::Screenshot
        }
        Some(profile) if profile == REMOTE_DESKTOP_PROFILE_MODERN_GPU => {
            MacosDesktopCaptureMode::H264
        }
        Some(_) => MacosDesktopCaptureMode::Legacy,
        None => MacosDesktopCaptureMode::H264,
    }
}

#[cfg(target_os = "macos")]
fn set_macos_session_display_profile(
    session_id: &str,
    requested_profile: Option<&str>,
) -> MacosDesktopCaptureMode {
    let mode = macos_capture_mode_for_profile(requested_profile);
    let map = MACOS_CAPTURE_MODE_BY_SESSION.get_or_init(|| Mutex::new(HashMap::new()));
    match map.lock() {
        Ok(mut guard) => {
            guard.insert(session_id.to_string(), mode);
        }
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            guard.insert(session_id.to_string(), mode);
        }
    }
    mode
}

#[cfg(target_os = "macos")]
fn set_macos_session_display_options(
    session_id: &str,
    requested_profile: Option<&str>,
    hide_cursor: bool,
) -> MacosDesktopCaptureMode {
    let mode = set_macos_session_display_profile(session_id, requested_profile);
    let map = MACOS_HIDE_CURSOR_BY_SESSION.get_or_init(|| Mutex::new(HashMap::new()));
    match map.lock() {
        Ok(mut guard) => {
            guard.insert(session_id.to_string(), hide_cursor);
        }
        Err(poisoned) => {
            let mut guard = poisoned.into_inner();
            guard.insert(session_id.to_string(), hide_cursor);
        }
    }
    mode
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_session_capture_mode(session_id: &str) -> MacosDesktopCaptureMode {
    MACOS_CAPTURE_MODE_BY_SESSION
        .get()
        .and_then(|map| {
            map.lock()
                .ok()
                .and_then(|guard| guard.get(session_id).copied())
        })
        .unwrap_or(MacosDesktopCaptureMode::H264)
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_session_hide_cursor(session_id: &str) -> bool {
    MACOS_HIDE_CURSOR_BY_SESSION
        .get()
        .and_then(|map| {
            map.lock()
                .ok()
                .and_then(|guard| guard.get(session_id).copied())
        })
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn clear_macos_session_display_profile(session_id: &str) {
    if let Some(map) = MACOS_CAPTURE_MODE_BY_SESSION.get() {
        if let Ok(mut guard) = map.lock() {
            guard.remove(session_id);
        }
    }
    if let Some(map) = MACOS_HIDE_CURSOR_BY_SESSION.get() {
        if let Ok(mut guard) = map.lock() {
            guard.remove(session_id);
        }
    }
}

#[cfg(target_os = "windows")]
async fn remove_remote_desktop_session_state(
    session_id: &str,
    control_pipe_writers: &Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    helper_target_sessions: &Arc<RwLock<HashMap<String, u32>>>,
    punch_sockets: &Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: &Arc<RwLock<HashSet<String>>>,
) {
    if control_pipe_writers
        .write()
        .await
        .remove(session_id)
        .is_some()
    {
        info!(session_id = %session_id, "control pipe writer removed during teardown");
    }
    if helper_target_sessions
        .write()
        .await
        .remove(session_id)
        .is_some()
    {
        info!(session_id = %session_id, "helper target session removed during teardown");
    }
    if punch_sockets.write().await.remove(session_id).is_some() {
        info!(session_id = %session_id, "punch socket removed during teardown");
    }
    if relay_sessions.write().await.remove(session_id) {
        info!(session_id = %session_id, "relay session removed during teardown");
    }
    clear_viewer_session_tracking(session_id);
}

#[cfg(target_os = "macos")]
async fn remove_remote_desktop_session_state(
    session_id: &str,
    control_pipe_writers: &Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    punch_sockets: &Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: &Arc<RwLock<HashSet<String>>>,
) {
    if control_pipe_writers
        .write()
        .await
        .remove(session_id)
        .is_some()
    {
        info!(session_id = %session_id, "macOS control pipe writer removed during teardown");
    }
    if punch_sockets.write().await.remove(session_id).is_some() {
        info!(session_id = %session_id, "macOS punch socket removed during teardown");
    }
    if relay_sessions.write().await.remove(session_id) {
        info!(session_id = %session_id, "macOS relay session removed during teardown");
    }
    clear_macos_session_display_profile(session_id);
}

#[cfg(target_os = "windows")]
struct SendHandle(winapi::um::winnt::HANDLE);

#[cfg(target_os = "windows")]
unsafe impl Send for SendHandle {}

#[cfg(target_os = "windows")]
unsafe impl Sync for SendHandle {}

#[cfg(any(target_os = "windows", target_family = "unix"))]
#[derive(Clone)]
struct PreparedShellSession {
    token: String,
    shell_io: Arc<tokio::sync::Mutex<Option<shell::SharedShellIo>>>,
}

fn init_file_logging() -> Result<(), std::io::Error> {
    let log_template = agent_log_path();
    let writer = talos_log_util::DailyFileMakeWriter::try_new(log_template.clone())?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            talos_protocol::rmm_tracing_filter_directive(),
        ))
        .with_timer(tracing_subscriber::fmt::time::SystemTime)
        .with_writer(writer)
        .with_ansi(false)
        .init();
    info!(path = %log_template.display(), "logging to file");
    Ok(())
}

/// Clear `RUST_LOG`; file log level uses `talos_protocol::rmm_tracing_filter_directive`
/// (`RMM_DEBUG`, `RMM_LOGLEVEL`, default `warn`).
fn strip_legacy_log_env_vars() {
    env::remove_var("RUST_LOG");
}

#[cfg(target_os = "windows")]
fn windows_log_path_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(base) = env::var("PROGRAMDATA") {
        paths.push(
            PathBuf::from(base)
                .join("Talos")
                .join("logs")
                .join("talos_worker.log"),
        );
    }
    paths.push(PathBuf::from(r"C:\ProgramData\Talos\logs\talos_worker.log"));
    paths.push(env::temp_dir().join("talos_worker.log"));
    paths.push(PathBuf::from(r"C:\Windows\Temp\talos_worker.log"));
    paths
}

#[cfg(target_os = "macos")]
fn windows_log_path_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    paths.push(PathBuf::from("/Library/Logs/Talos/talos_worker.log"));
    if let Ok(home) = env::var("HOME") {
        paths.push(
            PathBuf::from(home)
                .join("Library")
                .join("Logs")
                .join("Talos")
                .join("talos_worker.log"),
        );
    }
    paths.push(env::temp_dir().join("talos_worker.log"));
    paths
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn windows_log_path_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/var/log/talos/talos_worker.log"),
        env::temp_dir().join("talos_worker.log"),
    ]
}

fn resolve_log_path() -> PathBuf {
    for path in windows_log_path_candidates() {
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
    windows_log_path_candidates()
        .into_iter()
        .next()
        .unwrap_or_else(|| env::temp_dir().join("talos_worker.log"))
}

fn agent_log_path() -> PathBuf {
    AGENT_LOG_PATH.get_or_init(resolve_log_path).clone()
}

#[cfg(target_os = "windows")]
fn sibling_exe_path(preferred_file_name: &str, legacy_file_name: &str) -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let preferred = dir.join(preferred_file_name);
    if preferred.exists() {
        return Some(preferred);
    }
    Some(dir.join(legacy_file_name))
}

fn write_bootstrap_log(event: &str, data: Option<&str>) {
    use std::io::Write;

    let log_template = agent_log_path();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let line = match data {
        Some(value) => format!("{}  INFO bootstrap: {} {}\n", ts, event, value),
        None => format!("{}  INFO bootstrap: {}\n", ts, event),
    };

    if let Ok(mut file) = talos_log_util::open_today_log_append(&log_template) {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    } else {
        eprintln!("{}", line.trim_end());
    }
}

#[cfg(target_os = "macos")]
fn macos_full_disk_access_home() -> Option<PathBuf> {
    if let Ok(home) = env::var("HOME") {
        let home = home.trim();
        if !home.is_empty() && home != "/var/root" {
            return Some(PathBuf::from(home));
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn macos_full_disk_access_tcc_db_paths(home: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("/Library")
        .join("Application Support")
        .join("com.apple.TCC")
        .join("TCC.db")];
    if let Some(home) = home {
        paths.push(
            home.join("Library")
                .join("Application Support")
                .join("com.apple.TCC")
                .join("TCC.db"),
        );
    }
    paths
}

#[cfg(target_os = "macos")]
fn sqlite_scalar(db_path: &Path, sql: &str) -> Result<String, String> {
    let output = std::process::Command::new("/usr/bin/sqlite3")
        .arg("-batch")
        .arg(db_path)
        .arg(sql)
        .output()
        .map_err(|err| format!("run sqlite3 for {}: {err}", db_path.display()))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(target_os = "macos")]
fn parse_tcc_authorization_value(
    value: &str,
    authorized_value: &str,
) -> Result<Option<bool>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value == authorized_value {
        return Ok(Some(true));
    }
    value
        .parse::<i64>()
        .map(|_| Some(false))
        .map_err(|err| format!("unexpected TCC authorization value {value:?}: {err}"))
}

#[cfg(target_os = "macos")]
fn tcc_full_disk_access_status(db_path: &Path) -> Result<Option<bool>, String> {
    let auth_value_sql = format!(
        "SELECT auth_value FROM access WHERE service='kTCCServiceSystemPolicyAllFiles' AND client='{MACOS_WORKER_BUNDLE_ID}' AND client_type=0 ORDER BY last_modified DESC LIMIT 1;"
    );
    match sqlite_scalar(db_path, &auth_value_sql) {
        Ok(value) => return parse_tcc_authorization_value(&value, "2"),
        Err(err) if err.contains("no such column: auth_value") => {}
        Err(err) => return Err(err),
    }

    let allowed_sql = format!(
        "SELECT allowed FROM access WHERE service='kTCCServiceSystemPolicyAllFiles' AND client='{MACOS_WORKER_BUNDLE_ID}' AND client_type=0 ORDER BY last_modified DESC LIMIT 1;"
    );
    sqlite_scalar(db_path, &allowed_sql)
        .and_then(|value| parse_tcc_authorization_value(&value, "1"))
}

#[cfg(target_os = "macos")]
fn check_macos_full_disk_access() -> MacosFullDiskAccessCheck {
    let home = macos_full_disk_access_home();
    let mut last_error = None;

    for path in macos_full_disk_access_tcc_db_paths(home.as_deref()) {
        if !path.exists() {
            last_error = Some(format!("{}: not found", path.display()));
            continue;
        }
        match tcc_full_disk_access_status(&path) {
            Ok(Some(granted)) => {
                return MacosFullDiskAccessCheck {
                    permission: "full_disk_access",
                    granted,
                    probe_path: Some(path.to_string_lossy().to_string()),
                    error: if granted {
                        None
                    } else {
                        Some(format!(
                            "{MACOS_WORKER_BUNDLE_ID} is present in Full Disk Access but is not authorized"
                        ))
                    },
                };
            }
            Ok(None) => {
                last_error = Some(format!(
                    "no Full Disk Access row for {MACOS_WORKER_BUNDLE_ID} in {}",
                    path.display()
                ));
            }
            Err(err) => {
                last_error = Some(format!("{}: {err}", path.display()));
            }
        }
    }

    MacosFullDiskAccessCheck {
        permission: "full_disk_access",
        granted: false,
        probe_path: None,
        error: last_error.or_else(|| {
            Some(format!(
                "no Full Disk Access authorization row found for {MACOS_WORKER_BUNDLE_ID}"
            ))
        }),
    }
}

#[cfg(target_os = "macos")]
fn macos_full_disk_access_json_output_path() -> Option<String> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--json-output" {
            return args.next();
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn maybe_run_macos_full_disk_access_check() -> Result<Option<i32>> {
    if !env::args().any(|a| a == "--check-full-disk-access") {
        return Ok(None);
    }
    let check = check_macos_full_disk_access();
    if let Some(path) = macos_full_disk_access_json_output_path() {
        fs::write(&path, serde_json::to_vec(&check)?)
            .with_context(|| format!("write Full Disk Access check output: {path}"))?;
    }
    if env::args().any(|a| a == "--json") {
        println!("{}", serde_json::to_string(&check)?);
    } else if check.granted {
        println!("Full Disk Access granted");
    } else {
        println!(
            "Full Disk Access denied{}",
            check
                .error
                .as_deref()
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        );
    }
    Ok(Some(if check.granted { 0 } else { 2 }))
}

#[cfg(target_os = "macos")]
fn macos_update_account_ready(status: &MacosUpdateAccountStatus) -> bool {
    !status.required || matches!(status.status.as_str(), "ready" | "notRequired")
}

#[cfg(target_os = "macos")]
fn macos_startup_permission_helper_recently_surfaced() -> bool {
    let Ok(metadata) = fs::metadata(MACOS_STARTUP_PERMISSION_HELPER_MARKER_PATH) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|elapsed| elapsed < MACOS_STARTUP_PERMISSION_HELPER_COOLDOWN)
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn mark_macos_startup_permission_helper_surfaced() {
    if let Err(err) = fs::write(
        MACOS_STARTUP_PERMISSION_HELPER_MARKER_PATH,
        now_unix_ms().to_string(),
    ) {
        warn!(
            path = MACOS_STARTUP_PERMISSION_HELPER_MARKER_PATH,
            error = %err,
            "failed to write macOS startup permissions helper marker"
        );
    }
}

#[cfg(target_os = "macos")]
async fn surface_macos_startup_permissions_helper_if_needed(
    macos_update_status: MacosUpdateAccountStatus,
) {
    let full_disk_access = check_macos_full_disk_access();
    let mut missing = Vec::new();
    if !full_disk_access.granted {
        missing.push("full_disk_access");
    }

    match macos_desktop::check_active_console_helper_permissions().await {
        Ok(snapshot) => {
            if !snapshot.screen_recording {
                missing.push("screen_recording");
            }
            if !snapshot.accessibility {
                missing.push("accessibility");
            }
        }
        Err(err) => {
            warn!(
                error = %err,
                "macOS startup Worker Helper permission check skipped"
            );
        }
    }

    if !macos_update_account_ready(&macos_update_status) {
        missing.push("macos_update_account");
    }

    if missing.is_empty() {
        info!("macOS startup permissions helper not needed");
        return;
    }
    if macos_startup_permission_helper_recently_surfaced() {
        info!(
            missing = ?missing,
            "macOS startup permissions helper launch suppressed by cooldown"
        );
        return;
    }

    info!(
        missing = ?missing,
        full_disk_access_granted = full_disk_access.granted,
        macos_update_status = %macos_update_status.status,
        macos_update_failure_code = ?macos_update_status.failure_code,
        "macOS startup permissions missing; surfacing Permissions Helper from Worker"
    );
    if macos_desktop::surface_permissions_helper(Some("--after-install")).await {
        mark_macos_startup_permission_helper_surfaced();
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Clone)]
struct BufferedChunk {
    seq: u64,
    chunk: encode::IvfChunk,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Clone)]
struct ControlPipeWriter {
    tx: mpsc::Sender<Vec<u8>>,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Clone, Debug)]
struct CaptureFailure {
    reason: String,
    message: String,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn is_lossy_helper_control(message_type: u8) -> bool {
    matches!(
        message_type,
        talos_protocol::CONTROL_TYPE_MOUSE_MOVE | talos_protocol::CONTROL_TYPE_MOUSE_WHEEL
    )
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
async fn enqueue_helper_control_frame(
    session_id: &str,
    message_type: u8,
    frame: Vec<u8>,
    writer: &ControlPipeWriter,
) {
    match writer.tx.try_send(frame) {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(frame)) => {
            if is_lossy_helper_control(message_type) {
                warn!(
                    session_id = %session_id,
                    message_type,
                    "control frame dropped due backpressure"
                );
                return;
            }
            warn!(
                session_id = %session_id,
                message_type,
                "control queue full; waiting to deliver reliable control frame"
            );
            if writer.tx.send(frame).await.is_err() {
                warn!(
                    session_id = %session_id,
                    message_type,
                    "reliable control frame delivery failed because helper control channel closed"
                );
            }
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_frame)) => {
            warn!(
                session_id = %session_id,
                message_type,
                "control frame delivery failed because helper control channel closed"
            );
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
struct CapturePipeline {
    buffer: Arc<Mutex<VecDeque<BufferedChunk>>>,
    notify: broadcast::Sender<u64>,
    next_seq: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    active_streams: Arc<AtomicU64>,
    created_at_ms: u64,
    last_chunk_at_ms: Arc<AtomicU64>,
    first_frame_at_ms: Arc<AtomicU64>,
    last_frame_at_ms: Arc<AtomicU64>,
    failure: Arc<Mutex<Option<CaptureFailure>>>,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl CapturePipeline {
    fn new() -> Self {
        let (notify, _) = broadcast::channel(1024);
        let created_at_ms = now_unix_ms_u64();
        Self {
            buffer: Arc::new(Mutex::new(VecDeque::with_capacity(
                CAPTURE_BUFFER_MAX_CHUNKS,
            ))),
            notify,
            next_seq: Arc::new(AtomicU64::new(0)),
            stop: Arc::new(AtomicBool::new(false)),
            active_streams: Arc::new(AtomicU64::new(0)),
            created_at_ms,
            last_chunk_at_ms: Arc::new(AtomicU64::new(created_at_ms)),
            first_frame_at_ms: Arc::new(AtomicU64::new(0)),
            last_frame_at_ms: Arc::new(AtomicU64::new(0)),
            failure: Arc::new(Mutex::new(None)),
        }
    }

    fn push_chunk(&self, chunk: encode::IvfChunk) {
        let now_ms = now_unix_ms_u64();
        let is_frame = matches!(
            chunk,
            encode::IvfChunk::Frame(_)
                | encode::IvfChunk::DisplayKeyframe(_)
                | encode::IvfChunk::DisplayDelta(_)
        );
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut guard) = self.buffer.lock() {
            guard.push_back(BufferedChunk { seq, chunk });
            while guard.len() > CAPTURE_BUFFER_MAX_CHUNKS {
                guard.pop_front();
            }
        }
        self.last_chunk_at_ms.store(now_ms, Ordering::SeqCst);
        if is_frame {
            let _ = self.first_frame_at_ms.compare_exchange(
                0,
                now_ms,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
            self.last_frame_at_ms.store(now_ms, Ordering::SeqCst);
        }
        let _ = self.notify.send(seq);
    }

    fn touch_helper_alive(&self) {
        self.last_chunk_at_ms
            .store(now_unix_ms_u64(), Ordering::SeqCst);
    }

    fn set_failure(&self, reason: impl Into<String>, message: impl Into<String>) {
        if let Ok(mut guard) = self.failure.lock() {
            *guard = Some(CaptureFailure {
                reason: reason.into(),
                message: message.into(),
            });
        }
        self.last_chunk_at_ms
            .store(now_unix_ms_u64(), Ordering::SeqCst);
        let _ = self.notify.send(self.next_seq.load(Ordering::SeqCst));
    }

    fn failure(&self) -> Option<CaptureFailure> {
        self.failure.lock().ok().and_then(|guard| guard.clone())
    }

    fn snapshot(&self) -> Vec<BufferedChunk> {
        self.buffer
            .lock()
            .map(|guard| guard.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn subscribe(&self) -> broadcast::Receiver<u64> {
        self.notify.subscribe()
    }

    fn start_stream(&self) {
        self.active_streams.fetch_add(1, Ordering::SeqCst);
    }

    fn finish_stream(&self) -> bool {
        let remaining = self
            .active_streams
            .fetch_sub(1, Ordering::SeqCst)
            .saturating_sub(1);
        remaining == 0
    }

    fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    fn created_at_ms(&self) -> u64 {
        self.created_at_ms
    }

    fn first_frame_at_ms(&self) -> Option<u64> {
        let value = self.first_frame_at_ms.load(Ordering::SeqCst);
        if value == 0 {
            None
        } else {
            Some(value)
        }
    }

    fn last_frame_at_ms(&self) -> Option<u64> {
        let value = self.last_frame_at_ms.load(Ordering::SeqCst);
        if value == 0 {
            None
        } else {
            Some(value)
        }
    }

    fn last_chunk_at_ms(&self) -> u64 {
        self.last_chunk_at_ms.load(Ordering::SeqCst)
    }
}

#[cfg(target_os = "windows")]
fn build_pipe_name(session_id: &str, pipe_instance: u64) -> String {
    let mut sanitized: String = session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if sanitized.is_empty() {
        sanitized = uuid::Uuid::new_v4().to_string();
    }
    format!(r"\\.\pipe\RmmCapture_{}_{}", sanitized, pipe_instance)
}

#[cfg(target_os = "windows")]
fn build_control_pipe_name(session_id: &str, pipe_instance: u64) -> String {
    let mut sanitized: String = session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if sanitized.is_empty() {
        sanitized = uuid::Uuid::new_v4().to_string();
    }
    format!(r"\\.\pipe\RmmControl_{}_{}", sanitized, pipe_instance)
}

#[cfg(target_os = "windows")]
struct PipeSecurityAttributes {
    sa: winapi::um::minwinbase::SECURITY_ATTRIBUTES,
    sd_ptr: winapi::um::winnt::PSECURITY_DESCRIPTOR,
}

#[cfg(target_os = "windows")]
impl PipeSecurityAttributes {
    fn as_mut_ptr(&mut self) -> *mut winapi::um::minwinbase::SECURITY_ATTRIBUTES {
        &mut self.sa as *mut _
    }
}

#[cfg(target_os = "windows")]
impl Drop for PipeSecurityAttributes {
    fn drop(&mut self) {
        if !self.sd_ptr.is_null() {
            unsafe {
                winapi::um::winbase::LocalFree(self.sd_ptr as *mut _);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn build_pipe_sddl(allowed_user_sid: Option<&str>) -> String {
    // Allow:
    // - LocalSystem (service)
    // - Builtin Administrators
    // - The interactive user SID for the helper session (preferred), else INTERACTIVE as fallback.
    //
    // This replaces the previous NULL DACL (world-accessible) behavior.
    let mut sddl = String::from("D:");
    sddl.push_str("(A;;GA;;;SY)");
    sddl.push_str("(A;;GA;;;BA)");
    if let Some(sid) = allowed_user_sid.filter(|v| !v.trim().is_empty()) {
        sddl.push_str(&format!("(A;;GA;;;{sid})"));
    } else {
        // Interactive Users (best-effort fallback).
        sddl.push_str("(A;;GA;;;IU)");
    }
    sddl
}

#[cfg(target_os = "windows")]
fn build_pipe_security_attributes(
    allowed_user_sid: Option<&str>,
) -> Result<PipeSecurityAttributes, String> {
    use winapi::shared::sddl::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use winapi::um::errhandlingapi::GetLastError;

    let sddl = build_pipe_sddl(allowed_user_sid);
    let sddl_wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let mut sd_ptr: winapi::um::winnt::PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl_wide.as_ptr(),
            SDDL_REVISION_1 as u32,
            &mut sd_ptr,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 || sd_ptr.is_null() {
        let err = unsafe { GetLastError() };
        return Err(format!(
            "ConvertStringSecurityDescriptorToSecurityDescriptorW failed: {err}"
        ));
    }
    Ok(PipeSecurityAttributes {
        sa: winapi::um::minwinbase::SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<winapi::um::minwinbase::SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd_ptr as *mut _,
            bInheritHandle: 0,
        },
        sd_ptr,
    })
}

#[cfg(target_os = "windows")]
fn try_session_user_sid_string(session_id: u32) -> Option<String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use winapi::shared::minwindef::DWORD;
    use winapi::shared::sddl::ConvertSidToStringSidW;
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::securitybaseapi::GetTokenInformation;
    use winapi::um::winnt::{TokenUser, HANDLE, TOKEN_USER};
    use winapi::um::wtsapi32::WTSQueryUserToken;

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if WTSQueryUserToken(session_id, &mut token) == 0 || token.is_null() {
            return None;
        }

        let mut needed: DWORD = 0;
        let _ = GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
        if needed == 0 {
            CloseHandle(token);
            return None;
        }

        let mut buf = vec![0u8; needed as usize];
        let ok = GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr() as *mut _,
            needed,
            &mut needed,
        );
        if ok == 0 {
            CloseHandle(token);
            return None;
        }

        let token_user = &*(buf.as_ptr() as *const TOKEN_USER);
        let sid = token_user.User.Sid;
        if sid.is_null() {
            CloseHandle(token);
            return None;
        }

        let mut sid_wide: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(sid, &mut sid_wide) == 0 || sid_wide.is_null() {
            CloseHandle(token);
            return None;
        }

        // ConvertSidToStringSidW allocates via LocalAlloc; free with LocalFree.
        let mut len = 0usize;
        while *sid_wide.add(len) != 0 {
            len += 1;
        }
        let sid_slice = std::slice::from_raw_parts(sid_wide, len);
        let sid_string = OsString::from_wide(sid_slice).to_string_lossy().to_string();
        winapi::um::winbase::LocalFree(sid_wide as *mut _);
        CloseHandle(token);

        if sid_string.trim().is_empty() {
            None
        } else {
            Some(sid_string)
        }
    }
}

#[cfg(target_os = "windows")]
fn create_named_pipe_server(
    pipe_name: &str,
    allowed_user_sid: Option<&str>,
) -> Result<winapi::um::winnt::HANDLE, String> {
    use winapi::shared::winerror::{ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY};
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::handleapi::INVALID_HANDLE_VALUE;
    use winapi::um::namedpipeapi::CreateNamedPipeW;
    use winapi::um::winbase::{
        PIPE_ACCESS_INBOUND, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        PIPE_WAIT,
    };

    let pipe_wide: Vec<u16> = pipe_name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut sa_guard = build_pipe_security_attributes(allowed_user_sid)
        .or_else(|_| build_pipe_security_attributes(None))
        .ok();
    let sa_ptr = sa_guard
        .as_mut()
        .map_or(std::ptr::null_mut(), |g| g.as_mut_ptr());
    let mut last_err: u32 = 0;
    for attempt in 1..=60 {
        let handle = unsafe {
            CreateNamedPipeW(
                pipe_wide.as_ptr(),
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                512 * 1024,
                512 * 1024,
                0,
                sa_ptr,
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            return Ok(handle);
        }

        let err = unsafe { GetLastError() };
        last_err = err;
        if err == ERROR_PIPE_BUSY || err == ERROR_ACCESS_DENIED {
            if attempt == 1 || attempt % 10 == 0 {
                warn!(
                    pipe = %pipe_name,
                    attempt = attempt,
                    error = err,
                    "CreateNamedPipeW transient failure; retrying"
                );
            }
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        return Err(format!("CreateNamedPipeW failed: {}", err));
    }
    Err(format!(
        "CreateNamedPipeW failed after retries: {}",
        last_err
    ))
}

#[cfg(target_os = "windows")]
fn create_named_pipe_server_outbound(
    pipe_name: &str,
    allowed_user_sid: Option<&str>,
) -> Result<winapi::um::winnt::HANDLE, String> {
    use winapi::shared::winerror::{ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY};
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::handleapi::INVALID_HANDLE_VALUE;
    use winapi::um::namedpipeapi::CreateNamedPipeW;
    use winapi::um::winbase::{
        PIPE_ACCESS_DUPLEX, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        PIPE_WAIT,
    };

    let pipe_wide: Vec<u16> = pipe_name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut sa_guard = build_pipe_security_attributes(allowed_user_sid)
        .or_else(|_| build_pipe_security_attributes(None))
        .ok();
    let sa_ptr = sa_guard
        .as_mut()
        .map_or(std::ptr::null_mut(), |g| g.as_mut_ptr());
    let mut last_err: u32 = 0;
    for attempt in 1..=60 {
        let handle = unsafe {
            CreateNamedPipeW(
                pipe_wide.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64 * 1024,
                64 * 1024,
                0,
                sa_ptr,
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            return Ok(handle);
        }

        let err = unsafe { GetLastError() };
        last_err = err;
        if err == ERROR_PIPE_BUSY || err == ERROR_ACCESS_DENIED {
            if attempt == 1 || attempt % 10 == 0 {
                warn!(
                    pipe = %pipe_name,
                    attempt = attempt,
                    error = err,
                    "CreateNamedPipeW outbound transient failure; retrying"
                );
            }
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        return Err(format!("CreateNamedPipeW failed: {}", err));
    }
    Err(format!(
        "CreateNamedPipeW failed after retries: {}",
        last_err
    ))
}

#[cfg(target_os = "windows")]
fn read_pipe_exact(handle: winapi::um::winnt::HANDLE, buf: &mut [u8]) -> Result<(), String> {
    use winapi::shared::minwindef::DWORD;
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::fileapi::ReadFile;

    let mut offset: usize = 0;
    while offset < buf.len() {
        let mut read: DWORD = 0;
        let ok = unsafe {
            ReadFile(
                handle,
                buf[offset..].as_mut_ptr() as *mut _,
                (buf.len() - offset) as DWORD,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            return Err(format!("ReadFile failed: {}", err));
        }
        if read == 0 {
            return Err("ReadFile returned 0 bytes".to_string());
        }
        offset += read as usize;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn write_pipe_all(handle: winapi::um::winnt::HANDLE, buf: &[u8]) -> Result<(), String> {
    use winapi::shared::minwindef::DWORD;
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::fileapi::WriteFile;

    let mut offset: usize = 0;
    while offset < buf.len() {
        let mut written: DWORD = 0;
        let ok = unsafe {
            WriteFile(
                handle,
                buf[offset..].as_ptr() as *const _,
                (buf.len() - offset) as DWORD,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            return Err(format!("WriteFile failed: {}", err));
        }
        if written == 0 {
            return Err("WriteFile wrote 0 bytes".to_string());
        }
        offset += written as usize;
    }
    Ok(())
}

#[cfg(target_os = "windows")]
async fn setup_control_pipe_writer(
    session_id: String,
    pipe_handle_value: usize,
    writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    auth_token: String,
) {
    use winapi::shared::winerror::ERROR_PIPE_CONNECTED;
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::namedpipeapi::ConnectNamedPipe;

    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(1024);
    {
        let mut guard = writers.write().await;
        guard.insert(session_id.clone(), ControlPipeWriter { tx });
    }
    let writers_cleanup = writers.clone();
    let cleanup_session_id = session_id.clone();
    let rt = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let pipe_handle = pipe_handle_value as winapi::um::winnt::HANDLE;
        let ok = unsafe { ConnectNamedPipe(pipe_handle, std::ptr::null_mut()) };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            if err != ERROR_PIPE_CONNECTED {
                warn!(
                    session_id = %cleanup_session_id,
                    error = err,
                    "control pipe ConnectNamedPipe failed"
                );
                rt.block_on(async {
                    let mut guard = writers_cleanup.write().await;
                    guard.remove(&cleanup_session_id);
                });
                unsafe {
                    CloseHandle(pipe_handle);
                }
                return;
            }
        }

        // Authenticate the helper before allowing control frames.
        // Helper sends: magic + u16(version BE) + u16(len BE) + token bytes.
        let mut magic = [0u8; 4];
        if let Err(err) = read_pipe_exact(pipe_handle, &mut magic) {
            warn!(
                session_id = %cleanup_session_id,
                error = %err,
                "control pipe handshake read failed"
            );
            rt.block_on(async {
                let mut guard = writers_cleanup.write().await;
                guard.remove(&cleanup_session_id);
            });
            unsafe {
                CloseHandle(pipe_handle);
            }
            return;
        }
        if magic != HELPER_PIPE_HANDSHAKE_MAGIC {
            warn!(
                session_id = %cleanup_session_id,
                "control pipe handshake magic mismatch"
            );
            rt.block_on(async {
                let mut guard = writers_cleanup.write().await;
                guard.remove(&cleanup_session_id);
            });
            unsafe {
                CloseHandle(pipe_handle);
            }
            return;
        }
        let mut version_buf = [0u8; 2];
        if let Err(err) = read_pipe_exact(pipe_handle, &mut version_buf) {
            warn!(
                session_id = %cleanup_session_id,
                error = %err,
                "control pipe handshake version read failed"
            );
            rt.block_on(async {
                let mut guard = writers_cleanup.write().await;
                guard.remove(&cleanup_session_id);
            });
            unsafe {
                CloseHandle(pipe_handle);
            }
            return;
        }
        let version = u16::from_be_bytes(version_buf);
        if version != HELPER_PIPE_PROTOCOL_VERSION {
            warn!(
                session_id = %cleanup_session_id,
                received_version = version,
                supported_version = HELPER_PIPE_PROTOCOL_VERSION,
                "control pipe handshake version mismatch"
            );
            rt.block_on(async {
                let mut guard = writers_cleanup.write().await;
                guard.remove(&cleanup_session_id);
            });
            unsafe {
                CloseHandle(pipe_handle);
            }
            return;
        }
        let mut len_buf = [0u8; 2];
        if let Err(err) = read_pipe_exact(pipe_handle, &mut len_buf) {
            warn!(
                session_id = %cleanup_session_id,
                error = %err,
                "control pipe handshake length read failed"
            );
            rt.block_on(async {
                let mut guard = writers_cleanup.write().await;
                guard.remove(&cleanup_session_id);
            });
            unsafe {
                CloseHandle(pipe_handle);
            }
            return;
        }
        let token_len = u16::from_be_bytes(len_buf) as usize;
        if token_len == 0 || token_len > HELPER_PIPE_MAX_AUTH_TOKEN_LEN {
            warn!(
                session_id = %cleanup_session_id,
                token_len = token_len,
                "control pipe handshake token length invalid"
            );
            rt.block_on(async {
                let mut guard = writers_cleanup.write().await;
                guard.remove(&cleanup_session_id);
            });
            unsafe {
                CloseHandle(pipe_handle);
            }
            return;
        }
        let mut token_bytes = vec![0u8; token_len];
        if let Err(err) = read_pipe_exact(pipe_handle, &mut token_bytes) {
            warn!(
                session_id = %cleanup_session_id,
                error = %err,
                "control pipe handshake token read failed"
            );
            rt.block_on(async {
                let mut guard = writers_cleanup.write().await;
                guard.remove(&cleanup_session_id);
            });
            unsafe {
                CloseHandle(pipe_handle);
            }
            return;
        }
        if token_bytes != auth_token.as_bytes() {
            warn!(
                session_id = %cleanup_session_id,
                "control pipe handshake token mismatch"
            );
            rt.block_on(async {
                let mut guard = writers_cleanup.write().await;
                guard.remove(&cleanup_session_id);
            });
            unsafe {
                CloseHandle(pipe_handle);
            }
            return;
        }

        while let Some(buf) = rx.blocking_recv() {
            if let Err(err) = write_pipe_all(pipe_handle, &buf) {
                warn!(session_id = %cleanup_session_id, error = %err, "control pipe write failed");
                break;
            }
        }
        unsafe {
            CloseHandle(pipe_handle);
        }
        rt.block_on(async {
            let mut guard = writers_cleanup.write().await;
            guard.remove(&cleanup_session_id);
        });
    });
}

#[cfg(target_os = "windows")]
fn spawn_pipe_reader(
    pipe_handle: winapi::um::winnt::HANDLE,
    pipeline: Arc<CapturePipeline>,
    stop_flag: Arc<AtomicBool>,
    session_id: String,
    pipe_name: String,
    target_session_id: u32,
    auth_token: String,
) {
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::namedpipeapi::ConnectNamedPipe;

    let closed = Arc::new(AtomicBool::new(false));
    let closed_watcher = closed.clone();
    let stop_watcher = stop_flag.clone();
    let handle_watcher = SendHandle(pipe_handle);
    let handle_reader = SendHandle(pipe_handle);

    std::thread::spawn(move || {
        let handle = handle_watcher;
        while !stop_watcher.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
        if !closed_watcher.swap(true, Ordering::SeqCst) {
            unsafe {
                CloseHandle(handle.0);
            }
        }
    });

    std::thread::spawn(move || {
        let handle = handle_reader;
        let ok = unsafe { ConnectNamedPipe(handle.0, std::ptr::null_mut()) };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            if err != winapi::shared::winerror::ERROR_PIPE_CONNECTED {
                warn!(error = err, "ConnectNamedPipe failed");
                if !closed.swap(true, Ordering::SeqCst) {
                    unsafe {
                        CloseHandle(handle.0);
                    }
                }
                return;
            }
        }
        const AUTH_TAG: u8 = 3;

        let mut chunk_count: u64 = 0;
        let mut authenticated = false;
        loop {
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }
            let mut tag = [0u8; 1];
            if let Err(err) = read_pipe_exact(handle.0, &mut tag) {
                warn!(error = %err, "capture pipe tag read failed");
                break;
            }
            let mut len_buf = [0u8; 4];
            if let Err(err) = read_pipe_exact(handle.0, &mut len_buf) {
                warn!(error = %err, "capture pipe length read failed");
                break;
            }
            let len = u32::from_le_bytes(len_buf) as usize;
            if len > 128 * 1024 * 1024 {
                warn!(len, "pipe chunk too large; aborting");
                break;
            }
            let mut payload = vec![0u8; len];
            if let Err(err) = read_pipe_exact(handle.0, &mut payload) {
                warn!(error = %err, "capture pipe payload read failed");
                break;
            }

            let chunk_tag = tag[0];
            if !authenticated {
                if chunk_tag != AUTH_TAG {
                    warn!(
                        session_id = %session_id,
                        pipe = %pipe_name,
                        target_session_id = target_session_id,
                        tag = chunk_tag,
                        "capture pipe missing auth handshake"
                    );
                    break;
                }
                if payload.len() < 6 || payload.len() > 6 + HELPER_PIPE_MAX_AUTH_TOKEN_LEN {
                    warn!(
                        session_id = %session_id,
                        pipe = %pipe_name,
                        target_session_id = target_session_id,
                        len = payload.len(),
                        "capture pipe auth payload invalid length"
                    );
                    break;
                }
                if payload[..4] != HELPER_PIPE_HANDSHAKE_MAGIC {
                    warn!(
                        session_id = %session_id,
                        pipe = %pipe_name,
                        target_session_id = target_session_id,
                        "capture pipe auth magic mismatch"
                    );
                    break;
                }
                let version = u16::from_be_bytes([payload[4], payload[5]]);
                if version != HELPER_PIPE_PROTOCOL_VERSION {
                    warn!(
                        session_id = %session_id,
                        pipe = %pipe_name,
                        target_session_id = target_session_id,
                        received_version = version,
                        supported_version = HELPER_PIPE_PROTOCOL_VERSION,
                        "capture pipe auth version mismatch"
                    );
                    break;
                }
                if &payload[6..] != auth_token.as_bytes() {
                    warn!(
                        session_id = %session_id,
                        pipe = %pipe_name,
                        target_session_id = target_session_id,
                        "capture pipe auth token mismatch"
                    );
                    break;
                }
                authenticated = true;
                info!(
                    session_id = %session_id,
                    pipe = %pipe_name,
                    target_session_id = target_session_id,
                    "capture pipe authenticated"
                );
                if should_fault_inject_capture_pipe_fail_once(&session_id) {
                    warn!(
                        session_id = %session_id,
                        pipe = %pipe_name,
                        target_session_id = target_session_id,
                        "fault injection: simulating capture pipe failure after authentication"
                    );
                    break;
                }
                continue;
            }
            let chunk = match tag[0] {
                0 => encode::IvfChunk::Metadata(payload),
                1 => {
                    if payload.len() != 32 {
                        warn!(len = payload.len(), "invalid IVF header length");
                        continue;
                    }
                    let mut h = [0u8; 32];
                    h.copy_from_slice(&payload);
                    encode::IvfChunk::Header(h)
                }
                2 => encode::IvfChunk::Frame(payload),
                4 => encode::IvfChunk::DisplayKeyframe(payload),
                5 => encode::IvfChunk::DisplayDelta(payload),
                6 => {
                    pipeline.touch_helper_alive();
                    continue;
                }
                _ => {
                    warn!(tag = tag[0], "unknown pipe chunk tag");
                    continue;
                }
            };
            pipeline.push_chunk(chunk);
            chunk_count = chunk_count.saturating_add(1);
            if chunk_count == 1 || chunk_count.is_multiple_of(240) {
                info!(
                    chunk_count = chunk_count,
                    chunk_tag = chunk_tag,
                    "capture chunks flowing"
                );
            }
        }

        if !closed.swap(true, Ordering::SeqCst) {
            unsafe {
                CloseHandle(handle.0);
            }
        }
    });
}

#[derive(Clone)]
struct Config {
    server_url: String,
    agent_token: String,
    inventory_interval_secs: u64,
    agent_id_path: PathBuf,
    reconnect_max_secs: u64,
    ws_connect_timeout_secs: u64,
}

#[derive(Serialize)]
struct InventoryUpdate {
    agent_id: String,
    hostname: String,
    os: String,
    ip: String,
    version: String,
    inventory: InventorySnapshot,
}

#[derive(Serialize)]
struct InventorySnapshot {
    system: SystemInfo,
    cpu: CpuInfo,
    memory: MemoryInfo,
    disks: Vec<DiskInfo>,
    networks: Vec<NetworkInfo>,
    logged_in_users: Vec<LoggedInUserInfo>,
}

#[derive(Serialize)]
struct SystemInfo {
    hostname: String,
    os_name: String,
    os_version: String,
    kernel_version: String,
    distro: String,
    architecture: String,
    uptime_seconds: u64,
    boot_time: u64,
    ip_addresses: Vec<String>,
    last_seen: String,
}

#[derive(Serialize)]
struct CpuInfo {
    brand: String,
    cores: u32,
    frequency_mhz: u64,
}

#[derive(Serialize)]
struct MemoryInfo {
    total_bytes: u64,
    available_bytes: u64,
}

#[derive(Serialize, Clone)]
struct DiskInfo {
    name: String,
    mount_point: String,
    total_bytes: u64,
    available_bytes: u64,
    file_system: String,
}

#[derive(Serialize, Clone)]
struct NetworkAddressInfo {
    address: String,
    family: String,
    prefix: u8,
    netmask: String,
}

#[derive(Serialize, Clone)]
struct NetworkInfo {
    name: String,
    received_bytes: u64,
    transmitted_bytes: u64,
    ips: Vec<NetworkAddressInfo>,
    gateways: Vec<String>,
    dns_servers: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProcessInfo {
    pid: u32,
    name: String,
    cpu: f32,
    memory: u64,
    virtual_memory: u64,
}

#[derive(Serialize)]
struct LoggedInUserInfo {
    username: String,
    terminal: Option<String>,
    host: Option<String>,
}

#[cfg(any(target_os = "windows", target_family = "unix"))]
#[derive(Deserialize)]
struct SessionEndPayload {
    session_id: String,
    #[serde(default)]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct FetchDetailsRequest {
    request_id: String,
}

#[derive(Serialize)]
struct DeviceDetailsResponse {
    request_id: String,
    details: Value,
}

#[derive(Deserialize)]
struct RdpSessionsRequestPayload {
    request_id: String,
}

#[derive(Serialize)]
struct RdpSessionsResponsePayload {
    request_id: String,
    sessions: Vec<RdpSessionInfoWire>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RdpSessionInfoWire {
    session_id: u32,
    logical_session_id: u32,
    native_session_id: u32,
    kind: String,
    win_station: String,
    user_name: String,
    state: String,
}

#[cfg(target_os = "windows")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionPongMetaPayload {
    #[serde(rename = "type")]
    message_type: &'static str,
    echoed_at_ms: u64,
    agent_received_at_ms: u64,
}

struct PipelineRunner;

fn agent_platform() -> AgentPlatform {
    if cfg!(target_os = "windows") {
        AgentPlatform::Windows
    } else if cfg!(target_os = "linux") {
        AgentPlatform::Linux
    } else if cfg!(target_os = "macos") {
        AgentPlatform::Macos
    } else {
        AgentPlatform::Unknown
    }
}

fn agent_feature_capabilities() -> AgentFeatureCapabilities {
    let mut features = AgentFeatureCapabilities::for_platform(agent_platform());
    if cfg!(target_os = "macos") && !is_elevated() {
        features.system_shell = false;
    }
    features
}

#[cfg(target_os = "windows")]
fn spawn_wts_session_monitor() {
    std::thread::spawn(|| {
        use winapi::shared::minwindef::DWORD;
        use winapi::shared::ntdef::HANDLE;

        extern "system" {
            fn WTSWaitSystemEvent(
                hServer: HANDLE,
                EventMask: DWORD,
                pEventFlags: *mut DWORD,
            ) -> i32;
        }

        const WTS_EVENT_NONE: DWORD = 0x0000_0000;
        const WTS_EVENT_CREATE: DWORD = 0x0000_0001;
        const WTS_EVENT_DELETE: DWORD = 0x0000_0002;
        const WTS_EVENT_RENAME: DWORD = 0x0000_0004;
        const WTS_EVENT_CONNECT: DWORD = 0x0000_0008;
        const WTS_EVENT_DISCONNECT: DWORD = 0x0000_0010;
        const WTS_EVENT_LOGON: DWORD = 0x0000_0020;
        const WTS_EVENT_LOGOFF: DWORD = 0x0000_0040;
        const WTS_EVENT_STATECHANGE: DWORD = 0x0000_0080;
        const WTS_EVENT_LICENSE: DWORD = 0x0000_0100;
        const WTS_EVENT_ALL: DWORD = WTS_EVENT_CREATE
            | WTS_EVENT_DELETE
            | WTS_EVENT_RENAME
            | WTS_EVENT_CONNECT
            | WTS_EVENT_DISCONNECT
            | WTS_EVENT_LOGON
            | WTS_EVENT_LOGOFF
            | WTS_EVENT_STATECHANGE
            | WTS_EVENT_LICENSE;

        loop {
            let mut flags: DWORD = WTS_EVENT_NONE;
            let ok = unsafe {
                WTSWaitSystemEvent(
                    std::ptr::null_mut() as HANDLE,
                    WTS_EVENT_ALL,
                    &mut flags as *mut _,
                )
            };
            if ok == 0 {
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            if flags != WTS_EVENT_NONE {
                control::request_desktop_context_refresh();
                // Desktop-affecting events: trigger helper process rebuild.
                // Helpers launched with a specific desktop token cannot detect
                // transitions via OpenInputDesktop; they must be relaunched.
                const REBUILD_MASK: DWORD =
                    WTS_EVENT_LOGON | WTS_EVENT_LOGOFF | WTS_EVENT_CONNECT | WTS_EVENT_DISCONNECT;
                if flags & REBUILD_MASK != 0 {
                    control::request_pipeline_rebuild();
                }
                info!(
                    flags = flags,
                    "WTS session change signaled desktop context refresh"
                );
            }
        }
    });
}

impl PipelineRunner {
    fn new() -> Self {
        Self
    }

    fn get_capabilities(&self) -> RemoteDesktopCapabilities {
        #[cfg(target_os = "macos")]
        {
            return RemoteDesktopCapabilities {
                codecs: vec![
                    REMOTE_DESKTOP_CODEC_H264.to_string(),
                    REMOTE_DESKTOP_CODEC_VP8.to_string(),
                    REMOTE_DESKTOP_CODEC_BGRA_ATLAS_COMMANDS.to_string(),
                    REMOTE_DESKTOP_CODEC_SCREENSHOT_BGRA.to_string(),
                ],
                encoding: "software".to_string(),
                transports: vec!["quic".to_string(), "relay".to_string()],
                platform: agent_platform(),
                features: agent_feature_capabilities(),
                display_profiles: vec![
                    RemoteDesktopDisplayProfile::modern_gpu(),
                    RemoteDesktopDisplayProfile::experimental(),
                    RemoteDesktopDisplayProfile {
                        id: REMOTE_DESKTOP_PROFILE_MODERN_CPU.to_string(),
                        protocol: talos_protocol::REMOTE_DESKTOP_PROTOCOL_LEGACY_IVF.to_string(),
                        codec: REMOTE_DESKTOP_CODEC_VP8.to_string(),
                        compression: talos_protocol::REMOTE_DESKTOP_COMPRESSION_IVF.to_string(),
                        priority: 1,
                    },
                    RemoteDesktopDisplayProfile::legacy(),
                    RemoteDesktopDisplayProfile::screenshot_only(),
                ],
                selected_display_profile: Some(REMOTE_DESKTOP_PROFILE_MODERN_GPU.to_string()),
            };
        }

        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            return RemoteDesktopCapabilities {
                codecs: Vec::new(),
                encoding: "unsupported".to_string(),
                transports: vec!["quic".to_string(), "relay".to_string()],
                platform: agent_platform(),
                features: agent_feature_capabilities(),
                display_profiles: Vec::new(),
                selected_display_profile: None,
            };
        }

        #[cfg(target_os = "windows")]
        {
            let display_profiles =
                encode::advertised_display_profiles_for_effective_processing_mode(
                    "agent capabilities",
                );
            let selected_display_profile = Some(
                encode::selected_display_profile_for_effective_processing_mode(
                    "agent capabilities",
                    None,
                ),
            );
            let mut codecs = Vec::new();
            for profile in &display_profiles {
                if !codecs.iter().any(|codec| codec == &profile.codec) {
                    codecs.push(profile.codec.clone());
                }
            }
            RemoteDesktopCapabilities {
                codecs,
                encoding: "software".to_string(),
                transports: vec!["quic".to_string(), "relay".to_string()],
                platform: agent_platform(),
                features: agent_feature_capabilities(),
                display_profiles,
                selected_display_profile,
            }
        }
    }
}

/// Agent run loop (config load, reconnect loop). Used by main and by Windows service.
async fn run_agent() -> Result<()> {
    cleanup_legacy_local_dumps();

    let config = load_config()?;
    #[cfg(target_os = "windows")]
    let display_processing_mode = encode::effective_display_processing_mode_label("agent startup");
    #[cfg(target_os = "macos")]
    let display_processing_mode = REMOTE_DESKTOP_PROFILE_MODERN_GPU;
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let display_processing_mode = "unsupported";
    info!(
        server_url = %config.server_url,
        agent_token_set = !config.agent_token.is_empty(),
        display_processing_mode = display_processing_mode,
        "RMM agent config (from env)"
    );
    let agent_id = load_or_create_agent_id(&config.agent_id_path)?;

    let hostname = hostname::get()
        .ok()
        .and_then(|name| name.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string());

    let os = sysinfo::System::long_os_version()
        .or_else(sysinfo::System::name)
        .unwrap_or_else(|| "unknown".to_string());

    let version = env!("CARGO_PKG_VERSION").to_string();
    info!("agent-owned update checks removed; Talos Supervisor owns worker updates");
    let boot_session_id = compute_boot_session_id();
    let online = Arc::new(AtomicBool::new(false));
    let snapshot_in_progress = Arc::new(AtomicBool::new(false));
    let background_collection_started = Arc::new(AtomicBool::new(false));

    let mut sys = System::new_all();
    let mut disks = Disks::new_with_refreshed_list();
    let mut networks = Networks::new_with_refreshed_list();
    let punch_sockets = Arc::new(RwLock::new(HashMap::<String, Arc<UdpSocket>>::new()));
    #[cfg(target_os = "windows")]
    let control_queue = control::ControlQueue::new(512);
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let control_pipe_writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>> =
        Arc::new(RwLock::new(HashMap::new()));
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let helper_target_sessions: Arc<RwLock<HashMap<String, u32>>> =
        Arc::new(RwLock::new(HashMap::new()));
    #[cfg(target_os = "windows")]
    spawn_wts_session_monitor();

    let (live_events_tx, _) = broadcast::channel::<Value>(2048);
    let live_event_backlog = Arc::new(tokio::sync::Mutex::new(VecDeque::<Value>::with_capacity(
        2048,
    )));
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    start_live_event_stream_bridge(
        boot_session_id.clone(),
        live_events_tx.clone(),
        live_event_backlog.clone(),
    );

    let mut backoff = Backoff::new(config.reconnect_max_secs);

    loop {
        let ip = current_ip();
        online.store(true, Ordering::SeqCst);
        match connect_and_run(
            &config,
            &agent_id,
            &hostname,
            &os,
            &version,
            &mut sys,
            &mut disks,
            &mut networks,
            &ip,
            online.clone(),
            snapshot_in_progress.clone(),
            background_collection_started.clone(),
            boot_session_id.clone(),
            live_events_tx.clone(),
            live_event_backlog.clone(),
            punch_sockets.clone(),
            #[cfg(target_os = "windows")]
            control_queue.clone(),
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            control_pipe_writers.clone(),
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            helper_target_sessions.clone(),
        )
        .await
        {
            Ok(()) => {
                backoff.reset();
            }
            Err(err) => {
                online.store(false, Ordering::SeqCst);
                warn!(error = ?err, "connection error");
            }
        }

        let delay = backoff.next_delay();
        sleep(delay).await;
    }
}

fn compute_boot_session_id() -> String {
    let boot_time = System::boot_time();
    if boot_time > 0 {
        return format!("boot-{boot_time}");
    }

    let epoch_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("boot-{epoch_secs}")
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn jitter_secs_0_to_30m() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() % (30 * 60))
        .unwrap_or(0)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
const UNIX_INITIAL_SNAPSHOT_RETRY_SECS: u64 = 30;

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn start_unix_snapshot_sender(
    outbound_tx: mpsc::UnboundedSender<Message>,
    agent_id: String,
    hostname: String,
    boot_session_id: String,
    snapshot_in_progress: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        let platform_label = if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        };
        let mut initial_snapshot_queued = false;
        loop {
            if initial_snapshot_queued {
                sleep(Duration::from_secs(12 * 60 * 60 + jitter_secs_0_to_30m())).await;
            }

            if snapshot_in_progress
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                let retry_secs = if initial_snapshot_queued {
                    5
                } else {
                    UNIX_INITIAL_SNAPSHOT_RETRY_SECS
                };
                info!(
                    platform = platform_label,
                    initial_snapshot = !initial_snapshot_queued,
                    retry_secs,
                    "unix full_snapshot skipped because another snapshot is in progress"
                );
                sleep(Duration::from_secs(retry_secs)).await;
                continue;
            }

            let result = collect_full_snapshot_update(&agent_id, &hostname, &boot_session_id).await;
            snapshot_in_progress.store(false, Ordering::SeqCst);

            match result {
                Ok(payload) => {
                    let envelope = OutgoingEnvelope {
                        message_type: "full_snapshot",
                        data: payload,
                    };
                    let text = match serde_json::to_string(&envelope) {
                        Ok(text) => text,
                        Err(error) => {
                            warn!(
                                %error,
                                platform = platform_label,
                                "failed to serialize unix full_snapshot envelope"
                            );
                            continue;
                        }
                    };
                    if outbound_tx.send(Message::Text(text)).is_err() {
                        warn!(
                            platform = platform_label,
                            "unix full_snapshot sender stopped because websocket queue closed"
                        );
                        break;
                    }
                    info!(
                        platform = platform_label,
                        initial_snapshot = !initial_snapshot_queued,
                        "unix full_snapshot queued for websocket send"
                    );
                    initial_snapshot_queued = true;
                }
                Err(error) => {
                    warn!(
                        %error,
                        platform = platform_label,
                        initial_snapshot = !initial_snapshot_queued,
                        retry_secs = UNIX_INITIAL_SNAPSHOT_RETRY_SECS,
                        "unix full snapshot collection failed"
                    );
                    if !initial_snapshot_queued {
                        sleep(Duration::from_secs(UNIX_INITIAL_SNAPSHOT_RETRY_SECS)).await;
                    }
                }
            }
        }
    });
}

#[cfg(target_os = "windows")]
fn start_windows_snapshot_sender(
    outbound_tx: mpsc::UnboundedSender<Message>,
    agent_id: String,
    hostname: String,
    boot_session_id: String,
    snapshot_in_progress: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        let mut initial_snapshot_queued = false;
        loop {
            if initial_snapshot_queued {
                sleep(Duration::from_secs(12 * 60 * 60 + jitter_secs_0_to_30m())).await;
            }

            let label = if initial_snapshot_queued {
                "periodic"
            } else {
                "startup"
            };
            match collect_and_queue_full_snapshot(
                &outbound_tx,
                &agent_id,
                &hostname,
                &boot_session_id,
                None,
                &snapshot_in_progress,
                label,
            )
            .await
            {
                Ok(pending_update_count) => {
                    info!(
                        snapshot_kind = label,
                        pending_update_count, "windows full_snapshot queued for websocket send"
                    );
                    initial_snapshot_queued = true;
                }
                Err(error) => {
                    warn!(
                        %error,
                        snapshot_kind = label,
                        initial_snapshot = !initial_snapshot_queued,
                        "windows full snapshot failed"
                    );
                    if outbound_tx.is_closed() {
                        break;
                    }
                    if !initial_snapshot_queued {
                        sleep(Duration::from_secs(30)).await;
                    }
                }
            }
        }
    });
}

fn cleanup_legacy_local_dumps() {
    let temp_dir = PathBuf::from(r"C:\temp");
    let Ok(entries) = std::fs::read_dir(&temp_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if (!name.starts_with("rmm_collection_") || !name.ends_with(".json"))
            && name != "rmm_full_snapshot.json"
            && name != "rmm_events.jsonl"
        {
            continue;
        }
        if let Err(error) = std::fs::remove_file(&path) {
            warn!(path = %path.display(), %error, "failed to remove legacy local dump");
        } else {
            info!(path = %path.display(), "removed legacy local dump");
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_service {
    use std::ffi::OsString;
    use std::sync::mpsc;
    use std::time::Duration;

    use windows_service::{
        define_windows_service,
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher, Result,
    };

    const SERVICE_NAME: &str = "TalosWorker";
    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

    pub fn run() -> Result<()> {
        crate::write_bootstrap_log("service_dispatcher_start", Some(SERVICE_NAME));
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
    }

    define_windows_service!(ffi_service_main, talos_worker_service_main);

    fn talos_worker_service_main(_arguments: Vec<OsString>) {
        crate::write_bootstrap_log("service_main_enter", None);
        if let Err(e) = run_service() {
            crate::write_bootstrap_log("service_main_err", Some(&e.to_string()));
            tracing::error!(error = %e, "Talos Worker service failed");
        }
    }

    fn run_service() -> Result<()> {
        crate::write_bootstrap_log("service_run_start", None);
        let (shutdown_tx, shutdown_rx) = mpsc::channel();

        let event_handler = move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                ServiceControl::Stop => {
                    let _ = shutdown_tx.send(());
                    ServiceControlHandlerResult::NoError
                }
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };

        let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;
        crate::write_bootstrap_log("service_control_handler_registered", None);

        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;
        crate::write_bootstrap_log("service_status_running", None);

        std::thread::spawn(move || {
            crate::write_bootstrap_log("service_worker_start", None);
            if let Err(err) = crate::prepare_agent_runtime() {
                crate::write_bootstrap_log("service_prepare_runtime_err", Some(&err.to_string()));
                return;
            }
            let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
            let run_result = rt.block_on(async {
                // run_agent() already performs initial data collection once.
                crate::run_agent().await
            });
            if let Err(err) = run_result {
                crate::write_bootstrap_log("service_run_agent_err", Some(&err.to_string()));
            }
        });

        let _ = shutdown_rx.recv();
        crate::write_bootstrap_log("service_stop_requested", None);

        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;
        crate::write_bootstrap_log("service_status_stopped", None);

        std::process::exit(0);
    }
}

fn prepare_agent_runtime() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|e| anyhow!("install rustls CryptoProvider: {:?}", e))?;

    load_dotenv();
    write_bootstrap_log("dotenv_loaded", None);
    strip_legacy_log_env_vars();
    if let Err(err) = init_file_logging() {
        write_bootstrap_log("init_file_logging_err", Some(&err.to_string()));
        return Err(anyhow!("failed to initialize file logging: {}", err));
    }
    write_bootstrap_log("init_file_logging_ok", None);
    #[cfg(target_os = "windows")]
    if let Err(err) = ensure_windows_firewall_rules() {
        let error_chain = format!("{:#}", err);
        warn!(error = %err, error_chain = %error_chain, "failed to ensure Windows firewall rules");
    }
    #[cfg(target_os = "windows")]
    if let Err(err) = set_process_priority_above_normal() {
        warn!(error = %err, "failed to raise agent process priority");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn set_process_priority_above_normal() -> Result<()> {
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::processthreadsapi::GetCurrentProcess;
    use winapi::um::processthreadsapi::SetPriorityClass;
    use winapi::um::winbase::ABOVE_NORMAL_PRIORITY_CLASS;

    let ok = unsafe { SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS) };
    if ok == 0 {
        let err = unsafe { GetLastError() };
        anyhow::bail!("SetPriorityClass failed: {}", err);
    }
    info!("agent process priority set to ABOVE_NORMAL");
    Ok(())
}

#[cfg(target_os = "windows")]
fn ensure_windows_firewall_rules() -> Result<()> {
    let exe_path = env::current_exe().context("resolve current exe for firewall rules")?;
    let exe_path = exe_path
        .canonicalize()
        .unwrap_or_else(|_| exe_path.clone())
        .to_string_lossy()
        .to_string();
    let exe_path = normalize_windows_firewall_application_path(&exe_path);

    let rules = [
        FirewallRuleSpec {
            name: "Talos Worker QUIC UDP Inbound",
            direction: FirewallRuleDirection::Inbound,
            protocol: FirewallRuleProtocol::Udp,
            local_ports: None,
            remote_addresses: Some("LocalSubnet"),
            remote_ports: None,
        },
        FirewallRuleSpec {
            name: "Talos Worker QUIC UDP Outbound",
            direction: FirewallRuleDirection::Outbound,
            protocol: FirewallRuleProtocol::Udp,
            local_ports: None,
            remote_addresses: None,
            remote_ports: None,
        },
        FirewallRuleSpec {
            name: "Talos Worker Relay TCP 443 Outbound",
            direction: FirewallRuleDirection::Outbound,
            protocol: FirewallRuleProtocol::Tcp,
            local_ports: None,
            remote_addresses: None,
            remote_ports: Some("443"),
        },
    ];

    let policy = windows_firewall_policy()?;
    for rule in &rules {
        ensure_windows_firewall_rule(&policy, rule, &exe_path)?;
    }

    Ok(())
}

#[cfg(target_os = "windows")]
struct FirewallRuleSpec {
    name: &'static str,
    direction: FirewallRuleDirection,
    protocol: FirewallRuleProtocol,
    local_ports: Option<&'static str>,
    remote_addresses: Option<&'static str>,
    remote_ports: Option<&'static str>,
}

#[cfg(target_os = "windows")]
enum FirewallRuleDirection {
    Inbound,
    Outbound,
}

#[cfg(target_os = "windows")]
enum FirewallRuleProtocol {
    Tcp,
    Udp,
}

#[cfg(target_os = "windows")]
fn ensure_windows_firewall_rule(
    policy: &windows::Win32::NetworkManagement::WindowsFirewall::INetFwPolicy2,
    spec: &FirewallRuleSpec,
    exe_path: &str,
) -> Result<()> {
    if windows_firewall_rule_exists(policy, spec.name)? {
        info!(rule = spec.name, "Windows firewall rule already present");
        return Ok(());
    }

    debug!(
        rule = spec.name,
        direction = match spec.direction { FirewallRuleDirection::Inbound => "inbound", FirewallRuleDirection::Outbound => "outbound" },
        protocol = match spec.protocol { FirewallRuleProtocol::Tcp => "tcp", FirewallRuleProtocol::Udp => "udp" },
        local_ports = ?spec.local_ports,
        remote_addresses = ?spec.remote_addresses,
        remote_ports = ?spec.remote_ports,
        exe_path = %exe_path,
        "creating Windows firewall rule"
    );

    if let Err(err) = add_windows_firewall_rule(policy, spec, exe_path) {
        let error_chain = format!("{:#}", err);
        warn!(
            rule = spec.name,
            direction = match spec.direction { FirewallRuleDirection::Inbound => "inbound", FirewallRuleDirection::Outbound => "outbound" },
            protocol = match spec.protocol { FirewallRuleProtocol::Tcp => "tcp", FirewallRuleProtocol::Udp => "udp" },
            local_ports = ?spec.local_ports,
            remote_addresses = ?spec.remote_addresses,
            remote_ports = ?spec.remote_ports,
            exe_path = %exe_path,
            error = %err,
            error_chain = %error_chain,
            "Windows firewall rule creation failed"
        );
        return Err(err);
    }
    info!(rule = spec.name, "Windows firewall rule created");
    Ok(())
}

#[cfg(target_os = "windows")]
fn normalize_windows_firewall_application_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", rest)
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

#[cfg(target_os = "windows")]
fn windows_firewall_policy(
) -> Result<windows::Win32::NetworkManagement::WindowsFirewall::INetFwPolicy2> {
    use windows::Win32::NetworkManagement::WindowsFirewall::NetFwPolicy2;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER).context("create INetFwPolicy2")
    }
}

#[cfg(target_os = "windows")]
fn windows_firewall_rule_exists(
    policy: &windows::Win32::NetworkManagement::WindowsFirewall::INetFwPolicy2,
    rule_name: &str,
) -> Result<bool> {
    use windows::core::BSTR;

    let rules = unsafe { policy.Rules() }.context("get firewall rules collection")?;
    match unsafe { rules.Item(&BSTR::from(rule_name)) } {
        Ok(_) => Ok(true),
        Err(_) => {
            warn!(rule = rule_name, "Windows firewall rule missing");
            Ok(false)
        }
    }
}

#[cfg(target_os = "windows")]
fn add_windows_firewall_rule(
    policy: &windows::Win32::NetworkManagement::WindowsFirewall::INetFwPolicy2,
    spec: &FirewallRuleSpec,
    exe_path: &str,
) -> Result<()> {
    use windows::core::BSTR;
    use windows::Win32::Foundation::VARIANT_TRUE;
    use windows::Win32::NetworkManagement::WindowsFirewall::{
        INetFwRule, NetFwRule, NET_FW_ACTION_ALLOW, NET_FW_IP_PROTOCOL_TCP, NET_FW_IP_PROTOCOL_UDP,
        NET_FW_PROFILE2_ALL, NET_FW_RULE_DIR_IN, NET_FW_RULE_DIR_OUT,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

    let direction_label = match spec.direction {
        FirewallRuleDirection::Inbound => "inbound",
        FirewallRuleDirection::Outbound => "outbound",
    };
    let protocol_label = match spec.protocol {
        FirewallRuleProtocol::Tcp => "tcp",
        FirewallRuleProtocol::Udp => "udp",
    };
    let rules = unsafe { policy.Rules() }.context("get firewall rules collection")?;
    let rule: INetFwRule = unsafe { CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER) }
        .context("create INetFwRule")?;

    unsafe {
        debug!(
            rule = spec.name,
            value = spec.name,
            "setting firewall rule name"
        );
        rule.SetName(&BSTR::from(spec.name))
            .context("set firewall rule name")?;
        debug!(
            rule = spec.name,
            value = spec.name,
            "setting firewall rule description"
        );
        rule.SetDescription(&BSTR::from(spec.name))
            .context("set firewall rule description")?;
        debug!(rule = spec.name, exe_path = %exe_path, "setting firewall rule application");
        rule.SetApplicationName(&BSTR::from(exe_path))
            .context("set firewall rule application")?;
        debug!(rule = spec.name, enabled = true, "enabling firewall rule");
        rule.SetEnabled(VARIANT_TRUE)
            .context("enable firewall rule")?;
        debug!(
            rule = spec.name,
            action = "allow",
            "setting firewall rule action"
        );
        rule.SetAction(NET_FW_ACTION_ALLOW)
            .context("set firewall rule action")?;
        debug!(
            rule = spec.name,
            profiles = { NET_FW_PROFILE2_ALL.0 },
            "setting firewall rule profiles"
        );
        rule.SetProfiles(NET_FW_PROFILE2_ALL.0)
            .context("set firewall rule profiles")?;
    }

    match spec.direction {
        FirewallRuleDirection::Inbound => unsafe {
            debug!(
                rule = spec.name,
                direction = "inbound",
                "setting firewall rule direction"
            );
            rule.SetDirection(NET_FW_RULE_DIR_IN)
                .context("set inbound firewall direction")?
        },
        FirewallRuleDirection::Outbound => unsafe {
            debug!(
                rule = spec.name,
                direction = "outbound",
                "setting firewall rule direction"
            );
            rule.SetDirection(NET_FW_RULE_DIR_OUT)
                .context("set outbound firewall direction")?
        },
    }

    match spec.protocol {
        FirewallRuleProtocol::Tcp => unsafe {
            debug!(
                rule = spec.name,
                protocol = "tcp",
                "setting firewall rule protocol"
            );
            rule.SetProtocol(NET_FW_IP_PROTOCOL_TCP.0)
                .context("set TCP firewall protocol")?
        },
        FirewallRuleProtocol::Udp => unsafe {
            debug!(
                rule = spec.name,
                protocol = "udp",
                "setting firewall rule protocol"
            );
            rule.SetProtocol(NET_FW_IP_PROTOCOL_UDP.0)
                .context("set UDP firewall protocol")?
        },
    }

    if let Some(local_ports) = spec.local_ports {
        unsafe {
            debug!(
                rule = spec.name,
                local_ports = local_ports,
                "setting firewall local ports"
            );
            rule.SetLocalPorts(&BSTR::from(local_ports))
                .context("set firewall local ports")?;
        }
    }

    if let Some(remote_ports) = spec.remote_ports {
        unsafe {
            debug!(
                rule = spec.name,
                remote_ports = remote_ports,
                "setting firewall remote ports"
            );
            rule.SetRemotePorts(&BSTR::from(remote_ports))
                .context("set firewall remote ports")?;
        }
    }

    if let Some(remote_addresses) = spec.remote_addresses {
        unsafe {
            debug!(
                rule = spec.name,
                remote_addresses = remote_addresses,
                "setting firewall remote addresses"
            );
            rule.SetRemoteAddresses(&BSTR::from(remote_addresses))
                .context("set firewall remote addresses")?;
        }
    }

    debug!(
        rule = spec.name,
        direction = direction_label,
        protocol = protocol_label,
        local_ports = ?spec.local_ports,
        remote_ports = ?spec.remote_ports,
        remote_addresses = ?spec.remote_addresses,
        exe_path = %exe_path,
        "adding firewall rule to collection"
    );
    unsafe { rules.Add(&rule) }.context("add firewall rule")?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    write_bootstrap_log("main_start", None);

    #[cfg(target_os = "macos")]
    if let Some(code) = maybe_run_macos_full_disk_access_check()? {
        std::process::exit(code);
    }

    // Headless auto-logon: one-time setup so a console session exists at boot (no user present).
    #[cfg(target_os = "windows")]
    if env::args().any(|a| a == "--configure-headless") {
        let user = env::var("RMM_AGENT_HEADLESS_USER").unwrap_or_else(|_| String::new());
        let pass = env::var("RMM_AGENT_HEADLESS_PASSWORD").unwrap_or_else(|_| String::new());
        let domain = env::var("RMM_AGENT_HEADLESS_DOMAIN").ok();
        match display::configure_headless_auto_logon(&user, &pass, domain.as_deref()) {
            Ok(()) => {
                eprintln!("Headless auto-logon configured. Reboot the machine for display capture to work when the agent runs as a service.");
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("configure-headless failed: {}", e);
                std::process::exit(1);
            }
        }
    }

    #[cfg(target_os = "windows")]
    if env::args().any(|a| a == "--display-init-helper") {
        let code = match display::run_display_init_helper() {
            Ok(()) => 0,
            Err(()) => 1,
        };
        std::process::exit(code);
    }

    #[cfg(target_os = "windows")]
    {
        match windows_service::run() {
            Ok(()) => return Ok(()),
            Err(::windows_service::Error::Winapi(io_err))
                if io_err.raw_os_error() == Some(1063) =>
            {
                // Not launched by SCM; allow interactive/console mode.
                write_bootstrap_log("service_dispatcher_not_service_context", None);
            }
            Err(err) => {
                write_bootstrap_log("service_dispatcher_start_err", Some(&err.to_string()));
                return Err(anyhow!("service dispatcher start failed: {}", err));
            }
        }
    }

    if let Err(err) = prepare_agent_runtime() {
        eprintln!("{}", err);
        return Err(err);
    }

    #[cfg(target_os = "macos")]
    {
        talos_worker::macos_update_account::start_ipc_server();
        let status = talos_worker::macos_update_account::ensure_startup_status();
        tracing::info!(
            status = %status.status,
            failure_code = ?status.failure_code,
            "macOS update account startup check completed"
        );
        tokio::spawn(async move {
            surface_macos_startup_permissions_helper_if_needed(status).await;
        });
    }

    write_bootstrap_log("main_run_agent_begin", None);
    tracing::info!("console run_agent starting");
    run_agent().await
}

fn load_dotenv() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut candidates = Vec::new();
    if manifest
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "src-tauri")
    {
        candidates.push(manifest.join("..").join("..").join(".env"));
    }
    candidates.push(manifest.join("..").join(".env"));
    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join(".env"));
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("..").join("..").join(".env"));
        }
    }
    for path in candidates {
        if path.is_file() {
            let _ = dotenvy::from_path(&path);
            break;
        }
    }
}

async fn connect_and_run(
    config: &Config,
    agent_id: &str,
    hostname: &str,
    os: &str,
    version: &str,
    sys: &mut System,
    disks: &mut Disks,
    networks: &mut Networks,
    ip: &str,
    _online: Arc<AtomicBool>,
    snapshot_in_progress: Arc<AtomicBool>,
    background_collection_started: Arc<AtomicBool>,
    boot_session_id: String,
    live_events_tx: broadcast::Sender<Value>,
    live_event_backlog: Arc<tokio::sync::Mutex<VecDeque<Value>>>,
    punch_sockets: Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    #[cfg(target_os = "windows")] control_queue: control::ControlQueue,
    #[cfg(any(target_os = "windows", target_os = "macos"))] control_pipe_writers: Arc<
        RwLock<HashMap<String, ControlPipeWriter>>,
    >,
    #[cfg(any(target_os = "windows", target_os = "macos"))] helper_target_sessions: Arc<
        RwLock<HashMap<String, u32>>,
    >,
) -> Result<()> {
    let mut request = config
        .server_url
        .as_str()
        .into_client_request()
        .context("build websocket request")?;

    let value = format!("Bearer {}", config.agent_token);
    request
        .headers_mut()
        .insert("Authorization", value.parse().context("auth header")?);

    let (ws_stream, _) = timeout(
        Duration::from_secs(config.ws_connect_timeout_secs),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| anyhow!("connect websocket timed out"))?
    .map_err(|e| {
        use tokio_tungstenite::tungstenite::error::Error as WsError;
        match &e {
            WsError::Http(resp) if resp.status().as_u16() == 401 => {
                anyhow::anyhow!(
                    "server returned 401 Unauthorized — missing or invalid RMM_AGENT_TOKEN for enrollment"
                )
            }
            _ => anyhow::anyhow!("{}", e),
        }
    })
    .context("connect websocket")?;

    info!("connected to rmm server");
    let should_start_background_collection = background_collection_started
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    if should_start_background_collection {
        info!("first server connection established; starting background data collection");
        #[cfg(all(
            not(target_os = "windows"),
            not(target_os = "linux"),
            not(target_os = "macos")
        ))]
        info!("full snapshot background collection disabled on this platform");
    }

    let (mut write, mut read) = ws_stream.split();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<Message>();
    let (patch_jobs_tx, patch_jobs_rx) = mpsc::unbounded_channel::<patching::PatchJobsEnvelope>();
    let (patch_plan_tx, patch_plan_rx) =
        mpsc::unbounded_channel::<patching::PatchActionPlanEnvelope>();
    let (patch_wake_tx, patch_wake_rx) = mpsc::unbounded_channel::<()>();
    let (remediation_jobs_tx, remediation_jobs_rx) =
        mpsc::unbounded_channel::<remediation::RemediationJobsEnvelope>();
    let (remediation_wake_tx, remediation_wake_rx) = mpsc::unbounded_channel::<()>();
    let (preflight_jobs_tx, preflight_jobs_rx) =
        mpsc::unbounded_channel::<feature_upgrade_preflight::FeatureUpgradePreflightJobsEnvelope>();
    let (preflight_wake_tx, preflight_wake_rx) = mpsc::unbounded_channel::<()>();
    let (stage_iso_jobs_tx, stage_iso_jobs_rx) =
        mpsc::unbounded_channel::<feature_upgrade_stage_iso::FeatureUpgradeStageIsoJobsEnvelope>();
    let (stage_iso_wake_tx, stage_iso_wake_rx) = mpsc::unbounded_channel::<()>();
    let (start_upgrade_jobs_tx, start_upgrade_jobs_rx) =
        mpsc::unbounded_channel::<feature_upgrade_start::FeatureUpgradeStartJobsEnvelope>();
    let (start_upgrade_wake_tx, start_upgrade_wake_rx) = mpsc::unbounded_channel::<()>();
    #[cfg(target_os = "macos")]
    talos_worker::macos_update_account::configure_status_reporter(
        agent_id.to_string(),
        outbound_tx.clone(),
    );

    #[cfg(target_os = "windows")]
    start_windows_snapshot_sender(
        outbound_tx.clone(),
        agent_id.to_string(),
        hostname.to_string(),
        boot_session_id.clone(),
        snapshot_in_progress.clone(),
    );
    let mut live_events_rx = live_events_tx.subscribe();
    let pending_patch_snapshot = Arc::new(AtomicBool::new(false));

    patching::start_patch_manager(
        agent_id.to_string(),
        hostname.to_string(),
        boot_session_id.clone(),
        outbound_tx.clone(),
        patch_jobs_rx,
        patch_plan_rx,
        patch_wake_rx,
        snapshot_in_progress.clone(),
        pending_patch_snapshot.clone(),
    );
    remediation::start_remediation_manager(
        outbound_tx.clone(),
        remediation_jobs_rx,
        remediation_wake_rx,
    );
    feature_upgrade_preflight::start_preflight_manager(
        agent_id.to_string(),
        hostname.to_string(),
        boot_session_id.clone(),
        outbound_tx.clone(),
        preflight_jobs_rx,
        preflight_wake_rx,
        snapshot_in_progress.clone(),
    );
    feature_upgrade_stage_iso::start_stage_iso_manager(
        agent_id.to_string(),
        outbound_tx.clone(),
        stage_iso_jobs_rx,
        stage_iso_wake_rx,
    );
    feature_upgrade_start::start_start_manager(
        agent_id.to_string(),
        hostname.to_string(),
        boot_session_id.clone(),
        outbound_tx.clone(),
        start_upgrade_jobs_rx,
        start_upgrade_wake_rx,
        snapshot_in_progress.clone(),
    );

    {
        let outbound_tx_events = outbound_tx.clone();
        let events_agent_id = agent_id.to_string();
        let live_event_backlog = live_event_backlog.clone();
        tokio::spawn(async move {
            const EVENTS_BATCH_MAX: usize = 100;
            const EVENTS_FLUSH_SECS: u64 = 5;
            let mut batch: Vec<Value> = Vec::with_capacity(EVENTS_BATCH_MAX);
            let mut ticker = tokio::time::interval(Duration::from_secs(EVENTS_FLUSH_SECS));
            ticker.tick().await;

            {
                let mut backlog = live_event_backlog.lock().await;
                while let Some(event) = backlog.pop_front() {
                    batch.push(event);
                    if batch.len() >= EVENTS_BATCH_MAX {
                        drop(backlog);
                        if !queue_telemetry_events(
                            &outbound_tx_events,
                            &events_agent_id,
                            &mut batch,
                        ) {
                            return;
                        }
                        backlog = live_event_backlog.lock().await;
                    }
                }
            }
            if !batch.is_empty()
                && !queue_telemetry_events(&outbound_tx_events, &events_agent_id, &mut batch)
            {
                return;
            }

            loop {
                tokio::select! {
                    recv_result = live_events_rx.recv() => {
                        match recv_result {
                            Ok(event) => {
                                batch.push(event);
                                if batch.len() >= EVENTS_BATCH_MAX
                                    && !queue_telemetry_events(&outbound_tx_events, &events_agent_id, &mut batch)
                                {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                warn!(skipped, "live telemetry stream lagged");
                            }
                        }
                    }
                    _ = ticker.tick() => {
                        if !batch.is_empty()
                            && !queue_telemetry_events(&outbound_tx_events, &events_agent_id, &mut batch)
                        {
                            break;
                        }
                    }
                }
            }
        });
    }

    let hello = AgentHello {
        agent_id: agent_id.to_string(),
        hostname: hostname.to_string(),
        os: os.to_string(),
        ip: ip.to_string(),
        local_addrs: Some(local_addrs()),
        version: Some(version.to_string()),
        is_admin: is_elevated(),
        platform: agent_platform(),
        features: agent_feature_capabilities(),
    };

    send_message(&mut write, "agent_hello", hello).await?;
    #[cfg(target_os = "macos")]
    {
        let status = talos_worker::macos_update_account::ensure_startup_status();
        send_message(
            &mut write,
            "macos_update_account_status",
            MacosUpdateAccountStatusPayload {
                agent_id: agent_id.to_string(),
                status,
            },
        )
        .await?;
    }
    #[cfg(target_os = "linux")]
    if let Some(credential) = talos_worker::linux_account::ensure_managed_shell_credential()
        .context("ensure managed Linux shell credential")?
    {
        let payload = talos_protocol::LinuxShellCredentialPayload {
            agent_id: agent_id.to_string(),
            username: credential.username,
            password: credential.password,
            credential_id: credential.credential_id,
            version: credential.version,
            generated_at: credential.generated_at,
        };
        send_message(&mut write, "linux_shell_credential", payload).await?;
    }
    send_inventory(
        &mut write, agent_id, hostname, os, version, sys, disks, networks, ip,
    )
    .await?;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    start_unix_snapshot_sender(
        outbound_tx.clone(),
        agent_id.to_string(),
        hostname.to_string(),
        boot_session_id.clone(),
        snapshot_in_progress.clone(),
    );

    let mut interval = tokio::time::interval(Duration::from_secs(config.inventory_interval_secs));
    interval.tick().await;
    let runner = PipelineRunner::new();
    let relay_sessions = Arc::new(RwLock::new(HashSet::new()));
    let file_transfer_relay_sessions = Arc::new(RwLock::new(HashSet::new()));
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let chat_relay_sessions = Arc::new(RwLock::new(HashSet::new()));
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let chat_tunnels: Arc<RwLock<HashMap<String, chat::ChatTunnelMeta>>> =
        Arc::new(RwLock::new(HashMap::new()));
    #[cfg(any(target_os = "windows", target_family = "unix"))]
    let shell_prepared_sessions: Arc<RwLock<HashMap<String, PreparedShellSession>>> =
        Arc::new(RwLock::new(HashMap::new()));
    #[cfg(any(target_os = "windows", target_family = "unix"))]
    let shell_relay_sessions = Arc::new(RwLock::new(HashSet::new()));
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let capture_pipelines: Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>> =
        Arc::new(RwLock::new(HashMap::new()));

    // Background task: watch for desktop transitions and rebuild helper pipelines.
    // Helper processes launched with a specific desktop token (e.g. winlogon) cannot
    // detect desktop switches via OpenInputDesktop. The agent must kill and relaunch them.
    #[cfg(target_os = "windows")]
    {
        let rebuild_pipelines = capture_pipelines.clone();
        let rebuild_writers = control_pipe_writers.clone();
        let rebuild_targets = helper_target_sessions.clone();
        tokio::spawn(async move {
            let mut last_epoch = control::pipeline_rebuild_epoch();
            let mut last_rebuild_at = Instant::now() - Duration::from_secs(60);
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let current_epoch = control::pipeline_rebuild_epoch();
                if current_epoch == last_epoch {
                    continue;
                }
                // Debounce: WTS fires multiple events in rapid succession during login.
                // Wait for events to settle before rebuilding.
                info!("[REBUILD] desktop transition detected (epoch {}), waiting for events to settle", current_epoch);
                tokio::time::sleep(Duration::from_millis(2000)).await;
                // Re-read epoch in case more events arrived during debounce
                last_epoch = control::pipeline_rebuild_epoch();
                // WTS can emit clustered transition signals for one user action.
                // Keep a short cooldown to suppress immediate duplicates only.
                const REBUILD_COOLDOWN_SECS: u64 = 5;
                if last_rebuild_at.elapsed() < Duration::from_secs(REBUILD_COOLDOWN_SECS) {
                    info!(
                        elapsed_ms = last_rebuild_at.elapsed().as_millis() as u64,
                        cooldown_ms = REBUILD_COOLDOWN_SECS * 1000,
                        "[REBUILD] skipping transition rebuild due cooldown"
                    );
                    continue;
                }

                // Collect active session IDs before tearing down.
                // Prefer capture pipeline sessions when present. If the capture map is
                // temporarily empty, fall back to control writer sessions so we can
                // still rebuild an active stream.
                let (pipeline_sessions, writer_sessions, session_ids): (
                    Vec<String>,
                    Vec<String>,
                    Vec<String>,
                ) = {
                    let mut pipeline_ids: Vec<String> = {
                        let guard = rebuild_pipelines.read().await;
                        guard.keys().cloned().collect()
                    };
                    let mut writer_ids: Vec<String> = {
                        let guard = rebuild_writers.read().await;
                        guard.keys().cloned().collect()
                    };
                    pipeline_ids.sort();
                    writer_ids.sort();
                    let selected = if pipeline_ids.is_empty() {
                        writer_ids.clone()
                    } else {
                        pipeline_ids.clone()
                    };
                    (pipeline_ids, writer_ids, selected)
                };
                info!(
                    pipeline_sessions = ?pipeline_sessions,
                    writer_sessions = ?writer_sessions,
                    selected_sessions = ?session_ids,
                    selected_source = if pipeline_sessions.is_empty() { "writers_fallback" } else { "pipelines" },
                    "[REBUILD] session selection"
                );
                if session_ids.is_empty() {
                    info!("[REBUILD] no active pipelines to rebuild");
                    continue;
                }
                info!(
                    sessions = ?session_ids,
                    "[REBUILD] tearing down {} pipeline(s) for desktop transition",
                    session_ids.len()
                );

                // Stop all existing pipelines (kills pipe readers, which kills capture helpers)
                {
                    let mut guard = rebuild_pipelines.write().await;
                    let _session_ids_rebuild: Vec<String> = guard.keys().cloned().collect();
                    for (sid, pipeline) in guard.drain() {
                        info!(session_id = %sid, "[REBUILD] stopping pipeline");
                        pipeline.request_stop();
                    }
                }
                // Clear control pipe writers (drops senders, writer threads exit, pipes close)
                {
                    let mut guard = rebuild_writers.write().await;
                    let removed: Vec<String> = guard.keys().cloned().collect();
                    guard.clear();
                    info!(writers = ?removed, "[REBUILD] cleared control pipe writers");
                }
                // Wait for old helpers to die (pipe close propagation)
                tokio::time::sleep(Duration::from_millis(1000)).await;

                // Rebuild pipelines for each session.
                // Retry if transient pipe creation races leave us without a writer.
                for sid in &session_ids {
                    let mut rebuilt = false;
                    for attempt in 1..=12 {
                        info!(
                            session_id = %sid,
                            attempt = attempt,
                            "[REBUILD] recreating pipeline on new desktop"
                        );
                        ensure_capture_pipeline(
                            sid.clone(),
                            &rebuild_pipelines,
                            rebuild_writers.clone(),
                            rebuild_targets.clone(),
                        )
                        .await;

                        let has_writer = {
                            let guard = rebuild_writers.read().await;
                            guard.contains_key(sid)
                        };
                        if has_writer {
                            rebuilt = true;
                            break;
                        }

                        warn!(
                            session_id = %sid,
                            attempt = attempt,
                            "[REBUILD] recreate attempt has no control writer; retrying"
                        );
                        {
                            let mut guard = rebuild_pipelines.write().await;
                            guard.remove(sid);
                        }
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                    if !rebuilt {
                        warn!(
                            session_id = %sid,
                            "[REBUILD] failed to recreate pipeline after retries"
                        );
                    }
                }
                info!("[REBUILD] pipeline rebuild complete");
                last_rebuild_at = Instant::now();
            }
        });
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let watchdog_pipelines = capture_pipelines.clone();
        let watchdog_writers = control_pipe_writers.clone();
        #[cfg(target_os = "windows")]
        let watchdog_targets = helper_target_sessions.clone();
        tokio::spawn(async move {
            let startup_grace_secs = env::var("RMM_HELPER_STARTUP_GRACE_SECS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(12);
            let frame_stall_secs = env::var("RMM_HELPER_FRAME_STALL_SECS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(10);
            let poll_interval = Duration::from_secs(2);
            let restart_window = Duration::from_secs(60);
            let max_restarts_per_window = 4usize;
            let mut restart_history: HashMap<String, VecDeque<Instant>> = HashMap::new();

            loop {
                tokio::time::sleep(poll_interval).await;
                let now_ms = now_unix_ms_u64();
                let writer_sessions = {
                    let guard = watchdog_writers.read().await;
                    guard.keys().cloned().collect::<HashSet<_>>()
                };
                let sessions = {
                    let guard = watchdog_pipelines.read().await;
                    guard
                        .iter()
                        .map(|(session_id, pipeline)| {
                            (
                                session_id.clone(),
                                pipeline.clone(),
                                pipeline.active_streams.load(Ordering::SeqCst),
                                pipeline.created_at_ms(),
                                pipeline.first_frame_at_ms(),
                                pipeline.last_frame_at_ms(),
                                pipeline.last_chunk_at_ms(),
                                pipeline.stop_flag().load(Ordering::SeqCst),
                            )
                        })
                        .collect::<Vec<_>>()
                };

                for (
                    session_id,
                    pipeline,
                    active_streams,
                    created_at_ms,
                    first_frame_at_ms,
                    last_frame_at_ms,
                    last_chunk_at_ms,
                    stop_requested,
                ) in sessions
                {
                    if active_streams == 0 || stop_requested {
                        continue;
                    }

                    let age_ms = now_ms.saturating_sub(created_at_ms);
                    let has_writer = writer_sessions.contains(&session_id);
                    #[cfg(target_os = "macos")]
                    let screenshot_only_after_first_frame = macos_session_capture_mode(&session_id)
                        == MacosDesktopCaptureMode::Screenshot
                        && first_frame_at_ms.is_some();
                    let unhealthy_reason =
                        if !has_writer && age_ms > startup_grace_secs.saturating_mul(1000) {
                            Some("missing_control_writer")
                        } else if first_frame_at_ms.is_none()
                            && age_ms > startup_grace_secs.saturating_mul(1000)
                        {
                            Some("no_frames_after_startup")
                        } else {
                            let silent_ms = now_ms.saturating_sub(last_chunk_at_ms);
                            if silent_ms > frame_stall_secs.saturating_mul(1000) {
                                #[cfg(target_os = "macos")]
                                if screenshot_only_after_first_frame {
                                    continue;
                                }
                                Some("frame_stall")
                            } else {
                                None
                            }
                        };

                    let Some(reason) = unhealthy_reason else {
                        continue;
                    };

                    let history = restart_history.entry(session_id.clone()).or_default();
                    let now = Instant::now();
                    while let Some(oldest) = history.front().copied() {
                        if now.duration_since(oldest) > restart_window {
                            history.pop_front();
                        } else {
                            break;
                        }
                    }
                    if history.len() >= max_restarts_per_window {
                        warn!(
                            session_id = %session_id,
                            active_streams,
                            reason,
                            restarts_in_window = history.len(),
                            "helper watchdog suppressing restart because restart budget was exhausted"
                        );
                        continue;
                    }

                    history.push_back(now);
                    warn!(
                        session_id = %session_id,
                        active_streams,
                        reason,
                        age_ms,
                        first_frame_at_ms = first_frame_at_ms.unwrap_or(0),
                        last_frame_at_ms = last_frame_at_ms.unwrap_or(0),
                        last_chunk_at_ms = last_chunk_at_ms,
                        has_writer,
                        "helper watchdog rebuilding unhealthy capture pipeline"
                    );

                    #[cfg(target_os = "macos")]
                    let rebuild_result = {
                        let capture_mode = macos_session_capture_mode(&session_id);
                        macos_desktop::rebuild_capture_pipeline(
                            &session_id,
                            capture_mode,
                            &watchdog_pipelines,
                            &watchdog_writers,
                        )
                        .await
                    };
                    #[cfg(target_os = "windows")]
                    let rebuild_result = rebuild_pipeline_for_target_session(
                        &session_id,
                        &watchdog_pipelines,
                        &watchdog_writers,
                        &watchdog_targets,
                    )
                    .await;

                    if let Err(error) = rebuild_result {
                        warn!(
                            session_id = %session_id,
                            reason,
                            error = %error,
                            "helper watchdog rebuild failed"
                        );
                    } else {
                        let _ = pipeline;
                        info!(
                            session_id = %session_id,
                            reason,
                            "helper watchdog rebuild completed"
                        );
                    }
                }
            }
        });
    }

    loop {
        tokio::select! {
            Some(outbound) = outbound_rx.recv() => {
                write.send(outbound).await?;
            }
            _ = interval.tick() => {
                let ip = current_ip();
                send_inventory(&mut write, agent_id, hostname, os, version, sys, disks, networks, &ip).await?;
            }
            message = read.next() => {
                match message {
                    Some(Ok(Message::Ping(payload))) => {
                        write.send(Message::Pong(payload)).await?;
                    }
                    Some(Ok(Message::Text(text))) => {
                        handle_server_message(
                            &text,
                            &mut write,
                            &runner,
                            &punch_sockets,
                            &relay_sessions,
                            &file_transfer_relay_sessions,
                            #[cfg(any(target_os = "windows", target_os = "macos"))]
                            &chat_relay_sessions,
                            #[cfg(any(target_os = "windows", target_os = "macos"))]
                            &chat_tunnels,
                            #[cfg(any(target_os = "windows", target_family = "unix"))]
                            &shell_prepared_sessions,
                            #[cfg(any(target_os = "windows", target_family = "unix"))]
                            &shell_relay_sessions,
                            agent_id.to_string(),
                            hostname.to_string(),
                            version.to_string(),
                            boot_session_id.clone(),
                            &outbound_tx,
                            &patch_jobs_tx,
                            &patch_plan_tx,
                            &patch_wake_tx,
                            &preflight_jobs_tx,
                            &preflight_wake_tx,
                            &stage_iso_jobs_tx,
                            &stage_iso_wake_tx,
                            &start_upgrade_jobs_tx,
                            &start_upgrade_wake_tx,
                            &remediation_jobs_tx,
                            &remediation_wake_tx,
                            &snapshot_in_progress,
                            #[cfg(any(target_os = "windows", target_os = "macos"))]
                            &capture_pipelines,
                            #[cfg(target_os = "windows")]
                            control_queue.clone(),
                            #[cfg(any(target_os = "windows", target_os = "macos"))]
                            control_pipe_writers.clone(),
                            #[cfg(any(target_os = "windows", target_os = "macos"))]
                            helper_target_sessions.clone(),
                        )
                        .await?;
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        if let Ok(text) = String::from_utf8(bytes) {
                            handle_server_message(
                                &text,
                                &mut write,
                                &runner,
                                &punch_sockets,
                                &relay_sessions,
                                &file_transfer_relay_sessions,
                                #[cfg(any(target_os = "windows", target_os = "macos"))]
                                &chat_relay_sessions,
                                #[cfg(any(target_os = "windows", target_os = "macos"))]
                                &chat_tunnels,
                                #[cfg(any(target_os = "windows", target_family = "unix"))]
                                &shell_prepared_sessions,
                                #[cfg(any(target_os = "windows", target_family = "unix"))]
                                &shell_relay_sessions,
                                agent_id.to_string(),
                                hostname.to_string(),
                                version.to_string(),
                                boot_session_id.clone(),
                                &outbound_tx,
                                &patch_jobs_tx,
                                &patch_plan_tx,
                                &patch_wake_tx,
                                &preflight_jobs_tx,
                                &preflight_wake_tx,
                                &stage_iso_jobs_tx,
                                &stage_iso_wake_tx,
                                &start_upgrade_jobs_tx,
                                &start_upgrade_wake_tx,
                                &remediation_jobs_tx,
                                &remediation_wake_tx,
                                &snapshot_in_progress,
                                #[cfg(any(target_os = "windows", target_os = "macos"))]
                                &capture_pipelines,
                                #[cfg(target_os = "windows")]
                                control_queue.clone(),
                                #[cfg(any(target_os = "windows", target_os = "macos"))]
                                control_pipe_writers.clone(),
                                #[cfg(any(target_os = "windows", target_os = "macos"))]
                                helper_target_sessions.clone(),
                            )
                            .await?;
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        return Err(anyhow::anyhow!("server closed connection"));
                    }
                    Some(Ok(_)) => {}
                    Some(Err(err)) => return Err(err.into()),
                    None => return Err(anyhow::anyhow!("connection closed")),
                }
            }
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LiveEventInput {
    pub(crate) event_type: String,
    pub(crate) event_kind: String,
    pub(crate) scope_key: String,
    pub(crate) data: Value,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
const LIVE_EVENT_BACKLOG_CAPACITY: usize = 2048;

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl LiveEventInput {
    pub(crate) fn new(
        event_type: impl Into<String>,
        event_kind: impl Into<String>,
        scope_key: impl Into<String>,
        data: Value,
    ) -> Self {
        Self {
            event_type: event_type.into(),
            event_kind: event_kind.into(),
            scope_key: scope_key.into(),
            data,
        }
    }
}

#[cfg(target_os = "windows")]
impl From<EventInput> for LiveEventInput {
    fn from(input: EventInput) -> Self {
        Self {
            event_type: input.event_type,
            event_kind: input.event_kind,
            scope_key: input.scope_key,
            data: input.data,
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn event_severity(event_kind: &str) -> &'static str {
    let kind = event_kind.to_ascii_lowercase();
    if kind.contains("critical") || kind.contains("failed") {
        "error"
    } else if kind.contains("warning") || kind.contains("low_space") {
        "warning"
    } else {
        "info"
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn normalize_live_event(input: LiveEventInput) -> Value {
    let LiveEventInput {
        event_type,
        event_kind,
        scope_key,
        data,
    } = input;
    let occurred_at = chrono::Utc::now().to_rfc3339();
    let summary = format!("{event_type}.{event_kind}");
    serde_json::json!({
        "eventType": event_type,
        "occurredAt": occurred_at,
        "severity": event_severity(&event_kind),
        "source": "agent_event_stream",
        "code": event_kind,
        "message": summary,
        "attributes": {
            "scopeKey": scope_key,
            "data": data
        }
    })
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) async fn publish_live_event(
    live_events_tx: &broadcast::Sender<Value>,
    live_event_backlog: &Arc<tokio::sync::Mutex<VecDeque<Value>>>,
    event: Value,
) {
    if live_events_tx.receiver_count() == 0 {
        let mut backlog = live_event_backlog.lock().await;
        if backlog.len() >= LIVE_EVENT_BACKLOG_CAPACITY {
            backlog.pop_front();
        }
        backlog.push_back(event.clone());
    }

    let _ = live_events_tx.send(event);
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn start_live_event_stream_bridge(
    boot_session_id: String,
    live_events_tx: broadcast::Sender<Value>,
    live_event_backlog: Arc<tokio::sync::Mutex<VecDeque<Value>>>,
) {
    #[cfg(target_os = "macos")]
    {
        macos_events::start_macos_event_stream_bridge(
            boot_session_id,
            live_events_tx,
            live_event_backlog,
        );
    }

    #[cfg(target_os = "windows")]
    tokio::spawn(async move {
        let (tx, mut rx) = mpsc::channel::<EventInput>(2048);
        spawn_monitors(tx, MonitorConfig::default(), boot_session_id);

        while let Some(event) = rx.recv().await {
            publish_live_event(
                &live_events_tx,
                &live_event_backlog,
                normalize_live_event(event.into()),
            )
            .await;
        }
        warn!("live telemetry monitor stream stopped");
    });
}

fn queue_telemetry_events(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    agent_id: &str,
    batch: &mut Vec<Value>,
) -> bool {
    if batch.is_empty() {
        return true;
    }

    let events = std::mem::take(batch);
    let envelope = OutgoingEnvelope {
        message_type: "telemetry_events",
        data: TelemetryEventsUpdate {
            agent_id: agent_id.to_string(),
            events,
        },
    };

    let text = match serde_json::to_string(&envelope) {
        Ok(value) => value,
        Err(error) => {
            warn!(%error, "failed to serialize telemetry_events envelope");
            return true;
        }
    };

    if outbound_tx.send(Message::Text(text)).is_err() {
        return false;
    }
    true
}

async fn send_inventory(
    write: &mut WsSink,
    agent_id: &str,
    hostname: &str,
    os: &str,
    version: &str,
    sys: &mut System,
    disks: &mut Disks,
    networks: &mut Networks,
    ip: &str,
) -> Result<()> {
    let inventory = collect_inventory(sys, disks, networks);
    let update = InventoryUpdate {
        agent_id: agent_id.to_string(),
        hostname: hostname.to_string(),
        os: os.to_string(),
        ip: ip.to_string(),
        version: version.to_string(),
        inventory,
    };

    send_message(write, "inventory_update", update).await
}

#[cfg(target_os = "windows")]
async fn collect_full_snapshot_update(
    agent_id: &str,
    hostname: &str,
    boot_session_id: &str,
) -> Result<FullSnapshotUpdate> {
    let orchestrator = talos_collector::CollectionOrchestrator::full_collection();
    let agent_version = env!("CARGO_PKG_VERSION").to_string();
    let collection = orchestrator
        .collect_all(agent_id.to_string(), agent_version.clone())
        .await
        .map_err(|e| anyhow::anyhow!("collection failed: {}", e))?;

    let snapshot = talos_collector::snapshot::SnapshotDocument {
        metadata: talos_collector::snapshot::SnapshotMetadata {
            agent_id: agent_id.to_string(),
            device_name: hostname.to_string(),
            boot_session_id: boot_session_id.to_string(),
            agent_version,
            collection_profile: "full".to_string(),
            timestamp: chrono::Utc::now(),
        },
        collection,
    };

    let collected_at = snapshot.metadata.timestamp.to_rfc3339();
    let snapshot_json = serde_json::to_value(snapshot).context("serialize full snapshot")?;
    Ok(FullSnapshotUpdate {
        agent_id: agent_id.to_string(),
        collected_at,
        snapshot: snapshot_json,
        snapshot_request_id: None,
    })
}

#[cfg(not(target_os = "windows"))]
async fn collect_full_snapshot_update(
    agent_id: &str,
    hostname: &str,
    boot_session_id: &str,
) -> Result<FullSnapshotUpdate> {
    #[cfg(target_os = "linux")]
    let (collected_at, snapshot_json) = linux_telemetry::collect_snapshot(
        agent_id,
        hostname,
        boot_session_id,
        env!("CARGO_PKG_VERSION"),
    )
    .await?;

    #[cfg(target_os = "macos")]
    let (collected_at, snapshot_json) = macos_telemetry::collect_snapshot(
        agent_id,
        hostname,
        boot_session_id,
        env!("CARGO_PKG_VERSION"),
    )
    .await?;

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let (collected_at, snapshot_json) = {
        let collected_at = chrono::Utc::now().to_rfc3339();
        let mut sys = System::new_all();
        let mut disks = Disks::new_with_refreshed_list();
        let mut networks = Networks::new_with_refreshed_list();
        let inventory = collect_inventory(&mut sys, &mut disks, &mut networks);
        let mut collection_json =
            serde_json::to_value(&inventory).context("serialize unix inventory")?;
        let collection = collection_json
            .as_object_mut()
            .context("linux inventory did not serialize to an object")?;
        collection.insert(
            "unsupported_features".to_string(),
            serde_json::json!({
                "remote_desktop": "unsupported_platform",
                "remote_registry": "unsupported_platform",
                "chat": "unsupported_platform"
            }),
        );
        let snapshot_json = serde_json::json!({
            "metadata": {
                "agent_id": agent_id,
                "device_name": hostname,
                "boot_session_id": boot_session_id,
                "agent_version": env!("CARGO_PKG_VERSION"),
                "collection_profile": if cfg!(target_os = "macos") { "macos_mvp" } else { "unix_mvp" },
                "timestamp": collected_at,
            },
            "collection": collection_json
        });
        (collected_at, snapshot_json)
    };

    Ok(FullSnapshotUpdate {
        agent_id: agent_id.to_string(),
        collected_at,
        snapshot: snapshot_json,
        snapshot_request_id: None,
    })
}

pub(crate) async fn collect_and_queue_full_snapshot(
    outbound_tx: &mpsc::UnboundedSender<Message>,
    agent_id: &str,
    hostname: &str,
    boot_session_id: &str,
    snapshot_request_id: Option<String>,
    snapshot_in_progress: &Arc<AtomicBool>,
    label: &'static str,
) -> Result<usize> {
    while snapshot_in_progress
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        sleep(Duration::from_secs(2)).await;
        if outbound_tx.is_closed() {
            return Err(anyhow!(
                "websocket queue closed while waiting to collect {label} snapshot"
            ));
        }
    }

    let result = collect_full_snapshot_update(agent_id, hostname, boot_session_id).await;
    snapshot_in_progress.store(false, Ordering::SeqCst);

    let mut payload = result?;
    payload.snapshot_request_id = snapshot_request_id;
    let pending_update_count = patching::snapshot_pending_update_count(&payload);
    let envelope = OutgoingEnvelope {
        message_type: "full_snapshot",
        data: payload,
    };
    let text = serde_json::to_string(&envelope)
        .with_context(|| format!("serialize {label} full_snapshot envelope"))?;
    if outbound_tx.send(Message::Text(text)).is_err() {
        return Err(anyhow!(
            "websocket queue closed while sending {label} full_snapshot"
        ));
    }

    Ok(pending_update_count)
}

pub(crate) async fn send_message<T>(
    write: &mut WsSink,
    message_type: &'static str,
    payload: T,
) -> Result<()>
where
    T: Serialize,
{
    let envelope = OutgoingEnvelope {
        message_type,
        data: payload,
    };
    let text = serde_json::to_string(&envelope).context("serialize message")?;
    write.send(Message::Text(text)).await?;
    Ok(())
}

pub(crate) type WsSink = futures_util::stream::SplitSink<
    WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

async fn handle_server_message(
    text: &str,
    write: &mut WsSink,
    runner: &PipelineRunner,
    punch_sockets: &Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: &Arc<RwLock<HashSet<String>>>,
    file_transfer_relay_sessions: &Arc<RwLock<HashSet<String>>>,
    #[cfg(any(target_os = "windows", target_os = "macos"))] chat_relay_sessions: &Arc<
        RwLock<HashSet<String>>,
    >,
    #[cfg(any(target_os = "windows", target_os = "macos"))] chat_tunnels: &Arc<
        RwLock<HashMap<String, chat::ChatTunnelMeta>>,
    >,
    #[cfg(any(target_os = "windows", target_family = "unix"))] shell_prepared_sessions: &Arc<
        RwLock<HashMap<String, PreparedShellSession>>,
    >,
    #[cfg(any(target_os = "windows", target_family = "unix"))] shell_relay_sessions: &Arc<
        RwLock<HashSet<String>>,
    >,
    agent_id: String,
    hostname: String,
    _version: String,
    boot_session_id: String,
    outbound_tx: &mpsc::UnboundedSender<Message>,
    patch_jobs_tx: &mpsc::UnboundedSender<patching::PatchJobsEnvelope>,
    patch_plan_tx: &mpsc::UnboundedSender<patching::PatchActionPlanEnvelope>,
    patch_wake_tx: &mpsc::UnboundedSender<()>,
    preflight_jobs_tx: &mpsc::UnboundedSender<
        feature_upgrade_preflight::FeatureUpgradePreflightJobsEnvelope,
    >,
    preflight_wake_tx: &mpsc::UnboundedSender<()>,
    stage_iso_jobs_tx: &mpsc::UnboundedSender<
        feature_upgrade_stage_iso::FeatureUpgradeStageIsoJobsEnvelope,
    >,
    stage_iso_wake_tx: &mpsc::UnboundedSender<()>,
    start_upgrade_jobs_tx: &mpsc::UnboundedSender<
        feature_upgrade_start::FeatureUpgradeStartJobsEnvelope,
    >,
    start_upgrade_wake_tx: &mpsc::UnboundedSender<()>,
    remediation_jobs_tx: &mpsc::UnboundedSender<remediation::RemediationJobsEnvelope>,
    remediation_wake_tx: &mpsc::UnboundedSender<()>,
    snapshot_in_progress: &Arc<AtomicBool>,
    #[cfg(any(target_os = "windows", target_os = "macos"))] capture_pipelines: &Arc<
        RwLock<HashMap<String, Arc<CapturePipeline>>>,
    >,
    #[cfg(target_os = "windows")] control_queue: control::ControlQueue,
    #[cfg(any(target_os = "windows", target_os = "macos"))] control_pipe_writers: Arc<
        RwLock<HashMap<String, ControlPipeWriter>>,
    >,
    #[cfg(any(target_os = "windows", target_os = "macos"))] helper_target_sessions: Arc<
        RwLock<HashMap<String, u32>>,
    >,
) -> Result<()> {
    let envelope: IncomingEnvelope = serde_json::from_str(text).context("parse envelope")?;
    match envelope.message_type.as_str() {
        "fetch_details" => {
            let payload: FetchDetailsRequest =
                serde_json::from_value(envelope.data).context("parse fetch_details")?;
            let details = collect_device_details();
            let response = DeviceDetailsResponse {
                request_id: payload.request_id,
                details,
            };
            send_message(write, "device_details", response).await?;
        }
        "request_full_snapshot" => {
            let payload_request: RequestFullSnapshotPayload = serde_json::from_value(
                envelope.data.clone(),
            )
            .unwrap_or(RequestFullSnapshotPayload {
                snapshot_request_id: None,
            });
            let snapshot_request_id = payload_request.snapshot_request_id;
            let outbound_tx = outbound_tx.clone();
            let snapshot_in_progress = snapshot_in_progress.clone();
            let agent_id = agent_id.clone();
            let hostname = hostname.clone();
            let boot_session_id = boot_session_id.clone();
            tokio::spawn(async move {
                let requested_snapshot_id = snapshot_request_id.clone();
                match collect_and_queue_full_snapshot(
                    &outbound_tx,
                    &agent_id,
                    &hostname,
                    &boot_session_id,
                    snapshot_request_id,
                    &snapshot_in_progress,
                    "requested",
                )
                .await
                {
                    Ok(pending_update_count) => {
                        info!(
                            snapshot_request_id = ?requested_snapshot_id,
                            pending_update_count,
                            "requested full_snapshot queued for websocket send"
                        );
                    }
                    Err(error) => warn!(
                        %error,
                        snapshot_request_id = ?requested_snapshot_id,
                        "requested full snapshot failed"
                    ),
                }
            });
        }
        "patch_jobs" => {
            let payload: patching::PatchJobsEnvelope =
                serde_json::from_value(envelope.data).context("parse patch_jobs")?;
            let _ = patch_jobs_tx.send(payload);
        }
        "patch_action_plan" => {
            let payload: patching::PatchActionPlanEnvelope =
                serde_json::from_value(envelope.data).context("parse patch_action_plan")?;
            let _ = patch_plan_tx.send(payload);
        }
        "patch_jobs_available" => {
            let payload: patching::PatchJobsAvailablePayload = serde_json::from_value(
                envelope.data,
            )
            .unwrap_or(patching::PatchJobsAvailablePayload {
                reason: None,
                requested_by: None,
            });
            patching::send_patch_jobs_available_signal(patch_wake_tx, payload);
        }
        "feature_upgrade_preflight_jobs" => {
            let payload: feature_upgrade_preflight::FeatureUpgradePreflightJobsEnvelope =
                serde_json::from_value(envelope.data)
                    .context("parse feature_upgrade_preflight_jobs")?;
            let _ = preflight_jobs_tx.send(payload);
        }
        "feature_upgrade_preflight_jobs_available" => {
            let payload: feature_upgrade_preflight::FeatureUpgradePreflightJobsAvailablePayload =
                serde_json::from_value(envelope.data).unwrap_or(
                    feature_upgrade_preflight::FeatureUpgradePreflightJobsAvailablePayload {
                        reason: None,
                        requested_by: None,
                    },
                );
            feature_upgrade_preflight::send_preflight_jobs_available_signal(
                preflight_wake_tx,
                payload,
            );
        }
        "feature_upgrade_stage_iso_jobs" => {
            let payload: feature_upgrade_stage_iso::FeatureUpgradeStageIsoJobsEnvelope =
                serde_json::from_value(envelope.data)
                    .context("parse feature_upgrade_stage_iso_jobs")?;
            let _ = stage_iso_jobs_tx.send(payload);
        }
        "feature_upgrade_stage_iso_jobs_available" => {
            let payload: feature_upgrade_stage_iso::FeatureUpgradeStageIsoJobsAvailablePayload =
                serde_json::from_value(envelope.data).unwrap_or(
                    feature_upgrade_stage_iso::FeatureUpgradeStageIsoJobsAvailablePayload {
                        reason: None,
                        requested_by: None,
                    },
                );
            feature_upgrade_stage_iso::send_stage_iso_jobs_available_signal(
                stage_iso_wake_tx,
                payload,
            );
        }
        "feature_upgrade_start_jobs" => {
            let payload: feature_upgrade_start::FeatureUpgradeStartJobsEnvelope =
                serde_json::from_value(envelope.data)
                    .context("parse feature_upgrade_start_jobs")?;
            let _ = start_upgrade_jobs_tx.send(payload);
        }
        "feature_upgrade_start_jobs_available" => {
            let payload: feature_upgrade_start::FeatureUpgradeStartJobsAvailablePayload =
                serde_json::from_value(envelope.data).unwrap_or(
                    feature_upgrade_start::FeatureUpgradeStartJobsAvailablePayload {
                        reason: None,
                        requested_by: None,
                    },
                );
            feature_upgrade_start::send_start_jobs_available_signal(start_upgrade_wake_tx, payload);
        }
        "remediation_jobs" => {
            let payload: remediation::RemediationJobsEnvelope =
                serde_json::from_value(envelope.data).context("parse remediation_jobs")?;
            let _ = remediation_jobs_tx.send(payload);
        }
        "remediation_jobs_available" => {
            let payload: remediation::RemediationJobsAvailablePayload = serde_json::from_value(
                envelope.data,
            )
            .unwrap_or(remediation::RemediationJobsAvailablePayload {
                reason: None,
                requested_by: None,
            });
            remediation::send_remediation_jobs_available_signal(remediation_wake_tx, payload);
        }
        "linux_shell_credential_stored" => {
            let payload: LinuxShellCredentialStoredPayload = serde_json::from_value(envelope.data)
                .context("parse linux_shell_credential_stored")?;
            #[cfg(target_os = "linux")]
            {
                talos_worker::linux_account::mark_shell_credential_reported(&payload.credential_id)
                    .context("mark Linux shell credential reported")?;
            }
            #[cfg(not(target_os = "linux"))]
            let _ = payload;
        }
        "shell_command" => {
            let payload: ShellCommandPayload =
                serde_json::from_value(envelope.data).context("parse shell_command")?;
            let output = execute_powershell_command(&payload.command).await;
            let response = ShellOutputPayload {
                request_id: payload.request_id,
                output: output.stdout,
                exit_code: output.exit_code,
            };
            send_message(write, "shell_output", response).await?;
        }
        "rdp_sessions_request" => {
            let payload: RdpSessionsRequestPayload =
                serde_json::from_value(envelope.data).context("parse rdp_sessions_request")?;
            #[cfg(target_os = "windows")]
            let sessions = display::enumerate_wts_sessions()
                .into_iter()
                .map(|session| RdpSessionInfoWire {
                    session_id: session.session_id,
                    logical_session_id: session.logical_session_id,
                    native_session_id: session.native_session_id,
                    kind: session.kind,
                    win_station: session.win_station,
                    user_name: session.user_name,
                    state: session.state,
                })
                .collect::<Vec<_>>();
            #[cfg(not(target_os = "windows"))]
            let sessions = Vec::new();
            let response = RdpSessionsResponsePayload {
                request_id: payload.request_id,
                sessions,
            };
            send_message(write, "rdp_sessions_response", response).await?;
        }
        "shell_start" => {
            let payload: ShellStartPayload =
                serde_json::from_value(envelope.data).context("parse shell_start")?;
            let session_id = payload.session_id.clone();
            let token = payload.token.clone();
            let run_as = payload.run_as;
            let target_session_id = payload.target_session_id;
            let relay_url = payload.relay_url.clone();
            let e2e_key = payload.e2e_key.clone();
            let psk_cert_pem = payload.psk_cert_pem.clone();
            let psk_key_pem = payload.psk_key_pem.clone();

            info!(
                session_id = %session_id,
                run_as = ?run_as,
                target_session_id = ?target_session_id,
                has_quic = psk_cert_pem.is_some(),
                has_relay = relay_url.is_some(),
                "starting interactive shell session"
            );

            if psk_cert_pem.is_some() {
                // Multi-transport: QUIC + relay racing.
                match shell::start_shell_with_shared_io(
                    session_id.clone(),
                    run_as,
                    target_session_id,
                )
                .await
                {
                    Ok(shell_io) => {
                        {
                            let mut sessions = shell_prepared_sessions.write().await;
                            sessions.insert(
                                session_id.clone(),
                                PreparedShellSession {
                                    token: token.clone(),
                                    shell_io: shell_io.clone(),
                                },
                            );
                        }
                        // Build QUIC endpoint.
                        let mut offer_local_addrs = Vec::new();
                        let mut offer_reflex = None;

                        if let (Some(cert_pem), Some(key_pem)) = (&psk_cert_pem, &psk_key_pem) {
                            match build_quic_endpoint(cert_pem, key_pem).await {
                                Ok((endpoint, local_addr, punch_socket, stun_result)) => {
                                    info!(
                                        session_id = %session_id,
                                        local_addr = %local_addr,
                                        "shell quic server bound"
                                    );
                                    match stun_result {
                                        Ok(reflex) => {
                                            info!(
                                                session_id = %session_id,
                                                reflex_ip = %reflex.ip,
                                                reflex_port = reflex.port,
                                                "shell stun completed"
                                            );
                                            offer_reflex = Some(reflex);
                                        }
                                        Err(err) => {
                                            warn!(session_id = %session_id, error = %err, "shell stun failed");
                                            offer_reflex =
                                                Some(local_quic_reflex_fallback(local_addr));
                                        }
                                    }
                                    offer_local_addrs = local_addrs();

                                    // Store punch socket for hole-punching.
                                    {
                                        let mut sockets = punch_sockets.write().await;
                                        sockets.insert(session_id.clone(), Arc::new(punch_socket));
                                    }

                                    // Start QUIC listener in background.
                                    let quic_token = token.clone();
                                    let quic_io = shell_io.clone();
                                    let quic_sid = session_id.clone();
                                    let quic_sessions = shell_prepared_sessions.clone();
                                    let quic_punch_sockets = punch_sockets.clone();
                                    tokio::spawn(async move {
                                        shell::accept_shell_quic_connection(
                                            endpoint,
                                            quic_token,
                                            quic_io,
                                            quic_sid.clone(),
                                        )
                                        .await;
                                        quic_sessions.write().await.remove(&quic_sid);
                                        quic_punch_sockets.write().await.remove(&quic_sid);
                                    });
                                }
                                Err(err) => {
                                    warn!(session_id = %session_id, error = %err, "shell quic endpoint build failed");
                                }
                            }
                        }

                        let offer = ShellOfferPayload {
                            session_id: session_id.clone(),
                            stream_port: 0,
                            host: String::new(),
                            local_addrs: offer_local_addrs,
                            reflex: offer_reflex,
                        };
                        send_message(write, "shell_offer", offer).await?;
                    }
                    Err(e) => {
                        warn!(session_id = %session_id, error = %e, "failed to start shell session");
                        let error_payload = serde_json::json!({
                            "session_id": session_id,
                            "error": format!("{e:#}"),
                        });
                        send_message(write, "shell_error", error_payload).await?;
                    }
                }
            } else if let (Some(relay_url), Some(e2e_key)) = (relay_url, e2e_key) {
                // Legacy relay-only mode.
                let offer = ShellOfferPayload {
                    session_id: session_id.clone(),
                    stream_port: 0,
                    host: String::new(),
                    local_addrs: Vec::new(),
                    reflex: None,
                };
                send_message(write, "shell_offer", offer).await?;

                tokio::spawn(async move {
                    if let Err(err) = shell::run_shell_relay_session(
                        session_id.clone(),
                        token,
                        run_as,
                        target_session_id,
                        relay_url,
                        e2e_key,
                    )
                    .await
                    {
                        warn!(session_id = %session_id, error = %err, "shell relay session failed");
                    }
                });
            } else {
                // Direct TCP mode (LAN only, no relay or QUIC).
                match shell::ShellSession::start(
                    session_id.clone(),
                    token,
                    run_as,
                    target_session_id,
                )
                .await
                {
                    Ok((session, port)) => {
                        let host = current_ip();
                        let offer = ShellOfferPayload {
                            session_id: session_id.clone(),
                            stream_port: port,
                            host,
                            local_addrs: Vec::new(),
                            reflex: None,
                        };
                        send_message(write, "shell_offer", offer).await?;

                        tokio::spawn(async move {
                            session.run().await;
                        });
                    }
                    Err(e) => {
                        warn!(
                            session_id = %session_id,
                            error = %e,
                            "failed to start shell session"
                        );
                        let error_payload = serde_json::json!({
                            "session_id": session_id,
                            "error": format!("{e:#}"),
                        });
                        send_message(write, "shell_error", error_payload).await?;
                    }
                }
            }
        }
        "session_capabilities_request" => {
            let payload: SessionCapabilitiesRequest = serde_json::from_value(envelope.data)
                .context("parse session_capabilities_request")?;
            let response = SessionCapabilitiesResponse {
                request_id: payload.request_id,
                capabilities: runner.get_capabilities(),
            };
            send_message(write, "session_capabilities_response", response).await?;
        }
        "tunnel_prepare" => {
            let payload: TunnelPreparePayload =
                serde_json::from_value(envelope.data).context("parse tunnel_prepare")?;
            handle_tunnel_prepare(
                &payload,
                write,
                punch_sockets,
                relay_sessions,
                file_transfer_relay_sessions,
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                chat_relay_sessions,
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                chat_tunnels,
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                capture_pipelines,
                #[cfg(target_os = "windows")]
                control_queue.clone(),
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                control_pipe_writers.clone(),
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                helper_target_sessions.clone(),
            )
            .await?;
        }
        "punch_start" => {
            let payload: PunchStartPayload =
                serde_json::from_value(envelope.data).context("parse punch_start")?;
            handle_punch_start(payload, punch_sockets).await;
        }
        "relay_prepare" => {
            let payload: RelayPreparePayload =
                serde_json::from_value(envelope.data).context("parse relay_prepare")?;
            handle_relay_prepare(
                &payload,
                punch_sockets,
                relay_sessions,
                file_transfer_relay_sessions,
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                chat_relay_sessions,
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                chat_tunnels,
                #[cfg(any(target_os = "windows", target_family = "unix"))]
                shell_prepared_sessions,
                #[cfg(any(target_os = "windows", target_family = "unix"))]
                shell_relay_sessions,
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                capture_pipelines,
                #[cfg(target_os = "windows")]
                control_queue.clone(),
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                control_pipe_writers.clone(),
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                helper_target_sessions.clone(),
            )
            .await;
        }
        "session_end" => {
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            {
                let payload: SessionEndPayload =
                    serde_json::from_value(envelope.data).context("parse session_end")?;
                #[cfg(target_os = "windows")]
                handle_session_end(
                    &payload.session_id,
                    payload.kind.as_deref(),
                    capture_pipelines,
                    &control_pipe_writers,
                    &helper_target_sessions,
                    punch_sockets,
                    relay_sessions,
                    shell_prepared_sessions,
                    shell_relay_sessions,
                    chat_relay_sessions,
                    chat_tunnels,
                )
                .await;
                #[cfg(target_os = "macos")]
                handle_session_end(
                    &payload.session_id,
                    payload.kind.as_deref(),
                    capture_pipelines,
                    &control_pipe_writers,
                    punch_sockets,
                    relay_sessions,
                    shell_prepared_sessions,
                    shell_relay_sessions,
                    chat_relay_sessions,
                    chat_tunnels,
                )
                .await;
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                #[cfg(target_family = "unix")]
                {
                    let payload: SessionEndPayload =
                        serde_json::from_value(envelope.data).context("parse session_end")?;
                    if payload.kind.as_deref() == Some("shell") {
                        let prepared_removed = shell_prepared_sessions
                            .write()
                            .await
                            .remove(&payload.session_id)
                            .is_some();
                        let relay_removed = shell_relay_sessions
                            .write()
                            .await
                            .remove(&payload.session_id);
                        let punch_removed = punch_sockets
                            .write()
                            .await
                            .remove(&payload.session_id)
                            .is_some();
                        info!(
                            session_id = %payload.session_id,
                            prepared_removed,
                            relay_removed,
                            punch_removed,
                            "shell session_end cleaned up on Unix agent"
                        );
                    } else {
                        debug!(
                            kind = payload.kind.as_deref().unwrap_or("<unknown>"),
                            "session_end ignored on non-Windows agent"
                        );
                    }
                }
                #[cfg(not(target_family = "unix"))]
                {
                    debug!("session_end ignored on unsupported non-Windows agent");
                }
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(target_os = "windows")]
async fn handle_session_end(
    session_id: &str,
    kind: Option<&str>,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: &Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    helper_target_sessions: &Arc<RwLock<HashMap<String, u32>>>,
    punch_sockets: &Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: &Arc<RwLock<HashSet<String>>>,
    shell_prepared_sessions: &Arc<RwLock<HashMap<String, PreparedShellSession>>>,
    shell_relay_sessions: &Arc<RwLock<HashSet<String>>>,
    chat_relay_sessions: &Arc<RwLock<HashSet<String>>>,
    chat_tunnels: &Arc<RwLock<HashMap<String, chat::ChatTunnelMeta>>>,
) {
    if kind == Some("chat") {
        chat::cleanup_chat_session(session_id, chat_tunnels).await;
        if punch_sockets.write().await.remove(session_id).is_some() {
            info!(session_id = %session_id, "chat punch socket removed via session_end");
        }
        chat_relay_sessions.write().await.remove(session_id);
        clear_viewer_session_tracking(session_id);
        return;
    }

    let session_seq = viewer_session_seq(session_id).unwrap_or(0);
    info!(
        session_id = %session_id,
        session_seq = session_seq,
        kind = kind.unwrap_or("<unknown>"),
        "session_end received; forcing stop"
    );

    // 1) Ask helper to stop promptly (avoid lingering DXGI capture).
    send_stop_capture_to_helper(session_id, control_pipe_writers).await;

    // 2) Set pipeline stop flag so transport loops can exit even if sockets don't error promptly.
    if let Some(pipeline) = capture_pipelines.read().await.get(session_id).cloned() {
        pipeline.request_stop();
        info!(
            session_id = %session_id,
            session_seq = session_seq,
            "pipeline request_stop set via session_end"
        );
    }

    shell_prepared_sessions.write().await.remove(session_id);
    shell_relay_sessions.write().await.remove(session_id);
    remove_remote_desktop_session_state(
        session_id,
        control_pipe_writers,
        helper_target_sessions,
        punch_sockets,
        relay_sessions,
    )
    .await;
}

#[cfg(target_os = "macos")]
async fn handle_session_end(
    session_id: &str,
    kind: Option<&str>,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: &Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    punch_sockets: &Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: &Arc<RwLock<HashSet<String>>>,
    shell_prepared_sessions: &Arc<RwLock<HashMap<String, PreparedShellSession>>>,
    shell_relay_sessions: &Arc<RwLock<HashSet<String>>>,
    chat_relay_sessions: &Arc<RwLock<HashSet<String>>>,
    chat_tunnels: &Arc<RwLock<HashMap<String, chat::ChatTunnelMeta>>>,
) {
    if kind == Some("chat") {
        chat::cleanup_chat_session(session_id, chat_tunnels).await;
        if punch_sockets.write().await.remove(session_id).is_some() {
            info!(session_id = %session_id, "macOS chat punch socket removed via session_end");
        }
        chat_relay_sessions.write().await.remove(session_id);
        return;
    }

    if kind == Some("shell") {
        let prepared_removed = shell_prepared_sessions
            .write()
            .await
            .remove(session_id)
            .is_some();
        let relay_removed = shell_relay_sessions.write().await.remove(session_id);
        let punch_removed = punch_sockets.write().await.remove(session_id).is_some();
        info!(
            session_id = %session_id,
            prepared_removed,
            relay_removed,
            punch_removed,
            "macOS shell session_end cleaned up"
        );
        return;
    }

    if kind.is_some_and(|kind| kind != "remote_desktop") {
        debug!(
            session_id = %session_id,
            kind = kind.unwrap_or("<unknown>"),
            "macOS session_end ignored for unsupported session kind"
        );
        return;
    }

    info!(
        session_id = %session_id,
        kind = kind.unwrap_or("<unknown>"),
        "macOS remote desktop session_end received; forcing stop"
    );
    send_stop_capture_to_helper(session_id, control_pipe_writers).await;
    sleep(Duration::from_millis(200)).await;
    if let Some(pipeline) = capture_pipelines.write().await.remove(session_id) {
        pipeline.request_stop();
        info!(session_id = %session_id, "macOS capture pipeline removed via session_end");
    }
    remove_remote_desktop_session_state(
        session_id,
        control_pipe_writers,
        punch_sockets,
        relay_sessions,
    )
    .await;
}

async fn handle_tunnel_prepare(
    payload: &TunnelPreparePayload,
    write: &mut WsSink,
    punch_sockets: &Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: &Arc<RwLock<HashSet<String>>>,
    file_transfer_relay_sessions: &Arc<RwLock<HashSet<String>>>,
    #[cfg(any(target_os = "windows", target_os = "macos"))] chat_relay_sessions: &Arc<
        RwLock<HashSet<String>>,
    >,
    #[cfg(any(target_os = "windows", target_os = "macos"))] chat_tunnels: &Arc<
        RwLock<HashMap<String, chat::ChatTunnelMeta>>,
    >,
    #[cfg(any(target_os = "windows", target_os = "macos"))] capture_pipelines: &Arc<
        RwLock<HashMap<String, Arc<CapturePipeline>>>,
    >,
    #[cfg(target_os = "windows")] control_queue: control::ControlQueue,
    #[cfg(any(target_os = "windows", target_os = "macos"))] control_pipe_writers: Arc<
        RwLock<HashMap<String, ControlPipeWriter>>,
    >,
    #[cfg(any(target_os = "windows", target_os = "macos"))] helper_target_sessions: Arc<
        RwLock<HashMap<String, u32>>,
    >,
) -> Result<()> {
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let _ = relay_sessions;

    #[cfg(target_os = "windows")]
    let (session_seq, _assigned_new) = get_or_assign_viewer_session_seq(&payload.session_id);
    #[cfg(not(target_os = "windows"))]
    let session_seq: u64 = 0;
    info!(
        session_id = %payload.session_id,
        session_seq = session_seq,
        mode = ?payload.mode,
        has_relay = payload.relay_url.is_some(),
        requested_display_profile = ?payload.selected_display_profile,
        "tunnel_prepare received"
    );

    if payload.mode == SessionTransportMode::FileTransfer {
        return handle_file_transfer_tunnel_prepare(
            payload,
            write,
            punch_sockets,
            file_transfer_relay_sessions,
        )
        .await;
    }
    #[cfg(target_os = "windows")]
    if payload.mode == SessionTransportMode::RemoteRegistry {
        return handle_registry_tunnel_prepare(payload, write, punch_sockets, relay_sessions).await;
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    if payload.mode == SessionTransportMode::Chat {
        warn!(
            session_id = %payload.session_id,
            "chat tunnel_prepare ignored on unsupported non-Windows agent"
        );
        return Ok(());
    }
    #[cfg(not(target_os = "windows"))]
    if payload.mode == SessionTransportMode::RemoteRegistry {
        warn!(
            session_id = %payload.session_id,
            "remote registry tunnel_prepare ignored on non-Windows agent"
        );
        return Ok(());
    }
    #[cfg(not(target_os = "windows"))]
    if payload.mode == SessionTransportMode::Shell {
        warn!(
            session_id = %payload.session_id,
            "interactive shell tunnel_prepare ignored on non-Windows agent"
        );
        return Ok(());
    }
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    if payload.mode == SessionTransportMode::Chat {
        return chat::handle_chat_tunnel_prepare(
            payload,
            write,
            punch_sockets,
            chat_relay_sessions,
            chat_tunnels,
            helper_target_sessions,
        )
        .await;
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        let response = RemoteDesktopUnavailablePayload {
            session_id: payload.session_id.clone(),
            reason: "unsupported_platform".to_string(),
            message: Some("Remote desktop is not available on Linux agents".to_string()),
        };
        send_message(write, "remote_desktop_unavailable", response).await?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        let capture_mode = set_macos_session_display_options(
            &payload.session_id,
            payload.selected_display_profile.as_deref(),
            payload.hide_cursor,
        );
        if payload
            .selected_display_profile
            .as_deref()
            .is_some_and(|profile| {
                !matches!(
                    profile,
                    REMOTE_DESKTOP_PROFILE_LEGACY
                        | REMOTE_DESKTOP_PROFILE_MODERN_CPU
                        | REMOTE_DESKTOP_PROFILE_MODERN_GPU
                        | REMOTE_DESKTOP_PROFILE_EXPERIMENTAL
                        | REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY
                )
            })
        {
            warn!(
                session_id = %payload.session_id,
                requested_display_profile = ?payload.selected_display_profile,
                "macOS remote desktop requested unsupported display profile; using legacy"
            );
        }

        let capture_pipeline = macos_desktop::ensure_capture_pipeline(
            payload.session_id.clone(),
            capture_mode,
            capture_pipelines,
            control_pipe_writers.clone(),
        )
        .await;
        if let Some(failure) = macos_desktop::wait_for_startup_failure(&capture_pipeline).await {
            let response = RemoteDesktopUnavailablePayload {
                session_id: payload.session_id.clone(),
                reason: failure.reason,
                message: Some(failure.message),
            };
            send_message(write, "remote_desktop_unavailable", response).await?;
            return Ok(());
        }

        if let (Some(relay_url), Some(e2e_key)) =
            (payload.relay_url.clone(), payload.e2e_key.clone())
        {
            macos_desktop::start_relay_client_once(
                payload.session_id.clone(),
                relay_url,
                e2e_key,
                relay_sessions.clone(),
                punch_sockets.clone(),
                capture_pipelines.clone(),
                control_pipe_writers.clone(),
            )
            .await;
        }

        let (endpoint, local_addr, punch_socket, stun_result) =
            build_quic_endpoint(&payload.psk_cert_pem, &payload.psk_key_pem).await?;

        match stun_result {
            Ok(reflex) => {
                send_message(
                    write,
                    "quic_reflex",
                    QuicReflexPayload {
                        session_id: payload.session_id.clone(),
                        reflex,
                    },
                )
                .await?;
            }
            Err(err) => {
                warn!(error = %err, session_id = %payload.session_id, "macOS stun failed; sending local quic endpoint");
                send_message(
                    write,
                    "quic_reflex",
                    QuicReflexPayload {
                        session_id: payload.session_id.clone(),
                        reflex: local_quic_reflex_fallback(local_addr),
                    },
                )
                .await?;
            }
        }

        punch_sockets
            .write()
            .await
            .insert(payload.session_id.clone(), Arc::new(punch_socket));

        let session_id = payload.session_id.clone();
        let local_addrs = local_addrs();
        let punch_sockets = punch_sockets.clone();
        let relay_sessions = relay_sessions.clone();
        let capture_pipelines = capture_pipelines.clone();
        let control_pipe_writers = control_pipe_writers.clone();
        tokio::spawn(async move {
            if let Err(err) = macos_desktop::accept_quic_connections(
                endpoint,
                local_addrs,
                session_id,
                punch_sockets,
                relay_sessions,
                capture_pipelines,
                control_pipe_writers,
            )
            .await
            {
                warn!(error = %err, "macOS quic accept loop ended");
            }
        });

        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        // Start capture/encode independently from tunnel bring-up.
        let (selected_display_profile, profile_changed) = set_viewer_session_profile(
            &payload.session_id,
            payload.selected_display_profile.as_deref(),
        );
        #[cfg(target_os = "windows")]
        if profile_changed {
            send_stop_capture_to_helper(&payload.session_id, &control_pipe_writers).await;
            if let Some(pipeline) = capture_pipelines.write().await.remove(&payload.session_id) {
                pipeline.request_stop();
                info!(
                    session_id = %payload.session_id,
                    selected_display_profile = %selected_display_profile,
                    "capture pipeline removed for selected display profile change"
                );
            }
            control_pipe_writers
                .write()
                .await
                .remove(&payload.session_id);
        }
        #[cfg(target_os = "windows")]
        info!(
            session_id = %payload.session_id,
            selected_display_profile = %selected_display_profile,
            profile_changed = profile_changed,
            "remote desktop display profile selected"
        );

        // Start capture/encode independently from tunnel bring-up.
        #[cfg(target_os = "windows")]
        ensure_capture_pipeline(
            payload.session_id.clone(),
            capture_pipelines,
            control_pipe_writers.clone(),
            helper_target_sessions.clone(),
        )
        .await;

        if let (Some(relay_url), Some(e2e_key)) =
            (payload.relay_url.clone(), payload.e2e_key.clone())
        {
            #[cfg(target_os = "windows")]
            ensure_capture_pipeline(
                payload.session_id.clone(),
                capture_pipelines,
                control_pipe_writers.clone(),
                helper_target_sessions.clone(),
            )
            .await;
            start_relay_client_once(
                payload.session_id.clone(),
                relay_url,
                e2e_key,
                relay_sessions.clone(),
                punch_sockets.clone(),
                #[cfg(target_os = "windows")]
                capture_pipelines.clone(),
                #[cfg(target_os = "windows")]
                control_queue.clone(),
                #[cfg(target_os = "windows")]
                control_pipe_writers.clone(),
                #[cfg(target_os = "windows")]
                helper_target_sessions.clone(),
            )
            .await;
        }

        let (endpoint, local_addr, punch_socket, stun_result) =
            build_quic_endpoint(&payload.psk_cert_pem, &payload.psk_key_pem).await?;

        info!(
            session_id = %payload.session_id,
            local_addr = %local_addr,
            "quic server bound"
        );

        match stun_result {
            Ok(reflex) => {
                info!(
                    session_id = %payload.session_id,
                    reflex_ip = %reflex.ip,
                    reflex_port = reflex.port,
                    "stun completed"
                );
                let response = QuicReflexPayload {
                    session_id: payload.session_id.clone(),
                    reflex: reflex.clone(),
                };
                send_message(write, "quic_reflex", response).await?;
                info!(session_id = %payload.session_id, "quic_reflex sent");
            }
            Err(err) => {
                warn!(error = %err, session_id = %payload.session_id, "stun failed; continuing without reflex");
                let fallback = local_quic_reflex_fallback(local_addr);
                let response = QuicReflexPayload {
                    session_id: payload.session_id.clone(),
                    reflex: fallback.clone(),
                };
                send_message(write, "quic_reflex", response).await?;
                info!(
                    session_id = %payload.session_id,
                    fallback_ip = %fallback.ip,
                    fallback_port = fallback.port,
                    "local quic endpoint sent after stun failure"
                );
            }
        }

        {
            let mut sockets = punch_sockets.write().await;
            sockets.insert(payload.session_id.clone(), Arc::new(punch_socket));
        }

        let addrs = local_addrs();
        let session_id = payload.session_id.clone();
        let punch_sockets = punch_sockets.clone();
        let relay_sessions = relay_sessions.clone();
        #[cfg(target_os = "windows")]
        let capture_pipelines = capture_pipelines.clone();
        #[cfg(target_os = "windows")]
        let control_pipe_writers = control_pipe_writers.clone();
        #[cfg(target_os = "windows")]
        let control_queue = control_queue.clone();
        #[cfg(target_os = "windows")]
        let helper_target_sessions = helper_target_sessions.clone();
        #[cfg(target_os = "windows")]
        ensure_capture_pipeline(
            session_id.clone(),
            &capture_pipelines,
            control_pipe_writers.clone(),
            helper_target_sessions.clone(),
        )
        .await;
        tokio::spawn(async move {
            if let Err(err) = accept_quic_connections(
                endpoint,
                addrs,
                session_id.clone(),
                punch_sockets.clone(),
                relay_sessions.clone(),
                #[cfg(target_os = "windows")]
                capture_pipelines,
                #[cfg(target_os = "windows")]
                control_queue,
                #[cfg(target_os = "windows")]
                control_pipe_writers,
                #[cfg(target_os = "windows")]
                helper_target_sessions,
            )
            .await
            {
                warn!(error = %err, "quic accept loop ended");
            }
        });

        Ok(())
    }
}

async fn handle_file_transfer_tunnel_prepare(
    payload: &TunnelPreparePayload,
    write: &mut WsSink,
    punch_sockets: &Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    file_transfer_relay_sessions: &Arc<RwLock<HashSet<String>>>,
) -> Result<()> {
    info!(
        session_id = %payload.session_id,
        mode = "file_transfer",
        platform = ?agent_platform(),
        relay_configured = payload.relay_url.is_some(),
        "tunnel_prepare received"
    );

    if let (Some(relay_url), Some(e2e_key)) = (payload.relay_url.clone(), payload.e2e_key.clone()) {
        start_file_transfer_relay_client_once(
            payload.session_id.clone(),
            relay_url,
            e2e_key,
            file_transfer_relay_sessions.clone(),
        )
        .await;
    }

    let (endpoint, local_addr, punch_socket, stun_result) =
        build_quic_endpoint(&payload.psk_cert_pem, &payload.psk_key_pem).await?;

    info!(
        session_id = %payload.session_id,
        local_addr = %local_addr,
        platform = ?agent_platform(),
        "file transfer quic server bound"
    );

    match stun_result {
        Ok(reflex) => {
            let response = QuicReflexPayload {
                session_id: payload.session_id.clone(),
                reflex: reflex.clone(),
            };
            send_message(write, "quic_reflex", response).await?;
            info!(session_id = %payload.session_id, "file transfer quic_reflex sent");
        }
        Err(err) => {
            warn!(
                session_id = %payload.session_id,
                error = %err,
                "file transfer stun failed; continuing without reflex"
            );
            let fallback = local_quic_reflex_fallback(local_addr);
            let response = QuicReflexPayload {
                session_id: payload.session_id.clone(),
                reflex: fallback.clone(),
            };
            send_message(write, "quic_reflex", response).await?;
            info!(
                session_id = %payload.session_id,
                fallback_ip = %fallback.ip,
                fallback_port = fallback.port,
                "file transfer local quic endpoint sent after stun failure"
            );
        }
    }

    {
        let mut sockets = punch_sockets.write().await;
        sockets.insert(payload.session_id.clone(), Arc::new(punch_socket));
    }

    let addrs = local_addrs();
    let session_id = payload.session_id.clone();
    tokio::spawn(async move {
        if let Err(err) =
            accept_file_transfer_quic_connections(endpoint, addrs, session_id.clone()).await
        {
            warn!(
                session_id = %session_id,
                error = %err,
                "file transfer quic accept loop ended"
            );
        }
    });

    Ok(())
}

#[cfg(target_os = "windows")]
async fn handle_registry_tunnel_prepare(
    payload: &TunnelPreparePayload,
    write: &mut WsSink,
    punch_sockets: &Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: &Arc<RwLock<HashSet<String>>>,
) -> Result<()> {
    info!(
        session_id = %payload.session_id,
        mode = "remote_registry",
        "tunnel_prepare received"
    );

    if let (Some(relay_url), Some(e2e_key)) = (payload.relay_url.clone(), payload.e2e_key.clone()) {
        start_registry_relay_client_once(
            payload.session_id.clone(),
            relay_url,
            e2e_key,
            relay_sessions.clone(),
        )
        .await;
    }

    let (endpoint, local_addr, punch_socket, stun_result) =
        build_quic_endpoint(&payload.psk_cert_pem, &payload.psk_key_pem).await?;

    info!(
        session_id = %payload.session_id,
        local_addr = %local_addr,
        "registry quic server bound"
    );

    match stun_result {
        Ok(reflex) => {
            let response = QuicReflexPayload {
                session_id: payload.session_id.clone(),
                reflex: reflex.clone(),
            };
            send_message(write, "quic_reflex", response).await?;
            info!(session_id = %payload.session_id, "registry quic_reflex sent");
        }
        Err(err) => {
            warn!(
                session_id = %payload.session_id,
                error = %err,
                "registry stun failed; continuing without reflex"
            );
            let fallback = local_quic_reflex_fallback(local_addr);
            let response = QuicReflexPayload {
                session_id: payload.session_id.clone(),
                reflex: fallback.clone(),
            };
            send_message(write, "quic_reflex", response).await?;
            info!(
                session_id = %payload.session_id,
                fallback_ip = %fallback.ip,
                fallback_port = fallback.port,
                "registry local quic endpoint sent after stun failure"
            );
        }
    }

    {
        let mut sockets = punch_sockets.write().await;
        sockets.insert(payload.session_id.clone(), Arc::new(punch_socket));
    }

    let addrs = local_addrs();
    let session_id = payload.session_id.clone();
    tokio::spawn(async move {
        if let Err(err) =
            accept_registry_quic_connections(endpoint, addrs, session_id.clone()).await
        {
            warn!(
                session_id = %session_id,
                error = %err,
                "registry quic accept loop ended"
            );
        }
    });

    Ok(())
}

#[cfg(target_os = "windows")]
async fn handle_registry_relay_prepare(
    payload: &RelayPreparePayload,
    relay_sessions: &Arc<RwLock<HashSet<String>>>,
) {
    info!(
        session_id = %payload.session_id,
        mode = "remote_registry",
        "relay_prepare received"
    );
    start_registry_relay_client_once(
        payload.session_id.clone(),
        payload.relay_url.clone(),
        payload.e2e_key.clone(),
        relay_sessions.clone(),
    )
    .await;
}

#[cfg(target_os = "windows")]
async fn start_registry_relay_client_once(
    session_id: String,
    relay_url: String,
    e2e_key: String,
    relay_sessions: Arc<RwLock<HashSet<String>>>,
) {
    {
        let mut sessions = relay_sessions.write().await;
        if sessions.contains(&session_id) {
            return;
        }
        sessions.insert(session_id.clone());
    }

    tokio::spawn(async move {
        if let Err(err) = run_registry_relay_client(session_id.clone(), relay_url, e2e_key).await {
            warn!(
                session_id = %session_id,
                error = %err,
                "registry relay client ended unexpectedly"
            );
        }
        let mut sessions = relay_sessions.write().await;
        sessions.remove(&session_id);
    });
}

#[cfg(target_os = "windows")]
async fn accept_registry_quic_connections(
    endpoint: Endpoint,
    local_addrs: Vec<LocalAddr>,
    session_id: String,
) -> Result<()> {
    let active_connection: Arc<tokio::sync::Mutex<Option<Connection>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    loop {
        let Some(connecting) = endpoint.accept().await else {
            break;
        };
        let connection = match connecting.await {
            Ok(conn) => conn,
            Err(err) => {
                warn!(error = %err, "registry quic connection failed");
                continue;
            }
        };

        let source = if is_lan_connection(connection.remote_address(), &local_addrs) {
            "lan"
        } else {
            "reflex"
        };
        info!(
            session_id = %session_id,
            remote = %connection.remote_address(),
            source = source,
            "registry quic connection accepted"
        );

        {
            let mut guard = active_connection.lock().await;
            if let Some(prev) = guard.take() {
                prev.close(0u32.into(), b"replaced");
            }
            *guard = Some(connection.clone());
        }

        let session_id_for_task = session_id.clone();
        tokio::spawn(async move {
            if let Err(err) =
                handle_registry_quic_connection(session_id_for_task.clone(), connection).await
            {
                warn!(session_id = %session_id_for_task, error = %err, "registry quic connection ended");
            }
        });
    }
    Ok(())
}

#[cfg(target_os = "windows")]
async fn handle_registry_quic_connection(session_id: String, connection: Connection) -> Result<()> {
    let mut control = connection
        .accept_uni()
        .await
        .context("accept registry control stream")?;
    let mut send = connection
        .open_uni()
        .await
        .context("open registry response stream")?;

    loop {
        let Some(payload) = read_registry_control_frame(&mut control).await? else {
            break;
        };
        let response = execute_registry_request_payload(&session_id, &payload).await?;
        send.write_all(&response)
            .await
            .context("write registry response stream")?;
    }

    let _ = send.finish();
    Ok(())
}

#[cfg(target_os = "windows")]
async fn run_registry_relay_client(
    session_id: String,
    relay_url: String,
    e2e_key_b64: String,
) -> Result<()> {
    let relay_target = parse_relay_target(&relay_url)?;
    let addr = format!("{}:{}", relay_target.host, relay_target.port);
    let connect_timeout = Duration::from_secs(
        env::var("RMM_RELAY_CONNECT_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(10),
    );
    let tcp_stream = timeout(connect_timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| anyhow!("connect relay tcp timed out"))?
        .context("connect relay tcp")?;
    tcp_stream
        .set_nodelay(true)
        .context("set relay TCP_NODELAY")?;

    let tls_config = build_relay_client_tls_config(None, None)?;
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name =
        ServerName::try_from(relay_target.host.clone()).context("build relay server name")?;
    let mut stream = timeout(connect_timeout, connector.connect(server_name, tcp_stream))
        .await
        .map_err(|_| anyhow!("relay tls connect timed out"))?
        .context("relay tls connect")?;

    let request = format!(
        "GET /relay/{session_id} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        session_id = session_id,
        host = relay_target.host
    );
    stream
        .write_all(request.as_bytes())
        .await
        .context("write relay request")?;
    timeout(connect_timeout, read_http_response(&mut stream))
        .await
        .map_err(|_| anyhow!("read relay response timed out"))??;

    let key_bytes = BASE64_STANDARD
        .decode(e2e_key_b64.trim())
        .context("decode relay e2e key")?;
    let cipher = build_e2e_cipher(&key_bytes)?;

    let mut send_counter = 0u64;
    write_e2e_frame(&mut stream, &cipher, &mut send_counter, b"hello-world").await?;
    info!(session_id = %session_id, "registry relay hello-world frame sent");

    let (mut reader, mut writer) = tokio::io::split(stream);
    loop {
        let payload = match read_e2e_frame_from(&mut reader, &cipher).await {
            Ok(payload) => payload,
            Err(err) => {
                if is_relay_connection_closed(&err) {
                    info!(session_id = %session_id, "registry relay session ended");
                    return Ok(());
                }
                return Err(err);
            }
        };

        if payload == HEARTBEAT_PAYLOAD || payload == b"hello-world" {
            continue;
        }

        let frame = match parse_control_frame(&payload) {
            Ok(frame) => frame,
            Err(err) => {
                warn!(session_id = %session_id, error = %err, "invalid registry relay control frame");
                continue;
            }
        };
        if frame.message_type != CONTROL_TYPE_REGISTRY_REQUEST {
            warn!(
                session_id = %session_id,
                message_type = frame.message_type,
                "unexpected registry relay control message"
            );
            continue;
        }

        let response = execute_registry_request_payload(&session_id, frame.payload).await?;
        write_e2e_frame(&mut writer, &cipher, &mut send_counter, &response)
            .await
            .context("write registry relay response")?;
    }
}

#[cfg(target_os = "windows")]
async fn read_registry_control_frame(recv: &mut quinn::RecvStream) -> Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 2];
    if recv.read_exact(&mut len_buf).await.is_err() {
        return Ok(None);
    }
    let payload_len = u16::from_be_bytes(len_buf) as usize;
    let mut type_buf = [0u8; 1];
    recv.read_exact(&mut type_buf)
        .await
        .context("read registry control message type")?;
    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        recv.read_exact(&mut payload)
            .await
            .context("read registry control payload")?;
    }
    if type_buf[0] != CONTROL_TYPE_REGISTRY_REQUEST {
        return Err(anyhow!(
            "unexpected registry control message type {}",
            type_buf[0]
        ));
    }
    Ok(Some(payload))
}

#[cfg(target_os = "windows")]
async fn execute_registry_request_payload(
    expected_session_id: &str,
    payload: &[u8],
) -> Result<Vec<u8>> {
    let request = serde_json::from_slice::<RegistryRequest>(payload)
        .context("parse registry request payload")?;
    let request_session_id = match &request {
        RegistryRequest::ListKeys { session_id, .. }
        | RegistryRequest::ListValues { session_id, .. }
        | RegistryRequest::GetValue { session_id, .. }
        | RegistryRequest::SetValue { session_id, .. }
        | RegistryRequest::CreateKey { session_id, .. }
        | RegistryRequest::DeleteKey { session_id, .. }
        | RegistryRequest::DeleteValue { session_id, .. }
        | RegistryRequest::Cancel { session_id, .. } => session_id.as_str(),
    };
    if request_session_id != expected_session_id {
        let request_id = match &request {
            RegistryRequest::ListKeys { request_id, .. }
            | RegistryRequest::ListValues { request_id, .. }
            | RegistryRequest::GetValue { request_id, .. }
            | RegistryRequest::SetValue { request_id, .. }
            | RegistryRequest::CreateKey { request_id, .. }
            | RegistryRequest::DeleteKey { request_id, .. }
            | RegistryRequest::DeleteValue { request_id, .. }
            | RegistryRequest::Cancel { request_id, .. } => request_id.clone(),
        };
        let envelope = RegistryResponseEnvelope {
            message_type: REGISTRY_META_MESSAGE_TYPE.to_string(),
            request_id,
            session_id: expected_session_id.to_string(),
            response: talos_protocol::RegistryResponse::Error {
                code: talos_protocol::OperationErrorCode::StaleSession,
                message: format!(
                    "registry request session mismatch: expected {expected_session_id}, received {request_session_id}"
                ),
            },
        };
        return Ok(build_registry_response_frame(envelope));
    }
    let envelope = tokio::task::spawn_blocking(move || registry::handle_request(request))
        .await
        .context("registry task join failed")?;
    Ok(build_registry_response_frame(envelope))
}

#[cfg(target_os = "windows")]
fn build_registry_response_frame(envelope: RegistryResponseEnvelope) -> Vec<u8> {
    let mut json_bytes = match serde_json::to_vec(&envelope) {
        Ok(v) => v,
        Err(err) => {
            warn!(error = %err, "failed to serialize registry response envelope");
            return wrap_rmmd_payload(
                br#"{"type":"registry_response","requestId":"","sessionId":"","response":{"kind":"error","code":"internal","message":"failed to serialize registry response"}}"#,
            );
        }
    };

    const REGISTRY_META_MAX_JSON_BYTES: usize = 512 * 1024;
    if json_bytes.len() > REGISTRY_META_MAX_JSON_BYTES {
        let fallback = RegistryResponseEnvelope {
            message_type: REGISTRY_META_MESSAGE_TYPE.to_string(),
            request_id: envelope.request_id,
            session_id: envelope.session_id,
            response: talos_protocol::RegistryResponse::Error {
                code: talos_protocol::OperationErrorCode::PayloadTooLarge,
                message: format!(
                    "Registry response too large ({} bytes); refine your selection",
                    json_bytes.len()
                ),
            },
        };
        if let Ok(bytes) = serde_json::to_vec(&fallback) {
            json_bytes = bytes;
        }
    }

    wrap_rmmd_payload(&json_bytes)
}

async fn accept_file_transfer_quic_connections(
    endpoint: Endpoint,
    local_addrs: Vec<LocalAddr>,
    session_id: String,
) -> Result<()> {
    loop {
        let Some(connecting) = endpoint.accept().await else {
            break;
        };
        let connection = match connecting.await {
            Ok(conn) => conn,
            Err(err) => {
                warn!(error = %err, "file transfer quic connection failed");
                continue;
            }
        };

        let source = if is_lan_connection(connection.remote_address(), &local_addrs) {
            "lan"
        } else {
            "reflex"
        };
        info!(
            session_id = %session_id,
            remote = %connection.remote_address(),
            source = source,
            "file transfer quic connection accepted"
        );

        let session_id_for_connection = session_id.clone();
        tokio::spawn(async move {
            loop {
                let stream = connection.accept_bi().await;
                let (send, recv) = match stream {
                    Ok(stream) => stream,
                    Err(err) => {
                        if err.to_string().contains("closed") {
                            break;
                        }
                        warn!(error = %err, "file transfer quic bi stream accept failed");
                        break;
                    }
                };
                let session_id_for_stream = session_id_for_connection.clone();
                tokio::spawn(async move {
                    if let Err(err) =
                        handle_file_transfer_quic_stream(session_id_for_stream, send, recv).await
                    {
                        warn!(error = %err, "file transfer quic stream failed");
                    }
                });
            }
        });
    }
    Ok(())
}

async fn handle_file_transfer_quic_stream(
    session_id: String,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
) -> Result<()> {
    prune_file_transfer_resume_state().await;
    let Some((message_type, payload)) = read_file_transfer_quic_frame(&mut recv).await? else {
        let _ = send.finish();
        return Ok(());
    };
    if message_type != FILE_TRANSFER_MSG_JSON {
        write_file_transfer_json_quic(
            &mut send,
            &file_transfer_error_response(
                talos_protocol::OperationErrorCode::InvalidRequest,
                "expected json request frame",
                false,
            ),
        )
        .await?;
        let _ = send.finish();
        return Ok(());
    }

    let request: FileTransferRequest =
        serde_json::from_slice(&payload).context("parse file transfer request")?;
    match request {
        FileTransferRequest::ListDir { path } => match file_transfer::list_dir(&path) {
            Ok(response) => write_file_transfer_json_quic(&mut send, &response).await?,
            Err(error) => write_file_transfer_error_quic(&mut send, error).await?,
        },
        FileTransferRequest::Download {
            transfer_id,
            paths,
            resume_offset,
        } => {
            handle_file_transfer_download_quic(
                &session_id,
                &mut send,
                transfer_id,
                paths,
                resume_offset,
            )
            .await?;
        }
        FileTransferRequest::Rename { from_path, to_path } => {
            match file_transfer::rename_path(&from_path, &to_path) {
                Ok(response) => write_file_transfer_json_quic(&mut send, &response).await?,
                Err(error) => write_file_transfer_error_quic(&mut send, error).await?,
            }
        }
        FileTransferRequest::Delete { path, recursive } => {
            match file_transfer::delete_path(&path, recursive) {
                Ok(response) => write_file_transfer_json_quic(&mut send, &response).await?,
                Err(error) => write_file_transfer_error_quic(&mut send, error).await?,
            }
        }
        FileTransferRequest::Upload { .. } => {
            handle_file_transfer_upload_quic(&session_id, &mut send, &mut recv, request).await?;
        }
        FileTransferRequest::Cancel { transfer_id } => {
            clear_file_transfer_resume_state(&session_id, &transfer_id).await;
            write_file_transfer_json_quic(&mut send, &FileTransferResponse::Ok {}).await?;
        }
    }

    let _ = send.finish();
    Ok(())
}

async fn handle_file_transfer_download_quic(
    session_id: &str,
    send: &mut quinn::SendStream,
    transfer_id: String,
    paths: Vec<String>,
    requested_resume_offset: u64,
) -> Result<()> {
    let resume_key = file_transfer_resume_key(session_id, &transfer_id);
    let resumed_state = {
        let mut guard = file_transfer_download_resumes().lock().await;
        guard.remove(&resume_key)
    };
    let prepared = match resumed_state {
        Some(state) if state.requested_paths == paths => state.prepared,
        Some(state) => {
            cleanup_download_resume_state(state);
            let (progress_tx, mut progress_rx) =
                tokio::sync::mpsc::unbounded_channel::<file_transfer::ArchivePreparationProgress>();
            let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let paths_for_prepare = paths.clone();
            let cancelled_for_prepare = cancelled.clone();
            let mut prepare_task = tokio::task::spawn_blocking(move || {
                file_transfer::begin_download_with_progress_cancel(
                    &paths_for_prepare,
                    cancelled_for_prepare.as_ref(),
                    |progress| {
                        let _ = progress_tx.send(progress);
                    },
                )
            });

            loop {
                tokio::select! {
                    result = &mut prepare_task => {
                        match result {
                            Ok(Ok(prepared)) => break prepared,
                            Ok(Err(error)) => {
                                write_file_transfer_error_quic(send, error).await?;
                                return Ok(());
                            }
                            Err(error) => {
                                write_file_transfer_json_quic(
                                    send,
                                    &file_transfer_error_response(
                                        talos_protocol::OperationErrorCode::Internal,
                                        format!("download preparation failed: {error}"),
                                        true,
                                    ),
                                ).await?;
                                return Ok(());
                            }
                        }
                    }
                    Some(progress) = progress_rx.recv() => {
                        let message = if progress.files_total > 0 {
                            format!("Preparing archive: {} / {} file(s)", progress.files_done, progress.files_total)
                        } else {
                            "Preparing archive...".to_string()
                        };
                        if let Err(_err) = write_file_transfer_json_quic(
                            send,
                            &FileTransferResponse::Progress {
                                files_done: progress.files_done as u64,
                                files_total: progress.files_total as u64,
                                bytes_done: progress.bytes_done,
                                bytes_total: progress.bytes_total,
                                phase: Some("preparing".to_string()),
                                message: Some(message),
                            },
                        )
                        .await {
                            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
                            return Ok(());
                        }
                    }
                }
            }
        }
        None => {
            let (progress_tx, mut progress_rx) =
                tokio::sync::mpsc::unbounded_channel::<file_transfer::ArchivePreparationProgress>();
            let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let paths_for_prepare = paths.clone();
            let cancelled_for_prepare = cancelled.clone();
            let mut prepare_task = tokio::task::spawn_blocking(move || {
                file_transfer::begin_download_with_progress_cancel(
                    &paths_for_prepare,
                    cancelled_for_prepare.as_ref(),
                    |progress| {
                        let _ = progress_tx.send(progress);
                    },
                )
            });

            loop {
                tokio::select! {
                    result = &mut prepare_task => {
                        match result {
                            Ok(Ok(prepared)) => break prepared,
                            Ok(Err(error)) => {
                                write_file_transfer_error_quic(send, error).await?;
                                return Ok(());
                            }
                            Err(error) => {
                                write_file_transfer_json_quic(
                                    send,
                                    &file_transfer_error_response(
                                        talos_protocol::OperationErrorCode::Internal,
                                        format!("download preparation failed: {error}"),
                                        true,
                                    ),
                                ).await?;
                                return Ok(());
                            }
                        }
                    }
                    Some(progress) = progress_rx.recv() => {
                        let message = if progress.files_total > 0 {
                            format!("Preparing archive: {} / {} file(s)", progress.files_done, progress.files_total)
                        } else {
                            "Preparing archive...".to_string()
                        };
                        if let Err(_err) = write_file_transfer_json_quic(
                            send,
                            &FileTransferResponse::Progress {
                                files_done: progress.files_done as u64,
                                files_total: progress.files_total as u64,
                                bytes_done: progress.bytes_done,
                                bytes_total: progress.bytes_total,
                                phase: Some("preparing".to_string()),
                                message: Some(message),
                            },
                        )
                        .await {
                            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
                            return Ok(());
                        }
                    }
                }
            }
        }
    };

    let accepted_resume_offset = requested_resume_offset.min(prepared.size_bytes);

    let ready = FileTransferResponse::DownloadReady {
        transfer_id: transfer_id.clone(),
        file_name: prepared.file_name.clone(),
        size_bytes: prepared.size_bytes,
        is_archive: prepared.is_archive,
        resume_offset: accepted_resume_offset,
    };
    write_file_transfer_json_quic(send, &ready).await?;

    struct CleanupOnDrop(Option<std::path::PathBuf>);
    impl Drop for CleanupOnDrop {
        fn drop(&mut self) {
            if let Some(path) = self.0.take() {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    impl CleanupOnDrop {
        fn disarm(&mut self) {
            self.0 = None;
        }
    }
    let mut cleanup = if prepared.cleanup_source {
        CleanupOnDrop(Some(prepared.source_path.clone()))
    } else {
        CleanupOnDrop(None)
    };

    let mut file = fs::File::open(&prepared.source_path).context("open transfer source")?;
    file.seek(SeekFrom::Start(accepted_resume_offset))
        .context("seek transfer source to resume offset")?;
    let mut buffer = vec![0u8; talos_protocol::FILE_TRANSFER_DEFAULT_CHUNK_BYTES as usize];
    loop {
        let read = file.read(&mut buffer).context("read transfer source")?;
        if read == 0 {
            break;
        }
        if let Err(error) = write_file_transfer_data_quic(send, &buffer[..read]).await {
            cleanup.disarm();
            remember_download_resume_state(session_id, &transfer_id, paths, prepared).await;
            debug!(session_id = %session_id, transfer_id = %transfer_id, error = %error, "file transfer quic download paused for resume");
            return Ok(());
        }
    }
    if let Err(error) = write_file_transfer_finish_quic(send).await {
        cleanup.disarm();
        remember_download_resume_state(session_id, &transfer_id, paths, prepared).await;
        debug!(session_id = %session_id, transfer_id = %transfer_id, error = %error, "file transfer quic download finish write paused for resume");
        return Ok(());
    }
    cleanup.disarm();
    remember_download_resume_state(session_id, &transfer_id, paths, prepared).await;
    Ok(())
}

async fn handle_file_transfer_upload_quic(
    session_id: &str,
    send: &mut quinn::SendStream,
    recv: &mut quinn::RecvStream,
    request: FileTransferRequest,
) -> Result<()> {
    let (transfer_id, expected_size_bytes) = match &request {
        FileTransferRequest::Upload {
            transfer_id,
            expected_size_bytes,
            ..
        } => (transfer_id.clone(), *expected_size_bytes),
        _ => {
            write_file_transfer_json_quic(
                send,
                &file_transfer_error_response(
                    talos_protocol::OperationErrorCode::InvalidRequest,
                    "invalid request for upload",
                    false,
                ),
            )
            .await?;
            return Ok(());
        }
    };
    let resume_key = file_transfer_resume_key(session_id, &transfer_id);
    let mut upload = match file_transfer_upload_resumes()
        .lock()
        .await
        .remove(&resume_key)
    {
        Some(mut state) if resumable_upload_matches(&state, &request) => {
            let committed_bytes = fs::metadata(&state.upload.temp_input_path)
                .map(|metadata| metadata.len())
                .unwrap_or(state.upload.bytes_received);
            state.upload.bytes_received = committed_bytes;
            state.upload
        }
        Some(state) => {
            cleanup_upload_resume_state(state);
            match file_transfer::begin_upload(&request) {
                Ok(upload) => upload,
                Err(error) => {
                    write_file_transfer_error_quic(send, error).await?;
                    return Ok(());
                }
            }
        }
        None => match file_transfer::begin_upload(&request) {
            Ok(upload) => upload,
            Err(error) => {
                write_file_transfer_error_quic(send, error).await?;
                return Ok(());
            }
        },
    };

    struct CleanupOnDrop(Option<std::path::PathBuf>);
    impl Drop for CleanupOnDrop {
        fn drop(&mut self) {
            if let Some(path) = self.0.take() {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    impl CleanupOnDrop {
        fn disarm(&mut self) {
            self.0 = None;
        }
    }
    let mut cleanup = CleanupOnDrop(Some(upload.temp_input_path.clone()));

    write_file_transfer_json_quic(
        send,
        &FileTransferResponse::UploadReady {
            transfer_id: transfer_id.clone(),
            resume_offset: upload.bytes_received.min(expected_size_bytes),
        },
    )
    .await?;

    loop {
        let Some((message_type, payload)) = read_file_transfer_quic_frame(recv).await? else {
            cleanup.disarm();
            remember_upload_resume_state(
                session_id,
                ResumableUploadState {
                    upload,
                    expected_size_bytes,
                    touched_at: Instant::now(),
                },
            )
            .await;
            return Ok(());
        };

        if message_type == FILE_TRANSFER_MSG_FINISH {
            break;
        }
        if message_type != FILE_TRANSFER_MSG_DATA {
            write_file_transfer_json_quic(
                send,
                &file_transfer_error_response(
                    talos_protocol::OperationErrorCode::InvalidRequest,
                    "expected data frame during upload",
                    false,
                ),
            )
            .await?;
            return Ok(());
        }
        if let Err(error) = file_transfer::append_upload_chunk(&mut upload, &payload) {
            write_file_transfer_json_quic(send, &transfer_error_to_response_async(error).await)
                .await?;
            return Ok(());
        }
    }

    let _ = file_transfer_upload_resumes()
        .lock()
        .await
        .remove(&resume_key);
    let response = match file_transfer::finalize_upload(upload) {
        Ok(response) => response,
        Err(error) => transfer_error_to_response_async(error).await,
    };
    write_file_transfer_json_quic(send, &response).await?;
    Ok(())
}

async fn read_file_transfer_quic_frame(
    recv: &mut quinn::RecvStream,
) -> Result<Option<(u8, Vec<u8>)>> {
    let mut header = [0u8; 5];
    if let Err(err) = recv.read_exact(&mut header).await {
        let message = err.to_string();
        if message.contains("finished early")
            || message.contains("closed")
            || message.contains("reset")
        {
            return Ok(None);
        }
        return Err(anyhow!("read file transfer frame header: {message}"));
    }

    let payload_len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if payload_len > talos_protocol::FILE_TRANSFER_MAX_PAYLOAD_LEN {
        return Err(anyhow!("file transfer payload too large"));
    }

    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        recv.read_exact(&mut payload)
            .await
            .map_err(|error| anyhow!("read file transfer payload: {error}"))?;
    }
    Ok(Some((header[0], payload)))
}

async fn write_file_transfer_json_quic(
    send: &mut quinn::SendStream,
    response: &FileTransferResponse,
) -> Result<()> {
    let payload = serde_json::to_vec(response).context("serialize file transfer response")?;
    let frame = build_file_transfer_frame(FILE_TRANSFER_MSG_JSON, &payload)
        .context("build file transfer json frame")?;
    send.write_all(&frame)
        .await
        .context("write file transfer json frame")?;
    Ok(())
}

async fn write_file_transfer_data_quic(send: &mut quinn::SendStream, chunk: &[u8]) -> Result<()> {
    let frame = build_file_transfer_frame(FILE_TRANSFER_MSG_DATA, chunk)
        .context("build file transfer data frame")?;
    send.write_all(&frame)
        .await
        .context("write file transfer data frame")?;
    Ok(())
}

async fn write_file_transfer_finish_quic(send: &mut quinn::SendStream) -> Result<()> {
    let frame = build_file_transfer_frame(FILE_TRANSFER_MSG_FINISH, &[])
        .context("build file transfer finish frame")?;
    send.write_all(&frame)
        .await
        .context("write file transfer finish frame")?;
    Ok(())
}

async fn write_file_transfer_error_quic(
    send: &mut quinn::SendStream,
    error: file_transfer::TransferError,
) -> Result<()> {
    let response = transfer_error_to_response_async(error).await;
    write_file_transfer_json_quic(send, &response).await
}

async fn transfer_error_to_response_async(
    error: file_transfer::TransferError,
) -> FileTransferResponse {
    surface_macos_file_transfer_permissions_if_needed(&error).await;
    transfer_error_to_response(error)
}

#[cfg(target_os = "macos")]
async fn surface_macos_file_transfer_permissions_if_needed(error: &file_transfer::TransferError) {
    if error.operation_error_code() == talos_protocol::OperationErrorCode::PermissionDenied {
        macos_desktop::surface_permissions_helper(Some("--full-disk-access-required")).await;
    }
}

#[cfg(not(target_os = "macos"))]
async fn surface_macos_file_transfer_permissions_if_needed(_error: &file_transfer::TransferError) {}

fn transfer_error_to_response(error: file_transfer::TransferError) -> FileTransferResponse {
    match error {
        file_transfer::TransferError::Conflict { path, message } => {
            FileTransferResponse::Conflict { path, message }
        }
        other => {
            let code = other.operation_error_code();
            let retryable = matches!(
                code,
                talos_protocol::OperationErrorCode::Timeout
                    | talos_protocol::OperationErrorCode::TransportLost
                    | talos_protocol::OperationErrorCode::Backpressure
            );
            file_transfer_error_response(code, other.to_string(), retryable)
        }
    }
}

async fn handle_punch_start(
    payload: PunchStartPayload,
    punch_sockets: &Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
) {
    let peer_addr = format!("{}:{}", payload.peer_reflex.ip, payload.peer_reflex.port)
        .parse::<std::net::SocketAddr>();
    let Ok(peer_addr) = peer_addr else {
        warn!(session_id = %payload.session_id, "invalid punch_start address");
        return;
    };

    let socket = {
        let sockets = punch_sockets.read().await;
        sockets.get(&payload.session_id).cloned()
    };
    let Some(socket) = socket else {
        warn!(session_id = %payload.session_id, "missing punch socket");
        return;
    };

    match socket.send_to(b"punch", peer_addr) {
        Ok(_) => {
            info!(
                session_id = %payload.session_id,
                peer = %peer_addr,
                "punch packet sent"
            );
        }
        Err(err) => {
            warn!(error = %err, session_id = %payload.session_id, "failed to send punch packet");
        }
    }
}

#[cfg(target_os = "windows")]
async fn ensure_capture_pipeline(
    session_id: String,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    helper_target_sessions: Arc<RwLock<HashMap<String, u32>>>,
) -> Arc<CapturePipeline> {
    let (session_seq, assigned_new) = get_or_assign_viewer_session_seq(&session_id);
    if assigned_new {
        info!(
            session_id = %session_id,
            session_seq = session_seq,
            "viewer session sequence assigned"
        );
    }

    if let Some(pipeline) = capture_pipelines.read().await.get(&session_id).cloned() {
        return pipeline;
    }

    let mut write_guard = capture_pipelines.write().await;
    if let Some(pipeline) = write_guard.get(&session_id).cloned() {
        return pipeline;
    }

    let requested_session_id = helper_target_sessions
        .read()
        .await
        .get(&session_id)
        .copied();

    let console_session_id = unsafe { winapi::um::winbase::WTSGetActiveConsoleSessionId() };
    let override_session_id = std::env::var("RMM_HELPER_SESSION_ID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok());
    let target_session_id = requested_session_id
        .filter(|sid| *sid > 0 && *sid < 65536)
        .or_else(|| override_session_id.filter(|sid| *sid > 0 && *sid < 65536))
        .unwrap_or(console_session_id);
    info!(
        rmm_session_id = %session_id,
        session_seq = session_seq,
        requested_session_id = requested_session_id,
        target_session_id = target_session_id,
        "launching helper for target session"
    );

    // Tighten pipe ACLs to only the helper's target session user + SYSTEM/admins.
    let allowed_user_sid = try_session_user_sid_string(target_session_id);
    if allowed_user_sid.is_none() {
        warn!(
            session_id = %session_id,
            target_session_id = target_session_id,
            "failed to resolve target session SID; falling back to INTERACTIVE pipe ACL"
        );
    }

    let pipeline = Arc::new(CapturePipeline::new());
    let stop_flag = pipeline.stop_flag();
    let pipe_instance = PIPE_INSTANCE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let pipe_name = build_pipe_name(&session_id, pipe_instance);
    let control_pipe_name = build_control_pipe_name(&session_id, pipe_instance);
    let selected_display_profile = viewer_session_profile(&session_id);
    let display_stream_mode = encode::display_stream_mode_for_profile(&selected_display_profile);
    let display_processing_mode =
        encode::display_processing_mode_for_profile(&selected_display_profile);

    info!(
        session_id = %session_id,
        session_seq = session_seq,
        pipe_instance = pipe_instance,
        pipe_name = %pipe_name,
        control_pipe_name = %control_pipe_name,
        allowed_user_sid_present = allowed_user_sid.is_some(),
        selected_display_profile = %selected_display_profile,
        display_stream_mode = display_stream_mode.as_str(),
        display_processing_mode = display_processing_mode,
        "pipeline setup start"
    );

    if should_fault_inject_helper_startup_fail_once(&session_id) {
        warn!(
            session_id = %session_id,
            session_seq = session_seq,
            pipe_instance = pipe_instance,
            "fault injection: simulating helper startup failure"
        );
        write_guard.insert(session_id, pipeline.clone());
        return pipeline;
    }

    // Per-launch secret used to authenticate the helper over the pipes.
    let pipe_auth_token = uuid::Uuid::new_v4().simple().to_string();

    let helper_path = match sibling_exe_path("talos_worker_helper.exe", "talos_worker_helper.exe") {
        Some(p) => p,
        None => {
            debug!("failed to resolve Talos Worker helper path");
            write_guard.insert(session_id, pipeline.clone());
            return pipeline;
        }
    };

    // Run blocking Win32 calls (CreateNamedPipe, launch helper) on a thread pool so the
    // message loop stays responsive and can process session_capabilities_request etc.
    let pipe_name_clone = pipe_name.clone();
    let control_pipe_name_clone = control_pipe_name.clone();
    let auth_clone = pipe_auth_token.clone();
    let allowed_clone = allowed_user_sid.clone();
    let session_id_for_helper = session_id.clone();
    let session_seq_for_helper = session_seq;
    let pipe_instance_for_helper = pipe_instance;
    let blocking_result = tokio::task::spawn_blocking(move || {
        use winapi::um::handleapi::CloseHandle;
        let pipe_handle = match create_named_pipe_server(&pipe_name_clone, allowed_clone.as_deref())
        {
            Ok(h) => h,
            Err(err) => return (None, None, false, Some(format!("capture pipe: {err}"))),
        };
        let control_handle = match create_named_pipe_server_outbound(
            &control_pipe_name_clone,
            allowed_clone.as_deref(),
        ) {
            Ok(h) => Some(h),
            Err(err) => {
                unsafe { CloseHandle(pipe_handle) };
                return (None, None, false, Some(format!("control pipe: {err}")));
            }
        };
        let launch_ok = display::launch_capture_helper_in_console_session(
            &helper_path,
            &session_id_for_helper,
            session_seq_for_helper,
            pipe_instance_for_helper,
            &pipe_name_clone,
            &control_pipe_name_clone,
            &auth_clone,
            display_stream_mode.as_str(),
            display_processing_mode,
            target_session_id,
        );
        if launch_ok.is_none() {
            unsafe { CloseHandle(pipe_handle) };
            if let Some(h) = control_handle {
                unsafe { CloseHandle(h) };
            }
            return (
                None,
                None,
                false,
                Some("helper launch returned false".to_string()),
            );
        }
        // Return as usize so the result is Send across thread boundary.
        (
            Some(pipe_handle as usize),
            control_handle.map(|h| h as usize),
            true,
            None,
        )
    })
    .await;

    let (pipe_handle_opt, control_pipe_handle, launch_ok, _setup_error) = match blocking_result {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "spawn_blocking for pipeline setup failed");
            write_guard.insert(session_id, pipeline.clone());
            return pipeline;
        }
    };

    let pipe_handle = match (pipe_handle_opt, launch_ok) {
        (Some(h), true) => h as winapi::um::winnt::HANDLE,
        _ => {
            if !launch_ok {
                debug!("failed to create pipes or launch capture helper in console session");
            } else {
                warn!("failed to create capture pipe");
            }
            write_guard.insert(session_id, pipeline.clone());
            return pipeline;
        }
    };

    spawn_pipe_reader(
        pipe_handle,
        pipeline.clone(),
        stop_flag,
        session_id.clone(),
        pipe_name.clone(),
        target_session_id,
        pipe_auth_token.clone(),
    );
    let control_pipe_handle_usize = control_pipe_handle;
    if let Some(handle) = control_pipe_handle_usize {
        info!(
            session_id = %session_id,
            session_seq = session_seq,
            pipe_instance = pipe_instance,
            "control pipe writer setup start"
        );
        setup_control_pipe_writer(
            session_id.clone(),
            handle,
            control_pipe_writers,
            pipe_auth_token,
        )
        .await;
        info!(
            session_id = %session_id,
            session_seq = session_seq,
            pipe_instance = pipe_instance,
            "control pipe writer setup finished"
        );
    }

    write_guard.insert(session_id.clone(), pipeline.clone());
    info!(
        session_id = %session_id,
        session_seq = session_seq,
        pipe_instance = pipe_instance,
        "pipeline setup complete"
    );
    pipeline
}

#[cfg(not(target_os = "macos"))]
async fn start_relay_client_once(
    session_id: String,
    relay_url: String,
    e2e_key: String,
    relay_sessions: Arc<RwLock<HashSet<String>>>,
    punch_sockets: Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    #[cfg(target_os = "windows")] capture_pipelines: Arc<
        RwLock<HashMap<String, Arc<CapturePipeline>>>,
    >,
    #[cfg(target_os = "windows")] control_queue: control::ControlQueue,
    #[cfg(target_os = "windows")] control_pipe_writers: Arc<
        RwLock<HashMap<String, ControlPipeWriter>>,
    >,
    #[cfg(target_os = "windows")] helper_target_sessions: Arc<RwLock<HashMap<String, u32>>>,
) {
    {
        let mut sessions = relay_sessions.write().await;
        if sessions.contains(&session_id) {
            return;
        }
        sessions.insert(session_id.clone());
    }

    tokio::spawn(async move {
        if let Err(err) = run_relay_client(
            session_id.clone(),
            relay_url,
            e2e_key,
            punch_sockets,
            relay_sessions.clone(),
            #[cfg(target_os = "windows")]
            capture_pipelines,
            #[cfg(target_os = "windows")]
            control_queue,
            #[cfg(target_os = "windows")]
            control_pipe_writers,
            #[cfg(target_os = "windows")]
            helper_target_sessions,
        )
        .await
        {
            warn!(error = %err, session_id = %session_id, "relay client ended unexpectedly");
        }
        let mut sessions = relay_sessions.write().await;
        sessions.remove(&session_id);
    });
}

async fn handle_file_transfer_relay_prepare(
    payload: &RelayPreparePayload,
    file_transfer_relay_sessions: &Arc<RwLock<HashSet<String>>>,
) {
    info!(
        session_id = %payload.session_id,
        mode = "file_transfer",
        platform = ?agent_platform(),
        "relay_prepare received"
    );
    start_file_transfer_relay_client_once(
        payload.session_id.clone(),
        payload.relay_url.clone(),
        payload.e2e_key.clone(),
        file_transfer_relay_sessions.clone(),
    )
    .await;
}

#[cfg(any(target_os = "windows", target_family = "unix"))]
async fn handle_shell_relay_prepare(
    payload: &RelayPreparePayload,
    shell_prepared_sessions: &Arc<RwLock<HashMap<String, PreparedShellSession>>>,
    shell_relay_sessions: &Arc<RwLock<HashSet<String>>>,
) {
    let prepared = {
        let sessions = shell_prepared_sessions.read().await;
        sessions.get(&payload.session_id).cloned()
    };
    let Some(prepared) = prepared else {
        warn!(
            session_id = %payload.session_id,
            "shell relay_prepare received before shell session was prepared"
        );
        return;
    };

    {
        let mut sessions = shell_relay_sessions.write().await;
        if !sessions.insert(payload.session_id.clone()) {
            return;
        }
    }

    let session_id = payload.session_id.clone();
    let relay_url = payload.relay_url.clone();
    let e2e_key = payload.e2e_key.clone();
    let token = prepared.token.clone();
    let shell_io = prepared.shell_io.clone();
    let relay_sessions = shell_relay_sessions.clone();
    tokio::spawn(async move {
        if let Err(err) =
            shell::run_shell_relay_shared(session_id.clone(), token, relay_url, e2e_key, shell_io)
                .await
        {
            warn!(session_id = %session_id, error = %err, "shell relay shared session failed");
        }
        relay_sessions.write().await.remove(&session_id);
    });
}

async fn start_file_transfer_relay_client_once(
    session_id: String,
    relay_url: String,
    e2e_key: String,
    relay_sessions: Arc<RwLock<HashSet<String>>>,
) {
    {
        let mut sessions = relay_sessions.write().await;
        if sessions.contains(&session_id) {
            return;
        }
        sessions.insert(session_id.clone());
    }

    tokio::spawn(async move {
        if let Err(err) =
            run_file_transfer_relay_client(session_id.clone(), relay_url, e2e_key).await
        {
            warn!(
                session_id = %session_id,
                error = %err,
                "file transfer relay client ended unexpectedly"
            );
        }
        let mut sessions = relay_sessions.write().await;
        sessions.remove(&session_id);
    });
}

async fn run_file_transfer_relay_client(
    session_id: String,
    relay_url: String,
    e2e_key_b64: String,
) -> Result<()> {
    prune_file_transfer_resume_state().await;
    let relay_target = parse_relay_target(&relay_url)?;
    let addr = format!("{}:{}", relay_target.host, relay_target.port);
    let connect_timeout = Duration::from_secs(
        env::var("RMM_RELAY_CONNECT_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(10),
    );
    let tcp_stream = timeout(connect_timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| anyhow!("connect relay tcp timed out"))?
        .context("connect relay tcp")?;
    tcp_stream
        .set_nodelay(true)
        .context("set relay TCP_NODELAY")?;

    let tls_config = build_relay_client_tls_config(None, None)?;
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name =
        ServerName::try_from(relay_target.host.clone()).context("build relay server name")?;
    let mut stream = timeout(connect_timeout, connector.connect(server_name, tcp_stream))
        .await
        .map_err(|_| anyhow!("relay tls connect timed out"))?
        .context("relay tls connect")?;

    let request = format!(
        "GET /relay/{session_id} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        session_id = session_id,
        host = relay_target.host
    );
    stream
        .write_all(request.as_bytes())
        .await
        .context("write relay request")?;
    timeout(connect_timeout, read_http_response(&mut stream))
        .await
        .map_err(|_| anyhow!("read relay response timed out"))??;

    let key_bytes = BASE64_STANDARD
        .decode(e2e_key_b64.trim())
        .context("decode relay e2e key")?;
    let cipher = build_e2e_cipher(&key_bytes)?;

    let mut send_counter = 0u64;
    write_e2e_frame(&mut stream, &cipher, &mut send_counter, b"hello-world").await?;
    info!(session_id = %session_id, "file transfer relay hello-world frame sent");

    let (mut reader, mut writer) = tokio::io::split(stream);
    loop {
        let payload = match read_e2e_frame_from(&mut reader, &cipher).await {
            Ok(payload) => payload,
            Err(err) => {
                if is_relay_connection_closed(&err) {
                    info!(session_id = %session_id, "file transfer relay session ended");
                    return Ok(());
                }
                return Err(err);
            }
        };

        if payload == HEARTBEAT_PAYLOAD || payload == b"hello-world" {
            continue;
        }

        let frame = match parse_file_transfer_frame(&payload) {
            Ok(frame) => frame,
            Err(err) => {
                write_file_transfer_json_relay(
                    &mut writer,
                    &cipher,
                    &mut send_counter,
                    &file_transfer_error_response(
                        talos_protocol::OperationErrorCode::InvalidRequest,
                        format!("invalid frame: {err}"),
                        false,
                    ),
                )
                .await?;
                continue;
            }
        };
        if frame.message_type != FILE_TRANSFER_MSG_JSON {
            write_file_transfer_json_relay(
                &mut writer,
                &cipher,
                &mut send_counter,
                &file_transfer_error_response(
                    talos_protocol::OperationErrorCode::InvalidRequest,
                    "expected json request frame",
                    false,
                ),
            )
            .await?;
            continue;
        }

        let request: FileTransferRequest = match serde_json::from_slice(frame.payload) {
            Ok(request) => request,
            Err(error) => {
                write_file_transfer_json_relay(
                    &mut writer,
                    &cipher,
                    &mut send_counter,
                    &file_transfer_error_response(
                        talos_protocol::OperationErrorCode::InvalidRequest,
                        format!("invalid request payload: {error}"),
                        false,
                    ),
                )
                .await?;
                continue;
            }
        };

        match request {
            FileTransferRequest::ListDir { path } => match file_transfer::list_dir(&path) {
                Ok(response) => {
                    write_file_transfer_json_relay(
                        &mut writer,
                        &cipher,
                        &mut send_counter,
                        &response,
                    )
                    .await?;
                }
                Err(error) => {
                    write_file_transfer_error_relay(&mut writer, &cipher, &mut send_counter, error)
                        .await?;
                }
            },
            FileTransferRequest::Download {
                transfer_id,
                paths,
                resume_offset,
            } => {
                handle_file_transfer_download_relay(
                    &session_id,
                    &mut writer,
                    &cipher,
                    &mut send_counter,
                    transfer_id,
                    paths,
                    resume_offset,
                )
                .await?;
            }
            FileTransferRequest::Rename { from_path, to_path } => {
                match file_transfer::rename_path(&from_path, &to_path) {
                    Ok(response) => {
                        write_file_transfer_json_relay(
                            &mut writer,
                            &cipher,
                            &mut send_counter,
                            &response,
                        )
                        .await?;
                    }
                    Err(error) => {
                        write_file_transfer_error_relay(
                            &mut writer,
                            &cipher,
                            &mut send_counter,
                            error,
                        )
                        .await?;
                    }
                }
            }
            FileTransferRequest::Delete { path, recursive } => {
                match file_transfer::delete_path(&path, recursive) {
                    Ok(response) => {
                        write_file_transfer_json_relay(
                            &mut writer,
                            &cipher,
                            &mut send_counter,
                            &response,
                        )
                        .await?;
                    }
                    Err(error) => {
                        write_file_transfer_error_relay(
                            &mut writer,
                            &cipher,
                            &mut send_counter,
                            error,
                        )
                        .await?;
                    }
                }
            }
            FileTransferRequest::Upload { .. } => {
                handle_file_transfer_upload_relay(
                    &session_id,
                    &mut reader,
                    &mut writer,
                    &cipher,
                    &mut send_counter,
                    request,
                )
                .await?;
            }
            FileTransferRequest::Cancel { transfer_id } => {
                clear_file_transfer_resume_state(&session_id, &transfer_id).await;
                write_file_transfer_json_relay(
                    &mut writer,
                    &cipher,
                    &mut send_counter,
                    &FileTransferResponse::Ok {},
                )
                .await?;
            }
        }
    }
}

async fn handle_file_transfer_download_relay<W>(
    session_id: &str,
    writer: &mut W,
    cipher: &ChaCha20Poly1305,
    send_counter: &mut u64,
    transfer_id: String,
    paths: Vec<String>,
    requested_resume_offset: u64,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let resume_key = file_transfer_resume_key(session_id, &transfer_id);
    let resumed_state = {
        let mut guard = file_transfer_download_resumes().lock().await;
        guard.remove(&resume_key)
    };
    let prepared = match resumed_state {
        Some(state) if state.requested_paths == paths => state.prepared,
        Some(state) => {
            cleanup_download_resume_state(state);
            let (progress_tx, mut progress_rx) =
                tokio::sync::mpsc::unbounded_channel::<file_transfer::ArchivePreparationProgress>();
            let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let paths_for_prepare = paths.clone();
            let cancelled_for_prepare = cancelled.clone();
            let mut prepare_task = tokio::task::spawn_blocking(move || {
                file_transfer::begin_download_with_progress_cancel(
                    &paths_for_prepare,
                    cancelled_for_prepare.as_ref(),
                    |progress| {
                        let _ = progress_tx.send(progress);
                    },
                )
            });

            loop {
                tokio::select! {
                    result = &mut prepare_task => {
                        match result {
                            Ok(Ok(prepared)) => break prepared,
                            Ok(Err(error)) => {
                                write_file_transfer_error_relay(writer, cipher, send_counter, error).await?;
                                return Ok(());
                            }
                            Err(error) => {
                                write_file_transfer_json_relay(
                                    writer,
                                    cipher,
                                    send_counter,
                                    &file_transfer_error_response(
                                        talos_protocol::OperationErrorCode::Internal,
                                        format!("download preparation failed: {error}"),
                                        true,
                                    ),
                                ).await?;
                                return Ok(());
                            }
                        }
                    }
                    Some(progress) = progress_rx.recv() => {
                        let message = if progress.files_total > 0 {
                            format!("Preparing archive: {} / {} file(s)", progress.files_done, progress.files_total)
                        } else {
                            "Preparing archive...".to_string()
                        };
                        if let Err(_err) = write_file_transfer_json_relay(
                            writer,
                            cipher,
                            send_counter,
                            &FileTransferResponse::Progress {
                                files_done: progress.files_done as u64,
                                files_total: progress.files_total as u64,
                                bytes_done: progress.bytes_done,
                                bytes_total: progress.bytes_total,
                                phase: Some("preparing".to_string()),
                                message: Some(message),
                            },
                        )
                        .await {
                            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
                            return Ok(());
                        }
                    }
                }
            }
        }
        None => {
            let (progress_tx, mut progress_rx) =
                tokio::sync::mpsc::unbounded_channel::<file_transfer::ArchivePreparationProgress>();
            let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let paths_for_prepare = paths.clone();
            let cancelled_for_prepare = cancelled.clone();
            let mut prepare_task = tokio::task::spawn_blocking(move || {
                file_transfer::begin_download_with_progress_cancel(
                    &paths_for_prepare,
                    cancelled_for_prepare.as_ref(),
                    |progress| {
                        let _ = progress_tx.send(progress);
                    },
                )
            });

            loop {
                tokio::select! {
                    result = &mut prepare_task => {
                        match result {
                            Ok(Ok(prepared)) => break prepared,
                            Ok(Err(error)) => {
                                write_file_transfer_error_relay(writer, cipher, send_counter, error).await?;
                                return Ok(());
                            }
                            Err(error) => {
                                write_file_transfer_json_relay(
                                    writer,
                                    cipher,
                                    send_counter,
                                    &file_transfer_error_response(
                                        talos_protocol::OperationErrorCode::Internal,
                                        format!("download preparation failed: {error}"),
                                        true,
                                    ),
                                ).await?;
                                return Ok(());
                            }
                        }
                    }
                    Some(progress) = progress_rx.recv() => {
                        let message = if progress.files_total > 0 {
                            format!("Preparing archive: {} / {} file(s)", progress.files_done, progress.files_total)
                        } else {
                            "Preparing archive...".to_string()
                        };
                        if let Err(_err) = write_file_transfer_json_relay(
                            writer,
                            cipher,
                            send_counter,
                            &FileTransferResponse::Progress {
                                files_done: progress.files_done as u64,
                                files_total: progress.files_total as u64,
                                bytes_done: progress.bytes_done,
                                bytes_total: progress.bytes_total,
                                phase: Some("preparing".to_string()),
                                message: Some(message),
                            },
                        )
                        .await {
                            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
                            return Ok(());
                        }
                    }
                }
            }
        }
    };

    let accepted_resume_offset = requested_resume_offset.min(prepared.size_bytes);

    let ready = FileTransferResponse::DownloadReady {
        transfer_id: transfer_id.clone(),
        file_name: prepared.file_name.clone(),
        size_bytes: prepared.size_bytes,
        is_archive: prepared.is_archive,
        resume_offset: accepted_resume_offset,
    };
    write_file_transfer_json_relay(writer, cipher, send_counter, &ready).await?;

    struct CleanupOnDrop(Option<std::path::PathBuf>);
    impl Drop for CleanupOnDrop {
        fn drop(&mut self) {
            if let Some(path) = self.0.take() {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    impl CleanupOnDrop {
        fn disarm(&mut self) {
            self.0 = None;
        }
    }
    let mut cleanup = if prepared.cleanup_source {
        CleanupOnDrop(Some(prepared.source_path.clone()))
    } else {
        CleanupOnDrop(None)
    };

    let mut file = fs::File::open(&prepared.source_path).context("open transfer source")?;
    file.seek(SeekFrom::Start(accepted_resume_offset))
        .context("seek transfer source to resume offset")?;
    let mut buffer = vec![0u8; talos_protocol::FILE_TRANSFER_DEFAULT_CHUNK_BYTES as usize];
    loop {
        let read = file.read(&mut buffer).context("read transfer source")?;
        if read == 0 {
            break;
        }
        if let Err(error) = write_file_transfer_frame_relay(
            writer,
            cipher,
            send_counter,
            FILE_TRANSFER_MSG_DATA,
            &buffer[..read],
        )
        .await
        {
            cleanup.disarm();
            remember_download_resume_state(session_id, &transfer_id, paths, prepared).await;
            debug!(session_id = %session_id, transfer_id = %transfer_id, error = %error, "file transfer relay download paused for resume");
            return Ok(());
        }
    }
    if let Err(error) =
        write_file_transfer_frame_relay(writer, cipher, send_counter, FILE_TRANSFER_MSG_FINISH, &[])
            .await
    {
        cleanup.disarm();
        remember_download_resume_state(session_id, &transfer_id, paths, prepared).await;
        debug!(session_id = %session_id, transfer_id = %transfer_id, error = %error, "file transfer relay download finish write paused for resume");
        return Ok(());
    }
    cleanup.disarm();
    remember_download_resume_state(session_id, &transfer_id, paths, prepared).await;
    Ok(())
}

async fn handle_file_transfer_upload_relay<R, W>(
    session_id: &str,
    reader: &mut R,
    writer: &mut W,
    cipher: &ChaCha20Poly1305,
    send_counter: &mut u64,
    request: FileTransferRequest,
) -> Result<()>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let (transfer_id, expected_size_bytes) = match &request {
        FileTransferRequest::Upload {
            transfer_id,
            expected_size_bytes,
            ..
        } => (transfer_id.clone(), *expected_size_bytes),
        _ => {
            write_file_transfer_json_relay(
                writer,
                cipher,
                send_counter,
                &file_transfer_error_response(
                    talos_protocol::OperationErrorCode::InvalidRequest,
                    "invalid request for upload",
                    false,
                ),
            )
            .await?;
            return Ok(());
        }
    };
    let resume_key = file_transfer_resume_key(session_id, &transfer_id);
    let mut upload = match file_transfer_upload_resumes()
        .lock()
        .await
        .remove(&resume_key)
    {
        Some(mut state) if resumable_upload_matches(&state, &request) => {
            let committed_bytes = fs::metadata(&state.upload.temp_input_path)
                .map(|metadata| metadata.len())
                .unwrap_or(state.upload.bytes_received);
            state.upload.bytes_received = committed_bytes;
            state.upload
        }
        Some(state) => {
            cleanup_upload_resume_state(state);
            match file_transfer::begin_upload(&request) {
                Ok(upload) => upload,
                Err(error) => {
                    write_file_transfer_error_relay(writer, cipher, send_counter, error).await?;
                    return Ok(());
                }
            }
        }
        None => match file_transfer::begin_upload(&request) {
            Ok(upload) => upload,
            Err(error) => {
                write_file_transfer_error_relay(writer, cipher, send_counter, error).await?;
                return Ok(());
            }
        },
    };

    struct CleanupOnDrop(Option<std::path::PathBuf>);
    impl Drop for CleanupOnDrop {
        fn drop(&mut self) {
            if let Some(path) = self.0.take() {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    impl CleanupOnDrop {
        fn disarm(&mut self) {
            self.0 = None;
        }
    }
    let mut cleanup = CleanupOnDrop(Some(upload.temp_input_path.clone()));

    write_file_transfer_json_relay(
        writer,
        cipher,
        send_counter,
        &FileTransferResponse::UploadReady {
            transfer_id: transfer_id.clone(),
            resume_offset: upload.bytes_received.min(expected_size_bytes),
        },
    )
    .await?;

    loop {
        let payload = match read_e2e_frame_from(reader, cipher).await {
            Ok(payload) => payload,
            Err(error) if is_relay_connection_closed(&error) => {
                cleanup.disarm();
                remember_upload_resume_state(
                    session_id,
                    ResumableUploadState {
                        upload,
                        expected_size_bytes,
                        touched_at: Instant::now(),
                    },
                )
                .await;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        if payload == HEARTBEAT_PAYLOAD || payload == b"hello-world" {
            continue;
        }
        let frame = parse_file_transfer_frame(&payload).context("parse upload relay frame")?;
        if frame.message_type == FILE_TRANSFER_MSG_FINISH {
            break;
        }
        if frame.message_type != FILE_TRANSFER_MSG_DATA {
            write_file_transfer_json_relay(
                writer,
                cipher,
                send_counter,
                &file_transfer_error_response(
                    talos_protocol::OperationErrorCode::InvalidRequest,
                    "expected data frame during upload",
                    false,
                ),
            )
            .await?;
            return Ok(());
        }
        if let Err(error) = file_transfer::append_upload_chunk(&mut upload, frame.payload) {
            write_file_transfer_error_relay(writer, cipher, send_counter, error).await?;
            return Ok(());
        }
    }

    let _ = file_transfer_upload_resumes()
        .lock()
        .await
        .remove(&resume_key);
    let response = match file_transfer::finalize_upload(upload) {
        Ok(response) => response,
        Err(error) => transfer_error_to_response_async(error).await,
    };
    write_file_transfer_json_relay(writer, cipher, send_counter, &response).await?;
    Ok(())
}

async fn write_file_transfer_json_relay<W>(
    writer: &mut W,
    cipher: &ChaCha20Poly1305,
    send_counter: &mut u64,
    response: &FileTransferResponse,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let payload = serde_json::to_vec(response).context("serialize file transfer response")?;
    write_file_transfer_frame_relay(
        writer,
        cipher,
        send_counter,
        FILE_TRANSFER_MSG_JSON,
        &payload,
    )
    .await
}

async fn write_file_transfer_error_relay<W>(
    writer: &mut W,
    cipher: &ChaCha20Poly1305,
    send_counter: &mut u64,
    error: file_transfer::TransferError,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let response = transfer_error_to_response_async(error).await;
    write_file_transfer_json_relay(writer, cipher, send_counter, &response).await
}

async fn write_file_transfer_frame_relay<W>(
    writer: &mut W,
    cipher: &ChaCha20Poly1305,
    send_counter: &mut u64,
    message_type: u8,
    payload: &[u8],
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let frame = build_file_transfer_frame(message_type, payload)
        .context("build file transfer relay frame")?;
    write_e2e_frame(writer, cipher, send_counter, &frame).await
}

async fn handle_relay_prepare(
    payload: &RelayPreparePayload,
    punch_sockets: &Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: &Arc<RwLock<HashSet<String>>>,
    file_transfer_relay_sessions: &Arc<RwLock<HashSet<String>>>,
    #[cfg(any(target_os = "windows", target_os = "macos"))] chat_relay_sessions: &Arc<
        RwLock<HashSet<String>>,
    >,
    #[cfg(any(target_os = "windows", target_os = "macos"))] chat_tunnels: &Arc<
        RwLock<HashMap<String, chat::ChatTunnelMeta>>,
    >,
    #[cfg(any(target_os = "windows", target_family = "unix"))] shell_prepared_sessions: &Arc<
        RwLock<HashMap<String, PreparedShellSession>>,
    >,
    #[cfg(any(target_os = "windows", target_family = "unix"))] shell_relay_sessions: &Arc<
        RwLock<HashSet<String>>,
    >,
    #[cfg(any(target_os = "windows", target_os = "macos"))] capture_pipelines: &Arc<
        RwLock<HashMap<String, Arc<CapturePipeline>>>,
    >,
    #[cfg(target_os = "windows")] control_queue: control::ControlQueue,
    #[cfg(any(target_os = "windows", target_os = "macos"))] control_pipe_writers: Arc<
        RwLock<HashMap<String, ControlPipeWriter>>,
    >,
    #[cfg(any(target_os = "windows", target_os = "macos"))] helper_target_sessions: Arc<
        RwLock<HashMap<String, u32>>,
    >,
) {
    if payload.mode == SessionTransportMode::FileTransfer {
        handle_file_transfer_relay_prepare(payload, file_transfer_relay_sessions).await;
        return;
    }
    #[cfg(target_os = "windows")]
    if payload.mode == SessionTransportMode::RemoteRegistry {
        handle_registry_relay_prepare(payload, relay_sessions).await;
        return;
    }
    if payload.mode == SessionTransportMode::Shell {
        handle_shell_relay_prepare(payload, shell_prepared_sessions, shell_relay_sessions).await;
        return;
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    if payload.mode == SessionTransportMode::Chat {
        info!(
            session_id = %payload.session_id,
            "chat relay_prepare ignored on unsupported non-Windows agent"
        );
        return;
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    if matches!(
        payload.mode,
        SessionTransportMode::RemoteDesktop
            | SessionTransportMode::HeadlessRemoteDesktop
            | SessionTransportMode::RemoteRegistry
    ) {
        info!(
            session_id = %payload.session_id,
            mode = ?payload.mode,
            "relay_prepare ignored for unsupported non-Windows interactive mode"
        );
        return;
    }
    #[cfg(target_os = "macos")]
    if payload.mode == SessionTransportMode::RemoteRegistry {
        info!(
            session_id = %payload.session_id,
            "remote registry relay_prepare ignored on macOS agent"
        );
        return;
    }
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    if payload.mode == SessionTransportMode::Chat {
        chat::handle_chat_relay_prepare(
            payload,
            chat_relay_sessions,
            chat_tunnels,
            helper_target_sessions.clone(),
        )
        .await;
        return;
    }
    info!(
        session_id = %payload.session_id,
        requested_display_profile = ?payload.selected_display_profile,
        "relay_prepare received"
    );

    // Relay is a transport fallback; do not gate it on a separate display check.
    let session_id = payload.session_id.clone();
    let relay_url = payload.relay_url.clone();
    let e2e_key = payload.e2e_key.clone();
    #[cfg(target_os = "windows")]
    let (selected_display_profile, profile_changed) =
        set_viewer_session_profile(&session_id, payload.selected_display_profile.as_deref());
    #[cfg(target_os = "windows")]
    if profile_changed {
        send_stop_capture_to_helper(&session_id, &control_pipe_writers).await;
        if let Some(pipeline) = capture_pipelines.write().await.remove(&session_id) {
            pipeline.request_stop();
            info!(
                session_id = %session_id,
                selected_display_profile = %selected_display_profile,
                "capture pipeline removed for relay display profile change"
            );
        }
        control_pipe_writers.write().await.remove(&session_id);
    }
    #[cfg(target_os = "windows")]
    info!(
        session_id = %session_id,
        selected_display_profile = %selected_display_profile,
        profile_changed = profile_changed,
        "remote desktop relay display profile selected"
    );
    #[cfg(target_os = "windows")]
    ensure_capture_pipeline(
        session_id.clone(),
        capture_pipelines,
        control_pipe_writers.clone(),
        helper_target_sessions.clone(),
    )
    .await;
    #[cfg(target_os = "macos")]
    {
        let capture_mode = set_macos_session_display_options(
            &session_id,
            payload.selected_display_profile.as_deref(),
            payload.hide_cursor,
        );
        if payload
            .selected_display_profile
            .as_deref()
            .is_some_and(|profile| {
                !matches!(
                    profile,
                    REMOTE_DESKTOP_PROFILE_LEGACY
                        | REMOTE_DESKTOP_PROFILE_MODERN_CPU
                        | REMOTE_DESKTOP_PROFILE_MODERN_GPU
                        | REMOTE_DESKTOP_PROFILE_EXPERIMENTAL
                        | REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY
                )
            })
        {
            warn!(
                session_id = %session_id,
                requested_display_profile = ?payload.selected_display_profile,
                "macOS relay_prepare requested unsupported display profile; using legacy"
            );
        }
        macos_desktop::ensure_capture_pipeline(
            session_id.clone(),
            capture_mode,
            capture_pipelines,
            control_pipe_writers.clone(),
        )
        .await;
        macos_desktop::start_relay_client_once(
            session_id,
            relay_url,
            e2e_key,
            relay_sessions.clone(),
            punch_sockets.clone(),
            capture_pipelines.clone(),
            control_pipe_writers.clone(),
        )
        .await;
        return;
    }
    #[cfg(target_os = "windows")]
    {
        start_relay_client_once(
            session_id,
            relay_url,
            e2e_key,
            relay_sessions.clone(),
            punch_sockets.clone(),
            capture_pipelines.clone(),
            control_queue,
            control_pipe_writers.clone(),
            helper_target_sessions.clone(),
        )
        .await;
    }
}

pub(crate) async fn build_quic_endpoint(
    cert_pem: &str,
    key_pem: &str,
) -> Result<(
    Endpoint,
    std::net::SocketAddr,
    UdpSocket,
    Result<ReflexAddress, String>,
)> {
    let certs = parse_certs(cert_pem)?;
    let key = parse_key(key_pem)?;

    let mut server_config =
        quinn::ServerConfig::with_single_cert(certs, key).context("build quic server config")?;
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(4)));
    let idle_timeout = quinn::IdleTimeout::try_from(Duration::from_secs(180))
        .map_err(|_| anyhow!("build quic idle timeout"))?;
    transport.max_idle_timeout(Some(idle_timeout));
    server_config.transport = Arc::new(transport);

    let socket = std::net::UdpSocket::bind("0.0.0.0:0").context("bind quic socket")?;
    let _local_port = socket.local_addr().context("read quic local addr")?.port();
    let stun_socket = socket.try_clone().context("clone quic socket")?;
    let punch_socket = socket.try_clone().context("clone punch socket")?;
    // Query STUN before Quinn owns the socket. On Linux, the endpoint receive loop can
    // consume non-QUIC STUN replies from cloned UDP sockets before stunclient reads them.
    let stun_result = query_stun_reflex(stun_socket)
        .await
        .map_err(|error| format!("{error:#}"));

    socket.set_nonblocking(true).context("set nonblocking")?;

    let endpoint = Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(server_config),
        socket,
        Arc::new(quinn::TokioRuntime),
    )
    .context("create quic endpoint")?;
    let local_addr = endpoint.local_addr().context("get quic local addr")?;

    Ok((endpoint, local_addr, punch_socket, stun_result))
}

pub(crate) async fn query_stun_reflex(stun_socket: UdpSocket) -> Result<ReflexAddress> {
    let stun_server = talos_protocol::configured_stun_server()
        .context("validate RMM_STUN_SERVER")?
        .context(
            "STUN is disabled; set RMM_STUN_SERVER to opt in to direct public-UDP discovery",
        )?;
    let reflex_addr = tokio::task::spawn_blocking(move || {
        let stun_addr = stun_server
            .to_socket_addrs()
            .with_context(|| format!("resolve configured STUN server {stun_server}"))?
            .find(|addr| addr.is_ipv4())
            .context("configured STUN server did not resolve to an IPv4 address")?;
        let mut client = StunClient::new(stun_addr);
        client
            .set_timeout(Duration::from_secs(2))
            .set_retry_interval(Duration::from_millis(250));
        client
            .query_external_address(&stun_socket)
            .context("query stun server")
    })
    .await
    .context("stun query task failed")??;

    Ok(ReflexAddress {
        ip: reflex_addr.ip().to_string(),
        port: reflex_addr.port(),
    })
}

fn local_quic_reflex_fallback(local_addr: std::net::SocketAddr) -> ReflexAddress {
    let ip = local_addrs()
        .into_iter()
        .find_map(|addr| {
            addr.ip
                .parse::<Ipv4Addr>()
                .ok()
                .filter(|ip| !ip.is_loopback() && !ip.is_link_local())
                .map(|_| addr.ip)
        })
        .unwrap_or_else(|| local_addr.ip().to_string());

    ReflexAddress {
        ip,
        port: local_addr.port(),
    }
}

fn parse_certs(cert_pem: &str) -> Result<Vec<CertificateDer<'static>>> {
    let mut reader = std::io::BufReader::new(cert_pem.as_bytes());
    let certs = certs(&mut reader)
        .collect::<std::io::Result<Vec<_>>>()
        .context("read cert pem")?;
    if certs.is_empty() {
        return Err(anyhow!("no certificates found"));
    }
    Ok(certs)
}

fn parse_key(key_pem: &str) -> Result<PrivateKeyDer<'static>> {
    let mut reader = std::io::BufReader::new(key_pem.as_bytes());
    let mut keys = pkcs8_private_keys(&mut reader)
        .collect::<std::io::Result<Vec<_>>>()
        .context("read private key pem")?;
    let key = keys.pop().context("no private key found in pem")?;
    Ok(PrivateKeyDer::from(key))
}

#[cfg(target_os = "windows")]
async fn accept_quic_connections(
    endpoint: Endpoint,
    local_addrs: Vec<LocalAddr>,
    session_id: String,
    punch_sockets: Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: Arc<RwLock<HashSet<String>>>,
    #[cfg(target_os = "windows")] capture_pipelines: Arc<
        RwLock<HashMap<String, Arc<CapturePipeline>>>,
    >,
    #[cfg(target_os = "windows")] control_queue: control::ControlQueue,
    #[cfg(target_os = "windows")] control_pipe_writers: Arc<
        RwLock<HashMap<String, ControlPipeWriter>>,
    >,
    #[cfg(target_os = "windows")] helper_target_sessions: Arc<RwLock<HashMap<String, u32>>>,
) -> Result<()> {
    #[cfg(not(target_os = "windows"))]
    let _ = (&session_id, &punch_sockets, &relay_sessions);

    // Only keep one active QUIC connection per session_id. A quick disconnect/reconnect
    // should replace the prior connection instead of getting stuck behind it.
    let active_connection: Arc<tokio::sync::Mutex<Option<Connection>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    loop {
        let Some(connecting) = endpoint.accept().await else {
            break;
        };
        let connection = match connecting.await {
            Ok(conn) => conn,
            Err(err) => {
                warn!(error = %err, "quic connection failed");
                continue;
            }
        };

        let source = if is_lan_connection(connection.remote_address(), &local_addrs) {
            "lan"
        } else {
            "reflex"
        };
        info!(
            remote = %connection.remote_address(),
            source = source,
            "quic connection accepted"
        );

        // Replace any prior connection for this session.
        {
            let mut guard = active_connection.lock().await;
            if let Some(prev) = guard.take() {
                // Best-effort: tell the previous session to go away.
                prev.close(0u32.into(), b"replaced");
            }
            *guard = Some(connection.clone());
        }

        // Handle the connection without blocking accept() for subsequent reconnects.
        #[cfg(target_os = "windows")]
        {
            let control_queue = control_queue.clone();
            let connection_for_task = connection.clone();
            let control_connection = connection.clone();
            let control_session_id = session_id.clone();
            let stream_session_id = session_id.clone();
            let punch_sockets_for_stream = punch_sockets.clone();
            let relay_sessions_for_stream = relay_sessions.clone();
            let capture_pipelines_for_stream = capture_pipelines.clone();
            let control_pipe_writers_for_stream = control_pipe_writers.clone();
            let helper_target_sessions_for_stream = helper_target_sessions.clone();
            tokio::spawn(async move {
                // Control stream reader
                let control_pipe_writers_for_control = control_pipe_writers_for_stream.clone();
                let capture_pipelines_for_control = capture_pipelines_for_stream.clone();
                let helper_target_sessions_for_control = helper_target_sessions_for_stream.clone();
                let control_queue_for_control = control_queue.clone();
                let control_session_id_for_control = control_session_id.clone();
                tokio::spawn(async move {
                    if let Err(err) = read_quic_control_stream(
                        control_connection,
                        control_session_id_for_control,
                        control_queue_for_control,
                        control_pipe_writers_for_control,
                        capture_pipelines_for_control,
                        helper_target_sessions_for_control,
                    )
                    .await
                    {
                        warn!(error = %err, "quic control stream ended");
                    }
                });

                let send = match connection_for_task.open_uni().await {
                    Ok(stream) => stream,
                    Err(err) => {
                        warn!(error = %err, "failed to open quic stream");
                        return;
                    }
                };

                let pipeline = ensure_capture_pipeline(
                    stream_session_id.clone(),
                    &capture_pipelines_for_stream,
                    control_pipe_writers_for_stream.clone(),
                    helper_target_sessions_for_stream.clone(),
                )
                .await;
                if let Err(err) = stream_quic_ivf(
                    stream_session_id,
                    send,
                    pipeline,
                    punch_sockets_for_stream,
                    relay_sessions_for_stream,
                    capture_pipelines_for_stream,
                    control_pipe_writers_for_stream,
                    helper_target_sessions_for_stream,
                )
                .await
                {
                    warn!(error = %err, "quic IVF stream failed");
                }
            });
        }

        #[cfg(not(target_os = "windows"))]
        {
            let connection_for_task = connection.clone();
            tokio::spawn(async move {
                let mut send = match connection_for_task.open_uni().await {
                    Ok(stream) => stream,
                    Err(err) => {
                        warn!(error = %err, "failed to open quic stream");
                        return;
                    }
                };
                if let Err(err) = send.write_all(b"Hello from agent\n").await {
                    warn!(error = %err, "failed to send hello-world frame");
                    return;
                }
                if let Err(err) = send.finish() {
                    warn!(error = %err, "failed to finish quic stream");
                } else {
                    info!("hello-world frame sent");
                }
            });
        }
    }

    Ok(())
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn find_buffered_chunk(pipeline: &CapturePipeline, seq: u64) -> Option<encode::IvfChunk> {
    pipeline.buffer.lock().ok().and_then(|guard| {
        guard
            .iter()
            .find(|item| item.seq == seq)
            .map(|item| item.chunk.clone())
    })
}

#[cfg(target_os = "windows")]
fn display_delta_record_type(chunk: &encode::IvfChunk) -> Option<u8> {
    match chunk {
        encode::IvfChunk::DisplayDelta(bytes) => bytes.first().copied(),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
fn is_display_delta_frame_begin(chunk: &encode::IvfChunk) -> bool {
    display_delta_record_type(chunk) == Some(DISPLAY_RECORD_FRAME_BEGIN)
}

#[cfg(target_os = "windows")]
fn is_display_delta_frame_end(chunk: &encode::IvfChunk) -> bool {
    display_delta_record_type(chunk) == Some(DISPLAY_RECORD_FRAME_END)
}

#[cfg(target_os = "windows")]
fn buffered_display_frame_from(
    pipeline: &CapturePipeline,
    min_seq: u64,
) -> Option<Vec<BufferedChunk>> {
    let guard = pipeline.buffer.lock().ok()?;
    let mut frame = Vec::new();
    let mut collecting = false;
    for item in guard.iter().filter(|item| item.seq >= min_seq) {
        if is_display_delta_frame_begin(&item.chunk) {
            frame.clear();
            frame.push(item.clone());
            collecting = true;
            continue;
        }
        if !collecting {
            continue;
        }
        frame.push(item.clone());
        if is_display_delta_frame_end(&item.chunk) {
            return Some(frame);
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn frame_safe_snapshot(snapshot: Vec<BufferedChunk>) -> Vec<BufferedChunk> {
    let mut safe = Vec::with_capacity(snapshot.len());
    let mut pending_display_frame = Vec::new();
    let mut collecting_display_frame = false;

    for item in snapshot {
        if display_delta_record_type(&item.chunk).is_some() {
            if is_display_delta_frame_begin(&item.chunk) {
                pending_display_frame.clear();
                pending_display_frame.push(item);
                collecting_display_frame = true;
                continue;
            }
            if collecting_display_frame {
                let frame_done = is_display_delta_frame_end(&item.chunk);
                pending_display_frame.push(item);
                if frame_done {
                    safe.append(&mut pending_display_frame);
                    collecting_display_frame = false;
                }
            }
            continue;
        }

        safe.push(item);
    }

    safe
}

#[cfg(target_os = "windows")]
async fn send_quic_buffered_chunks(
    send: &mut quinn::SendStream,
    chunks: &[BufferedChunk],
) -> Result<Option<u64>> {
    let mut last_seq = None;
    for item in chunks {
        send_quic_chunk(send, &item.chunk).await?;
        last_seq = Some(item.seq);
    }
    Ok(last_seq)
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
async fn send_quic_chunk(send: &mut quinn::SendStream, chunk: &encode::IvfChunk) -> Result<()> {
    match chunk {
        encode::IvfChunk::Metadata(m) => send.write_all(m).await?,
        encode::IvfChunk::Header(h) => send.write_all(h).await?,
        encode::IvfChunk::Frame(f) => send.write_all(f).await?,
        encode::IvfChunk::DisplayKeyframe(f) | encode::IvfChunk::DisplayDelta(f) => {
            send.write_all(f).await?
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn build_rdp_sessions_payload_json() -> (Vec<u8>, usize) {
    let sessions = display::enumerate_wts_sessions();
    let session_count = sessions.len();
    let sessions_payload: Vec<Value> = sessions
        .iter()
        .map(|session| {
            json!({
                "sessionId": session.session_id,
                "logicalSessionId": session.logical_session_id,
                "nativeSessionId": session.native_session_id,
                "kind": session.kind,
                "winStation": session.win_station,
                "userName": session.user_name,
                "state": session.state,
            })
        })
        .collect();
    let payload = json!({
        "type": "rdp_sessions",
        "sessions": sessions_payload,
    });
    match serde_json::to_vec(&payload) {
        Ok(bytes) => (bytes, session_count),
        Err(err) => {
            warn!(error = %err, "failed to serialize rdp_sessions payload");
            (br#"{"type":"rdp_sessions","sessions":[]}"#.to_vec(), 0)
        }
    }
}

#[cfg(target_os = "windows")]
fn wrap_rmmd_payload(json_payload: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(8 + json_payload.len());
    msg.extend_from_slice(b"RMMD");
    msg.extend_from_slice(&(json_payload.len() as u32).to_le_bytes());
    msg.extend_from_slice(json_payload);
    msg
}

#[cfg(target_os = "windows")]
fn parse_connection_ping_payload(payload: &[u8]) -> Option<u64> {
    if payload.len() != CONTROL_PAYLOAD_TIMESTAMP_LEN {
        return None;
    }
    Some(u64::from_be_bytes(payload.try_into().ok()?))
}

#[cfg(target_os = "windows")]
async fn emit_connection_pong_metadata(
    session_id: &str,
    echoed_at_ms: u64,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
) {
    let payload = ConnectionPongMetaPayload {
        message_type: "connection_pong",
        echoed_at_ms,
        agent_received_at_ms: now_unix_ms().min(u128::from(u64::MAX)) as u64,
    };
    let json_bytes = match serde_json::to_vec(&payload) {
        Ok(bytes) => bytes,
        Err(err) => {
            warn!(session_id = %session_id, error = %err, "failed to serialize connection pong metadata");
            return;
        }
    };
    let rmmd = wrap_rmmd_payload(&json_bytes);
    let pipeline = {
        let guard = capture_pipelines.read().await;
        guard.get(session_id).cloned()
    };
    if let Some(pipeline) = pipeline {
        pipeline.push_chunk(encode::IvfChunk::Metadata(rmmd));
    } else {
        warn!(session_id = %session_id, "dropping connection pong metadata because capture pipeline is missing");
    }
}

#[cfg(target_os = "windows")]
async fn stream_quic_ivf(
    session_id: String,
    mut send: quinn::SendStream,
    mut pipeline: Arc<CapturePipeline>,
    punch_sockets: Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: Arc<RwLock<HashSet<String>>>,
    capture_pipelines: Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    helper_target_sessions: Arc<RwLock<HashMap<String, u32>>>,
) -> Result<()> {
    let session_seq = viewer_session_seq(&session_id).unwrap_or(0);
    let streams_before = pipeline.active_streams.load(Ordering::SeqCst);
    pipeline.start_stream();
    let streams_after = pipeline.active_streams.load(Ordering::SeqCst);
    info!(
        session_id = %session_id,
        session_seq = session_seq,
        transport = "quic",
        streams_before = streams_before,
        streams_after = streams_after,
        "ivf stream started"
    );
    let mut relay_fallback_removed = false;
    let (rdp_sessions_payload, session_count) = build_rdp_sessions_payload_json();
    let rdp_sessions_meta = wrap_rmmd_payload(&rdp_sessions_payload);
    send.write_all(&rdp_sessions_meta)
        .await
        .context("send rdp session metadata over quic")?;
    info!(
        session_id = %session_id,
        session_count = session_count,
        "rdp_sessions metadata sent over quic"
    );

    let mut rx = pipeline.subscribe();
    let snapshot = frame_safe_snapshot(pipeline.snapshot());
    let mut last_seq: Option<u64> = None;
    let mut startup_meta: u64 = 0;
    let mut startup_header: u64 = 0;
    let mut startup_frame: u64 = 0;
    for item in snapshot {
        match &item.chunk {
            encode::IvfChunk::Metadata(_) => startup_meta = startup_meta.saturating_add(1),
            encode::IvfChunk::Header(_) => startup_header = startup_header.saturating_add(1),
            encode::IvfChunk::Frame(_)
            | encode::IvfChunk::DisplayKeyframe(_)
            | encode::IvfChunk::DisplayDelta(_) => startup_frame = startup_frame.saturating_add(1),
        }
        if let Err(e) = send_quic_chunk(&mut send, &item.chunk).await {
            let err = Err(e);
            let _ = stop_pipeline_if_idle(
                &session_id,
                &pipeline,
                &capture_pipelines,
                &control_pipe_writers,
                &helper_target_sessions,
                &punch_sockets,
                &relay_sessions,
            )
            .await;
            return err;
        }
        last_seq = Some(item.seq);
    }
    info!(
        session_id = %session_id,
        startup_meta = startup_meta,
        startup_header = startup_header,
        startup_frame = startup_frame,
        "startup snapshot sent"
    );

    let mut stop_flag = pipeline.stop_flag();
    let mut switched_pipeline = false;
    let mut switched_live_meta_skipped: u64 = 0;
    let mut switched_live_header_skipped: u64 = 0;
    let mut stop_without_replacement_ticks: u8 = 0;
    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(seq) => {
                        if let Some(last) = last_seq {
                            if seq <= last {
                                continue;
                            }
                        }
                        let Some(chunk) = find_buffered_chunk(&pipeline, seq) else {
                            let min_seq = last_seq.map_or(0, |last| last.saturating_add(1));
                            if let Some(frame) = buffered_display_frame_from(&pipeline, min_seq) {
                                let sent_last = match send_quic_buffered_chunks(&mut send, &frame).await {
                                    Ok(sent_last) => sent_last,
                                    Err(_e) => break,
                                };
                                if !relay_fallback_removed {
                                    relay_fallback_removed = true;
                                    if relay_sessions.write().await.remove(&session_id) {
                                        info!(
                                            session_id = %session_id,
                                            session_seq = session_seq,
                                            "relay fallback stream marked for shutdown after quic display frame resync"
                                        );
                                    }
                                }
                                last_seq = sent_last;
                            }
                            continue;
                        };
                        if display_delta_record_type(&chunk).is_some() {
                            let min_seq = last_seq.map_or(0, |last| last.saturating_add(1));
                            let Some(frame) = buffered_display_frame_from(&pipeline, min_seq) else {
                                continue;
                            };
                            let sent_last = match send_quic_buffered_chunks(&mut send, &frame).await {
                                Ok(sent_last) => sent_last,
                                Err(_e) => break,
                            };
                            if !relay_fallback_removed {
                                relay_fallback_removed = true;
                                if relay_sessions.write().await.remove(&session_id) {
                                    info!(
                                        session_id = %session_id,
                                        session_seq = session_seq,
                                        "relay fallback stream marked for shutdown after quic live display frame sent"
                                    );
                                } else {
                                    debug!(
                                        session_id = %session_id,
                                        session_seq = session_seq,
                                        "relay fallback was already inactive when quic live display frame was sent"
                                    );
                                }
                            }
                            if switched_pipeline {
                                switched_pipeline = false;
                                switched_live_meta_skipped = 0;
                                switched_live_header_skipped = 0;
                            }
                            last_seq = sent_last;
                            continue;
                        }
                        if switched_pipeline {
                            match &chunk {
                                encode::IvfChunk::Metadata(_) => {
                                    switched_live_meta_skipped =
                                        switched_live_meta_skipped.saturating_add(1);
                                    last_seq = Some(seq);
                                    continue;
                                }
                                encode::IvfChunk::Header(_) => {
                                    switched_live_header_skipped =
                                        switched_live_header_skipped.saturating_add(1);
                                    last_seq = Some(seq);
                                    continue;
                                }
                                encode::IvfChunk::Frame(_)
                                | encode::IvfChunk::DisplayKeyframe(_)
                                | encode::IvfChunk::DisplayDelta(_) => {}
                            }
                        }
                        if let Err(_e) = send_quic_chunk(&mut send, &chunk).await {
                            break;
                        }
                        if !relay_fallback_removed
                            && matches!(
                                &chunk,
                                encode::IvfChunk::Frame(_)
                                    | encode::IvfChunk::DisplayKeyframe(_)
                                    | encode::IvfChunk::DisplayDelta(_)
                            )
                        {
                            relay_fallback_removed = true;
                            if relay_sessions.write().await.remove(&session_id) {
                                info!(
                                    session_id = %session_id,
                                    session_seq = session_seq,
                                    "relay fallback stream marked for shutdown after quic live display frame sent"
                                );
                            } else {
                                debug!(
                                    session_id = %session_id,
                                    session_seq = session_seq,
                                    "relay fallback was already inactive when quic live display frame was sent"
                                );
                            }
                        }
                        if switched_pipeline {
                            switched_pipeline = false;
                            switched_live_meta_skipped = 0;
                            switched_live_header_skipped = 0;
                        }
                        last_seq = Some(seq);
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        let min_seq = last_seq.map_or(0, |last| last.saturating_add(1));
                        let Some(frame) = buffered_display_frame_from(&pipeline, min_seq) else {
                            continue;
                        };
                        warn!(
                            session_id = %session_id,
                            session_seq = session_seq,
                            skipped = skipped,
                            min_seq = min_seq,
                            resync_seq = frame.first().map(|item| item.seq),
                            "quic display stream lagged; resyncing at next complete display frame"
                        );
                        let sent_last = match send_quic_buffered_chunks(&mut send, &frame).await {
                            Ok(sent_last) => sent_last,
                            Err(_e) => break,
                        };
                        if !relay_fallback_removed {
                            relay_fallback_removed = true;
                            if relay_sessions.write().await.remove(&session_id) {
                                info!(
                                    session_id = %session_id,
                                    session_seq = session_seq,
                                    "relay fallback stream marked for shutdown after quic display frame resync"
                                );
                            }
                        }
                        last_seq = sent_last;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = sleep(Duration::from_millis(500)) => {
                let replacement = {
                    let guard = capture_pipelines.read().await;
                    guard.get(&session_id).cloned()
                };
                if let Some(new_pipeline) = replacement {
                    if !Arc::ptr_eq(&new_pipeline, &pipeline) {
                        info!(
                            session_id = %session_id,
                            "[REBUILD] quic stream switching to rebuilt pipeline"
                        );
                        let _ = stop_pipeline_if_idle(
                            &session_id,
                            &pipeline,
                            &capture_pipelines,
                            &control_pipe_writers,
                            &helper_target_sessions,
                            &punch_sockets,
                            &relay_sessions,
                        )
                        .await;
                        pipeline = new_pipeline;
                        switched_pipeline = true;
                        switched_live_meta_skipped = 0;
                        switched_live_header_skipped = 0;
                        pipeline.start_stream();
                        stop_flag = pipeline.stop_flag();

                        let (rdp_sessions_payload, session_count) = build_rdp_sessions_payload_json();
                        let rdp_sessions_meta = wrap_rmmd_payload(&rdp_sessions_payload);
                        let mut switch_snapshot_failed = false;
                        if let Err(_e) = send.write_all(&rdp_sessions_meta).await {
                            switch_snapshot_failed = true;
                        }

                        let mut switch_rx = pipeline.subscribe();
                        let snapshot = frame_safe_snapshot(pipeline.snapshot());
                        last_seq = None;
                        let switch_meta_sent: u64 = if switch_snapshot_failed { 0 } else { 1 };
                        let mut switch_meta_skipped: u64 = 0;
                        let mut switch_header_skipped: u64 = 0;
                        let mut switch_frame_sent: u64 = 0;
                        let mut switch_waited_for_live_frame = false;
                        let mut switch_live_frame_timeout = false;
                        let mut switch_wait_meta_skipped: u64 = 0;
                        let mut switch_wait_header_skipped: u64 = 0;
                        for item in snapshot {
                            if switch_snapshot_failed {
                                break;
                            }
                            match &item.chunk {
                                // Skip helper RMMD chunks; we already emitted a fresh session-list RMMD.
                                encode::IvfChunk::Metadata(_) => {
                                    switch_meta_skipped = switch_meta_skipped.saturating_add(1);
                                }
                                // Skip DKIF header during hot-swap to keep the decoder stable in-place.
                                encode::IvfChunk::Header(_) => {
                                    switch_header_skipped = switch_header_skipped.saturating_add(1);
                                }
                                encode::IvfChunk::Frame(_)
                                | encode::IvfChunk::DisplayKeyframe(_)
                                | encode::IvfChunk::DisplayDelta(_) => {
                                    switch_frame_sent = switch_frame_sent.saturating_add(1);
                                    if let Err(_e) = send_quic_chunk(&mut send, &item.chunk).await {
                                        switch_snapshot_failed = true;
                                        break;
                                    }
                                }
                            }
                            last_seq = Some(item.seq);
                        }
                        if !switch_snapshot_failed && switch_frame_sent == 0 {
                            switch_waited_for_live_frame = true;
                            let waited = timeout(Duration::from_millis(1500), async {
                                loop {
                                    let seq = switch_rx.recv().await.ok()?;
                                    let chunk = find_buffered_chunk(&pipeline, seq)?;
                                    match &chunk {
                                        encode::IvfChunk::Metadata(_) => {
                                            switch_wait_meta_skipped = switch_wait_meta_skipped.saturating_add(1);
                                            continue;
                                        }
                                        encode::IvfChunk::Header(_) => {
                                            switch_wait_header_skipped = switch_wait_header_skipped.saturating_add(1);
                                            continue;
                                        }
                                        encode::IvfChunk::DisplayDelta(_) => {
                                            let min_seq = last_seq.map_or(0, |last| last.saturating_add(1));
                                            if let Some(frame) = buffered_display_frame_from(&pipeline, min_seq) {
                                                return Some(frame);
                                            }
                                            continue;
                                        }
                                        encode::IvfChunk::Frame(_)
                                        | encode::IvfChunk::DisplayKeyframe(_) => {
                                            return Some(vec![BufferedChunk { seq, chunk }])
                                        }
                                    }
                                }
                            })
                            .await;
                            match waited {
                                Ok(Some(chunks)) => {
                                    match send_quic_buffered_chunks(&mut send, &chunks).await {
                                        Ok(sent_last) => {
                                            switch_frame_sent = switch_frame_sent.saturating_add(chunks.len() as u64);
                                            last_seq = sent_last;
                                        }
                                        Err(_e) => {
                                            switch_snapshot_failed = true;
                                        }
                                    }
                                }
                                _ => {
                                    switch_live_frame_timeout = true;
                                }
                            }
                        }
                        switched_live_meta_skipped =
                            switched_live_meta_skipped.saturating_add(switch_wait_meta_skipped);
                        switched_live_header_skipped = switched_live_header_skipped
                            .saturating_add(switch_wait_header_skipped);
                        info!(
                            session_id = %session_id,
                            switch_session_count = session_count,
                            switch_meta_sent = switch_meta_sent,
                            switch_meta_skipped = switch_meta_skipped,
                            switch_header_skipped = switch_header_skipped,
                            switch_frame_sent = switch_frame_sent,
                            switch_waited_for_live_frame = switch_waited_for_live_frame,
                            switch_live_frame_timeout = switch_live_frame_timeout,
                            switch_wait_meta_skipped = switch_wait_meta_skipped,
                            switch_wait_header_skipped = switch_wait_header_skipped,
                            switch_live_meta_skipped = switched_live_meta_skipped,
                            switch_live_header_skipped = switched_live_header_skipped,
                            switch_snapshot_failed = switch_snapshot_failed,
                            "switched pipeline snapshot filtered"
                        );

                        if switch_snapshot_failed {
                            break;
                        }
                        rx = switch_rx;
                        stop_without_replacement_ticks = 0;
                    }
                }
                if stop_flag.load(Ordering::SeqCst) {
                    stop_without_replacement_ticks = stop_without_replacement_ticks.saturating_add(1);
                    if stop_without_replacement_ticks >= 4 {
                        info!(
                            session_id = %session_id,
                            session_seq = session_seq,
                            transport = "quic",
                            "quic stream stopping due to stop_flag"
                        );
                        break;
                    }
                } else {
                    stop_without_replacement_ticks = 0;
                }
            }
        }
    }

    let _ = send.finish();
    info!(
        session_id = %session_id,
        session_seq = session_seq,
        transport = "quic",
        streams_before_finish = pipeline.active_streams.load(Ordering::SeqCst),
        "quic IVF stream finished"
    );
    let _ = stop_pipeline_if_idle(
        &session_id,
        &pipeline,
        &capture_pipelines,
        &control_pipe_writers,
        &helper_target_sessions,
        &punch_sockets,
        &relay_sessions,
    )
    .await;
    Ok(())
}

#[cfg(target_os = "windows")]
fn parse_target_session_id_payload(payload: &[u8]) -> Option<u32> {
    if payload.len() != CONTROL_PAYLOAD_SESSION_ID_LEN {
        return None;
    }
    Some(u32::from_be_bytes([
        payload[0], payload[1], payload[2], payload[3],
    ]))
}

#[cfg(target_os = "windows")]
fn is_valid_target_session_id(session_id: u32) -> bool {
    session_id > 0 && session_id < 65_536
}

#[cfg(target_os = "windows")]
async fn rebuild_pipeline_for_target_session(
    session_id: &str,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: &Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    helper_target_sessions: &Arc<RwLock<HashMap<String, u32>>>,
) -> Result<()> {
    {
        let mut guard = capture_pipelines.write().await;
        if let Some(existing) = guard.remove(session_id) {
            existing.request_stop();
        }
    }
    {
        let mut guard = control_pipe_writers.write().await;
        guard.remove(session_id);
    }

    // Allow old pipe handles to close before relaunching.
    tokio::time::sleep(Duration::from_millis(350)).await;

    for attempt in 1..=8 {
        ensure_capture_pipeline(
            session_id.to_string(),
            capture_pipelines,
            control_pipe_writers.clone(),
            helper_target_sessions.clone(),
        )
        .await;

        let has_writer = {
            let guard = control_pipe_writers.read().await;
            guard.contains_key(session_id)
        };
        if has_writer {
            info!(session_id = %session_id, attempt = attempt, "pipeline rebuild completed for session switch");
            return Ok(());
        }

        warn!(
            session_id = %session_id,
            attempt = attempt,
            "pipeline rebuild attempt missing control writer; retrying"
        );
        {
            let mut guard = capture_pipelines.write().await;
            if let Some(existing) = guard.remove(session_id) {
                existing.request_stop();
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    Err(anyhow!("pipeline rebuild failed after retries"))
}

#[cfg(target_os = "windows")]
async fn dispatch_control_message(
    session_id: &str,
    message_type: u8,
    payload: &[u8],
    control_pipe_writers: &Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    helper_target_sessions: &Arc<RwLock<HashMap<String, u32>>>,
) {
    if message_type == CONTROL_TYPE_CONNECTION_PING {
        let Some(echoed_at_ms) = parse_connection_ping_payload(payload) else {
            warn!(
                session_id = %session_id,
                payload_len = payload.len(),
                "invalid connection ping payload"
            );
            return;
        };
        emit_connection_pong_metadata(session_id, echoed_at_ms, capture_pipelines).await;
        return;
    }

    if message_type == CONTROL_TYPE_REGISTRY_REQUEST {
        warn!(
            session_id = %session_id,
            payload_len = payload.len(),
            "registry request received on remote desktop control channel; remote registry now requires its own session"
        );
        return;
    }

    if message_type == CONTROL_TYPE_SESSION_SWITCH {
        let Some(target_session_id) = parse_target_session_id_payload(payload) else {
            warn!(
                session_id = %session_id,
                payload_len = payload.len(),
                "invalid session switch payload"
            );
            return;
        };
        if !is_valid_target_session_id(target_session_id) {
            warn!(session_id = %session_id, target_session_id = target_session_id, "rejected invalid target session id");
            return;
        }
        let known_session = display::enumerate_wts_sessions()
            .into_iter()
            .find(|session| session.session_id == target_session_id);
        let Some(target_session) = known_session else {
            warn!(
                session_id = %session_id,
                target_session_id = target_session_id,
                "target session not found during switch"
            );
            return;
        };

        {
            let mut guard = helper_target_sessions.write().await;
            guard.insert(session_id.to_string(), target_session_id);
        }
        info!(
            session_id = %session_id,
            target_session_id = target_session_id,
            target_state = %target_session.state,
            target_user = %target_session.user_name,
            "session switch requested"
        );
        if let Err(err) = rebuild_pipeline_for_target_session(
            session_id,
            capture_pipelines,
            control_pipe_writers,
            helper_target_sessions,
        )
        .await
        {
            warn!(
                session_id = %session_id,
                target_session_id = target_session_id,
                error = %err,
                "session switch rebuild failed"
            );
        }
        return;
    }

    if message_type == CONTROL_TYPE_SESSION_LOGOFF {
        let Some(target_session_id) = parse_target_session_id_payload(payload) else {
            warn!(
                session_id = %session_id,
                payload_len = payload.len(),
                "invalid session logoff payload"
            );
            return;
        };
        if target_session_id <= 1 || !is_valid_target_session_id(target_session_id) {
            warn!(session_id = %session_id, target_session_id = target_session_id, "rejected session logoff request");
            return;
        }
        if display::logoff_wts_session(target_session_id) {
            info!(
                session_id = %session_id,
                target_session_id = target_session_id,
                "session logoff requested"
            );

            {
                let mut guard = helper_target_sessions.write().await;
                if guard.get(session_id).copied() == Some(target_session_id) {
                    guard.insert(session_id.to_string(), 1);
                }
            }
            if let Err(err) = rebuild_pipeline_for_target_session(
                session_id,
                capture_pipelines,
                control_pipe_writers,
                helper_target_sessions,
            )
            .await
            {
                warn!(
                    session_id = %session_id,
                    target_session_id = target_session_id,
                    error = %err,
                    "pipeline rebuild after logoff failed"
                );
            }
        } else {
            warn!(
                session_id = %session_id,
                target_session_id = target_session_id,
                "failed to log off session"
            );
        }
        return;
    }

    if let Some(writer) = control_pipe_writers.read().await.get(session_id).cloned() {
        let mut frame = Vec::with_capacity(3 + payload.len());
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        frame.push(message_type);
        frame.extend_from_slice(payload);
        enqueue_helper_control_frame(session_id, message_type, frame, &writer).await;
    }
}

/// Send stop_capture to the helper so it exits the capture loop promptly when the session is closed.
#[cfg(any(target_os = "windows", target_os = "macos"))]
async fn send_stop_capture_to_helper(
    session_id: &str,
    control_pipe_writers: &Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
) {
    if let Some(writer) = control_pipe_writers.read().await.get(session_id).cloned() {
        let frame: Vec<u8> = vec![0, 0, CONTROL_TYPE_STOP_CAPTURE]; // len=0 BE, type
        match writer.tx.try_send(frame) {
            Ok(()) => info!(session_id = %session_id, "stop_capture sent to helper"),
            Err(tokio::sync::mpsc::error::TrySendError::Full(frame)) => {
                info!(
                    session_id = %session_id,
                    "stop_capture queue full; waiting to deliver helper shutdown frame"
                );
                if writer.tx.send(frame).await.is_err() {
                    info!(
                        session_id = %session_id,
                        "stop_capture delivery failed because helper control channel closed"
                    );
                } else {
                    info!(session_id = %session_id, "stop_capture sent to helper");
                }
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_frame)) => {
                info!(
                    session_id = %session_id,
                    "stop_capture skipped because helper control channel is already closed"
                );
            }
        }
    } else {
        info!(session_id = %session_id, "stop_capture skipped (no control writer for session)");
    }
}

#[cfg(target_os = "windows")]
async fn read_quic_control_stream(
    connection: quinn::Connection,
    session_id: String,
    _control_queue: control::ControlQueue,
    control_pipe_writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    capture_pipelines: Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    helper_target_sessions: Arc<RwLock<HashMap<String, u32>>>,
) -> Result<()> {
    let mut recv = connection
        .accept_uni()
        .await
        .context("accept control stream")?;
    loop {
        let mut len_buf = [0u8; 2];
        if recv.read_exact(&mut len_buf).await.is_err() {
            break;
        }
        let payload_len = u16::from_be_bytes(len_buf) as usize;
        let mut type_buf = [0u8; 1];
        if recv.read_exact(&mut type_buf).await.is_err() {
            break;
        }
        let mut payload = vec![0u8; payload_len];
        if payload_len > 0 && recv.read_exact(&mut payload).await.is_err() {
            break;
        }
        dispatch_control_message(
            &session_id,
            type_buf[0],
            &payload,
            &control_pipe_writers,
            &capture_pipelines,
            &helper_target_sessions,
        )
        .await;
    }
    Ok(())
}

/// Heartbeat interval and disconnect threshold: viewer sends every 15s; 3 missed = disconnect.
#[cfg(target_os = "windows")]
const HEARTBEAT_INTERVAL_SECS: u64 = 15;
#[cfg(target_os = "windows")]
const HEARTBEAT_MISSED_THRESHOLD: u32 = 3;

#[cfg(any(target_os = "windows", target_os = "macos"))]
async fn send_relay_chunk<W>(
    stream: &mut W,
    cipher: &ChaCha20Poly1305,
    counter: &mut u64,
    chunk: &encode::IvfChunk,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    match chunk {
        encode::IvfChunk::Metadata(m) => write_e2e_frame_flush(stream, cipher, counter, m).await,
        encode::IvfChunk::Header(h) => write_e2e_frame_flush(stream, cipher, counter, h).await,
        encode::IvfChunk::Frame(f) => write_e2e_frame_flush(stream, cipher, counter, f).await,
        encode::IvfChunk::DisplayKeyframe(f) | encode::IvfChunk::DisplayDelta(f) => {
            write_e2e_frame_flush(stream, cipher, counter, f).await
        }
    }
}

#[cfg(target_os = "windows")]
async fn send_relay_buffered_chunks<W>(
    stream: &mut W,
    cipher: &ChaCha20Poly1305,
    counter: &mut u64,
    chunks: &[BufferedChunk],
) -> Result<Option<u64>>
where
    W: AsyncWriteExt + Unpin,
{
    let mut last_seq = None;
    for item in chunks {
        send_relay_chunk(stream, cipher, counter, &item.chunk).await?;
        last_seq = Some(item.seq);
    }
    Ok(last_seq)
}

#[cfg(target_os = "windows")]
async fn stream_relay_ivf<W>(
    session_id: String,
    stream: &mut W,
    cipher: &ChaCha20Poly1305,
    send_counter: &mut u64,
    mut pipeline: Arc<CapturePipeline>,
    punch_sockets: Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: Arc<RwLock<HashSet<String>>>,
    capture_pipelines: Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    helper_target_sessions: Arc<RwLock<HashMap<String, u32>>>,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let session_seq = viewer_session_seq(&session_id).unwrap_or(0);
    let streams_before = pipeline.active_streams.load(Ordering::SeqCst);
    pipeline.start_stream();
    let streams_after = pipeline.active_streams.load(Ordering::SeqCst);
    info!(
        session_id = %session_id,
        session_seq = session_seq,
        transport = "relay",
        streams_before = streams_before,
        streams_after = streams_after,
        "ivf stream started"
    );
    let mut rx = pipeline.subscribe();
    let snapshot = frame_safe_snapshot(pipeline.snapshot());
    let mut last_seq: Option<u64> = None;
    for item in snapshot {
        if let Err(e) = send_relay_chunk(stream, cipher, send_counter, &item.chunk).await {
            let _ = stop_pipeline_if_idle(
                &session_id,
                &pipeline,
                &capture_pipelines,
                &control_pipe_writers,
                &helper_target_sessions,
                &punch_sockets,
                &relay_sessions,
            )
            .await;
            return Err(e);
        }
        last_seq = Some(item.seq);
    }

    let mut stop_flag = pipeline.stop_flag();
    let mut stop_without_replacement_ticks: u8 = 0;
    let mut last_send_at = Instant::now();
    let mut idle_log_ticks: u16 = 0;
    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(seq) => {
                        if let Some(last) = last_seq {
                            if seq <= last {
                                continue;
                            }
                        }
                        let Some(chunk) = find_buffered_chunk(&pipeline, seq) else {
                            let min_seq = last_seq.map_or(0, |last| last.saturating_add(1));
                            if let Some(frame) = buffered_display_frame_from(&pipeline, min_seq) {
                                match send_relay_buffered_chunks(
                                    stream,
                                    cipher,
                                    send_counter,
                                    &frame,
                                )
                                .await
                                {
                                    Ok(sent_last) => {
                                        last_send_at = Instant::now();
                                        last_seq = sent_last;
                                    }
                                    Err(e) => {
                                        let _ = stop_pipeline_if_idle(
                                            &session_id,
                                            &pipeline,
                                            &capture_pipelines,
                                            &control_pipe_writers,
                                            &helper_target_sessions,
                                            &punch_sockets,
                                            &relay_sessions,
                                        )
                                        .await;
                                        return Err(e);
                                    }
                                }
                            }
                            continue;
                        };
                        if display_delta_record_type(&chunk).is_some() {
                            let min_seq = last_seq.map_or(0, |last| last.saturating_add(1));
                            let Some(frame) = buffered_display_frame_from(&pipeline, min_seq) else {
                                continue;
                            };
                            match send_relay_buffered_chunks(stream, cipher, send_counter, &frame).await {
                                Ok(sent_last) => {
                                    last_send_at = Instant::now();
                                    last_seq = sent_last;
                                }
                                Err(e) => {
                                    let _ = stop_pipeline_if_idle(
                                        &session_id,
                                        &pipeline,
                                        &capture_pipelines,
                                        &control_pipe_writers,
                                        &helper_target_sessions,
                                        &punch_sockets,
                                        &relay_sessions,
                                    )
                                    .await;
                                    return Err(e);
                                }
                            }
                            continue;
                        }
                        if let Err(e) = send_relay_chunk(stream, cipher, send_counter, &chunk).await {
                            let _ = stop_pipeline_if_idle(
                                &session_id,
                                &pipeline,
                                &capture_pipelines,
                                &control_pipe_writers,
                                &helper_target_sessions,
                                &punch_sockets,
                                &relay_sessions,
                            )
                            .await;
                            return Err(e);
                        }
                        last_send_at = Instant::now();
                        last_seq = Some(seq);
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        let min_seq = last_seq.map_or(0, |last| last.saturating_add(1));
                        let Some(frame) = buffered_display_frame_from(&pipeline, min_seq) else {
                            continue;
                        };
                        warn!(
                            session_id = %session_id,
                            session_seq = session_seq,
                            skipped = skipped,
                            min_seq = min_seq,
                            resync_seq = frame.first().map(|item| item.seq),
                            "relay display stream lagged; resyncing at next complete display frame"
                        );
                        match send_relay_buffered_chunks(stream, cipher, send_counter, &frame).await {
                            Ok(sent_last) => {
                                last_send_at = Instant::now();
                                last_seq = sent_last;
                            }
                            Err(e) => {
                                let _ = stop_pipeline_if_idle(
                                    &session_id,
                                    &pipeline,
                                    &capture_pipelines,
                                    &control_pipe_writers,
                                    &helper_target_sessions,
                                    &punch_sockets,
                                    &relay_sessions,
                                )
                                .await;
                                return Err(e);
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = sleep(Duration::from_millis(500)) => {
                let relay_still_active = {
                    let guard = relay_sessions.read().await;
                    guard.contains(&session_id)
                };
                if !relay_still_active {
                    info!(
                        session_id = %session_id,
                        session_seq = session_seq,
                        transport = "relay",
                        "relay stream stopping because it was replaced"
                    );
                    break;
                }
                idle_log_ticks = idle_log_ticks.saturating_add(1);
                if idle_log_ticks >= 20 {
                    idle_log_ticks = 0;
                    let idle_ms = last_send_at.elapsed().as_millis();
                    info!(
                        session_id = %session_id,
                        session_seq = session_seq,
                        transport = "relay",
                        idle_ms = idle_ms,
                        last_seq = last_seq,
                        stop_flag = stop_flag.load(Ordering::SeqCst),
                        "relay stream idle tick"
                    );
                }
                let replacement = {
                    let guard = capture_pipelines.read().await;
                    guard.get(&session_id).cloned()
                };
                if let Some(new_pipeline) = replacement {
                    if !Arc::ptr_eq(&new_pipeline, &pipeline) {
                        info!(
                            session_id = %session_id,
                            "[REBUILD] relay stream switching to rebuilt pipeline"
                        );
                        let _ = stop_pipeline_if_idle(
                            &session_id,
                            &pipeline,
                            &capture_pipelines,
                            &control_pipe_writers,
                            &helper_target_sessions,
                            &punch_sockets,
                            &relay_sessions,
                        )
                        .await;
                        pipeline = new_pipeline;
                        pipeline.start_stream();
                        stop_flag = pipeline.stop_flag();

                        let (rdp_sessions_payload, session_count) = build_rdp_sessions_payload_json();
                        write_e2e_frame(
                            stream,
                            cipher,
                            send_counter,
                            &rdp_sessions_payload,
                        )
                        .await
                        .context("send refreshed rdp_sessions metadata over relay")?;

                        let mut switch_rx = pipeline.subscribe();
                        let snapshot = frame_safe_snapshot(pipeline.snapshot());
                        last_seq = None;
                        let mut switch_meta_skipped: u64 = 0;
                        let mut switch_header_skipped: u64 = 0;
                        let mut switch_frame_sent: u64 = 0;
                        let mut switch_waited_for_live_frame = false;
                        let mut switch_live_frame_timeout = false;
                        for item in snapshot {
                            match &item.chunk {
                                encode::IvfChunk::Metadata(_) => {
                                    switch_meta_skipped = switch_meta_skipped.saturating_add(1);
                                }
                                encode::IvfChunk::Header(_) => {
                                    switch_header_skipped = switch_header_skipped.saturating_add(1);
                                }
                                encode::IvfChunk::Frame(_)
                                | encode::IvfChunk::DisplayKeyframe(_)
                                | encode::IvfChunk::DisplayDelta(_) => {
                                    switch_frame_sent = switch_frame_sent.saturating_add(1);
                                    if let Err(e) = send_relay_chunk(stream, cipher, send_counter, &item.chunk).await {
                                        let _ = stop_pipeline_if_idle(
                                            &session_id,
                                            &pipeline,
                                            &capture_pipelines,
                                            &control_pipe_writers,
                                            &helper_target_sessions,
                                            &punch_sockets,
                                            &relay_sessions,
                                        )
                                        .await;
                                        return Err(e);
                                    }
                                }
                            }
                            last_seq = Some(item.seq);
                        }
                        if switch_frame_sent == 0 {
                            switch_waited_for_live_frame = true;
                            let waited = timeout(Duration::from_millis(1500), async {
                                loop {
                                    let seq = switch_rx.recv().await.ok()?;
                                    let chunk = find_buffered_chunk(&pipeline, seq)?;
                                    match &chunk {
                                        encode::IvfChunk::Metadata(_) | encode::IvfChunk::Header(_) => continue,
                                        encode::IvfChunk::DisplayDelta(_) => {
                                            let min_seq = last_seq.map_or(0, |last| last.saturating_add(1));
                                            if let Some(frame) = buffered_display_frame_from(&pipeline, min_seq) {
                                                return Some(frame);
                                            }
                                            continue;
                                        }
                                        encode::IvfChunk::Frame(_)
                                        | encode::IvfChunk::DisplayKeyframe(_) => {
                                            return Some(vec![BufferedChunk { seq, chunk }])
                                        }
                                    }
                                }
                            })
                            .await;
                            match waited {
                                Ok(Some(chunks)) => {
                                    match send_relay_buffered_chunks(stream, cipher, send_counter, &chunks).await {
                                        Ok(sent_last) => {
                                            switch_frame_sent = switch_frame_sent.saturating_add(chunks.len() as u64);
                                            last_seq = sent_last;
                                        }
                                        Err(e) => {
                                            let _ = stop_pipeline_if_idle(
                                                &session_id,
                                                &pipeline,
                                                &capture_pipelines,
                                                &control_pipe_writers,
                                                &helper_target_sessions,
                                                &punch_sockets,
                                                &relay_sessions,
                                            )
                                            .await;
                                            return Err(e);
                                        }
                                    }
                                }
                                _ => {
                                    switch_live_frame_timeout = true;
                                }
                            }
                        }
                        info!(
                            session_id = %session_id,
                            switch_session_count = session_count,
                            switch_meta_skipped = switch_meta_skipped,
                            switch_header_skipped = switch_header_skipped,
                            switch_frame_sent = switch_frame_sent,
                            switch_waited_for_live_frame = switch_waited_for_live_frame,
                            switch_live_frame_timeout = switch_live_frame_timeout,
                            "relay switched pipeline snapshot filtered"
                        );
                        rx = switch_rx;
                        stop_without_replacement_ticks = 0;
                        continue;
                    }
                }
                if stop_flag.load(Ordering::SeqCst) {
                    stop_without_replacement_ticks = stop_without_replacement_ticks.saturating_add(1);
                    if stop_without_replacement_ticks >= 4 {
                        info!(
                            session_id = %session_id,
                            session_seq = session_seq,
                            transport = "relay",
                            "relay stream stopping due to stop_flag"
                        );
                        break;
                    }
                } else {
                    stop_without_replacement_ticks = 0;
                }
            }
        }
    }
    info!(
        session_id = %session_id,
        session_seq = session_seq,
        transport = "relay",
        streams_before_finish = pipeline.active_streams.load(Ordering::SeqCst),
        "relay IVF stream finished"
    );
    let _ = stop_pipeline_if_idle(
        &session_id,
        &pipeline,
        &capture_pipelines,
        &control_pipe_writers,
        &helper_target_sessions,
        &punch_sockets,
        &relay_sessions,
    )
    .await;
    Ok(())
}

#[cfg(target_os = "windows")]
async fn stop_pipeline_if_idle(
    session_id: &str,
    pipeline: &Arc<CapturePipeline>,
    capture_pipelines: &Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    control_pipe_writers: &Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    helper_target_sessions: &Arc<RwLock<HashMap<String, u32>>>,
    punch_sockets: &Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: &Arc<RwLock<HashSet<String>>>,
) -> Result<()> {
    let session_seq = viewer_session_seq(session_id).unwrap_or(0);
    let streams_before = pipeline.active_streams.load(Ordering::SeqCst);
    let should_stop = pipeline.finish_stream();
    let current_pipeline_matches = {
        let guard = capture_pipelines.read().await;
        guard
            .get(session_id)
            .map(|current| Arc::ptr_eq(current, pipeline))
            .unwrap_or(false)
    };
    let (writers_len, writer_present) = {
        let guard = control_pipe_writers.read().await;
        (guard.len(), guard.contains_key(session_id))
    };
    info!(
        session_id = %session_id,
        session_seq = session_seq,
        streams_before = streams_before,
        should_stop = should_stop,
        current_pipeline_matches = current_pipeline_matches,
        control_writers_len = writers_len,
        control_writer_present = writer_present,
        "pipeline stream finished; teardown decision"
    );

    if should_stop {
        if !current_pipeline_matches {
            pipeline.request_stop();
            info!(
                session_id = %session_id,
                session_seq = session_seq,
                "pipeline teardown skipped because session already points to a rebuilt pipeline"
            );
            return Ok(());
        }
        let started_ms = viewer_session_started_ms(session_id);
        let elapsed_ms = started_ms.map(|s| now_unix_ms().saturating_sub(s));
        info!(
            session_id = %session_id,
            session_seq = session_seq,
            started_ms = started_ms,
            elapsed_ms = elapsed_ms,
            "pipeline teardown start"
        );
        // Tell the capture helper to exit promptly so it doesn't linger (avoids accumulation / freezes).
        send_stop_capture_to_helper(session_id, control_pipe_writers).await;
        // Brief delay so the helper's control thread can receive stop_capture and set the stop flag
        // before we close the pipe (reduces chance of helper still in DXGI when pipe breaks).
        tokio::time::sleep(Duration::from_millis(200)).await;
        pipeline.request_stop();
        info!(
            session_id = %session_id,
            session_seq = session_seq,
            "pipeline request_stop set"
        );
        let mut guard = capture_pipelines.write().await;
        let removed_current = guard
            .get(session_id)
            .map(|current| Arc::ptr_eq(current, pipeline))
            .unwrap_or(false);
        if removed_current {
            guard.remove(session_id);
            info!(
                session_id = %session_id,
                session_seq = session_seq,
                "pipeline removed from map"
            );
            remove_remote_desktop_session_state(
                session_id,
                control_pipe_writers,
                helper_target_sessions,
                punch_sockets,
                relay_sessions,
            )
            .await;
        } else {
            info!(
                session_id = %session_id,
                session_seq = session_seq,
                "pipeline map removal skipped because session already points to a rebuilt pipeline"
            );
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
async fn run_relay_client(
    session_id: String,
    relay_url: String,
    e2e_key_b64: String,
    punch_sockets: Arc<RwLock<HashMap<String, Arc<UdpSocket>>>>,
    relay_sessions: Arc<RwLock<HashSet<String>>>,
    #[cfg(target_os = "windows")] capture_pipelines: Arc<
        RwLock<HashMap<String, Arc<CapturePipeline>>>,
    >,
    #[cfg(target_os = "windows")] control_queue: control::ControlQueue,
    #[cfg(target_os = "windows")] control_pipe_writers: Arc<
        RwLock<HashMap<String, ControlPipeWriter>>,
    >,
    #[cfg(target_os = "windows")] helper_target_sessions: Arc<RwLock<HashMap<String, u32>>>,
) -> Result<()> {
    #[cfg(not(target_os = "windows"))]
    let _ = (&punch_sockets, &relay_sessions);

    let relay_target = parse_relay_target(&relay_url)?;
    let addr = format!("{}:{}", relay_target.host, relay_target.port);
    let connect_timeout = Duration::from_secs(
        env::var("RMM_RELAY_CONNECT_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(10),
    );
    let tcp_stream = timeout(connect_timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| anyhow!("connect relay tcp timed out"))?
        .context("connect relay tcp")?;
    tcp_stream
        .set_nodelay(true)
        .context("set relay TCP_NODELAY")?;

    let tls_config = build_relay_client_tls_config(None, None)?;
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name =
        ServerName::try_from(relay_target.host.clone()).context("build relay server name")?;
    let mut stream = timeout(connect_timeout, connector.connect(server_name, tcp_stream))
        .await
        .map_err(|_| anyhow!("relay tls connect timed out"))?
        .context("relay tls connect")?;

    let request = format!(
        "GET /relay/{session_id} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        session_id = session_id,
        host = relay_target.host
    );
    stream
        .write_all(request.as_bytes())
        .await
        .context("write relay request")?;
    timeout(connect_timeout, read_http_response(&mut stream))
        .await
        .map_err(|_| anyhow!("read relay response timed out"))??;

    let key_bytes = BASE64_STANDARD
        .decode(e2e_key_b64.trim())
        .context("decode relay e2e key")?;
    let cipher = build_e2e_cipher(&key_bytes)?;

    let mut send_counter = 0u64;
    write_e2e_frame(&mut stream, &cipher, &mut send_counter, b"hello-world").await?;
    info!(session_id = %session_id, "relay hello-world frame sent");

    #[cfg(target_os = "windows")]
    {
        let (rdp_sessions_payload, session_count) = build_rdp_sessions_payload_json();
        write_e2e_frame(
            &mut stream,
            &cipher,
            &mut send_counter,
            &rdp_sessions_payload,
        )
        .await
        .context("send rdp session list over relay")?;
        info!(
            session_id = %session_id,
            session_count = session_count,
            "rdp_sessions payload sent over relay"
        );

        let pipeline = ensure_capture_pipeline(
            session_id.clone(),
            &capture_pipelines,
            control_pipe_writers.clone(),
            helper_target_sessions.clone(),
        )
        .await;
        let (reader, mut writer) = tokio::io::split(stream);
        let cipher_read = build_e2e_cipher(&key_bytes)?;
        tokio::spawn(run_heartbeat_read_loop(
            session_id.clone(),
            reader,
            cipher_read,
            pipeline.clone(),
            control_queue.clone(),
            control_pipe_writers.clone(),
            capture_pipelines.clone(),
            helper_target_sessions.clone(),
        ));
        if let Err(e) = stream_relay_ivf(
            session_id.clone(),
            &mut writer,
            &cipher,
            &mut send_counter,
            pipeline,
            punch_sockets.clone(),
            relay_sessions.clone(),
            capture_pipelines.clone(),
            control_pipe_writers.clone(),
            helper_target_sessions.clone(),
        )
        .await
        {
            if is_relay_connection_closed(&e) {
                info!(session_id = %session_id, "relay session ended (viewer disconnected)");
                return Ok(());
            }
            return Err(e);
        }
        info!(session_id = %session_id, "relay IVF stream finished");
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    loop {
        match read_e2e_frame_from(&mut stream, &cipher).await {
            Ok(payload) => {
                let message = String::from_utf8_lossy(&payload);
                info!(session_id = %session_id, payload = %message.trim(), "relay frame received");
            }
            Err(e) => {
                if is_relay_connection_closed(&e) {
                    info!(session_id = %session_id, "relay session ended (viewer disconnected)");
                    return Ok(());
                }
                return Err(e);
            }
        }
    }
}

fn is_relay_connection_closed(err: &anyhow::Error) -> bool {
    use std::io::ErrorKind;
    for cause in err.chain() {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            return matches!(
                io.kind(),
                ErrorKind::UnexpectedEof
                    | ErrorKind::ConnectionReset
                    | ErrorKind::BrokenPipe
                    | ErrorKind::ConnectionAborted
            );
        }
    }
    false
}

#[cfg(target_os = "windows")]
async fn run_heartbeat_read_loop<R>(
    session_id: String,
    mut reader: R,
    cipher: ChaCha20Poly1305,
    pipeline: Arc<CapturePipeline>,
    _control_queue: control::ControlQueue,
    control_pipe_writers: Arc<RwLock<HashMap<String, ControlPipeWriter>>>,
    capture_pipelines: Arc<RwLock<HashMap<String, Arc<CapturePipeline>>>>,
    helper_target_sessions: Arc<RwLock<HashMap<String, u32>>>,
) where
    R: AsyncReadExt + Unpin + Send,
{
    let heartbeat_timeout = Duration::from_secs(HEARTBEAT_INTERVAL_SECS + 2); // slightly over 15s per expected heartbeat
    let mut missed: u32 = 0;
    while missed < HEARTBEAT_MISSED_THRESHOLD {
        match timeout(heartbeat_timeout, read_e2e_frame_from(&mut reader, &cipher)).await {
            Ok(Ok(payload)) => {
                if payload == HEARTBEAT_PAYLOAD || payload == b"hello-world" {
                    missed = 0;
                } else {
                    match parse_control_frame(&payload) {
                        Ok(frame) => {
                            dispatch_control_message(
                                &session_id,
                                frame.message_type,
                                frame.payload,
                                &control_pipe_writers,
                                &capture_pipelines,
                                &helper_target_sessions,
                            )
                            .await;
                        }
                        Err(err) => {
                            warn!(
                                session_id = %session_id,
                                error = %err,
                                len = payload.len(),
                                "invalid control frame"
                            );
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                if is_relay_connection_closed(&e) {
                    info!(session_id = %session_id, "viewer connection closed, stopping capture");
                    pipeline.request_stop();
                    return;
                }
                missed += 1;
                warn!(session_id = %session_id, missed, error = %e, "heartbeat read error");
            }
            Err(_) => {
                missed += 1;
                warn!(session_id = %session_id, missed, "heartbeat timeout (no frame received)");
            }
        }
    }
    info!(
        session_id = %session_id,
        "viewer heartbeat missed 3 times, stopping capture gracefully"
    );
    pipeline.request_stop();
}

fn os_release_value(key: &str) -> Option<String> {
    let content = fs::read_to_string("/etc/os-release").ok()?;
    for line in content.lines() {
        let Some((line_key, raw_value)) = line.split_once('=') else {
            continue;
        };
        if line_key != key {
            continue;
        }
        return Some(raw_value.trim_matches('"').to_string());
    }
    None
}

fn linux_distro_name() -> String {
    os_release_value("PRETTY_NAME")
        .or_else(|| {
            let name = os_release_value("NAME")?;
            let version = os_release_value("VERSION_ID");
            Some(match version {
                Some(version) if !version.is_empty() => format!("{name} {version}"),
                _ => name,
            })
        })
        .unwrap_or_else(|| System::long_os_version().unwrap_or_default())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn collect_logged_in_users() -> Vec<LoggedInUserInfo> {
    let output = std::process::Command::new("who").output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let username = parts.next()?.to_string();
            let terminal = parts.next().map(|value| value.to_string());
            let host = line
                .split_once('(')
                .and_then(|(_, rest)| rest.split_once(')').map(|(host, _)| host.to_string()));
            Some(LoggedInUserInfo {
                username,
                terminal,
                host,
            })
        })
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn collect_logged_in_users() -> Vec<LoggedInUserInfo> {
    Vec::new()
}

fn collect_disk_info(disks: &Disks) -> Vec<DiskInfo> {
    let mut by_device: HashMap<String, DiskInfo> = HashMap::new();
    for disk in disks.iter() {
        let info = DiskInfo {
            name: disk.name().to_string_lossy().to_string(),
            mount_point: disk.mount_point().to_string_lossy().to_string(),
            total_bytes: disk.total_space(),
            available_bytes: disk.available_space(),
            file_system: disk.file_system().to_string_lossy().to_string(),
        };
        if info.total_bytes == 0 || info.mount_point.is_empty() {
            continue;
        }
        let key = disk_dedupe_key(&info);
        let replace = by_device
            .get(&key)
            .map(|existing| {
                disk_mount_rank(&info.mount_point) < disk_mount_rank(&existing.mount_point)
            })
            .unwrap_or(true);
        if replace {
            by_device.insert(key, info);
        }
    }

    let mut disks = by_device.into_values().collect::<Vec<_>>();
    disks.sort_by(|a, b| {
        disk_mount_rank(&a.mount_point)
            .cmp(&disk_mount_rank(&b.mount_point))
            .then_with(|| a.mount_point.cmp(&b.mount_point))
    });
    disks
}

fn disk_dedupe_key(disk: &DiskInfo) -> String {
    if disk.name.trim().is_empty() {
        format!(
            "mount:{}|{}|{}",
            disk.mount_point, disk.total_bytes, disk.file_system
        )
    } else {
        format!("{}|{}|{}", disk.name, disk.total_bytes, disk.file_system)
    }
}

fn disk_mount_rank(mount_point: &str) -> (u8, usize, usize) {
    if mount_point == "/" {
        return (0, 0, 1);
    }
    if mount_point.starts_with("/boot") {
        return (1, mount_point.matches('/').count(), mount_point.len());
    }
    (2, mount_point.matches('/').count(), mount_point.len())
}

fn collect_network_info(networks: &Networks) -> Vec<NetworkInfo> {
    let mut traffic_by_name = HashMap::new();
    for (name, data) in networks.iter() {
        traffic_by_name.insert(name.to_string(), (data.received(), data.transmitted()));
    }

    let mut addresses_by_name = collect_interface_addresses();
    let mut gateways_by_name = collect_default_gateways();
    let dns_servers = collect_dns_servers();

    let mut names = HashSet::new();
    names.extend(traffic_by_name.keys().cloned());
    names.extend(addresses_by_name.keys().cloned());
    names.extend(gateways_by_name.keys().cloned());

    let mut result = Vec::new();
    for name in names {
        let ips = addresses_by_name.remove(&name).unwrap_or_default();
        let gateways = gateways_by_name.remove(&name).unwrap_or_default();
        if ips.is_empty() && gateways.is_empty() {
            continue;
        }
        let (received_bytes, transmitted_bytes) = traffic_by_name.remove(&name).unwrap_or((0, 0));
        result.push(NetworkInfo {
            name,
            received_bytes,
            transmitted_bytes,
            ips,
            gateways,
            dns_servers: dns_servers.clone(),
        });
    }

    result.sort_by(|a, b| {
        let a_default = !a.gateways.is_empty();
        let b_default = !b.gateways.is_empty();
        b_default.cmp(&a_default).then_with(|| a.name.cmp(&b.name))
    });
    result
}

fn collect_interface_addresses() -> HashMap<String, Vec<NetworkAddressInfo>> {
    let mut addresses_by_name: HashMap<String, Vec<NetworkAddressInfo>> = HashMap::new();
    let Ok(interfaces) = get_if_addrs() else {
        return addresses_by_name;
    };

    for iface in interfaces {
        match iface.addr {
            IfAddr::V4(v4) => {
                if v4.ip.is_loopback() || v4.ip.is_unspecified() || v4.ip.is_link_local() {
                    continue;
                }
                addresses_by_name
                    .entry(iface.name)
                    .or_default()
                    .push(NetworkAddressInfo {
                        address: v4.ip.to_string(),
                        family: "ipv4".to_string(),
                        prefix: netmask_to_prefix(v4.netmask),
                        netmask: v4.netmask.to_string(),
                    });
            }
            IfAddr::V6(v6) => {
                if v6.ip.is_loopback() || v6.ip.is_unspecified() || is_ipv6_link_local(v6.ip) {
                    continue;
                }
                addresses_by_name
                    .entry(iface.name)
                    .or_default()
                    .push(NetworkAddressInfo {
                        address: v6.ip.to_string(),
                        family: "ipv6".to_string(),
                        prefix: ipv6_netmask_to_prefix(v6.netmask),
                        netmask: v6.netmask.to_string(),
                    });
            }
        }
    }

    for addresses in addresses_by_name.values_mut() {
        let mut seen = HashSet::new();
        addresses.retain(|addr| {
            let key = format!("{}|{}|{}", addr.address, addr.prefix, addr.netmask);
            seen.insert(key)
        });
        addresses.sort_by(|a, b| {
            a.family
                .cmp(&b.family)
                .then_with(|| a.address.cmp(&b.address))
        });
    }
    addresses_by_name
}

fn collect_default_gateways() -> HashMap<String, Vec<String>> {
    let mut gateways_by_name: HashMap<String, Vec<String>> = HashMap::new();
    let Ok(contents) = fs::read_to_string("/proc/net/route") else {
        return gateways_by_name;
    };

    for line in contents.lines().skip(1) {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 3 || columns[1] != "00000000" || columns[2] == "00000000" {
            continue;
        }
        let Some(gateway) = parse_linux_route_gateway(columns[2]) else {
            continue;
        };
        let gateways = gateways_by_name.entry(columns[0].to_string()).or_default();
        if !gateways.contains(&gateway) {
            gateways.push(gateway);
        }
    }

    gateways_by_name
}

fn parse_linux_route_gateway(hex: &str) -> Option<String> {
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(Ipv4Addr::from(value.to_le_bytes()).to_string())
}

fn collect_dns_servers() -> Vec<String> {
    let Ok(contents) = fs::read_to_string("/etc/resolv.conf") else {
        return Vec::new();
    };
    let mut servers = Vec::new();
    let mut seen = HashSet::new();
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        if parts.next() != Some("nameserver") {
            continue;
        }
        let Some(server) = parts.next() else {
            continue;
        };
        if seen.insert(server.to_string()) {
            servers.push(server.to_string());
        }
    }
    servers
}

fn collect_process_info(sys: &System) -> Vec<ProcessInfo> {
    let mut processes = sys
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            let cpu = process.cpu_usage();
            let memory = process.memory();
            if cpu <= 0.0 && memory == 0 {
                return None;
            }
            Some(ProcessInfo {
                pid: pid.as_u32(),
                name: process.name().to_string_lossy().to_string(),
                cpu,
                memory,
                virtual_memory: process.virtual_memory(),
            })
        })
        .collect::<Vec<_>>();

    processes.sort_by(|a, b| {
        b.cpu
            .partial_cmp(&a.cpu)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.memory.cmp(&a.memory))
            .then_with(|| a.name.cmp(&b.name))
    });
    processes.truncate(100);
    processes
}

fn collect_inventory(
    sys: &mut System,
    disks: &mut Disks,
    networks: &mut Networks,
) -> InventorySnapshot {
    sys.refresh_cpu_all();
    sys.refresh_memory();
    disks.refresh_list();
    disks.refresh();
    networks.refresh_list();
    networks.refresh();

    let cpu = sys.cpus().first();
    let cpu_info = CpuInfo {
        brand: cpu.map(|cpu| cpu.brand().to_string()).unwrap_or_default(),
        cores: sys.cpus().len() as u32,
        frequency_mhz: cpu.map(|cpu| cpu.frequency()).unwrap_or(0),
    };

    let memory = MemoryInfo {
        total_bytes: sys.total_memory(),
        available_bytes: sys.available_memory(),
    };

    let disks = collect_disk_info(disks);
    let networks = collect_network_info(networks);

    let system = SystemInfo {
        hostname: hostname::get()
            .ok()
            .and_then(|name| name.into_string().ok())
            .unwrap_or_else(|| "unknown".to_string()),
        os_name: System::name().unwrap_or_default(),
        os_version: System::long_os_version().unwrap_or_default(),
        kernel_version: System::kernel_version().unwrap_or_default(),
        distro: linux_distro_name(),
        architecture: std::env::consts::ARCH.to_string(),
        uptime_seconds: System::uptime(),
        boot_time: System::boot_time(),
        ip_addresses: local_addrs().into_iter().map(|addr| addr.ip).collect(),
        last_seen: chrono::Utc::now().to_rfc3339(),
    };

    InventorySnapshot {
        system,
        cpu: cpu_info,
        memory,
        disks,
        networks,
        logged_in_users: collect_logged_in_users(),
    }
}

fn collect_device_details() -> Value {
    let mut sys = System::new_all();
    sys.refresh_all();
    std::thread::sleep(Duration::from_millis(250));
    sys.refresh_processes(ProcessesToUpdate::All);
    sys.refresh_cpu_all();

    let mut disks = Disks::new_with_refreshed_list();
    disks.refresh();

    let mut networks = Networks::new_with_refreshed_list();
    networks.refresh();

    let cpu = sys.cpus().first();
    let disks = collect_disk_info(&disks);
    let networks = collect_network_info(&networks);
    let processes = collect_process_info(&sys);

    json!({
        "system": {
            "hostname": hostname::get().ok().and_then(|value| value.into_string().ok()).unwrap_or_default(),
            "name": System::name().unwrap_or_default(),
            "kernelVersion": System::kernel_version().unwrap_or_default(),
            "osVersion": System::long_os_version().unwrap_or_default(),
            "distro": linux_distro_name(),
            "architecture": std::env::consts::ARCH,
            "uptimeSeconds": System::uptime(),
            "bootTime": System::boot_time(),
            "ipAddresses": local_addrs().into_iter().map(|addr| addr.ip).collect::<Vec<_>>(),
            "lastSeen": chrono::Utc::now().to_rfc3339()
        },
        "cpu": {
            "brand": cpu.map(|value| value.brand().to_string()).unwrap_or_default(),
            "cores": sys.cpus().len(),
            "frequencyMHz": cpu.map(|value| value.frequency()).unwrap_or(0)
        },
        "memory": {
            "totalBytes": sys.total_memory(),
            "availableBytes": sys.available_memory()
        },
        "disks": disks.iter().map(|disk| {
            json!({
                "name": &disk.name,
                "mountPoint": &disk.mount_point,
                "totalBytes": disk.total_bytes,
                "availableBytes": disk.available_bytes,
                "fileSystem": &disk.file_system
            })
        }).collect::<Vec<_>>(),
        "networks": networks.iter().map(|network| {
            json!({
                "name": &network.name,
                "ips": &network.ips,
                "gateways": &network.gateways,
                "dnsServers": &network.dns_servers,
                "receivedBytes": network.received_bytes,
                "transmittedBytes": network.transmitted_bytes
            })
        }).collect::<Vec<_>>(),
        "loggedInUsers": collect_logged_in_users(),
        "processes": processes
    })
}

struct CommandOutput {
    stdout: String,
    exit_code: Option<i32>,
}

#[cfg(target_os = "windows")]
async fn execute_powershell_command(command: &str) -> CommandOutput {
    use tokio::process::Command;

    let result = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", command])
        .output()
        .await;

    match result {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let combined = if stderr.trim().is_empty() {
                stdout
            } else if stdout.trim().is_empty() {
                format!("Errors:\n{stderr}")
            } else {
                format!("{stdout}\n\nErrors:\n{stderr}")
            };
            CommandOutput {
                stdout: combined,
                exit_code: output.status.code(),
            }
        }
        Err(err) => CommandOutput {
            stdout: format!("Failed to execute command: {err}"),
            exit_code: Some(-1),
        },
    }
}

#[cfg(not(target_os = "windows"))]
async fn execute_powershell_command(command: &str) -> CommandOutput {
    use tokio::process::Command;

    let shell = env::var("SHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(default_unix_command_shell);
    let timeout_secs = env::var("RMM_COMMAND_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(120);

    let output = timeout(
        Duration::from_secs(timeout_secs),
        Command::new(shell)
            .arg("-lc")
            .arg(command)
            .env("PATH", default_unix_command_path())
            .kill_on_drop(true)
            .output(),
    )
    .await;

    match output {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let combined = if stderr.trim().is_empty() {
                stdout
            } else if stdout.trim().is_empty() {
                format!("Errors:\n{stderr}")
            } else {
                format!("{stdout}\n\nErrors:\n{stderr}")
            };
            CommandOutput {
                stdout: combined,
                exit_code: output.status.code(),
            }
        }
        Ok(Err(err)) => CommandOutput {
            stdout: format!("Failed to execute shell command: {err}"),
            exit_code: Some(-1),
        },
        Err(_) => CommandOutput {
            stdout: format!("Command timed out after {timeout_secs}s"),
            exit_code: Some(-1),
        },
    }
}

#[cfg(target_os = "macos")]
fn default_unix_command_shell() -> String {
    if Path::new("/bin/zsh").exists() {
        "/bin/zsh".to_string()
    } else {
        "/bin/sh".to_string()
    }
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn default_unix_command_shell() -> String {
    "/bin/sh".to_string()
}

#[cfg(target_os = "macos")]
fn default_unix_command_path() -> &'static str {
    "/opt/homebrew/sbin:/opt/homebrew/bin:/usr/local/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn default_unix_command_path() -> &'static str {
    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
}

#[cfg(target_os = "windows")]
fn is_elevated() -> bool {
    use std::ptr;
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::processthreadsapi::{GetCurrentProcess, OpenProcessToken};
    use winapi::um::securitybaseapi::GetTokenInformation;
    use winapi::um::winnt::{TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};

    unsafe {
        let mut handle = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) == 0 {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut size = 0;
        let result = GetTokenInformation(
            handle,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );
        CloseHandle(handle);
        result != 0 && elevation.TokenIsElevated != 0
    }
}

#[cfg(target_family = "unix")]
fn is_elevated() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(any(target_os = "windows", target_family = "unix")))]
fn is_elevated() -> bool {
    false
}

fn load_config() -> Result<Config> {
    let server_url =
        env::var("RMM_SERVER_URL").unwrap_or_else(|_| "ws://127.0.0.1:17110/agent/ws".to_string());
    let server_url = server_url.trim().to_string();
    ensure!(
        !server_url.is_empty(),
        "RMM_SERVER_URL must be set and non-empty"
    );

    let agent_token =
        env::var("RMM_AGENT_TOKEN").context("RMM_AGENT_TOKEN must be set and non-empty")?;
    let agent_token = agent_token.trim().to_string();
    ensure!(
        !agent_token.is_empty(),
        "RMM_AGENT_TOKEN must be set and non-empty"
    );

    let inventory_interval_secs = env::var("RMM_INVENTORY_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30);

    let reconnect_max_secs = env::var("RMM_RECONNECT_MAX_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30);

    let ws_connect_timeout_secs = env::var("RMM_WS_CONNECT_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10);

    let agent_id_path = env::var("RMM_AGENT_ID_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_agent_id_path());

    Ok(Config {
        server_url,
        agent_token,
        inventory_interval_secs,
        agent_id_path,
        reconnect_max_secs,
        ws_connect_timeout_secs,
    })
}

fn default_agent_id_path() -> PathBuf {
    if cfg!(target_os = "windows") {
        if let Ok(base) = env::var("PROGRAMDATA") {
            return PathBuf::from(base)
                .join("Talos")
                .join("talos_worker_id.txt");
        }
        if let Ok(profile) = env::var("USERPROFILE") {
            return PathBuf::from(profile)
                .join("AppData")
                .join("Local")
                .join("Talos")
                .join("talos_worker_id.txt");
        }
    }

    if cfg!(target_os = "macos") {
        return PathBuf::from("/Library/Application Support/Talos/talos_worker_id.txt");
    }

    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home)
            .join(".talos")
            .join("talos_worker_id.txt");
    }

    env::temp_dir().join("talos_talos_worker_id.txt")
}

fn load_or_create_agent_id(path: &Path) -> Result<String> {
    if let Ok(existing) = fs::read_to_string(path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    let new_id = uuid::Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("create agent id directory")?;
    }
    fs::write(path, &new_id).context("write agent id")?;
    Ok(new_id)
}

fn current_ip() -> String {
    local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

pub(crate) fn local_addrs() -> Vec<LocalAddr> {
    let mut addrs = Vec::new();
    if let Ok(interfaces) = get_if_addrs() {
        for iface in interfaces {
            match iface.addr {
                IfAddr::V4(v4) => {
                    if v4.is_loopback() || v4.ip.is_link_local() {
                        continue;
                    }
                    let prefix = netmask_to_prefix(v4.netmask);
                    addrs.push(LocalAddr {
                        ip: v4.ip.to_string(),
                        prefix,
                    });
                }
                IfAddr::V6(_) => {}
            }
        }
    }
    addrs
}

fn netmask_to_prefix(mask: Ipv4Addr) -> u8 {
    mask.octets()
        .iter()
        .map(|octet| octet.count_ones())
        .sum::<u32>() as u8
}

fn ipv6_netmask_to_prefix(mask: Ipv6Addr) -> u8 {
    mask.octets()
        .iter()
        .map(|octet| octet.count_ones())
        .sum::<u32>() as u8
}

fn is_ipv6_link_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

fn network_id(ip: Ipv4Addr, prefix: u8) -> u32 {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    u32::from(ip) & mask
}

pub(crate) fn is_lan_connection(remote: std::net::SocketAddr, local_addrs: &[LocalAddr]) -> bool {
    let std::net::SocketAddr::V4(remote_v4) = remote else {
        return false;
    };
    let remote_ip = *remote_v4.ip();
    for addr in local_addrs {
        if let Ok(local_ip) = addr.ip.parse::<Ipv4Addr>() {
            if network_id(remote_ip, addr.prefix) == network_id(local_ip, addr.prefix) {
                return true;
            }
        }
    }
    false
}

struct Backoff {
    initial: Duration,
    max: Duration,
    current: Duration,
}

impl Backoff {
    fn new(max_secs: u64) -> Self {
        let initial = Duration::from_secs(2);
        let max = Duration::from_secs(max_secs.max(2));
        Self {
            initial,
            max,
            current: initial,
        }
    }

    fn reset(&mut self) {
        self.current = self.initial;
    }

    fn next_delay(&mut self) -> Duration {
        let delay = self.current;
        self.current = (self.current * 2).min(self.max);
        delay
    }
}

#[cfg(all(test, target_os = "windows"))]
mod registry_plumbing_tests {
    use super::*;

    fn extract_rmmd_meta(payload: &[u8]) -> Option<&[u8]> {
        if payload.len() < 8 || payload.get(0..4) != Some(b"RMMD") {
            return None;
        }
        let len = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
        if payload.len() < 8 + len {
            return None;
        }
        Some(&payload[8..8 + len])
    }

    #[tokio::test]
    async fn registry_request_emits_rmmd_response() {
        // Use a deliberately non-existent path so the test is deterministic and does not mutate state.
        let request = RegistryRequest::ListKeys {
            request_id: "reg-test-1".to_string(),
            session_id: "reg-session-test".to_string(),
            hive: talos_protocol::RegistryHive::HKCU,
            path: "___RMM_CODEX__NON_EXISTENT_TEST_KEY___".to_string(),
            offset: 0,
            limit: 128,
        };
        let payload = serde_json::to_vec(&request).expect("serialize registry request");
        let rmmd = execute_registry_request_payload("reg-session-test", &payload)
            .await
            .expect("build registry response frame");
        let json_bytes = extract_rmmd_meta(&rmmd)
            .expect("registry response wrapped as RMMD")
            .to_vec();

        let envelope: RegistryResponseEnvelope =
            serde_json::from_slice(&json_bytes).expect("deserialize registry response envelope");
        assert_eq!(envelope.message_type, REGISTRY_META_MESSAGE_TYPE);
        assert_eq!(envelope.request_id, "reg-test-1");

        // The specific winreg error code can vary slightly by OS/build; we only validate plumbing.
        match envelope.response {
            talos_protocol::RegistryResponse::ListKeys { .. } => {}
            talos_protocol::RegistryResponse::Error { .. } => {}
            other => panic!("unexpected registry response: {other:?}"),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_remote_desktop_profile_tests {
    use super::*;

    #[test]
    fn macos_helper_control_backpressure_keeps_discrete_inputs_reliable() {
        assert!(is_lossy_helper_control(
            talos_protocol::CONTROL_TYPE_MOUSE_MOVE
        ));
        assert!(is_lossy_helper_control(
            talos_protocol::CONTROL_TYPE_MOUSE_WHEEL
        ));
        assert!(!is_lossy_helper_control(
            talos_protocol::CONTROL_TYPE_MOUSE_BUTTON
        ));
        assert!(!is_lossy_helper_control(
            talos_protocol::CONTROL_TYPE_MOUSE_DOUBLE_CLICK
        ));
        assert!(!is_lossy_helper_control(
            talos_protocol::CONTROL_TYPE_KEY_DOWN
        ));
        assert!(!is_lossy_helper_control(
            talos_protocol::CONTROL_TYPE_KEY_UP
        ));
        assert!(!is_lossy_helper_control(
            talos_protocol::CONTROL_TYPE_CLIPBOARD
        ));
    }

    #[test]
    fn macos_profiles_map_to_capture_modes() {
        assert_eq!(
            macos_capture_mode_for_profile(Some(REMOTE_DESKTOP_PROFILE_LEGACY)),
            MacosDesktopCaptureMode::Legacy
        );
        assert_eq!(
            macos_capture_mode_for_profile(Some(REMOTE_DESKTOP_PROFILE_MODERN_CPU)),
            MacosDesktopCaptureMode::Legacy
        );
        assert_eq!(
            macos_capture_mode_for_profile(Some(REMOTE_DESKTOP_PROFILE_MODERN_GPU)),
            MacosDesktopCaptureMode::H264
        );
        assert_eq!(
            macos_capture_mode_for_profile(Some(REMOTE_DESKTOP_PROFILE_EXPERIMENTAL)),
            MacosDesktopCaptureMode::Atx2
        );
        assert_eq!(
            macos_capture_mode_for_profile(Some(REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY)),
            MacosDesktopCaptureMode::Screenshot
        );
        assert_eq!(
            macos_capture_mode_for_profile(None),
            MacosDesktopCaptureMode::H264
        );
        assert_eq!(
            macos_capture_mode_for_profile(Some("unknown")),
            MacosDesktopCaptureMode::Legacy
        );
    }

    #[test]
    fn macos_capabilities_advertise_all_supported_display_profiles() {
        let capabilities = PipelineRunner::new().get_capabilities();
        let profile_ids = capabilities
            .display_profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect::<Vec<_>>();

        assert!(profile_ids.contains(&REMOTE_DESKTOP_PROFILE_MODERN_GPU));
        assert!(profile_ids.contains(&REMOTE_DESKTOP_PROFILE_EXPERIMENTAL));
        assert!(profile_ids.contains(&REMOTE_DESKTOP_PROFILE_MODERN_CPU));
        assert!(profile_ids.contains(&REMOTE_DESKTOP_PROFILE_LEGACY));
        assert!(profile_ids.contains(&REMOTE_DESKTOP_PROFILE_SCREENSHOT_ONLY));
        assert_eq!(
            capabilities.selected_display_profile.as_deref(),
            Some(REMOTE_DESKTOP_PROFILE_MODERN_GPU)
        );
        assert!(capabilities.features.chat);
    }
}
