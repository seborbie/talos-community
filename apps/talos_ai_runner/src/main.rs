//! Talos AI Runner: a bounded internal executor for Command Center desktop goals.

use std::{
    collections::HashMap,
    env,
    ffi::CStr,
    net::SocketAddr,
    panic::AssertUnwindSafe,
    ptr, slice,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use base64::engine::general_purpose::{
    STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD,
};
use base64::Engine as _;
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use talos_protocol::relay_transport::{
    build_e2e_cipher, build_relay_client_tls_config, parse_relay_target, read_e2e_frame_from,
    read_http_response, write_e2e_frame,
};
use talos_protocol::{
    build_chat_frame, build_control_frame, build_shell_frame, decode_display_record,
    parse_chat_frame, parse_shell_exit_payload, ChatAckPayload,
    ChatSessionCapabilitiesHttpResponse, ChatWireErrorPayload, ChatWirePayload, DisplayRecord,
    OperationErrorCode, WorkerChatControlPayload, CHAT_MSG_ACK, CHAT_MSG_AUTH, CHAT_MSG_CONTROL,
    CHAT_MSG_ERROR, CHAT_MSG_TEXT, CONTROL_MOD_ALT, CONTROL_MOD_CTRL, CONTROL_MOD_SHIFT,
    CONTROL_MOD_WIN, CONTROL_PAYLOAD_KEY_LEN, CONTROL_PAYLOAD_MOUSE_BUTTON_LEN,
    CONTROL_PAYLOAD_MOUSE_DOUBLE_CLICK_LEN, CONTROL_PAYLOAD_MOUSE_MOVE_LEN,
    CONTROL_PAYLOAD_MOUSE_WHEEL_LEN, CONTROL_TYPE_KEY_DOWN, CONTROL_TYPE_KEY_UP,
    CONTROL_TYPE_MOUSE_BUTTON, CONTROL_TYPE_MOUSE_DOUBLE_CLICK, CONTROL_TYPE_MOUSE_MOVE,
    CONTROL_TYPE_MOUSE_WHEEL, CONTROL_TYPE_STOP_CAPTURE, CONTROL_TYPE_TYPED_INPUT,
    HEARTBEAT_PAYLOAD, REMOTE_DESKTOP_PROFILE_LEGACY, REMOTE_DESKTOP_PROFILE_MODERN_CPU,
    REMOTE_DESKTOP_PROTOCOL_LEGACY_IVF, SHELL_MSG_AUTH, SHELL_MSG_ERROR, SHELL_MSG_EXIT,
    SHELL_MSG_INPUT, SHELL_MSG_OUTPUT,
};
use tokio::{
    io::{split, AsyncRead, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    sync::{mpsc, Mutex, Notify},
    time::{interval, timeout, Duration, MissedTickBehavior},
};
use tokio_rustls::{
    rustls::{self, pki_types::ServerName},
    TlsConnector,
};
use tracing::{debug, error, info, trace, warn};
use vpx_sys::*;

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:3010";
const DEFAULT_RUNNER_ID: &str = "talos-ai-runner-local";
const DEFAULT_MAX_CONCURRENT_JOBS: usize = 2;
const DEFAULT_JOB_TIMEOUT_SECS: u64 = 420;
const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 30;
const DEFAULT_SCREENSHOT_READ_TIMEOUT_SECS: u64 = 60;
const DEFAULT_APPROVAL_TIMEOUT_SECS: u64 = 300;
const NO_INTERACTIVE_USER_APPROVAL_REASON: &str = "no_interactive_user";
const NO_INTERACTIVE_USER_APPROVAL_MESSAGE: &str = "Endpoint approval could not be requested because no user is currently logged in on this device. Ask someone to sign in, then retry.";
const DEFAULT_LEASE_HEARTBEAT_SECS: u64 = 15;
const DEFAULT_SHELL_COMMAND_MAX_WAIT_SECS: u64 = 60;
const DEFAULT_SHELL_COMMAND_CHECKPOINT_MS: u64 = 10_000;
const JOB_TYPE_DESKTOP_GOAL: &str = "desktop_goal";
const JOB_TYPE_SHELL_GOAL: &str = "shell_goal";
const JOB_TYPE_DEBUG_SCREENSHOT: &str = "debug_screenshot";
const MAX_RELAY_SCREENSHOT_PAYLOADS: usize = 24;
const MAX_SCREENSHOT_ARTIFACT_BASE64_CHARS: usize = 7_900_000;
const RELAY_STOP_CAPTURE_GRACE_MS: u64 = 150;
const LIVE_RELAY_FRAME_TIMEOUT_SECS: u64 = 60;
const LIVE_RELAY_UNCHANGED_FRAME_WAIT_SECS: u64 = 10;
const LIVE_RELAY_CONNECT_TIMEOUT_SECS: u64 = 15;
const LIVE_RELAY_HEARTBEAT_SECS: u64 = 15;
const MAX_LEGACY_VP8_PAYLOAD_LEN: usize = 128 * 1024 * 1024;
const AI_ASSIST_DEFAULT_SETTLE_MS: u64 = 500;
const AI_ASSIST_MAX_SETTLE_MS: u64 = 30_000;
const SHELL_COMMAND_APPROVAL_POLL_MS: u64 = 1_000;
const SHELL_COMMAND_OUTPUT_FLUSH_MS: u64 = 250;
const SHELL_COMMAND_OUTPUT_CHUNK_CHARS: usize = 4_000;
const SHELL_COMMAND_WAIT_MIN_MS: u64 = 1_000;
const SHELL_COMMAND_WAIT_MAX_MS: u64 = 60_000;
const SHELL_COMMAND_INTERRUPT_GRACE_MS: u64 = 5_000;
const SHELL_GOAL_MAX_TURNS: u32 = 20;
const SHELL_TRANSCRIPT_MAX_CHARS: usize = 24_000;
const SHELL_TRANSCRIPT_ARTIFACT_TYPE: &str = "runner-shell-transcript";
const SHELL_TRANSCRIPT_CHUNK_CHARS: usize = 1_000_000;

type SessionCleanupSlot = Arc<Mutex<Option<SessionCleanup>>>;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    jobs: Arc<Mutex<HashMap<String, JobRecord>>>,
    active_jobs: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    session_cleanups: Arc<Mutex<HashMap<String, SessionCleanupSlot>>>,
    client: reqwest::Client,
}

#[derive(Debug, Clone)]
struct Config {
    bind_addr: String,
    runner_id: String,
    service_key: String,
    rmm_server_key: String,
    rmm_server_url: String,
    api_callback_base_url: String,
    max_concurrent_jobs: usize,
    job_timeout_secs: u64,
    screenshot_read_timeout_secs: u64,
    approval_timeout_secs: u64,
    lease_heartbeat_secs: u64,
    shell_command_max_wait_secs: u64,
    shell_command_checkpoint_ms: u64,
    relay_ca_path: Option<String>,
    relay_verify_hostname: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum JobStatus {
    Running,
    ApprovalDenied,
    ApprovalExpired,
    Succeeded,
    Failed,
    Stopping,
    Stopped,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobRecord {
    job_id: String,
    status: JobStatus,
    runner_id: String,
    lease_id: Option<String>,
    callback_base: String,
    agent_id: String,
    organization_id: String,
    message: Option<String>,
    error: Option<String>,
    result: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartJobRequest {
    job_id: String,
    organization_id: String,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    conversation_id: Option<String>,
    agent_id: String,
    #[serde(default = "default_job_type")]
    job_type: String,
    #[serde(default)]
    goal: Option<String>,
    #[serde(default)]
    device_context: Option<Value>,
    #[serde(default)]
    generated_secrets: Vec<GeneratedSecretSummary>,
    #[serde(default)]
    callback_base_url: Option<String>,
    #[serde(default)]
    approval_mode: Option<String>,
    #[serde(default)]
    approval: Option<ApprovalRequest>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartJobResponse {
    accepted: bool,
    job_id: String,
    runner_id: String,
    lease_id: Option<String>,
    lease_expires_at: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LeaseEnvelope {
    lease: LeaseRecord,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LeaseRecord {
    accepted: bool,
    reason: Option<String>,
    lease_id: Option<String>,
    lease_expires_at: Option<String>,
    cancel_requested_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    ok: bool,
    runner_id: String,
    max_concurrent_jobs: usize,
    active_jobs: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StopJobResponse {
    ok: bool,
    job_id: String,
    stopped: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectResponse {
    url: String,
    session_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalRequest {
    approval_id: String,
    requester_label: String,
    #[serde(default)]
    requester_email: Option<String>,
    #[serde(default)]
    organization_name: Option<String>,
    device_label: String,
    reason: String,
    expires_at_unix_ms: u64,
    approval_window_expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ApprovalDecision {
    Approved,
    Denied,
    Expired,
    Skipped,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionCapabilities {
    relay_url: Option<String>,
    e2e_key: Option<String>,
    selected_display_profile: Option<String>,
    #[serde(default)]
    display_profiles: Vec<SessionDisplayProfile>,
    #[serde(default)]
    platform: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionDisplayProfile {
    id: String,
    protocol: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScreenshotArtifact {
    frame_id: u64,
    width: u32,
    height: u32,
    payload_bytes: usize,
    png_bytes: usize,
    base64_content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RelayScreenshotRead {
    first_payload_bytes: Option<usize>,
    artifact: ScreenshotArtifact,
}

#[derive(Default)]
struct ScreenshotFrameAssembler {
    active_frame_id: Option<u64>,
    active_width: u32,
    active_height: u32,
    pending_artifact: Option<ScreenshotArtifact>,
}

#[derive(Debug, Clone)]
struct LiveFrame {
    seq: u64,
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PointerState {
    visible: bool,
    x: u32,
    y: u32,
}

impl PointerState {
    fn update(&mut self, x: u32, y: u32, width: u32, height: u32) {
        self.visible = true;
        self.x = clamp_pointer_coord(x, width);
        self.y = clamp_pointer_coord(y, height);
    }

    fn apply_action(&mut self, action: &AiDesktopAction, width: u32, height: u32) {
        match action {
            AiDesktopAction::Move { x, y, .. }
            | AiDesktopAction::Click { x, y, .. }
            | AiDesktopAction::DoubleClick { x, y, .. }
            | AiDesktopAction::Scroll { x, y, .. } => self.update(*x, *y, width, height),
            AiDesktopAction::Drag { path, .. } => {
                if let Some(point) = path.last() {
                    self.update(point.x, point.y, width, height);
                }
            }
            AiDesktopAction::Type { .. }
            | AiDesktopAction::InjectSecret { .. }
            | AiDesktopAction::Keypress { .. }
            | AiDesktopAction::Wait { .. } => {}
        }
    }

    fn metadata(self, width: u32, height: u32) -> Value {
        if self.visible {
            json!({
                "visible": true,
                "x": self.x,
                "y": self.y,
                "width": width,
                "height": height,
            })
        } else {
            json!({
                "visible": false,
                "width": width,
                "height": height,
            })
        }
    }
}

fn clamp_pointer_coord(value: u32, dimension: u32) -> u32 {
    if dimension == 0 {
        value
    } else {
        value.min(dimension.saturating_sub(1))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesktopConnectMode {
    Interactive,
    ScreenshotOnly,
}

impl DesktopConnectMode {
    fn desktop_mode(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::ScreenshotOnly => "screenshot_only",
        }
    }

    fn display_profile_preference(self) -> Vec<&'static str> {
        match self {
            Self::Interactive => vec![
                REMOTE_DESKTOP_PROFILE_MODERN_CPU,
                REMOTE_DESKTOP_PROFILE_LEGACY,
            ],
            Self::ScreenshotOnly => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AiDesktopTaskStepResponse {
    task_id: String,
    status: String,
    #[serde(default)]
    plan: Vec<String>,
    #[serde(default)]
    assistant_message: String,
    #[serde(default)]
    actions: Vec<AiDesktopAction>,
    #[serde(default)]
    response_id: Option<String>,
    #[serde(default)]
    step_index: u32,
    #[serde(default)]
    max_steps: u32,
    #[serde(default)]
    generated_secrets: Vec<GeneratedSecretSummary>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AiShellAssistResponse {
    action: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    wait_ms: u64,
    explanation: String,
    risk: String,
    #[serde(default)]
    notes: Vec<String>,
    #[serde(default)]
    message: String,
    #[serde(default)]
    response_id: Option<String>,
    #[serde(default)]
    generated_secrets: Vec<GeneratedSecretSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellAssistAction {
    Command,
    Wait,
    Interrupt,
    Done,
    NeedsInput,
    OpenDesktop,
}

impl ShellAssistAction {
    fn as_str(self) -> &'static str {
        match self {
            ShellAssistAction::Command => "command",
            ShellAssistAction::Wait => "wait",
            ShellAssistAction::Interrupt => "interrupt",
            ShellAssistAction::Done => "done",
            ShellAssistAction::NeedsInput => "needs_input",
            ShellAssistAction::OpenDesktop => "open_desktop",
        }
    }
}

fn parse_shell_assist_action(value: &str) -> Result<ShellAssistAction> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "command" => Ok(ShellAssistAction::Command),
        "wait" => Ok(ShellAssistAction::Wait),
        "interrupt" => Ok(ShellAssistAction::Interrupt),
        "done" => Ok(ShellAssistAction::Done),
        "needs_input" => Ok(ShellAssistAction::NeedsInput),
        "open_desktop" => Ok(ShellAssistAction::OpenDesktop),
        other => Err(anyhow!("unsupported shell assist action: {other}")),
    }
}

fn ensure_shell_action_allowed_without_active_command(action: ShellAssistAction) -> Result<()> {
    match action {
        ShellAssistAction::Interrupt => {
            return Err(anyhow!(
                "shell assist requested interrupt but no command is running"
            ));
        }
        ShellAssistAction::Wait => {
            return Err(anyhow!(
                "shell assist requested wait but no command is running"
            ));
        }
        _ => {}
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeneratedSecretSummary {
    secret_handle: String,
    #[serde(default)]
    shell_reference: Option<String>,
    #[serde(default)]
    desktop_reference: Option<String>,
    secure_note_url: String,
    expires_at: String,
    #[serde(default)]
    purpose: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedGeneratedSecret {
    secret: String,
    #[serde(default)]
    shell_reference: Option<String>,
    #[serde(default)]
    desktop_reference: Option<String>,
    secure_note_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AiShellAssistHistoryEntry {
    command: String,
    approved: bool,
    output: Option<String>,
    response_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AiShellAssistActiveCommand {
    command: String,
    approval_id: String,
    turn_index: u32,
    elapsed_ms: u64,
    checkpoint_count: u32,
    recent_output: String,
    remaining_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandApprovalEnvelope {
    approval: CommandApprovalResponse,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandApprovalResponse {
    id: String,
    status: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum AiDesktopAction {
    Move {
        x: u32,
        y: u32,
        #[serde(default)]
        keys: Vec<String>,
    },
    Click {
        x: u32,
        y: u32,
        #[serde(default = "default_mouse_button")]
        button: String,
        #[serde(default)]
        keys: Vec<String>,
    },
    DoubleClick {
        x: u32,
        y: u32,
        #[serde(default = "default_mouse_button")]
        button: String,
        #[serde(default)]
        keys: Vec<String>,
    },
    Drag {
        #[serde(default = "default_mouse_button")]
        button: String,
        #[serde(default)]
        path: Vec<AiDesktopPoint>,
        #[serde(default)]
        keys: Vec<String>,
    },
    Scroll {
        x: u32,
        y: u32,
        #[serde(default)]
        scroll_x: i32,
        #[serde(default)]
        scroll_y: i32,
        #[serde(default)]
        keys: Vec<String>,
    },
    Type {
        text: String,
    },
    InjectSecret {
        secret_handle: String,
    },
    Keypress {
        #[serde(default)]
        keys: Vec<String>,
    },
    Wait {
        ms: u64,
    },
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct AiDesktopPoint {
    x: u32,
    y: u32,
}

fn default_mouse_button() -> String {
    "left".to_string()
}

fn default_job_type() -> String {
    JOB_TYPE_DESKTOP_GOAL.to_string()
}

fn parse_usize_env(name: &str, fallback: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn parse_u64_env(name: &str, fallback: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn clamp_shell_command_wait_ms(value: u64) -> u64 {
    value.clamp(SHELL_COMMAND_WAIT_MIN_MS, SHELL_COMMAND_WAIT_MAX_MS)
}

fn duration_ms_u64(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn parse_u32_env(name: &str, fallback: u32) -> u32 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn required_env(name: &str, fallback: Option<&str>) -> Result<String> {
    env::var(name)
        .ok()
        .or_else(|| fallback.map(ToOwned::to_owned))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("{name} is required"))
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn load_config() -> Result<Config> {
    let bind_addr =
        env::var("TALOS_AI_RUNNER_BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_string());
    let runner_id =
        optional_env("TALOS_AI_RUNNER_ID").unwrap_or_else(|| DEFAULT_RUNNER_ID.to_string());
    let service_key = required_env(
        "TALOS_AI_RUNNER_SERVICE_KEY",
        env::var("SERVICE_KEY").ok().as_deref(),
    )?;
    let rmm_server_key = required_env(
        "RMM_SERVER_API_KEY",
        env::var("TALOS_AI_RUNNER_RMM_SERVER_KEY").ok().as_deref(),
    )?;
    let rmm_server_url = optional_env("RMM_SERVER_HTTP_URL")
        .or_else(|| optional_env("PUBLIC_RMM_API_URL"))
        .or_else(|| optional_env("RMM_API_URL"))
        .unwrap_or_else(|| "http://localhost:3002".to_string());
    let api_callback_base_url = optional_env("TALOS_AI_RUNNER_CALLBACK_BASE_URL")
        .or_else(|| optional_env("API_BACKEND_URL"))
        .or_else(|| optional_env("PUBLIC_API_URL"))
        .unwrap_or_else(|| "http://localhost:3001".to_string());

    Ok(Config {
        bind_addr,
        runner_id,
        service_key,
        rmm_server_key,
        rmm_server_url: trim_trailing_slash(&rmm_server_url),
        api_callback_base_url: trim_trailing_slash(&api_callback_base_url),
        max_concurrent_jobs: parse_usize_env(
            "TALOS_AI_RUNNER_MAX_CONCURRENT_JOBS",
            DEFAULT_MAX_CONCURRENT_JOBS,
        ),
        job_timeout_secs: parse_u64_env(
            "TALOS_AI_RUNNER_JOB_TIMEOUT_SECS",
            DEFAULT_JOB_TIMEOUT_SECS,
        ),
        screenshot_read_timeout_secs: parse_u64_env(
            "TALOS_AI_RUNNER_SCREENSHOT_READ_TIMEOUT_SECS",
            DEFAULT_SCREENSHOT_READ_TIMEOUT_SECS,
        ),
        approval_timeout_secs: parse_u64_env(
            "TALOS_AI_RUNNER_APPROVAL_TIMEOUT_SECS",
            DEFAULT_APPROVAL_TIMEOUT_SECS,
        ),
        lease_heartbeat_secs: parse_u64_env(
            "TALOS_AI_RUNNER_LEASE_HEARTBEAT_SECS",
            DEFAULT_LEASE_HEARTBEAT_SECS,
        ),
        shell_command_max_wait_secs: parse_u64_env(
            "TALOS_AI_RUNNER_COMMAND_MAX_WAIT_SECS",
            DEFAULT_SHELL_COMMAND_MAX_WAIT_SECS,
        ),
        shell_command_checkpoint_ms: clamp_shell_command_wait_ms(parse_u64_env(
            "TALOS_AI_RUNNER_COMMAND_CHECKPOINT_MS",
            DEFAULT_SHELL_COMMAND_CHECKPOINT_MS,
        )),
        relay_ca_path: optional_env("TALOS_AI_RUNNER_RELAY_CA_PATH"),
        relay_verify_hostname: optional_env("TALOS_AI_RUNNER_RELAY_VERIFY_HOSTNAME"),
    })
}

fn trim_trailing_slash(value: &str) -> String {
    value.trim().trim_end_matches('/').to_string()
}

fn presented_service_key(headers: &HeaderMap) -> &str {
    headers
        .get("x-service-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .unwrap_or("")
}

fn require_service_key(headers: &HeaderMap, expected: &str) -> Result<(), (StatusCode, String)> {
    let presented = presented_service_key(headers);
    if presented.is_empty() {
        warn!("AI runner request rejected: missing service key");
        return Err((
            StatusCode::UNAUTHORIZED,
            "missing x-service-key".to_string(),
        ));
    }
    if presented != expected {
        warn!("AI runner request rejected: invalid service key");
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid x-service-key".to_string(),
        ));
    }
    Ok(())
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let active_jobs = state.active_jobs.lock().await.len();
    debug!(
        runner_id = %state.config.runner_id,
        active_jobs,
        max_concurrent_jobs = state.config.max_concurrent_jobs,
        "AI runner health requested"
    );
    Json(HealthResponse {
        ok: true,
        runner_id: state.config.runner_id.clone(),
        max_concurrent_jobs: state.config.max_concurrent_jobs,
        active_jobs,
    })
}

async fn start_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<StartJobRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_service_key(&headers, &state.config.service_key)?;
    validate_start_job(&body)?;

    let job_id = body.job_id.trim().to_string();
    let agent_id = body.agent_id.trim().to_string();
    let organization_id = body.organization_id.trim().to_string();
    debug!(
        job_id = %job_id,
        agent_id = %agent_id,
        organization_id = %organization_id,
        job_type = %body.job_type,
        "AI runner start request received"
    );
    let active_jobs = state.active_jobs.lock().await;
    if active_jobs.contains_key(&job_id) {
        info!(
            job_id = %job_id,
            agent_id = %agent_id,
            organization_id = %organization_id,
            "AI runner job already running"
        );
        return Ok((
            StatusCode::ACCEPTED,
            Json(StartJobResponse {
                accepted: true,
                job_id,
                runner_id: state.config.runner_id.clone(),
                lease_id: None,
                lease_expires_at: None,
                reason: Some("already_running".to_string()),
            }),
        ));
    }
    if active_jobs.len() >= state.config.max_concurrent_jobs {
        warn!(
            job_id = %job_id,
            agent_id = %agent_id,
            organization_id = %organization_id,
            active_jobs = active_jobs.len(),
            max_concurrent_jobs = state.config.max_concurrent_jobs,
            "AI runner capacity full"
        );
        return Ok((
            StatusCode::TOO_MANY_REQUESTS,
            Json(StartJobResponse {
                accepted: false,
                job_id,
                runner_id: state.config.runner_id.clone(),
                lease_id: None,
                lease_expires_at: None,
                reason: Some("capacity_full".to_string()),
            }),
        ));
    }
    drop(active_jobs);

    let lease = acquire_job_lease(&state, &body).await.map_err(|error| {
        warn!(
            job_id = %job_id,
            error = %error,
            "AI runner lease acquisition failed"
        );
        (
            StatusCode::BAD_GATEWAY,
            format!("lease acquisition failed: {error}"),
        )
    })?;
    let Some(lease_id) = lease
        .lease_id
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        let reason = lease
            .reason
            .clone()
            .unwrap_or_else(|| "lease_not_acquired".to_string());
        warn!(
            job_id = %job_id,
            reason = %reason,
            "AI runner start rejected because backend lease was not acquired"
        );
        return Ok((
            StatusCode::CONFLICT,
            Json(StartJobResponse {
                accepted: false,
                job_id,
                runner_id: state.config.runner_id.clone(),
                lease_id: None,
                lease_expires_at: lease.lease_expires_at,
                reason: Some(reason),
            }),
        ));
    };
    let callback_base = request_callback_base(&state, &body);

    let cancel = Arc::new(AtomicBool::new(false));
    let mut active_jobs = state.active_jobs.lock().await;
    if active_jobs.contains_key(&job_id) || active_jobs.len() >= state.config.max_concurrent_jobs {
        drop(active_jobs);
        release_job_lease(&state, &callback_base, &body.job_id, &lease_id).await;
        return Ok((
            StatusCode::TOO_MANY_REQUESTS,
            Json(StartJobResponse {
                accepted: false,
                job_id,
                runner_id: state.config.runner_id.clone(),
                lease_id: None,
                lease_expires_at: None,
                reason: Some("capacity_full".to_string()),
            }),
        ));
    }
    active_jobs.insert(job_id.clone(), Arc::clone(&cancel));
    drop(active_jobs);

    let mut jobs = state.jobs.lock().await;
    jobs.insert(
        job_id.clone(),
        JobRecord {
            job_id: job_id.clone(),
            status: JobStatus::Running,
            runner_id: state.config.runner_id.clone(),
            lease_id: Some(lease_id.clone()),
            callback_base: callback_base.clone(),
            agent_id: agent_id.clone(),
            organization_id: organization_id.clone(),
            message: Some("Starting runner job".to_string()),
            error: None,
            result: None,
        },
    );
    drop(jobs);

    let task_state = state.clone();
    let task_cancel = Arc::clone(&cancel);
    let task_lease_id = lease_id.clone();
    let task_job_id = job_id.clone();
    let task_callback_base = callback_base.clone();
    tokio::spawn(async move {
        let panic_state = task_state.clone();
        let result = AssertUnwindSafe(run_job_task(task_state, body, task_cancel, task_lease_id))
            .catch_unwind()
            .await;
        if let Err(payload) = result {
            handle_job_panic(&panic_state, &task_job_id, &task_callback_base, payload).await;
        }
    });

    info!(
        job_id = %job_id,
        agent_id = %agent_id,
        organization_id = %organization_id,
        runner_id = %state.config.runner_id,
        "AI runner job queued"
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(StartJobResponse {
            accepted: true,
            job_id,
            runner_id: state.config.runner_id.clone(),
            lease_id: Some(lease_id),
            lease_expires_at: lease.lease_expires_at,
            reason: None,
        }),
    ))
}

fn validate_start_job(body: &StartJobRequest) -> Result<(), (StatusCode, String)> {
    if body.job_id.trim().is_empty() {
        warn!("AI runner start request rejected: missing jobId");
        return Err((StatusCode::BAD_REQUEST, "jobId is required".to_string()));
    }
    if body.organization_id.trim().is_empty() {
        warn!(job_id = %body.job_id, "AI runner start request rejected: missing organizationId");
        return Err((
            StatusCode::BAD_REQUEST,
            "organizationId is required".to_string(),
        ));
    }
    if body.agent_id.trim().is_empty() {
        warn!(job_id = %body.job_id, "AI runner start request rejected: missing agentId");
        return Err((StatusCode::BAD_REQUEST, "agentId is required".to_string()));
    }
    let job_type = body.job_type.trim();
    if job_type != JOB_TYPE_DESKTOP_GOAL
        && job_type != JOB_TYPE_SHELL_GOAL
        && job_type != JOB_TYPE_DEBUG_SCREENSHOT
    {
        warn!(
            job_id = %body.job_id,
            job_type = %body.job_type,
            "AI runner start request rejected: unsupported job type"
        );
        return Err((StatusCode::BAD_REQUEST, "unsupported jobType".to_string()));
    }
    let approval_mode = body.approval_mode.as_deref().unwrap_or("request").trim();
    if approval_mode != "request" && approval_mode != "already_granted" {
        warn!(
            job_id = %body.job_id,
            approval_mode,
            "AI runner start request rejected: unsupported approval mode"
        );
        return Err((
            StatusCode::BAD_REQUEST,
            "unsupported approvalMode".to_string(),
        ));
    }
    if approval_mode == "request" && body.approval.is_none() {
        warn!(job_id = %body.job_id, "AI runner start request rejected: missing approval");
        return Err((StatusCode::BAD_REQUEST, "approval is required".to_string()));
    }
    Ok(())
}

fn request_callback_base(state: &AppState, request: &StartJobRequest) -> String {
    request
        .callback_base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(trim_trailing_slash)
        .unwrap_or_else(|| state.config.api_callback_base_url.clone())
}

async fn acquire_job_lease(state: &AppState, request: &StartJobRequest) -> Result<LeaseRecord> {
    let callback_base = request_callback_base(state, request);
    let url = format!(
        "{}/command-center/internal/ai-runner/jobs/{}/lease",
        callback_base,
        encode_path_segment(&request.job_id)
    );
    let response = state
        .client
        .post(url)
        .header("x-service-key", &state.config.service_key)
        .json(&json!({ "runnerId": state.config.runner_id }))
        .send()
        .await
        .context("acquire AI runner job lease")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let envelope: LeaseEnvelope =
        serde_json::from_str(&body).with_context(|| format!("decode lease response: {body}"))?;
    if !status.is_success() && envelope.lease.accepted {
        return Err(anyhow!(
            "lease endpoint returned {status} for accepted lease"
        ));
    }
    Ok(envelope.lease)
}

async fn heartbeat_job_lease(
    state: &AppState,
    callback_base: &str,
    job_id: &str,
    lease_id: &str,
) -> Result<LeaseRecord> {
    let url = format!(
        "{}/command-center/internal/ai-runner/jobs/{}/lease/{}/heartbeat",
        callback_base,
        encode_path_segment(job_id),
        encode_path_segment(lease_id)
    );
    let response = state
        .client
        .post(url)
        .header("x-service-key", &state.config.service_key)
        .json(&json!({ "runnerId": state.config.runner_id }))
        .send()
        .await
        .context("heartbeat AI runner job lease")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let envelope: LeaseEnvelope = serde_json::from_str(&body)
        .with_context(|| format!("decode heartbeat response: {body}"))?;
    if !status.is_success() && envelope.lease.accepted {
        return Err(anyhow!(
            "heartbeat endpoint returned {status} for accepted lease"
        ));
    }
    Ok(envelope.lease)
}

async fn release_job_lease(state: &AppState, callback_base: &str, job_id: &str, lease_id: &str) {
    let url = format!(
        "{}/command-center/internal/ai-runner/jobs/{}/lease/{}/release",
        callback_base,
        encode_path_segment(job_id),
        encode_path_segment(lease_id)
    );
    if let Err(error) = state
        .client
        .post(url)
        .header("x-service-key", &state.config.service_key)
        .json(&json!({ "runnerId": state.config.runner_id }))
        .send()
        .await
    {
        warn!(job_id = %job_id, lease_id = %lease_id, error = %error, "AI runner lease release failed");
    }
}

async fn job_lease_id(state: &AppState, job_id: &str) -> Option<String> {
    let jobs = state.jobs.lock().await;
    jobs.get(job_id).and_then(|job| job.lease_id.clone())
}

async fn get_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_service_key(&headers, &state.config.service_key)?;
    let jobs = state.jobs.lock().await;
    let Some(job) = jobs.get(&job_id) else {
        warn!(job_id = %job_id, "AI runner job lookup failed");
        return Err((StatusCode::NOT_FOUND, "job not found".to_string()));
    };
    debug!(job_id = %job_id, status = ?job.status, "AI runner job lookup");
    Ok(Json(job.clone()))
}

async fn stop_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    require_service_key(&headers, &state.config.service_key)?;
    let active_jobs = state.active_jobs.lock().await;
    let cancel = active_jobs.get(&job_id).cloned();
    drop(active_jobs);

    let mut jobs = state.jobs.lock().await;
    let stopped = if let (Some(job), Some(cancel)) = (jobs.get_mut(&job_id), cancel) {
        job.status = JobStatus::Stopping;
        cancel.store(true, Ordering::SeqCst);
        true
    } else {
        false
    };
    if !stopped {
        warn!(job_id = %job_id, "AI runner stop request failed: job not found");
        return Err((StatusCode::NOT_FOUND, "job not found".to_string()));
    }
    drop(jobs);

    info!(job_id = %job_id, "AI runner stop requested");
    Ok(Json(StopJobResponse {
        ok: true,
        job_id,
        stopped,
    }))
}

async fn run_job_task(
    state: AppState,
    request: StartJobRequest,
    cancel: Arc<AtomicBool>,
    lease_id: String,
) {
    let job_id = request.job_id.trim().to_string();
    let agent_id = request.agent_id.trim().to_string();
    let organization_id = request.organization_id.trim().to_string();
    let callback_base = request
        .callback_base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(trim_trailing_slash)
        .unwrap_or_else(|| state.config.api_callback_base_url.clone());
    let heartbeat_handle = spawn_job_lease_heartbeat(
        state.clone(),
        callback_base.clone(),
        job_id.clone(),
        lease_id.clone(),
        Arc::clone(&cancel),
    );

    info!(
        job_id = %job_id,
        agent_id = %agent_id,
        organization_id = %organization_id,
        callback_base = %callback_base,
        timeout_secs = state.config.job_timeout_secs,
        "AI runner job started"
    );
    let session_cleanup: SessionCleanupSlot = Arc::new(Mutex::new(None));
    state
        .session_cleanups
        .lock()
        .await
        .insert(job_id.clone(), Arc::clone(&session_cleanup));
    let result = AssertUnwindSafe(timeout(
        Duration::from_secs(state.config.job_timeout_secs),
        run_job_inner(
            &state,
            &request,
            &callback_base,
            &cancel,
            Arc::clone(&session_cleanup),
        ),
    ))
    .catch_unwind()
    .await;
    end_registered_headless_session(&state, &job_id, &callback_base, session_cleanup).await;
    state.session_cleanups.lock().await.remove(&job_id);

    let final_result = match result {
        Ok(Ok(Ok(result))) => {
            info!(job_id = %job_id, "AI runner job succeeded");
            let completion_message = result
                .get("summary")
                .and_then(Value::as_str)
                .or_else(|| result.get("message").and_then(Value::as_str))
                .unwrap_or("Desktop goal completed")
                .to_string();
            let _ = post_status(
                &state,
                &callback_base,
                &job_id,
                "succeeded",
                Some(completion_message),
                Some(result.clone()),
                None,
            )
            .await;
            update_job_record(&state, &job_id, JobStatus::Succeeded, Some(result), None).await;
            Ok(())
        }
        Ok(Ok(Err(error))) => {
            let (status_text, status, message, result) = classify_job_error(&error);
            if matches!(status, JobStatus::Stopped) {
                info!(job_id = %job_id, "AI runner job stopped");
            } else {
                warn!(job_id = %job_id, error = %error, "AI runner job failed");
            }
            let _ = post_status(
                &state,
                &callback_base,
                &job_id,
                status_text,
                Some(message.clone()),
                result.clone(),
                Some(error.to_string()),
            )
            .await;
            update_job_record(&state, &job_id, status, result, Some(error.to_string())).await;
            Err(error)
        }
        Ok(Err(_)) => {
            let error = anyhow!("runner job timed out");
            warn!(
                job_id = %job_id,
                timeout_secs = state.config.job_timeout_secs,
                "AI runner job timed out"
            );
            let _ = post_status(
                &state,
                &callback_base,
                &job_id,
                "failed",
                Some("Runner job timed out".to_string()),
                None,
                Some(error.to_string()),
            )
            .await;
            update_job_record(
                &state,
                &job_id,
                JobStatus::Failed,
                None,
                Some(error.to_string()),
            )
            .await;
            Err(error)
        }
        Err(payload) => {
            handle_job_panic(&state, &job_id, &callback_base, payload).await;
            return;
        }
    };

    if let Err(error) = final_result {
        debug!(job_id = %job_id, error = %error, "AI runner job finished with error");
    }
    heartbeat_handle.abort();
    release_job_lease(&state, &callback_base, &job_id, &lease_id).await;
    state.active_jobs.lock().await.remove(&job_id);
    debug!(job_id = %job_id, "AI runner job removed from active set");
}

fn spawn_job_lease_heartbeat(
    state: AppState,
    callback_base: String,
    job_id: String,
    lease_id: String,
    cancel: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(
            state.config.lease_heartbeat_secs.max(1),
        ));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match heartbeat_job_lease(&state, &callback_base, &job_id, &lease_id).await {
                Ok(lease) if !lease.accepted => {
                    warn!(
                        job_id = %job_id,
                        lease_id = %lease_id,
                        reason = ?lease.reason,
                        "AI runner lease heartbeat lost ownership"
                    );
                    apply_heartbeat_lease(&lease, &cancel);
                    break;
                }
                Ok(lease) => {
                    apply_heartbeat_lease(&lease, &cancel);
                }
                Err(error) => {
                    warn!(
                        job_id = %job_id,
                        lease_id = %lease_id,
                        error = %error,
                        "AI runner lease heartbeat failed"
                    );
                }
            }
        }
    })
}

fn apply_heartbeat_lease(lease: &LeaseRecord, cancel: &AtomicBool) -> bool {
    if !lease.accepted {
        cancel.store(true, Ordering::SeqCst);
        return false;
    }
    if lease.cancel_requested_at.is_some() {
        cancel.store(true, Ordering::SeqCst);
    }
    true
}

#[derive(Debug)]
struct EndpointApprovalUnavailableError {
    reason: &'static str,
    message: &'static str,
}

impl EndpointApprovalUnavailableError {
    fn no_interactive_user() -> Self {
        Self {
            reason: NO_INTERACTIVE_USER_APPROVAL_REASON,
            message: NO_INTERACTIVE_USER_APPROVAL_MESSAGE,
        }
    }

    fn result_payload(&self) -> Value {
        json!({
            "phase": "approval_unavailable",
            "reason": self.reason,
            "message": self.message,
            "summary": self.message,
        })
    }
}

impl std::fmt::Display for EndpointApprovalUnavailableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for EndpointApprovalUnavailableError {}

fn classify_job_error(error: &anyhow::Error) -> (&'static str, JobStatus, String, Option<Value>) {
    if let Some(approval_error) = error.downcast_ref::<EndpointApprovalUnavailableError>() {
        return (
            "failed",
            JobStatus::Failed,
            approval_error.message.to_string(),
            Some(approval_error.result_payload()),
        );
    }
    let message = error.to_string();
    if message.contains("runner job was stopped") || message.contains("stopped") {
        return (
            "stopped",
            JobStatus::Stopped,
            "Runner job stopped".to_string(),
            None,
        );
    }
    if message.contains("endpoint approval denied") {
        return (
            "approval_denied",
            JobStatus::ApprovalDenied,
            "Endpoint approval was denied".to_string(),
            None,
        );
    }
    if message.contains("endpoint approval expired") {
        return (
            "approval_expired",
            JobStatus::ApprovalExpired,
            "Endpoint approval request expired".to_string(),
            None,
        );
    }
    (
        "failed",
        JobStatus::Failed,
        "Runner job failed".to_string(),
        None,
    )
}

async fn handle_job_panic(
    state: &AppState,
    job_id: &str,
    callback_base: &str,
    payload: Box<dyn std::any::Any + Send>,
) {
    let panic_message = panic_payload_to_string(&payload);
    let error = format!("runner job panicked: {panic_message}");
    let _ = post_status(
        state,
        callback_base,
        job_id,
        "failed",
        Some("Runner job failed".to_string()),
        None,
        Some(error.clone()),
    )
    .await;
    update_job_record(state, job_id, JobStatus::Failed, None, Some(error.clone())).await;
    if let Some(lease_id) = job_lease_id(state, job_id).await {
        release_job_lease(state, callback_base, job_id, &lease_id).await;
    }
    state.active_jobs.lock().await.remove(job_id);
    error!(job_id = %job_id, error = %error, "AI runner job panicked");
}

fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic payload".to_string()
}

async fn update_job_record(
    state: &AppState,
    job_id: &str,
    status: JobStatus,
    result: Option<Value>,
    error: Option<String>,
) {
    let mut jobs = state.jobs.lock().await;
    if let Some(job) = jobs.get_mut(job_id) {
        job.status = status;
        job.message = None;
        job.result = result;
        job.error = error;
    }
}

async fn run_job_inner(
    state: &AppState,
    request: &StartJobRequest,
    callback_base: &str,
    cancel: &AtomicBool,
    session_cleanup: SessionCleanupSlot,
) -> Result<Value> {
    ensure_not_cancelled(cancel)?;
    let approval = ensure_ai_runner_approval(state, request, callback_base, cancel).await?;
    match approval {
        ApprovalDecision::Denied => return Err(anyhow!("endpoint approval denied")),
        ApprovalDecision::Expired => return Err(anyhow!("endpoint approval expired")),
        ApprovalDecision::Approved | ApprovalDecision::Skipped => {}
    }

    if request.job_type.trim() == JOB_TYPE_DEBUG_SCREENSHOT {
        return run_screenshot_capture_job(state, request, callback_base, cancel, session_cleanup)
            .await;
    }
    if request.job_type.trim() == JOB_TYPE_SHELL_GOAL {
        return run_shell_goal_job(state, request, callback_base, cancel, session_cleanup).await;
    }

    run_desktop_goal_job(state, request, callback_base, cancel, session_cleanup).await
}

async fn run_screenshot_capture_job(
    state: &AppState,
    request: &StartJobRequest,
    callback_base: &str,
    cancel: &AtomicBool,
    session_cleanup: SessionCleanupSlot,
) -> Result<Value> {
    info!(
        job_id = %request.job_id,
        agent_id = %request.agent_id,
        "AI runner opening screenshot-only desktop"
    );
    let (connect_response, token) =
        connect_desktop_session(state, request, DesktopConnectMode::ScreenshotOnly).await?;
    info!(
        job_id = %request.job_id,
        session_id = %connect_response.session_id,
        "AI runner headless desktop connected"
    );
    let cleanup = SessionCleanup {
        rmm_base: state.config.rmm_server_url.clone(),
        session_id: connect_response.session_id.clone(),
        token: token.clone(),
        kind: SessionCleanupKind::Desktop,
    };
    register_headless_session_cleanup(&session_cleanup, cleanup).await;
    let _ = post_runner_event(
        state,
        callback_base,
        &request.job_id,
        "session_started",
        format!("session_started:desktop:{}", connect_response.session_id),
        json!({
            "sessionId": connect_response.session_id.clone(),
            "kind": "desktop",
            "agentId": request.agent_id.clone(),
        }),
    )
    .await;

    async {
        ensure_not_cancelled(cancel)?;
        post_status(
            state,
            callback_base,
            &request.job_id,
            "running",
            Some("Waiting for screenshot pixels".to_string()),
            None,
            None,
        )
        .await?;

        debug!(
            job_id = %request.job_id,
            session_id = %connect_response.session_id,
            "AI runner fetching session capabilities"
        );
        let capabilities =
            get_session_capabilities(state, &connect_response.session_id, &token).await?;
        debug!(
            job_id = %request.job_id,
            session_id = %connect_response.session_id,
            relay_url_present = capabilities.relay_url.as_ref().is_some_and(|value| !value.trim().is_empty()),
            e2e_key_present = capabilities.e2e_key.as_ref().is_some_and(|value| !value.trim().is_empty()),
            selected_display_profile = ?capabilities.selected_display_profile,
            "AI runner session capabilities loaded"
        );
        request_relay(state, &connect_response.session_id, &token).await?;
        post_viewer_connected(state, &connect_response.session_id, &token).await?;

        ensure_not_cancelled(cancel)?;
        post_status(
            state,
            callback_base,
            &request.job_id,
            "running",
            Some("Capturing debug screenshot".to_string()),
            None,
            None,
        )
        .await?;

        info!(
            job_id = %request.job_id,
            session_id = %connect_response.session_id,
            "AI runner capturing screenshot"
        );
        let screenshot =
            capture_screenshot_from_relay(state, &connect_response.session_id, &capabilities)
                .await?;
        info!(
            job_id = %request.job_id,
            session_id = %connect_response.session_id,
            frame_id = screenshot.artifact.frame_id,
            width = screenshot.artifact.width,
            height = screenshot.artifact.height,
            png_bytes = screenshot.artifact.png_bytes,
            "AI runner screenshot captured"
        );
        let artifact_name = format!(
            "runner-screenshot-frame-{}.png",
            screenshot.artifact.frame_id
        );
        debug!(
            job_id = %request.job_id,
            artifact_name = %artifact_name,
            content_base64_chars = screenshot.artifact.base64_content.len(),
            "AI runner posting screenshot artifact"
        );
        post_artifact(
            state,
            callback_base,
            &request.job_id,
            "runner-screenshot",
            &artifact_name,
            "image/png",
            screenshot.artifact.base64_content.clone(),
            json!({
                "frameId": screenshot.artifact.frame_id,
                "width": screenshot.artifact.width,
                "height": screenshot.artifact.height,
                "payloadBytes": screenshot.artifact.payload_bytes,
                "pngBytes": screenshot.artifact.png_bytes,
                "firstRelayPayloadBytes": screenshot.first_payload_bytes,
                "source": "screenshot_only_relay_stream",
            }),
            false,
            None,
            None,
        )
        .await?;

        Ok(json!({
            "message": "screenshot captured",
            "summary": "Screenshot captured.",
            "sessionId": connect_response.session_id,
            "agentId": request.agent_id,
            "userId": request.user_id,
            "conversationId": request.conversation_id,
            "goal": request.goal,
            "selectedDisplayProfile": capabilities.selected_display_profile,
            "screenshot": {
                "name": artifact_name,
                "width": screenshot.artifact.width,
                "height": screenshot.artifact.height,
                "pngBytes": screenshot.artifact.png_bytes,
            }
        }))
    }
    .await
}

enum ShellGoalOutcome {
    Completed(Value),
    UseDesktopFallback { reason: String },
}

async fn run_shell_goal_job(
    state: &AppState,
    request: &StartJobRequest,
    callback_base: &str,
    cancel: &AtomicBool,
    session_cleanup: SessionCleanupSlot,
) -> Result<Value> {
    let goal = request
        .goal
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Perform the requested shell goal");

    post_status(
        state,
        callback_base,
        &request.job_id,
        "running",
        Some("Opening system shell".to_string()),
        Some(json!({ "phase": "opening_shell" })),
        None,
    )
    .await?;

    let (connect_response, token) = match connect_shell_session(state, request).await {
        Ok(value) => value,
        Err(error) => {
            warn!(
                job_id = %request.job_id,
                agent_id = %request.agent_id,
                error = %error,
                "AI runner shell connection failed; falling back to desktop"
            );
            post_status(
                state,
                callback_base,
                &request.job_id,
                "running",
                Some("System shell unavailable; falling back to desktop".to_string()),
                Some(json!({ "phase": "desktop_fallback", "shellError": error.to_string() })),
                None,
            )
            .await?;
            return run_desktop_goal_job(state, request, callback_base, cancel, session_cleanup)
                .await;
        }
    };

    let cleanup = SessionCleanup {
        rmm_base: state.config.rmm_server_url.clone(),
        session_id: connect_response.session_id.clone(),
        token: token.clone(),
        kind: SessionCleanupKind::Shell,
    };
    register_headless_session_cleanup(&session_cleanup, cleanup).await;
    let _ = post_runner_event(
        state,
        callback_base,
        &request.job_id,
        "session_started",
        format!("session_started:shell:{}", connect_response.session_id),
        json!({
            "sessionId": connect_response.session_id.clone(),
            "kind": "shell",
            "agentId": request.agent_id.clone(),
        }),
    )
    .await;

    let capabilities =
        get_shell_session_capabilities(state, &connect_response.session_id, &token).await?;
    request_shell_relay(state, &connect_response.session_id, &token).await?;
    post_shell_viewer_connected(state, &connect_response.session_id, &token).await?;
    let shell_session =
        ShellRelaySession::connect(state, &connect_response.session_id, &token, &capabilities)
            .await?;

    post_status(
        state,
        callback_base,
        &request.job_id,
        "running",
        Some("Planning shell command".to_string()),
        Some(json!({ "phase": "planning_shell" })),
        None,
    )
    .await?;

    let mut history: Vec<AiShellAssistHistoryEntry> = Vec::new();
    let mut known_generated_secrets: Vec<GeneratedSecretSummary> =
        request.generated_secrets.clone();
    let mut materialized_secret_handles: Vec<String> = Vec::new();
    let mut redacted_secret_values: Vec<String> = Vec::new();
    let shell_outcome: Result<ShellGoalOutcome> = async {
        let mut deferred_proposal: Option<AiShellAssistResponse> = None;
        'turns: for turn_index in 0..SHELL_GOAL_MAX_TURNS {
            ensure_not_cancelled(cancel)?;
            let proposal = if let Some(proposal) = deferred_proposal.take() {
                proposal
            } else {
                let transcript = redact_generated_secrets(
                    &shell_session.transcript().await,
                    &redacted_secret_values,
                );
                request_shell_assist_proposal(
                    state,
                    callback_base,
                    request,
                    &connect_response.session_id,
                    &token,
                    goal,
                    &transcript,
                    &history,
                    None,
                    capabilities.platform.as_deref(),
                    cancel,
                )
                .await?
            };
            merge_generated_secret_summaries(
                &mut known_generated_secrets,
                &proposal.generated_secrets,
            );
            let action = parse_shell_assist_action(&proposal.action)?;
            ensure_shell_action_allowed_without_active_command(action)?;
            match action {
                ShellAssistAction::Done => {
                    return Ok(ShellGoalOutcome::Completed(json!({
                        "message": proposal.message,
                        "summary": if proposal.message.trim().is_empty() {
                            "Shell goal completed."
                        } else {
                            proposal.message.as_str()
                        },
                        "mode": "shell_goal",
                        "turns": history.len(),
                    })));
                }
                ShellAssistAction::NeedsInput => {
                    return Err(anyhow!(
                        "{}",
                        if proposal.message.trim().is_empty() {
                            "Shell goal needs operator input"
                        } else {
                            proposal.message.as_str()
                        }
                    ));
                }
                ShellAssistAction::OpenDesktop => {
                    return Ok(ShellGoalOutcome::UseDesktopFallback {
                        reason: proposal.message,
                    });
                }
                ShellAssistAction::Command => {}
                other => return Err(anyhow!("unsupported shell assist action: {}", other.as_str())),
            }

            let command = proposal.command.trim();
            if command.is_empty() {
                return Err(anyhow!("shell assist proposed an empty command"));
            }
            validate_shell_generated_secret_references(command, &known_generated_secrets)?;
            validate_shell_generated_secret_command_contract(
                command,
                capabilities.platform.as_deref(),
            )?;

            post_status(
                state,
                callback_base,
                &request.job_id,
                "running",
                Some("Waiting for command approval".to_string()),
                Some(json!({ "phase": "waiting_command_approval", "turnIndex": turn_index })),
                None,
            )
            .await?;

            let approval =
                create_command_approval(state, callback_base, request, turn_index, &proposal)
                    .await?;
            match approval.status.as_str() {
                "policy_blocked" => {
                    return Err(anyhow!("command blocked by policy"));
                }
                "pending" => {}
                status => return Err(anyhow!("unexpected command approval status: {status}")),
            }

            let approved =
                wait_for_command_approval(state, callback_base, request, &approval.id, cancel)
                    .await?;
            match approved.status.as_str() {
                "approved" => {}
                "desktop_control_requested" => {
                    return Ok(ShellGoalOutcome::UseDesktopFallback {
                        reason: "Operator requested desktop control.".to_string(),
                    });
                }
                "denied" => return Err(anyhow!("runner job was stopped: command denied")),
                "expired" => return Err(anyhow!("command approval expired")),
                "policy_blocked" => return Err(anyhow!("command blocked by policy")),
                status => return Err(anyhow!("unexpected command approval status: {status}")),
            }

            if let Err(error) = materialize_shell_generated_secrets(
                state,
                callback_base,
                request,
                &shell_session,
                capabilities.platform.as_deref(),
                command,
                &known_generated_secrets,
                &mut materialized_secret_handles,
                &mut redacted_secret_values,
                cancel,
            )
            .await
            {
                let message = redact_generated_secrets(&error.to_string(), &redacted_secret_values);
                let _ = post_command_approval_result(
                    state,
                    callback_base,
                    request,
                    &approval.id,
                    "failed",
                    None,
                    None,
                    Some(&message),
                )
                .await;
                return Err(anyhow!(
                    "prepare generated secrets for approved command: {message}"
                ));
            }
            post_command_approval_result(
                state,
                callback_base,
                request,
                &approval.id,
                "executing",
                None,
                None,
                None,
            )
            .await?;
            post_status(
                state,
                callback_base,
                &request.job_id,
                "running",
                Some("Executing approved command".to_string()),
                Some(json!({ "phase": "executing_command", "turnIndex": turn_index })),
                None,
            )
            .await?;

            let mut output_sink = ShellCommandOutputSink {
                state,
                callback_base,
                request,
                approval_id: &approval.id,
                turn_index,
                redacted_secret_values: &redacted_secret_values,
                sequence: 0,
                output_offset: 0,
            };
            let mut active_command = shell_session
                .start_command(command, capabilities.platform.as_deref(), &mut output_sink)
                .await?;
            let mut next_wait_ms = state.config.shell_command_checkpoint_ms;
            let mut checkpoint_count = 0u32;
            let execution = match 'command_wait: loop {
                match shell_session
                    .wait_for_command_checkpoint(
                        &mut active_command,
                        next_wait_ms,
                        state.config.shell_command_max_wait_secs,
                        cancel,
                        &mut output_sink,
                    )
                    .await
                {
                    Ok(ShellCommandWaitOutcome::Completed(execution)) => break Ok(execution),
                    Ok(ShellCommandWaitOutcome::Checkpoint(checkpoint)) => {
                        checkpoint_count = checkpoint_count.saturating_add(1);
                        let checkpoint_output =
                            redact_generated_secrets(&checkpoint.output, &redacted_secret_values);
                        post_status(
                            state,
                            callback_base,
                            &request.job_id,
                            "running",
                            Some("Checking long-running command".to_string()),
                            Some(json!({
                                "phase": "checking_command",
                                "turnIndex": turn_index,
                                "approvalId": approval.id.clone(),
                                "elapsedMs": checkpoint.elapsed_ms,
                                "remainingMs": checkpoint.remaining_ms,
                                "checkpointCount": checkpoint_count,
                            })),
                            None,
                        )
                        .await?;
                        let transcript = redact_generated_secrets(
                            &shell_session.transcript().await,
                            &redacted_secret_values,
                        );
                        let active_context = AiShellAssistActiveCommand {
                            command: command.to_string(),
                            approval_id: approval.id.clone(),
                            turn_index,
                            elapsed_ms: checkpoint.elapsed_ms,
                            checkpoint_count,
                            recent_output: checkpoint_output.clone(),
                            remaining_ms: checkpoint.remaining_ms,
                        };
                        let checkpoint_proposal = {
                            let checkpoint_request = request_shell_assist_proposal(
                                state,
                                callback_base,
                                request,
                                &connect_response.session_id,
                                &token,
                                goal,
                                &transcript,
                                &history,
                                Some(&active_context),
                                capabilities.platform.as_deref(),
                                cancel,
                            );
                            tokio::pin!(checkpoint_request);
                            loop {
                                tokio::select! {
                                    biased;
                                    wait_result = shell_session.wait_for_command_checkpoint(
                                        &mut active_command,
                                        SHELL_COMMAND_WAIT_MIN_MS,
                                        state.config.shell_command_max_wait_secs,
                                        cancel,
                                        &mut output_sink,
                                    ) => {
                                        match wait_result {
                                            Ok(ShellCommandWaitOutcome::Completed(execution)) => {
                                                break 'command_wait Ok(execution);
                                            }
                                            Ok(ShellCommandWaitOutcome::Checkpoint(_)) => continue,
                                            Err(error) => break 'command_wait Err(error),
                                        }
                                    }
                                    proposal_result = &mut checkpoint_request => {
                                        match proposal_result {
                                            Ok(proposal) => break proposal,
                                            Err(error) => {
                                                if let Err(interrupt_error) = shell_session.send_interrupt().await {
                                                    warn!(
                                                        error = %interrupt_error,
                                                        "AI runner failed to interrupt shell command after checkpoint proposal failure"
                                                    );
                                                }
                                                output_sink.publish("", true).await;
                                                let message = redact_generated_secrets(
                                                    &error.to_string(),
                                                    &redacted_secret_values,
                                                );
                                                let _ = post_command_approval_result(
                                                    state,
                                                    callback_base,
                                                    request,
                                                    &approval.id,
                                                    "failed",
                                                    Some(&checkpoint_output),
                                                    None,
                                                    Some(&message),
                                                )
                                                .await;
                                                return Err(error.context("checkpoint AI shell assist proposal"));
                                            }
                                        }
                                    }
                                }
                            }
                        };
                        merge_generated_secret_summaries(
                            &mut known_generated_secrets,
                            &checkpoint_proposal.generated_secrets,
                        );
                        let action = parse_shell_assist_action(&checkpoint_proposal.action)?;
                        if action == ShellAssistAction::Wait {
                            next_wait_ms = if checkpoint_proposal.wait_ms == 0 {
                                state.config.shell_command_checkpoint_ms
                            } else {
                                clamp_shell_command_wait_ms(checkpoint_proposal.wait_ms)
                            };
                            post_status(
                                state,
                                callback_base,
                                &request.job_id,
                                "running",
                                Some("Continuing approved command".to_string()),
                                Some(json!({
                                    "phase": "executing_command",
                                    "turnIndex": turn_index,
                                    "approvalId": approval.id.clone(),
                                    "waitMs": next_wait_ms,
                                    "elapsedMs": checkpoint.elapsed_ms,
                                    "checkpointCount": checkpoint_count,
                                })),
                                None,
                            )
                            .await?;
                            continue;
                        }

                        if action == ShellAssistAction::Interrupt {
                            let interrupt_message =
                                "Command interrupted by AI shell assist after checkpoint.";
                            if let Err(error) = shell_session.send_interrupt().await {
                                warn!(
                                    error = %error,
                                    "AI runner failed to interrupt shell command after interrupt checkpoint action"
                                );
                                output_sink.publish("", true).await;
                                let message = format!(
                                    "AI shell assist requested interrupt, but Ctrl+C could not be sent: {error}"
                                );
                                let _ = post_command_approval_result(
                                    state,
                                    callback_base,
                                    request,
                                    &approval.id,
                                    "failed",
                                    Some(&checkpoint_output),
                                    None,
                                    Some(&message),
                                )
                                .await;
                                return Err(error.context("interrupt approved shell command"));
                            }

                            let interrupt_result = shell_session
                                .wait_for_command_checkpoint(
                                    &mut active_command,
                                    SHELL_COMMAND_INTERRUPT_GRACE_MS,
                                    u64::MAX,
                                    cancel,
                                    &mut output_sink,
                                )
                                .await;
                            match interrupt_result {
                                Ok(ShellCommandWaitOutcome::Completed(execution)) => {
                                    let output = redact_generated_secrets(
                                        &execution.output,
                                        &redacted_secret_values,
                                    );
                                    let _ = post_command_approval_result(
                                        state,
                                        callback_base,
                                        request,
                                        &approval.id,
                                        "failed",
                                        Some(&output),
                                        execution.exit_code,
                                        Some(interrupt_message),
                                    )
                                    .await;
                                    let history_output = if output.trim().is_empty() {
                                        interrupt_message.to_string()
                                    } else {
                                        format!("{}\n\n{}", output.trim_end(), interrupt_message)
                                    };
                                    history.push(AiShellAssistHistoryEntry {
                                        command: command.to_string(),
                                        approved: true,
                                        output: Some(history_output),
                                        response_id: checkpoint_proposal.response_id.clone(),
                                    });
                                    continue 'turns;
                                }
                                Ok(ShellCommandWaitOutcome::Checkpoint(grace_checkpoint)) => {
                                    output_sink.publish("", true).await;
                                    let output = redact_generated_secrets(
                                        &grace_checkpoint.output,
                                        &redacted_secret_values,
                                    );
                                    let message = format!(
                                        "Command did not stop within {SHELL_COMMAND_INTERRUPT_GRACE_MS} ms after AI shell assist interrupt."
                                    );
                                    let _ = post_command_approval_result(
                                        state,
                                        callback_base,
                                        request,
                                        &approval.id,
                                        "failed",
                                        Some(&output),
                                        None,
                                        Some(&message),
                                    )
                                    .await;
                                    return Err(anyhow!("{message}"));
                                }
                                Err(error) => {
                                    output_sink.publish("", true).await;
                                    let message = redact_generated_secrets(
                                        &format!(
                                            "AI shell assist interrupted the command, but waiting for completion failed: {error}"
                                        ),
                                        &redacted_secret_values,
                                    );
                                    let _ = post_command_approval_result(
                                        state,
                                        callback_base,
                                        request,
                                        &approval.id,
                                        "failed",
                                        Some(&checkpoint_output),
                                        None,
                                        Some(&message),
                                    )
                                    .await;
                                    return Err(error.context("wait for interrupted shell command"));
                                }
                            }
                        }

                        let action_name = action.as_str();
                        if let Err(error) = shell_session.send_interrupt().await {
                            warn!(
                                error = %error,
                                "AI runner failed to interrupt shell command after non-wait checkpoint action"
                            );
                        }
                        output_sink.publish("", true).await;
                        let interrupt_message = format!(
                            "Command interrupted after checkpoint so Talos can continue with action '{action_name}'."
                        );
                        let _ = post_command_approval_result(
                            state,
                            callback_base,
                            request,
                            &approval.id,
                            "failed",
                            Some(&checkpoint_output),
                            None,
                            Some(&interrupt_message),
                        )
                        .await;
                        let history_output = if checkpoint_output.trim().is_empty() {
                            interrupt_message.clone()
                        } else {
                            format!("{}\n\n{}", checkpoint_output.trim_end(), interrupt_message)
                        };
                        history.push(AiShellAssistHistoryEntry {
                            command: command.to_string(),
                            approved: true,
                            output: Some(history_output),
                            response_id: checkpoint_proposal.response_id.clone(),
                        });
                        match action {
                            ShellAssistAction::Command => {
                                deferred_proposal = Some(checkpoint_proposal);
                                continue 'turns;
                            }
                            ShellAssistAction::Done => {
                                return Ok(ShellGoalOutcome::Completed(json!({
                                    "message": checkpoint_proposal.message,
                                    "summary": if checkpoint_proposal.message.trim().is_empty() {
                                        "Shell goal completed."
                                    } else {
                                        checkpoint_proposal.message.as_str()
                                    },
                                    "mode": "shell_goal",
                                    "turns": history.len(),
                                })));
                            }
                            ShellAssistAction::NeedsInput => {
                                return Err(anyhow!(
                                    "{}",
                                    if checkpoint_proposal.message.trim().is_empty() {
                                        "Shell goal needs operator input"
                                    } else {
                                        checkpoint_proposal.message.as_str()
                                    }
                                ));
                            }
                            ShellAssistAction::OpenDesktop => {
                                return Ok(ShellGoalOutcome::UseDesktopFallback {
                                    reason: checkpoint_proposal.message,
                                });
                            }
                            other => {
                                return Err(anyhow!(
                                    "unsupported shell assist action: {}",
                                    other.as_str()
                                ))
                            }
                        }
                    }
                    Err(error) => break Err(error),
                }
            } {
                Ok(execution) => execution,
                Err(error) => {
                    if let Err(interrupt_error) = shell_session.send_interrupt().await {
                        warn!(
                            error = %interrupt_error,
                            "AI runner failed to interrupt shell command after execution error"
                        );
                    }
                    let message =
                        redact_generated_secrets(&error.to_string(), &redacted_secret_values);
                    let _ = post_command_approval_result(
                        state,
                        callback_base,
                        request,
                        &approval.id,
                        "failed",
                        None,
                        None,
                        Some(&message),
                    )
                    .await;
                    return Err(error.context("execute approved shell command"));
                }
            };
            let output = redact_generated_secrets(&execution.output, &redacted_secret_values);
            post_command_approval_result(
                state,
                callback_base,
                request,
                &approval.id,
                "executed",
                Some(&output),
                execution.exit_code,
                None,
            )
            .await?;
            history.push(AiShellAssistHistoryEntry {
                command: command.to_string(),
                approved: true,
                output: Some(output),
                response_id: proposal.response_id.clone(),
            });
        }

        Err(anyhow!("shell goal stopped at runner turn limit"))
    }
    .await;

    if let Err(error) = post_shell_transcript_artifacts(
        state,
        callback_base,
        request,
        &connect_response.session_id,
        &shell_session,
        &redacted_secret_values,
    )
    .await
    {
        warn!(
            job_id = %request.job_id,
            error = %error,
            "AI runner failed to publish shell transcript artifact"
        );
    }
    shell_session.shutdown(&connect_response.session_id).await;

    match shell_outcome? {
        ShellGoalOutcome::Completed(value) => Ok(value),
        ShellGoalOutcome::UseDesktopFallback { reason } => {
            end_registered_headless_session(
                state,
                &request.job_id,
                callback_base,
                Arc::clone(&session_cleanup),
            )
            .await;
            post_status(
                state,
                callback_base,
                &request.job_id,
                "running",
                Some("Opening desktop control".to_string()),
                Some(json!({ "phase": "desktop_fallback", "reason": reason })),
                None,
            )
            .await?;
            run_desktop_goal_job(state, request, callback_base, cancel, session_cleanup).await
        }
    }
}

async fn run_desktop_goal_job(
    state: &AppState,
    request: &StartJobRequest,
    callback_base: &str,
    cancel: &AtomicBool,
    session_cleanup: SessionCleanupSlot,
) -> Result<Value> {
    let goal = request
        .goal
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Perform the requested desktop goal");
    info!(
        job_id = %request.job_id,
        agent_id = %request.agent_id,
        goal,
        "AI runner opening interactive desktop goal session"
    );
    let (connect_response, token) =
        connect_desktop_session(state, request, DesktopConnectMode::Interactive).await?;
    let cleanup = SessionCleanup {
        rmm_base: state.config.rmm_server_url.clone(),
        session_id: connect_response.session_id.clone(),
        token: token.clone(),
        kind: SessionCleanupKind::Desktop,
    };
    register_headless_session_cleanup(&session_cleanup, cleanup).await;
    let _ = post_runner_event(
        state,
        callback_base,
        &request.job_id,
        "session_started",
        format!("session_started:desktop:{}", connect_response.session_id),
        json!({
            "sessionId": connect_response.session_id.clone(),
            "kind": "desktop",
            "agentId": request.agent_id.clone(),
        }),
    )
    .await;

    post_status(
        state,
        callback_base,
        &request.job_id,
        "running",
        Some("Observing the desktop".to_string()),
        None,
        None,
    )
    .await?;

    let capabilities =
        get_session_capabilities(state, &connect_response.session_id, &token).await?;
    ensure_live_vp8_profile(&capabilities)?;
    request_relay(state, &connect_response.session_id, &token).await?;
    post_viewer_connected(state, &connect_response.session_id, &token).await?;
    let relay_session =
        LiveRelaySession::connect(state, &connect_response.session_id, &capabilities).await?;
    let result = run_desktop_goal_loop(
        state,
        request,
        callback_base,
        cancel,
        goal,
        &connect_response,
        &token,
        &capabilities,
        &relay_session,
    )
    .await;
    relay_session.shutdown(&connect_response.session_id).await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn run_desktop_goal_loop(
    state: &AppState,
    request: &StartJobRequest,
    callback_base: &str,
    cancel: &AtomicBool,
    goal: &str,
    connect_response: &ConnectResponse,
    token: &str,
    capabilities: &SessionCapabilities,
    relay_session: &LiveRelaySession,
) -> Result<Value> {
    ensure_not_cancelled(cancel)?;
    let mut last_frame = relay_session
        .wait_for_frame_after(
            0,
            Duration::from_secs(LIVE_RELAY_FRAME_TIMEOUT_SECS),
            cancel,
        )
        .await?;
    let mut pointer_state = PointerState::default();
    let mut screenshots_posted = 0u32;
    let initial_artifact = post_live_frame_artifact(
        state,
        callback_base,
        request,
        &last_frame,
        &pointer_state,
        None,
        0,
        "Inspecting the desktop before taking action.",
    )
    .await?;
    screenshots_posted = screenshots_posted.saturating_add(1);

    post_status(
        state,
        callback_base,
        &request.job_id,
        "running",
        Some("Planning desktop actions".to_string()),
        None,
        None,
    )
    .await?;
    let mut response = request_desktop_task_start(
        state,
        callback_base,
        request,
        goal,
        connect_response,
        token,
        capabilities,
        &initial_artifact,
        cancel,
    )
    .await?;
    let mut step_guard = 0u32;
    let mut last_step_result = String::new();
    let unchanged_frame_wait_secs = parse_u64_env(
        "RMM_AI_ASSIST_UNCHANGED_FRAME_WAIT_SECS",
        LIVE_RELAY_UNCHANGED_FRAME_WAIT_SECS,
    );

    loop {
        ensure_not_cancelled(cancel)?;
        step_guard = step_guard.saturating_add(1);
        let status = response.status.trim().to_lowercase();
        let progress = desktop_progress_label(&response);
        post_status(
            state,
            callback_base,
            &request.job_id,
            "running",
            Some(progress),
            Some(json!({
                "taskId": &response.task_id,
                "taskStatus": &response.status,
                "stepIndex": response.step_index,
                "maxSteps": response.max_steps,
                "assistantMessage": &response.assistant_message,
                "plan": &response.plan,
                "actions": response.actions.len(),
                "responseId": &response.response_id,
                "generatedSecrets": &response.generated_secrets,
            })),
            None,
        )
        .await?;

        match status.as_str() {
            "complete" => {
                let summary = response.assistant_message.trim().to_string();
                let summary = if summary.is_empty() {
                    "Desktop goal completed.".to_string()
                } else {
                    summary
                };
                return Ok(json!({
                    "message": summary,
                    "summary": summary,
                    "sessionId": connect_response.session_id,
                    "agentId": request.agent_id,
                    "userId": request.user_id,
                    "conversationId": request.conversation_id,
                    "goal": request.goal,
                    "taskId": response.task_id,
                    "stepIndex": response.step_index,
                    "maxSteps": response.max_steps,
                    "plan": response.plan,
                    "responseId": response.response_id,
                    "generatedSecrets": response.generated_secrets,
                    "selectedDisplayProfile": capabilities.selected_display_profile,
                    "screenshotsPosted": screenshots_posted,
                    "lastStepResult": last_step_result,
                }));
            }
            "failed" => {
                let message = if response.assistant_message.trim().is_empty() {
                    "AI desktop task failed".to_string()
                } else {
                    response.assistant_message.trim().to_string()
                };
                return Err(anyhow!("{message}"));
            }
            "needs_approval" => {
                return Err(anyhow!("AI desktop task needs approval before continuing"));
            }
            "running" => {}
            other => {
                return Err(anyhow!(
                    "AI desktop task returned unsupported status: {other}"
                ))
            }
        }

        if response.actions.is_empty() {
            return Err(anyhow!(
                "AI desktop task is running but returned no actions to execute"
            ));
        }
        let max_steps = if response.max_steps > 0 {
            response.max_steps
        } else {
            parse_u32_env("RMM_AI_ASSIST_MAX_STEPS", 12)
        };
        if step_guard > max_steps {
            return Err(anyhow!("AI desktop task stopped at runner step limit"));
        }

        let action_narration =
            desktop_action_batch_narration(&response.actions, last_frame.width, last_frame.height);
        let execution_status = if action_narration.is_empty() {
            format!(
                "Executing step {}/{}",
                response.step_index, response.max_steps
            )
        } else {
            action_narration.clone()
        };
        post_status(
            state,
            callback_base,
            &request.job_id,
            "running",
            Some(execution_status),
            Some(json!({
                "taskId": &response.task_id,
                "taskStatus": &response.status,
                "stepIndex": response.step_index,
                "maxSteps": response.max_steps,
                "assistantMessage": &response.assistant_message,
                "plan": &response.plan,
                "actions": response.actions.len(),
                "actionNarration": &action_narration,
                "responseId": &response.response_id,
                "generatedSecrets": &response.generated_secrets,
            })),
            None,
        )
        .await?;
        let executed = execute_desktop_action_batch(
            state,
            callback_base,
            request,
            relay_session,
            &response.actions,
            &last_frame,
            &mut pointer_state,
            cancel,
        )
        .await?;
        last_step_result = if action_narration.is_empty() {
            format!(
                "Executed {} action(s) at step {}.",
                executed.executed_actions, response.step_index
            )
        } else {
            format!(
                "{action_narration} Executed {} action(s).",
                executed.executed_actions
            )
        };

        ensure_not_cancelled(cancel)?;
        if executed.settle_ms > 0 {
            post_status(
                state,
                callback_base,
                &request.job_id,
                "running",
                Some("Waiting for updated screen".to_string()),
                None,
                None,
            )
            .await?;
            cancellable_sleep(Duration::from_millis(executed.settle_ms), cancel).await?;
        }

        let previous_seq = last_frame.seq;
        let frame_observation = relay_session
            .wait_for_frame_after_or_latest(
                previous_seq,
                Duration::from_secs(unchanged_frame_wait_secs),
                cancel,
            )
            .await?;
        if !frame_observation.updated {
            info!(
                job_id = %request.job_id,
                task_id = %response.task_id,
                step_index = response.step_index,
                previous_frame_seq = previous_seq,
                latest_frame_seq = frame_observation.frame.seq,
                wait_secs = unchanged_frame_wait_secs,
                "AI runner continuing desktop task with unchanged live frame"
            );
            last_step_result = format!(
                "{} No newer desktop frame arrived within {} seconds, so the latest available screenshot appears unchanged.",
                last_step_result.trim(),
                unchanged_frame_wait_secs
            )
            .trim()
            .to_string();
        }
        last_frame = frame_observation.frame;
        let live_frame_message = live_frame_action_message(
            &response,
            &action_narration,
            !frame_observation.updated,
            unchanged_frame_wait_secs,
        );
        let artifact = post_live_frame_artifact(
            state,
            callback_base,
            request,
            &last_frame,
            &pointer_state,
            Some(&response.task_id),
            response.step_index,
            &live_frame_message,
        )
        .await?;
        screenshots_posted = screenshots_posted.saturating_add(1);

        post_status(
            state,
            callback_base,
            &request.job_id,
            "running",
            Some("Continuing desktop task".to_string()),
            None,
            None,
        )
        .await?;
        response = request_desktop_task_continue(
            state,
            callback_base,
            request,
            &response.task_id,
            connect_response,
            token,
            capabilities,
            &artifact,
            &last_step_result,
            cancel,
        )
        .await?;
    }
}

fn desktop_progress_label(response: &AiDesktopTaskStepResponse) -> String {
    match response.status.trim().to_lowercase().as_str() {
        "complete" => "Desktop goal complete".to_string(),
        "failed" => "Desktop goal failed".to_string(),
        "needs_approval" => "Desktop goal needs approval".to_string(),
        _ if response.step_index > 0 && response.max_steps > 0 => {
            format!(
                "Planning step {}/{}",
                response.step_index, response.max_steps
            )
        }
        _ => "Planning desktop actions".to_string(),
    }
}

fn live_frame_action_message(
    response: &AiDesktopTaskStepResponse,
    action_narration: &str,
    unchanged_screen: bool,
    unchanged_wait_secs: u64,
) -> String {
    let action_narration = action_narration.trim();
    let base = if !action_narration.is_empty() {
        action_narration.to_string()
    } else {
        let assistant_message = response.assistant_message.trim();
        if !assistant_message.is_empty() {
            assistant_message.to_string()
        } else {
            "Waiting for the desktop to update.".to_string()
        }
    };
    if unchanged_screen {
        format!(
            "{base}\n\nNo newer desktop frame arrived within {unchanged_wait_secs} seconds, so this screenshot appears unchanged. Reassess the visible state and choose the next action."
        )
    } else {
        base
    }
}

fn desktop_action_batch_narration(actions: &[AiDesktopAction], width: u32, height: u32) -> String {
    let immediate_len = ai_assist_immediate_action_len(actions)
        .min(parse_usize_env("RMM_AI_ASSIST_MAX_ACTIONS_PER_STEP", 6));
    let descriptions: Vec<String> = if immediate_len == 0 {
        actions
            .iter()
            .take(1)
            .filter_map(|action| describe_desktop_action(action, width, height))
            .collect()
    } else {
        actions
            .iter()
            .take(immediate_len)
            .filter_map(|action| describe_desktop_action(action, width, height))
            .collect()
    };
    human_join(descriptions)
}

fn describe_desktop_action(action: &AiDesktopAction, width: u32, height: u32) -> Option<String> {
    match action {
        AiDesktopAction::Move { x, y, keys } => Some(with_modifier_context(
            format!(
                "Move the mouse to the {}",
                desktop_position_label(*x, *y, width, height)
            ),
            keys,
        )),
        AiDesktopAction::Click { x, y, button, keys } => Some(with_modifier_context(
            format!(
                "{} in the {}",
                click_verb("Click", button),
                desktop_position_label(*x, *y, width, height)
            ),
            keys,
        )),
        AiDesktopAction::DoubleClick { x, y, button, keys } => Some(with_modifier_context(
            format!(
                "{} in the {}",
                click_verb("Double-click", button),
                desktop_position_label(*x, *y, width, height)
            ),
            keys,
        )),
        AiDesktopAction::Drag { path, keys, .. } => {
            let start = path.first()?;
            let end = path.last()?;
            Some(with_modifier_context(
                format!(
                    "Drag from the {} to the {}",
                    desktop_position_label(start.x, start.y, width, height),
                    desktop_position_label(end.x, end.y, width, height)
                ),
                keys,
            ))
        }
        AiDesktopAction::Scroll {
            x,
            y,
            scroll_x,
            scroll_y,
            keys,
        } => {
            let delta = if *scroll_y != 0 { *scroll_y } else { *scroll_x };
            let direction = if delta < 0 {
                "up"
            } else if delta > 0 {
                "down"
            } else {
                "slightly"
            };
            Some(with_modifier_context(
                format!(
                    "Scroll {direction} near the {}",
                    desktop_position_label(*x, *y, width, height)
                ),
                keys,
            ))
        }
        AiDesktopAction::Type { text } => Some(format!("Type {}", quoted_text_preview(text))),
        AiDesktopAction::InjectSecret { .. } => Some("Inject generated secret".to_string()),
        AiDesktopAction::Keypress { keys } => {
            let label = keypress_label(keys);
            if label.is_empty() {
                None
            } else {
                Some(format!("Press {label}"))
            }
        }
        AiDesktopAction::Wait { ms } => Some(format!("Wait {}", duration_label(*ms))),
    }
}

fn click_verb(prefix: &str, button: &str) -> String {
    match button.trim().to_lowercase().as_str() {
        "" | "left" => prefix.to_string(),
        other => format!("{prefix} the {other} mouse button"),
    }
}

fn with_modifier_context(action: String, keys: &[String]) -> String {
    let modifiers = modifier_label(keys);
    if modifiers.is_empty() {
        action
    } else {
        format!("{action} while holding {modifiers}")
    }
}

fn modifier_label(keys: &[String]) -> String {
    let labels: Vec<String> = split_ai_keys(keys)
        .into_iter()
        .filter(|key| ai_modifier_bit(key) != 0)
        .map(|key| display_key_name(&key))
        .collect();
    human_join(labels)
}

fn keypress_label(keys: &[String]) -> String {
    human_join(
        split_ai_keys(keys)
            .into_iter()
            .map(|key| display_key_name(&key))
            .collect(),
    )
}

fn display_key_name(key: &str) -> String {
    match normalize_ai_key_name(key).as_str() {
        "CTRL" | "CONTROL" => "Control".to_string(),
        "SHIFT" => "Shift".to_string(),
        "ALT" | "OPTION" => "Option".to_string(),
        "WIN" | "META" | "CMD" | "COMMAND" => "Command".to_string(),
        "ESC" | "ESCAPE" => "Escape".to_string(),
        "ENTER" | "RETURN" => "Return".to_string(),
        "BACKSPACE" => "Backspace".to_string(),
        "DELETE" | "DEL" => "Delete".to_string(),
        "TAB" => "Tab".to_string(),
        "SPACE" => "Space".to_string(),
        "UP" | "ARROWUP" => "Up Arrow".to_string(),
        "DOWN" | "ARROWDOWN" => "Down Arrow".to_string(),
        "LEFT" | "ARROWLEFT" => "Left Arrow".to_string(),
        "RIGHT" | "ARROWRIGHT" => "Right Arrow".to_string(),
        other => other.to_string(),
    }
}

fn desktop_position_label(x: u32, y: u32, width: u32, height: u32) -> String {
    let horizontal = axis_label(x, width, "left", "center", "right");
    let vertical = axis_label(y, height, "upper", "middle", "lower");
    if horizontal == "center" && vertical == "middle" {
        "center of the screen".to_string()
    } else if horizontal == "center" {
        format!("{vertical} center of the screen")
    } else if vertical == "middle" {
        format!("middle {horizontal} of the screen")
    } else {
        format!("{vertical}-{horizontal} of the screen")
    }
}

fn axis_label<'a>(
    value: u32,
    dimension: u32,
    low: &'a str,
    middle: &'a str,
    high: &'a str,
) -> &'a str {
    if dimension == 0 {
        return middle;
    }
    let ratio = value as f64 / dimension as f64;
    if ratio < 0.33 {
        low
    } else if ratio > 0.66 {
        high
    } else {
        middle
    }
}

fn quoted_text_preview(text: &str) -> String {
    let sanitized = text
        .chars()
        .map(|ch| match ch {
            '\n' | '\r' | '\t' => ' ',
            other => other,
        })
        .collect::<String>();
    let trimmed = sanitized.trim();
    let mut preview = trimmed.chars().take(48).collect::<String>();
    if trimmed.chars().count() > 48 {
        preview.push_str("...");
    }
    format!("\"{preview}\"")
}

fn duration_label(ms: u64) -> String {
    let clamped = clamp_ai_assist_wait_ms(ms);
    if clamped < 1_000 {
        format!("{} ms", clamped)
    } else {
        let seconds = clamped as f64 / 1_000.0;
        format!("{seconds:.1} seconds")
    }
}

fn human_join(items: Vec<String>) -> String {
    match items.len() {
        0 => String::new(),
        1 => items.into_iter().next().unwrap_or_default(),
        2 => format!("{} then {}", items[0], lower_first(&items[1])),
        _ => {
            let mut parts = items;
            let last = parts.pop().unwrap_or_default();
            let first = parts.remove(0);
            let middle = parts
                .into_iter()
                .map(|item| lower_first(&item))
                .collect::<Vec<_>>();
            let prefix = if middle.is_empty() {
                first
            } else {
                format!("{first}, {}", middle.join(", "))
            };
            format!("{}, then {}", prefix, lower_first(&last))
        }
    }
}

fn lower_first(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

async fn post_live_frame_artifact(
    state: &AppState,
    callback_base: &str,
    request: &StartJobRequest,
    frame: &LiveFrame,
    pointer_state: &PointerState,
    task_id: Option<&str>,
    step_index: u32,
    message_content: &str,
) -> Result<ScreenshotArtifact> {
    let artifact = build_screenshot_artifact(
        frame.seq,
        frame.width,
        frame.height,
        frame.bgra.len() as u32,
        &frame.bgra,
    )?;
    let artifact_name = format!("desktop-goal-frame-{}.png", frame.seq);
    post_artifact(
        state,
        callback_base,
        &request.job_id,
        "runner-screenshot",
        &artifact_name,
        "image/png",
        artifact.base64_content.clone(),
        json!({
            "frameId": artifact.frame_id,
            "frameSeq": frame.seq,
            "width": artifact.width,
            "height": artifact.height,
            "payloadBytes": artifact.payload_bytes,
            "pngBytes": artifact.png_bytes,
            "source": "live_vp8_relay_stream",
            "taskId": task_id,
            "stepIndex": step_index,
            "cursor": pointer_state.metadata(frame.width, frame.height),
            "displayText": message_content,
            "capturedAtUnixMs": now_unix_ms(),
        }),
        true,
        Some(message_content),
        Some("live_frame"),
    )
    .await?;
    Ok(artifact)
}

fn desktop_task_start_body(
    request: &StartJobRequest,
    goal: &str,
    connect_response: &ConnectResponse,
    token: &str,
    rmm_api_base: &str,
    capabilities: &SessionCapabilities,
    artifact: &ScreenshotArtifact,
    device_context: Option<&Value>,
) -> Value {
    json!({
        "goal": goal,
        "screenshotBase64": artifact.base64_content.as_str(),
        "width": artifact.width,
        "height": artifact.height,
        "sessionId": connect_response.session_id,
        "sessionToken": token,
        "rmmApiBase": rmm_api_base,
        "platform": capabilities.platform,
        "deviceContext": device_context.cloned(),
        "generatedSecrets": &request.generated_secrets,
        "jobId": request.job_id,
        "organizationId": request.organization_id,
        "userId": request.user_id,
        "conversationId": request.conversation_id,
        "agentId": request.agent_id,
    })
}

fn desktop_task_continue_body(
    request: &StartJobRequest,
    connect_response: &ConnectResponse,
    token: &str,
    rmm_api_base: &str,
    capabilities: &SessionCapabilities,
    artifact: &ScreenshotArtifact,
    last_step_result: &str,
    device_context: Option<&Value>,
) -> Value {
    json!({
        "screenshotBase64": artifact.base64_content.as_str(),
        "width": artifact.width,
        "height": artifact.height,
        "sessionId": connect_response.session_id,
        "sessionToken": token,
        "rmmApiBase": rmm_api_base,
        "platform": capabilities.platform,
        "lastStepResult": last_step_result,
        "deviceContext": device_context.cloned(),
        "generatedSecrets": &request.generated_secrets,
        "jobId": request.job_id,
        "organizationId": request.organization_id,
        "userId": request.user_id,
        "conversationId": request.conversation_id,
        "agentId": request.agent_id,
    })
}

#[allow(clippy::too_many_arguments)]
fn shell_assist_proposal_body(
    request: &StartJobRequest,
    goal: &str,
    transcript: &str,
    history: &[AiShellAssistHistoryEntry],
    active_command: Option<&AiShellAssistActiveCommand>,
    session_id: &str,
    token: &str,
    rmm_api_base: &str,
    platform: Option<&str>,
    device_context: Option<&Value>,
) -> Value {
    json!({
        "prompt": goal,
        "transcript": transcript,
        "history": history,
        "activeCommand": active_command,
        "sessionId": session_id,
        "sessionToken": token,
        "rmmApiBase": rmm_api_base,
        "platform": platform,
        "deviceContext": device_context.cloned(),
        "generatedSecrets": &request.generated_secrets,
        "jobId": request.job_id,
        "organizationId": request.organization_id,
        "userId": request.user_id,
        "conversationId": request.conversation_id,
        "agentId": request.agent_id,
    })
}

async fn request_desktop_task_start(
    state: &AppState,
    callback_base: &str,
    request: &StartJobRequest,
    goal: &str,
    connect_response: &ConnectResponse,
    token: &str,
    capabilities: &SessionCapabilities,
    artifact: &ScreenshotArtifact,
    cancel: &AtomicBool,
) -> Result<AiDesktopTaskStepResponse> {
    ensure_not_cancelled(cancel)?;
    let url = format!("{}/rmm/ai/desktop-task/start", callback_base);
    let response = run_cancelable(
        async {
            state
                .client
                .post(url)
                .timeout(Duration::from_secs(90))
                .json(&desktop_task_start_body(
                    request,
                    goal,
                    connect_response,
                    token,
                    &state.config.rmm_server_url,
                    capabilities,
                    artifact,
                    request.device_context.as_ref(),
                ))
                .send()
                .await
                .context("start AI desktop task")
        },
        cancel,
    )
    .await?;
    run_cancelable(
        decode_desktop_task_response(response, "start AI desktop task"),
        cancel,
    )
    .await
}

async fn request_desktop_task_continue(
    state: &AppState,
    callback_base: &str,
    request: &StartJobRequest,
    task_id: &str,
    connect_response: &ConnectResponse,
    token: &str,
    capabilities: &SessionCapabilities,
    artifact: &ScreenshotArtifact,
    last_step_result: &str,
    cancel: &AtomicBool,
) -> Result<AiDesktopTaskStepResponse> {
    ensure_not_cancelled(cancel)?;
    let url = format!(
        "{}/rmm/ai/desktop-task/{}/continue",
        callback_base,
        encode_path_segment(task_id)
    );
    let response = run_cancelable(
        async {
            state
                .client
                .post(url)
                .timeout(Duration::from_secs(90))
                .json(&desktop_task_continue_body(
                    request,
                    connect_response,
                    token,
                    &state.config.rmm_server_url,
                    capabilities,
                    artifact,
                    last_step_result,
                    request.device_context.as_ref(),
                ))
                .send()
                .await
                .context("continue AI desktop task")
        },
        cancel,
    )
    .await?;
    run_cancelable(
        decode_desktop_task_response(response, "continue AI desktop task"),
        cancel,
    )
    .await
}

async fn decode_desktop_task_response(
    response: reqwest::Response,
    context_label: &str,
) -> Result<AiDesktopTaskStepResponse> {
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(anyhow!("{context_label} failed: {status}: {text}"));
    }
    response
        .json()
        .await
        .with_context(|| format!("decode {context_label} response"))
}

#[allow(clippy::too_many_arguments)]
async fn request_shell_assist_proposal(
    state: &AppState,
    callback_base: &str,
    request: &StartJobRequest,
    session_id: &str,
    token: &str,
    goal: &str,
    transcript: &str,
    history: &[AiShellAssistHistoryEntry],
    active_command: Option<&AiShellAssistActiveCommand>,
    platform: Option<&str>,
    cancel: &AtomicBool,
) -> Result<AiShellAssistResponse> {
    ensure_not_cancelled(cancel)?;
    let url = format!("{}/rmm/ai/shell-assist", callback_base);
    let response = run_cancelable(
        async {
            state
                .client
                .post(url)
                .timeout(Duration::from_secs(90))
                .json(&shell_assist_proposal_body(
                    request,
                    goal,
                    transcript,
                    history,
                    active_command,
                    session_id,
                    token,
                    &state.config.rmm_server_url,
                    platform,
                    request.device_context.as_ref(),
                ))
                .send()
                .await
                .context("request AI shell assist proposal")
        },
        cancel,
    )
    .await?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(anyhow!("AI shell assist proposal failed: {status}: {text}"));
    }
    let proposal = run_cancelable(
        async {
            response
                .json::<AiShellAssistResponse>()
                .await
                .context("decode AI shell assist proposal")
        },
        cancel,
    )
    .await?;
    debug!(
        job_id = %request.job_id,
        action = %proposal.action,
        command_chars = proposal.command.len(),
        "AI runner received shell assist proposal"
    );
    Ok(proposal)
}

async fn resolve_generated_secret_for_runner(
    state: &AppState,
    callback_base: &str,
    request: &StartJobRequest,
    secret_handle: &str,
) -> Result<ResolvedGeneratedSecret> {
    let handle = secret_handle.trim();
    if !is_valid_secret_handle(handle) {
        return Err(anyhow!("generated secret handle is invalid"));
    }
    let lease_id = job_lease_id(state, &request.job_id).await;
    let url = format!(
        "{}/secure-notes/internal/runner-secrets/{}/reveal",
        callback_base,
        encode_path_segment(handle)
    );
    let response = state
        .client
        .post(url)
        .header("x-service-key", &state.config.service_key)
        .json(&json!({
            "jobId": request.job_id,
            "runnerId": state.config.runner_id,
            "leaseId": lease_id,
        }))
        .send()
        .await
        .context("resolve generated secret for runner")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("generated secret resolve failed: {status}: {body}"));
    }
    response
        .json::<ResolvedGeneratedSecret>()
        .await
        .context("decode generated secret resolve response")
}

fn is_valid_secret_handle(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("sec_") else {
        return false;
    };
    suffix.len() == 16
        && suffix
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
}

fn merge_generated_secret_summaries(
    known: &mut Vec<GeneratedSecretSummary>,
    incoming: &[GeneratedSecretSummary],
) {
    for item in incoming {
        if !known
            .iter()
            .any(|existing| existing.secret_handle == item.secret_handle)
        {
            known.push(item.clone());
        }
    }
}

fn redact_generated_secrets(text: &str, secrets: &[String]) -> String {
    let mut redacted = text.to_string();
    for secret in secrets {
        if !secret.is_empty() {
            redacted = redacted.replace(secret, "[generated secret redacted]");
        }
    }
    redacted
}

fn talos_shell_secret_references(command: &str) -> Vec<String> {
    const PREFIX: &str = "$__talos_secret_";
    let mut references = Vec::new();
    let mut search_from = 0usize;
    while let Some(relative_start) = command[search_from..].find(PREFIX) {
        let start = search_from + relative_start;
        let mut end = start + PREFIX.len();
        for (offset, ch) in command[end..].char_indices() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                end = start + PREFIX.len() + offset + ch.len_utf8();
            } else {
                break;
            }
        }
        if end > start + PREFIX.len() {
            references.push(command[start..end].to_string());
        }
        search_from = end;
        if search_from >= command.len() {
            break;
        }
    }
    references.sort();
    references.dedup();
    references
}

fn validate_shell_generated_secret_references(
    command: &str,
    known_generated_secrets: &[GeneratedSecretSummary],
) -> Result<()> {
    for reference in talos_shell_secret_references(command) {
        let known = known_generated_secrets
            .iter()
            .filter_map(|summary| summary.shell_reference.as_deref())
            .any(|known_reference| known_reference == reference);
        if !known {
            return Err(anyhow!(
                "command references unknown generated secret variable {reference}"
            ));
        }
    }
    Ok(())
}

fn validate_shell_generated_secret_command_contract(
    command: &str,
    platform: Option<&str>,
) -> Result<()> {
    if !platform_is_windows(platform) {
        return Ok(());
    }
    if talos_shell_secret_references(command).is_empty() {
        return Ok(());
    }
    if command
        .to_ascii_lowercase()
        .contains("convertto-securestring")
    {
        return Err(anyhow!(
            "Windows generated secret references are already PowerShell SecureString variables; pass the shellReference directly to SecureString-compatible cmdlet parameters instead of ConvertTo-SecureString"
        ));
    }
    Ok(())
}

async fn materialize_shell_generated_secrets(
    state: &AppState,
    callback_base: &str,
    request: &StartJobRequest,
    shell_session: &ShellRelaySession,
    platform: Option<&str>,
    command: &str,
    known_generated_secrets: &[GeneratedSecretSummary],
    materialized_secret_handles: &mut Vec<String>,
    redacted_secret_values: &mut Vec<String>,
    cancel: &AtomicBool,
) -> Result<()> {
    validate_shell_generated_secret_references(command, known_generated_secrets)?;
    validate_shell_generated_secret_command_contract(command, platform)?;
    for summary in known_generated_secrets {
        let Some(shell_reference) = summary.shell_reference.as_deref() else {
            continue;
        };
        if !command.contains(shell_reference)
            || materialized_secret_handles
                .iter()
                .any(|handle| handle == &summary.secret_handle)
        {
            continue;
        }
        post_status(
            state,
            callback_base,
            &request.job_id,
            "running",
            Some("Preparing generated secret".to_string()),
            Some(json!({
                "phase": "materializing_generated_secret",
                "secretHandle": summary.secret_handle,
                "shellReference": shell_reference,
                "secureNoteUrl": summary.secure_note_url,
            })),
            None,
        )
        .await?;
        let resolved = resolve_generated_secret_for_runner(
            state,
            callback_base,
            request,
            &summary.secret_handle,
        )
        .await?;
        let resolved_reference = resolved
            .shell_reference
            .as_deref()
            .unwrap_or(shell_reference);
        redacted_secret_values.push(resolved.secret.clone());
        shell_session
            .materialize_generated_secret(resolved_reference, &resolved.secret, platform, cancel)
            .await?;
        materialized_secret_handles.push(summary.secret_handle.clone());
    }
    Ok(())
}

async fn create_command_approval(
    state: &AppState,
    callback_base: &str,
    request: &StartJobRequest,
    turn_index: u32,
    proposal: &AiShellAssistResponse,
) -> Result<CommandApprovalResponse> {
    let lease_id = job_lease_id(state, &request.job_id).await;
    let event_key = format!("command_proposal:{turn_index}");
    let url = format!(
        "{}/command-center/internal/ai-runner/jobs/{}/command-approvals",
        callback_base,
        encode_path_segment(&request.job_id)
    );
    let response = state
        .client
        .post(url)
        .header("x-service-key", &state.config.service_key)
        .json(&json!({
            "runnerId": state.config.runner_id,
            "leaseId": lease_id,
            "eventKey": event_key,
            "turnIndex": turn_index,
            "command": proposal.command,
            "explanation": proposal.explanation,
            "risk": proposal.risk,
            "notes": proposal.notes,
            "message": proposal.message,
            "modelResponseId": proposal.response_id,
        }))
        .send()
        .await
        .context("create command approval")?;
    decode_command_approval_response(response, "create command approval").await
}

async fn wait_for_command_approval(
    state: &AppState,
    callback_base: &str,
    request: &StartJobRequest,
    approval_id: &str,
    cancel: &AtomicBool,
) -> Result<CommandApprovalResponse> {
    let mut ticker = interval(Duration::from_millis(SHELL_COMMAND_APPROVAL_POLL_MS));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ensure_not_cancelled(cancel)?;
        ticker.tick().await;
        let url = format!(
            "{}/command-center/internal/ai-runner/jobs/{}/command-approvals/{}",
            callback_base,
            encode_path_segment(&request.job_id),
            encode_path_segment(approval_id)
        );
        let response = state
            .client
            .get(url)
            .header("x-service-key", &state.config.service_key)
            .send()
            .await
            .context("poll command approval")?;
        let approval = decode_command_approval_response(response, "poll command approval").await?;
        match approval.status.as_str() {
            "pending" => continue,
            _ => return Ok(approval),
        }
    }
}

async fn post_command_approval_result(
    state: &AppState,
    callback_base: &str,
    request: &StartJobRequest,
    approval_id: &str,
    status: &str,
    output: Option<&str>,
    exit_code: Option<i32>,
    error: Option<&str>,
) -> Result<CommandApprovalResponse> {
    let lease_id = job_lease_id(state, &request.job_id).await;
    let event_key = format!("command_result:{approval_id}:{status}");
    let url = format!(
        "{}/command-center/internal/ai-runner/jobs/{}/command-approvals/{}/result",
        callback_base,
        encode_path_segment(&request.job_id),
        encode_path_segment(approval_id)
    );
    let response = state
        .client
        .post(url)
        .header("x-service-key", &state.config.service_key)
        .json(&json!({
            "runnerId": state.config.runner_id,
            "leaseId": lease_id,
            "eventKey": event_key,
            "status": status,
            "output": output,
            "exitCode": exit_code,
            "error": error,
        }))
        .send()
        .await
        .context("post command approval result")?;
    decode_command_approval_response(response, "post command approval result").await
}

async fn post_command_output_delta(
    state: &AppState,
    callback_base: &str,
    request: &StartJobRequest,
    approval_id: &str,
    turn_index: u32,
    sequence: u64,
    output_offset: usize,
    text: &str,
    terminal: bool,
) -> Result<()> {
    let lease_id = job_lease_id(state, &request.job_id).await;
    let url = format!(
        "{}/command-center/internal/ai-runner/jobs/{}/events",
        callback_base,
        encode_path_segment(&request.job_id)
    );
    post_json(
        state,
        &url,
        json!({
            "eventKey": format!("command_output:{approval_id}:{sequence:010}"),
            "eventType": "command_output_delta",
            "runnerId": state.config.runner_id,
            "leaseId": lease_id,
            "turnIndex": turn_index,
            "commandApprovalId": approval_id,
            "payload": {
                "jobId": request.job_id,
                "approvalId": approval_id,
                "turnIndex": turn_index,
                "sequence": sequence,
                "text": text,
                "outputOffset": output_offset,
                "terminal": terminal,
            },
        }),
    )
    .await
}

async fn decode_command_approval_response(
    response: reqwest::Response,
    context_label: &str,
) -> Result<CommandApprovalResponse> {
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(anyhow!("{context_label} failed: {status}: {text}"));
    }
    let envelope = response
        .json::<CommandApprovalEnvelope>()
        .await
        .with_context(|| format!("decode {context_label} response"))?;
    Ok(envelope.approval)
}

fn ensure_live_vp8_profile(capabilities: &SessionCapabilities) -> Result<()> {
    let selected = capabilities
        .selected_display_profile
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("session capabilities did not include selectedDisplayProfile"))?;
    if selected != REMOTE_DESKTOP_PROFILE_MODERN_CPU && selected != REMOTE_DESKTOP_PROFILE_LEGACY {
        return Err(anyhow!(
            "desktop_goal POC requires VP8/IVF profile modern_cpu or legacy; selected {selected}"
        ));
    }
    if let Some(profile) = capabilities
        .display_profiles
        .iter()
        .find(|profile| profile.id == selected)
    {
        if profile.protocol != REMOTE_DESKTOP_PROTOCOL_LEGACY_IVF {
            return Err(anyhow!(
                "desktop_goal POC requires VP8/IVF protocol {}; selected profile {} uses {}",
                REMOTE_DESKTOP_PROTOCOL_LEGACY_IVF,
                profile.id,
                profile.protocol
            ));
        }
    }
    Ok(())
}

async fn register_headless_session_cleanup(
    session_cleanup: &SessionCleanupSlot,
    cleanup: SessionCleanup,
) {
    let mut guard = session_cleanup.lock().await;
    *guard = Some(cleanup);
}

async fn end_registered_headless_session(
    state: &AppState,
    job_id: &str,
    callback_base: &str,
    session_cleanup: SessionCleanupSlot,
) {
    let cleanup = {
        let mut guard = session_cleanup.lock().await;
        guard.take()
    };
    let Some(cleanup) = cleanup else {
        debug!(job_id = %job_id, "AI runner had no headless desktop session to clean up");
        return;
    };
    let session_id = cleanup.session_id.clone();
    let kind = cleanup.kind;
    cleanup.end(state).await;
    let _ = post_runner_event(
        state,
        callback_base,
        job_id,
        "session_cleanup_finished",
        format!("session_cleanup_finished:{}:{}", kind.as_str(), session_id),
        json!({
            "sessionId": session_id,
            "kind": kind.as_str(),
        }),
    )
    .await;
    debug!(
        job_id = %job_id,
        session_id = %session_id,
        "AI runner cleanup completed"
    );
}

fn ensure_not_cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        return Err(anyhow!("runner job was stopped"));
    }
    Ok(())
}

async fn wait_until_cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn run_cancelable<T, F>(future: F, cancel: &AtomicBool) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    ensure_not_cancelled(cancel)?;
    tokio::select! {
        result = future => result,
        _ = wait_until_cancelled(cancel) => Err(anyhow!("runner job was stopped")),
    }
}

async fn cancellable_sleep(duration: Duration, cancel: &AtomicBool) -> Result<()> {
    ensure_not_cancelled(cancel)?;
    let mut remaining = duration;
    while !remaining.is_zero() {
        ensure_not_cancelled(cancel)?;
        let chunk = remaining.min(Duration::from_millis(50));
        tokio::time::sleep(chunk).await;
        remaining = remaining.saturating_sub(chunk);
    }
    ensure_not_cancelled(cancel)
}

struct SessionCleanup {
    rmm_base: String,
    session_id: String,
    token: String,
    kind: SessionCleanupKind,
}

#[derive(Debug, Clone, Copy)]
enum SessionCleanupKind {
    Desktop,
    Shell,
}

impl SessionCleanupKind {
    fn as_str(self) -> &'static str {
        match self {
            SessionCleanupKind::Desktop => "desktop",
            SessionCleanupKind::Shell => "shell",
        }
    }
}

impl SessionCleanup {
    async fn end(self, state: &AppState) {
        debug!(session_id = %self.session_id, kind = ?self.kind, "AI runner ending leased session");
        let path = match self.kind {
            SessionCleanupKind::Desktop => "session",
            SessionCleanupKind::Shell => "shell/session",
        };
        let url = format!(
            "{}/api/rmm/{}/{}/end?token={}",
            self.rmm_base,
            path,
            encode_path_segment(&self.session_id),
            encode_query_component(&self.token)
        );
        match state.client.post(url).send().await {
            Ok(response) if response.status().is_success() => {
                info!(session_id = %self.session_id, kind = ?self.kind, "AI runner ended leased session");
            }
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                warn!(
                    session_id = %self.session_id,
                    kind = ?self.kind,
                    status = %status,
                    body = %body,
                    "AI runner failed to end leased session"
                );
            }
            Err(error) => {
                warn!(session_id = %self.session_id, kind = ?self.kind, error = %error, "AI runner failed to end leased session");
            }
        }
    }
}

struct ChatSessionCleanup {
    rmm_base: String,
    session_id: String,
    token: String,
}

impl ChatSessionCleanup {
    async fn end(self, state: &AppState) {
        debug!(session_id = %self.session_id, "AI runner ending chat approval session");
        let url = format!(
            "{}/api/rmm/chat/session/{}/end?token={}",
            self.rmm_base,
            encode_path_segment(&self.session_id),
            encode_query_component(&self.token)
        );
        match state.client.post(url).send().await {
            Ok(response) if response.status().is_success() => {
                info!(session_id = %self.session_id, "AI runner ended chat approval session");
            }
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                warn!(
                    session_id = %self.session_id,
                    status = %status,
                    body = %body,
                    "AI runner failed to end chat approval session"
                );
            }
            Err(error) => {
                warn!(session_id = %self.session_id, error = %error, "AI runner failed to end chat approval session");
            }
        }
    }
}

async fn ensure_ai_runner_approval(
    state: &AppState,
    request: &StartJobRequest,
    callback_base: &str,
    cancel: &AtomicBool,
) -> Result<ApprovalDecision> {
    let approval_mode = request.approval_mode.as_deref().unwrap_or("request");
    if approval_mode == "already_granted" {
        post_status(
            state,
            callback_base,
            &request.job_id,
            "running",
            Some("Opening a secure desktop view".to_string()),
            Some(json!({
                "approvalMode": "already_granted",
            })),
            None,
        )
        .await?;
        return Ok(ApprovalDecision::Skipped);
    }

    let approval = request
        .approval
        .as_ref()
        .ok_or_else(|| anyhow!("approval request is required"))?;
    post_status(
        state,
        callback_base,
        &request.job_id,
        "approval_pending",
        Some("Waiting for endpoint approval".to_string()),
        Some(json!({
            "approvalId": approval.approval_id,
            "approvalExpiresAtUnixMs": approval.expires_at_unix_ms,
            "approvalWindowExpiresAtUnixMs": approval.approval_window_expires_at_unix_ms,
        })),
        None,
    )
    .await?;

    let decision =
        request_ai_runner_chat_approval(state, request, approval, callback_base, cancel).await?;
    match decision {
        ApprovalDecision::Approved => {
            post_status(
                state,
                callback_base,
                &request.job_id,
                "approval_granted",
                Some("Endpoint approval granted".to_string()),
                Some(json!({
                    "approvalId": approval.approval_id,
                    "approvalExpiresAtUnixMs": approval.expires_at_unix_ms,
                    "approvalWindowExpiresAtUnixMs": approval.approval_window_expires_at_unix_ms,
                })),
                None,
            )
            .await?;
            post_status(
                state,
                callback_base,
                &request.job_id,
                "running",
                Some("Opening a secure desktop view".to_string()),
                Some(json!({
                    "approvalId": approval.approval_id,
                    "approvalMode": "granted",
                })),
                None,
            )
            .await?;
            Ok(ApprovalDecision::Approved)
        }
        ApprovalDecision::Denied => {
            post_status(
                state,
                callback_base,
                &request.job_id,
                "approval_denied",
                Some("Endpoint approval denied".to_string()),
                Some(json!({
                    "approvalId": approval.approval_id,
                    "approvalExpiresAtUnixMs": approval.expires_at_unix_ms,
                    "approvalWindowExpiresAtUnixMs": approval.approval_window_expires_at_unix_ms,
                })),
                Some("endpoint approval denied".to_string()),
            )
            .await?;
            Ok(ApprovalDecision::Denied)
        }
        ApprovalDecision::Expired => {
            post_status(
                state,
                callback_base,
                &request.job_id,
                "approval_expired",
                Some("Endpoint approval request expired".to_string()),
                Some(json!({
                    "approvalId": approval.approval_id,
                    "approvalExpiresAtUnixMs": approval.expires_at_unix_ms,
                    "approvalWindowExpiresAtUnixMs": approval.approval_window_expires_at_unix_ms,
                })),
                Some("endpoint approval expired".to_string()),
            )
            .await?;
            Ok(ApprovalDecision::Expired)
        }
        ApprovalDecision::Skipped => Ok(ApprovalDecision::Skipped),
    }
}

async fn connect_desktop_session(
    state: &AppState,
    request: &StartJobRequest,
    mode: DesktopConnectMode,
) -> Result<(ConnectResponse, String)> {
    let profile_preference = mode.display_profile_preference();
    debug!(
        job_id = %request.job_id,
        agent_id = %request.agent_id,
        rmm_server_url = %state.config.rmm_server_url,
        desktop_mode = mode.desktop_mode(),
        display_profile_preference = ?profile_preference,
        "AI runner requesting desktop connect"
    );
    let url = format!(
        "{}/api/rmm/internal/ai-runner/devices/{}/connect",
        state.config.rmm_server_url,
        encode_path_segment(request.agent_id.trim())
    );
    let response = state
        .client
        .post(url)
        .header("x-rmm-server-key", &state.config.rmm_server_key)
        .json(&json!({
            "organizationId": request.organization_id,
            "jobId": request.job_id,
            "apiBaseUrl": state.config.rmm_server_url,
            "desktopMode": mode.desktop_mode(),
            "displayProfilePreference": profile_preference,
            "hideCursor": mode == DesktopConnectMode::Interactive,
        }))
        .send()
        .await
        .context("request desktop connect")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("desktop connect failed: {status}: {body}"));
    }
    let connect: ConnectResponse = response
        .json()
        .await
        .context("decode desktop connect response")?;
    let token = extract_query_param(&connect.url, "token")
        .ok_or_else(|| anyhow!("desktop connect response did not include token"))?;
    debug!(
        job_id = %request.job_id,
        session_id = %connect.session_id,
        token_present = !token.is_empty(),
        "AI runner decoded desktop connect response"
    );
    Ok((connect, token))
}

async fn connect_shell_session(
    state: &AppState,
    request: &StartJobRequest,
) -> Result<(ConnectResponse, String)> {
    debug!(
        job_id = %request.job_id,
        agent_id = %request.agent_id,
        "AI runner requesting shell connect"
    );
    let url = format!(
        "{}/api/rmm/internal/ai-runner/devices/{}/connect-shell",
        state.config.rmm_server_url,
        encode_path_segment(request.agent_id.trim())
    );
    let response = state
        .client
        .post(url)
        .header("x-rmm-server-key", &state.config.rmm_server_key)
        .json(&json!({
            "organizationId": request.organization_id,
            "jobId": request.job_id,
            "apiBaseUrl": state.config.rmm_server_url,
            "runAs": "system",
        }))
        .send()
        .await
        .context("request shell connect")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("shell connect failed: {status}: {body}"));
    }
    let connect: ConnectResponse = response
        .json()
        .await
        .context("decode shell connect response")?;
    let token = extract_query_param(&connect.url, "token")
        .ok_or_else(|| anyhow!("shell connect response did not include token"))?;
    Ok((connect, token))
}

async fn get_session_capabilities(
    state: &AppState,
    session_id: &str,
    token: &str,
) -> Result<SessionCapabilities> {
    debug!(session_id = %session_id, "AI runner requesting session capabilities");
    let url = format!(
        "{}/api/rmm/session/{}/capabilities?token={}",
        state.config.rmm_server_url,
        encode_path_segment(session_id),
        encode_query_component(token)
    );
    let response = state
        .client
        .get(url)
        .send()
        .await
        .context("get session capabilities")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("session capabilities failed: {status}: {body}"));
    }
    let capabilities = response
        .json()
        .await
        .context("decode session capabilities")?;
    debug!(session_id = %session_id, "AI runner decoded session capabilities");
    Ok(capabilities)
}

async fn get_shell_session_capabilities(
    state: &AppState,
    session_id: &str,
    token: &str,
) -> Result<SessionCapabilities> {
    debug!(session_id = %session_id, "AI runner requesting shell session capabilities");
    let url = format!(
        "{}/api/rmm/shell/session/{}/capabilities?token={}",
        state.config.rmm_server_url,
        encode_path_segment(session_id),
        encode_query_component(token)
    );
    let response = state
        .client
        .get(url)
        .send()
        .await
        .context("get shell session capabilities")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "shell session capabilities failed: {status}: {body}"
        ));
    }
    response
        .json()
        .await
        .context("decode shell session capabilities")
}

async fn request_relay(state: &AppState, session_id: &str, token: &str) -> Result<()> {
    debug!(session_id = %session_id, "AI runner requesting relay");
    let url = format!(
        "{}/api/rmm/session/{}/request-relay?token={}",
        state.config.rmm_server_url,
        encode_path_segment(session_id),
        encode_query_component(token)
    );
    let response = state
        .client
        .post(url)
        .send()
        .await
        .context("request session relay")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("request relay failed: {status}: {body}"));
    }
    debug!(session_id = %session_id, "AI runner relay requested");
    Ok(())
}

async fn request_shell_relay(state: &AppState, session_id: &str, token: &str) -> Result<()> {
    debug!(session_id = %session_id, "AI runner requesting shell relay");
    let url = format!(
        "{}/api/rmm/shell/session/{}/request-relay?token={}",
        state.config.rmm_server_url,
        encode_path_segment(session_id),
        encode_query_component(token)
    );
    let response = state
        .client
        .post(url)
        .send()
        .await
        .context("request shell relay")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("request shell relay failed: {status}: {body}"));
    }
    Ok(())
}

async fn post_viewer_connected(state: &AppState, session_id: &str, token: &str) -> Result<()> {
    debug!(session_id = %session_id, "AI runner marking viewer connected");
    let url = format!(
        "{}/api/rmm/session/{}/viewer-connected?token={}",
        state.config.rmm_server_url,
        encode_path_segment(session_id),
        encode_query_component(token)
    );
    let response = state
        .client
        .post(url)
        .send()
        .await
        .context("mark viewer connected")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("viewer connected failed: {status}: {body}"));
    }
    debug!(session_id = %session_id, "AI runner viewer-connected notification posted");
    Ok(())
}

async fn post_shell_viewer_connected(
    state: &AppState,
    session_id: &str,
    token: &str,
) -> Result<()> {
    debug!(session_id = %session_id, "AI runner marking shell viewer connected");
    let url = format!(
        "{}/api/rmm/shell/session/{}/viewer-connected?token={}",
        state.config.rmm_server_url,
        encode_path_segment(session_id),
        encode_query_component(token)
    );
    let response = state
        .client
        .post(url)
        .send()
        .await
        .context("mark shell viewer connected")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("shell viewer connected failed: {status}: {body}"));
    }
    Ok(())
}

async fn request_ai_runner_chat_approval(
    state: &AppState,
    request: &StartJobRequest,
    approval: &ApprovalRequest,
    callback_base: &str,
    cancel: &AtomicBool,
) -> Result<ApprovalDecision> {
    ensure_not_cancelled(cancel)?;
    let (connect_response, token) = connect_chat_approval_session(state, request).await?;
    let cleanup = ChatSessionCleanup {
        rmm_base: state.config.rmm_server_url.clone(),
        session_id: connect_response.session_id.clone(),
        token: token.clone(),
    };
    let _ = post_runner_event(
        state,
        callback_base,
        &request.job_id,
        "session_started",
        format!("session_started:chat:{}", connect_response.session_id),
        json!({
            "sessionId": connect_response.session_id.clone(),
            "kind": "chat",
            "agentId": request.agent_id.clone(),
        }),
    )
    .await;
    let result = async {
        ensure_not_cancelled(cancel)?;
        let capabilities =
            get_chat_session_capabilities(state, &connect_response.session_id, &token).await?;
        request_chat_relay(state, &connect_response.session_id, &token).await?;
        wait_for_chat_approval_decision(
            state,
            &connect_response.session_id,
            &token,
            &capabilities,
            approval,
            cancel,
        )
        .await
    }
    .await;
    let chat_session_id = cleanup.session_id.clone();
    cleanup.end(state).await;
    let _ = post_runner_event(
        state,
        callback_base,
        &request.job_id,
        "session_cleanup_finished",
        format!("session_cleanup_finished:chat:{}", chat_session_id),
        json!({
            "sessionId": chat_session_id,
            "kind": "chat",
        }),
    )
    .await;
    result
}

async fn connect_chat_approval_session(
    state: &AppState,
    request: &StartJobRequest,
) -> Result<(ConnectResponse, String)> {
    let url = format!(
        "{}/api/rmm/internal/ai-runner/devices/{}/chat-approval",
        state.config.rmm_server_url,
        encode_path_segment(request.agent_id.trim())
    );
    let response = state
        .client
        .post(url)
        .header("x-rmm-server-key", &state.config.rmm_server_key)
        .json(&json!({
            "organizationId": request.organization_id,
            "jobId": request.job_id,
            "apiBaseUrl": state.config.rmm_server_url,
        }))
        .send()
        .await
        .context("request chat approval session")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("chat approval session failed: {status}: {body}"));
    }
    let connect: ConnectResponse = response
        .json()
        .await
        .context("decode chat approval connect response")?;
    let token = extract_query_param(&connect.url, "token")
        .ok_or_else(|| anyhow!("chat approval response did not include token"))?;
    info!(
        job_id = %request.job_id,
        session_id = %connect.session_id,
        "AI runner chat approval session created"
    );
    Ok((connect, token))
}

async fn get_chat_session_capabilities(
    state: &AppState,
    session_id: &str,
    token: &str,
) -> Result<ChatSessionCapabilitiesHttpResponse> {
    let url = format!(
        "{}/api/rmm/chat/session/{}/capabilities?token={}",
        state.config.rmm_server_url,
        encode_path_segment(session_id),
        encode_query_component(token)
    );
    let response = state
        .client
        .get(url)
        .send()
        .await
        .context("get chat session capabilities")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "chat session capabilities failed: {status}: {body}"
        ));
    }
    response
        .json()
        .await
        .context("decode chat session capabilities")
}

async fn request_chat_relay(state: &AppState, session_id: &str, token: &str) -> Result<()> {
    let url = format!(
        "{}/api/rmm/chat/session/{}/request-relay?token={}",
        state.config.rmm_server_url,
        encode_path_segment(session_id),
        encode_query_component(token)
    );
    let response = state
        .client
        .post(url)
        .send()
        .await
        .context("request chat relay")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("request chat relay failed: {status}: {body}"));
    }
    Ok(())
}

async fn post_chat_viewer_connected(state: &AppState, session_id: &str, token: &str) -> Result<()> {
    let url = format!(
        "{}/api/rmm/chat/session/{}/viewer-connected?token={}",
        state.config.rmm_server_url,
        encode_path_segment(session_id),
        encode_query_component(token)
    );
    let response = state
        .client
        .post(url)
        .send()
        .await
        .context("mark chat viewer connected")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("chat viewer connected failed: {status}: {body}"));
    }
    Ok(())
}

async fn post_chat_viewer_heartbeat(state: &AppState, session_id: &str, token: &str) {
    let url = format!(
        "{}/api/rmm/chat/session/{}/viewer-heartbeat?token={}",
        state.config.rmm_server_url,
        encode_path_segment(session_id),
        encode_query_component(token)
    );
    let _ = state.client.post(url).send().await;
}

async fn wait_for_chat_approval_decision(
    state: &AppState,
    session_id: &str,
    token: &str,
    capabilities: &ChatSessionCapabilitiesHttpResponse,
    approval: &ApprovalRequest,
    cancel: &AtomicBool,
) -> Result<ApprovalDecision> {
    let relay_url = capabilities
        .relay_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("chat approval relay url missing"))?;
    let e2e_key = capabilities
        .e2e_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("chat approval e2e key missing"))?;
    let relay_target = parse_relay_target(relay_url).context("parse chat relay target")?;
    let addr = format!("{}:{}", relay_target.host, relay_target.port);
    let connect_timeout = Duration::from_secs(15);
    let tcp_stream = timeout(connect_timeout, TcpStream::connect(&addr))
        .await
        .context("connect chat relay tcp timed out")?
        .with_context(|| format!("connect chat relay tcp {addr}"))?;
    tcp_stream
        .set_nodelay(true)
        .context("set chat relay tcp nodelay")?;

    let tls_config = build_relay_client_tls_config(
        state.config.relay_ca_path.as_deref(),
        state.config.relay_verify_hostname.as_deref(),
    )
    .context("build chat relay TLS config")?;
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name =
        ServerName::try_from(relay_target.host.clone()).context("build chat relay server name")?;
    let mut stream = timeout(connect_timeout, connector.connect(server_name, tcp_stream))
        .await
        .context("chat relay tls connect timed out")?
        .context("chat relay tls connect")?;

    let request = format!(
        "GET /relay/{session_id} HTTP/1.1\r\nHost: {host}\r\n\r\n",
        host = relay_target.host
    );
    stream
        .write_all(request.as_bytes())
        .await
        .context("write chat relay request")?;
    timeout(connect_timeout, read_http_response(&mut stream))
        .await
        .context("read chat relay response timed out")?
        .context("read chat relay response")?;

    let key_bytes = BASE64_STANDARD
        .decode(e2e_key)
        .or_else(|_| BASE64_URL_SAFE_NO_PAD.decode(e2e_key))
        .context("decode chat relay e2e key")?;
    let cipher = build_e2e_cipher(&key_bytes).context("build chat relay e2e cipher")?;
    let mut send_counter = 0u64;
    write_e2e_frame(&mut stream, &cipher, &mut send_counter, b"hello-world")
        .await
        .context("send chat relay hello")?;
    stream.flush().await.context("flush chat relay hello")?;

    let (mut reader, mut writer) = split(stream);
    let auth_frame =
        build_chat_frame(CHAT_MSG_AUTH, token.as_bytes()).map_err(|error| anyhow!("{error}"))?;
    write_e2e_frame(&mut writer, &cipher, &mut send_counter, &auth_frame)
        .await
        .context("send chat approval auth")?;
    post_chat_viewer_connected(state, session_id, token).await?;

    let request_payload = WorkerChatControlPayload::AiRunnerApprovalRequest {
        approval_id: approval.approval_id.clone(),
        requester_label: approval.requester_label.clone(),
        requester_email: approval.requester_email.clone(),
        organization_name: approval.organization_name.clone(),
        device_label: approval.device_label.clone(),
        reason: approval.reason.clone(),
        expires_at_unix_ms: approval.expires_at_unix_ms,
        approval_window_expires_at_unix_ms: approval.approval_window_expires_at_unix_ms,
    };
    let body = serde_json::to_vec(&request_payload).context("serialize approval request")?;
    let frame = build_chat_frame(CHAT_MSG_CONTROL, &body).map_err(|error| anyhow!("{error}"))?;
    write_e2e_frame(&mut writer, &cipher, &mut send_counter, &frame)
        .await
        .context("send chat approval request")?;
    writer
        .flush()
        .await
        .context("flush chat approval request")?;
    info!(session_id = %session_id, approval_id = %approval.approval_id, "AI runner approval request sent");

    let now = now_unix_ms();
    let expires_in_ms = approval.expires_at_unix_ms.saturating_sub(now);
    let timeout_ms = expires_in_ms.min(state.config.approval_timeout_secs.saturating_mul(1000));
    let approval_timeout = Duration::from_millis(timeout_ms.max(1_000));
    let mut heartbeat = interval(Duration::from_secs(2));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let decision = timeout(approval_timeout, async {
        loop {
            ensure_not_cancelled(cancel)?;
            tokio::select! {
                _ = heartbeat.tick() => {
                    write_e2e_frame(&mut writer, &cipher, &mut send_counter, HEARTBEAT_PAYLOAD)
                        .await
                        .context("send chat approval heartbeat")?;
                    post_chat_viewer_heartbeat(state, session_id, token).await;
                }
                payload = read_e2e_frame_from(&mut reader, &cipher) => {
                    let payload = payload.context("read chat approval relay payload")?;
                    if payload == HEARTBEAT_PAYLOAD || payload == b"hello-world" {
                        continue;
                    }
                    let (message_type, body) = parse_chat_frame(&payload)
                        .map_err(|error| anyhow!("parse chat approval frame: {error}"))?;
                    if let Some(decision) =
                        handle_chat_approval_frame(message_type, body, &approval.approval_id)?
                    {
                        return Ok(decision);
                    }
                }
            }
        }
    })
    .await;

    match decision {
        Ok(result) => result,
        Err(_) => Ok(ApprovalDecision::Expired),
    }
}

fn handle_chat_approval_frame(
    message_type: u8,
    body: &[u8],
    approval_id: &str,
) -> Result<Option<ApprovalDecision>> {
    if message_type == CHAT_MSG_CONTROL {
        let payload: WorkerChatControlPayload =
            serde_json::from_slice(body).context("decode approval control payload")?;
        if let WorkerChatControlPayload::AiRunnerApprovalDecision {
            approval_id: received_id,
            approved,
        } = payload
        {
            if received_id == approval_id {
                return Ok(Some(if approved {
                    ApprovalDecision::Approved
                } else {
                    ApprovalDecision::Denied
                }));
            }
            warn!(
                approval_id,
                received_approval_id = %received_id,
                "AI runner ignored approval decision for another request"
            );
        }
        return Ok(None);
    }
    if message_type == CHAT_MSG_TEXT {
        if let Ok(ChatWirePayload::Message { id, .. }) = serde_json::from_slice(body) {
            debug!(message_id = %id, "AI runner ignored chat text during approval wait");
        }
        return Ok(None);
    }
    if message_type == CHAT_MSG_ERROR {
        if let Ok(payload) = serde_json::from_slice::<ChatWireErrorPayload>(body) {
            if payload.code == OperationErrorCode::NoInteractiveUser {
                return Err(EndpointApprovalUnavailableError::no_interactive_user().into());
            }
            return Err(anyhow!(
                "chat approval endpoint error: {:?}: {}",
                payload.code,
                payload.message
            ));
        }
        let message = String::from_utf8_lossy(body).trim().to_string();
        return Err(anyhow!(
            "chat approval endpoint error: {}",
            if message.is_empty() {
                "unknown error"
            } else {
                message.as_str()
            }
        ));
    }
    if message_type == CHAT_MSG_ACK {
        if let Ok(ack) = serde_json::from_slice::<ChatAckPayload>(body) {
            trace!(message_id = %ack.message_id, "AI runner approval chat ack received");
        }
    }
    Ok(None)
}

async fn capture_screenshot_from_relay(
    state: &AppState,
    session_id: &str,
    capabilities: &SessionCapabilities,
) -> Result<RelayScreenshotRead> {
    let relay_url = capabilities
        .relay_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("session capabilities did not include relayUrl"))?;
    let e2e_key = capabilities
        .e2e_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("session capabilities did not include e2eKey"))?;
    let relay_target = parse_relay_target(relay_url).context("parse relay target")?;
    let addr = format!("{}:{}", relay_target.host, relay_target.port);
    let connect_timeout = Duration::from_secs(15);
    let screenshot_read_timeout =
        Duration::from_secs(state.config.screenshot_read_timeout_secs.max(1));
    debug!(
        session_id = %session_id,
        relay_host = %relay_target.host,
        relay_port = relay_target.port,
        connect_timeout_secs = connect_timeout.as_secs(),
        screenshot_read_timeout_secs = screenshot_read_timeout.as_secs(),
        "AI runner connecting to relay"
    );
    let tcp_stream = timeout(connect_timeout, TcpStream::connect(&addr))
        .await
        .context("connect relay tcp timed out")?
        .with_context(|| format!("connect relay tcp {addr}"))?;
    tcp_stream
        .set_nodelay(true)
        .context("set relay tcp nodelay")?;

    let tls_config = build_relay_client_tls_config(
        state.config.relay_ca_path.as_deref(),
        state.config.relay_verify_hostname.as_deref(),
    )
    .context("build relay TLS config")?;
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name =
        ServerName::try_from(relay_target.host.clone()).context("build relay server name")?;
    let mut stream = timeout(connect_timeout, connector.connect(server_name, tcp_stream))
        .await
        .context("relay tls connect timed out")?
        .context("relay tls connect")?;
    debug!(session_id = %session_id, "AI runner relay TLS connected");

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
        .context("read relay response timed out")?
        .context("read relay response")?;
    debug!(session_id = %session_id, "AI runner relay HTTP response read");

    let key_bytes = BASE64_STANDARD
        .decode(e2e_key)
        .or_else(|_| BASE64_URL_SAFE_NO_PAD.decode(e2e_key))
        .context("decode relay e2e key")?;
    let cipher = build_e2e_cipher(&key_bytes).context("build relay e2e cipher")?;
    let mut send_counter = 0u64;
    write_e2e_frame(&mut stream, &cipher, &mut send_counter, b"hello-world")
        .await
        .context("send relay hello")?;
    stream.flush().await.context("flush relay hello")?;
    debug!(session_id = %session_id, "AI runner relay hello sent");

    let result =
        capture_first_screenshot_from_relay_stream(&mut stream, &cipher, screenshot_read_timeout)
            .await;

    if let Err(error) =
        send_relay_stop_capture(session_id, &mut stream, &cipher, &mut send_counter).await
    {
        warn!(
            session_id = %session_id,
            error = %error,
            "AI runner failed to send relay stop_capture during cleanup"
        );
    }

    tokio::time::sleep(Duration::from_millis(RELAY_STOP_CAPTURE_GRACE_MS)).await;
    if let Err(error) = stream.shutdown().await {
        debug!(
            session_id = %session_id,
            error = %error,
            "AI runner relay stream shutdown during cleanup failed"
        );
    }

    result
}

async fn send_relay_stop_capture<W>(
    session_id: &str,
    writer: &mut W,
    cipher: &chacha20poly1305::ChaCha20Poly1305,
    send_counter: &mut u64,
) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let frame = build_control_frame(CONTROL_TYPE_STOP_CAPTURE, &[])
        .map_err(|error| anyhow!("build stop_capture control frame: {error}"))?;
    write_e2e_frame(writer, cipher, send_counter, &frame)
        .await
        .context("send relay stop_capture")?;
    writer.flush().await.context("flush relay stop_capture")?;
    info!(session_id = %session_id, "AI runner sent relay stop_capture");
    Ok(())
}

async fn capture_first_screenshot_from_relay_stream<R>(
    reader: &mut R,
    cipher: &chacha20poly1305::ChaCha20Poly1305,
    read_timeout: Duration,
) -> Result<RelayScreenshotRead>
where
    R: AsyncRead + Unpin,
{
    let mut assembler = ScreenshotFrameAssembler::default();
    let mut first_payload_bytes = None;
    let mut last_nonfatal_error: Option<String> = None;
    let mut payloads_seen = 0usize;
    let mut ignored_payloads = 0usize;
    let mut candidate_payloads = 0usize;
    let mut parse_errors = 0usize;
    let mut total_payload_bytes = 0usize;
    let mut last_payload_bytes = 0usize;

    debug!(
        max_payloads = MAX_RELAY_SCREENSHOT_PAYLOADS,
        read_timeout_secs = read_timeout.as_secs(),
        "AI runner waiting for screenshot relay payloads"
    );
    for payload_index in 0..MAX_RELAY_SCREENSHOT_PAYLOADS {
        let payload = match timeout(read_timeout, read_e2e_frame_from(reader, cipher)).await {
            Ok(result) => result.context("read encrypted relay screenshot payload")?,
            Err(_) => {
                warn!(
                    payloads_seen,
                    ignored_payloads,
                    candidate_payloads,
                    parse_errors,
                    total_payload_bytes,
                    last_payload_bytes,
                    frame_state = assembler.state_label(),
                    "AI runner timed out waiting for screenshot relay payload"
                );
                return Err(anyhow!(
                    "timed out waiting for screenshot relay payload after {}s ({})",
                    read_timeout.as_secs(),
                    screenshot_wait_stats(
                        payloads_seen,
                        ignored_payloads,
                        candidate_payloads,
                        parse_errors,
                        total_payload_bytes,
                        last_payload_bytes,
                        &assembler,
                        last_nonfatal_error.as_deref(),
                    )
                ));
            }
        };
        payloads_seen = payloads_seen.saturating_add(1);
        total_payload_bytes = total_payload_bytes.saturating_add(payload.len());
        last_payload_bytes = payload.len();
        first_payload_bytes.get_or_insert(payload.len());
        if should_ignore_relay_payload(&payload) {
            ignored_payloads = ignored_payloads.saturating_add(1);
            debug!(
                payload_index,
                payload_bytes = payload.len(),
                ignored_payloads,
                "AI runner ignored relay payload while waiting for screenshot"
            );
            continue;
        }
        candidate_payloads = candidate_payloads.saturating_add(1);
        match assembler.handle_payload(&payload) {
            Ok(Some(artifact)) => {
                debug!(
                    payload_index,
                    payloads_seen,
                    candidate_payloads,
                    frame_id = artifact.frame_id,
                    width = artifact.width,
                    height = artifact.height,
                    payload_bytes = artifact.payload_bytes,
                    png_bytes = artifact.png_bytes,
                    "AI runner assembled screenshot frame"
                );
                return Ok(RelayScreenshotRead {
                    first_payload_bytes,
                    artifact,
                });
            }
            Ok(None) => {
                debug!(
                    payload_index,
                    payload_bytes = payload.len(),
                    frame_state = assembler.state_label(),
                    "AI runner processed partial screenshot relay payload"
                );
            }
            Err(error) => {
                parse_errors = parse_errors.saturating_add(1);
                warn!(
                    payload_index,
                    payload_bytes = payload.len(),
                    parse_errors,
                    error = %error,
                    "AI runner could not use relay payload for screenshot"
                );
                last_nonfatal_error = Some(error.to_string());
            }
        }
    }

    warn!(
        payloads_seen,
        ignored_payloads,
        candidate_payloads,
        parse_errors,
        total_payload_bytes,
        last_payload_bytes,
        frame_state = assembler.state_label(),
        "AI runner exhausted screenshot relay payloads"
    );
    Err(anyhow!(
        "{} ({})",
        last_nonfatal_error.unwrap_or_else(|| {
            format!(
                "no complete screenshot-only display frame found in {MAX_RELAY_SCREENSHOT_PAYLOADS} relay payloads"
            )
        }),
        screenshot_wait_stats(
            payloads_seen,
            ignored_payloads,
            candidate_payloads,
            parse_errors,
            total_payload_bytes,
            last_payload_bytes,
            &assembler,
            None,
        )
    ))
}

fn screenshot_wait_stats(
    payloads_seen: usize,
    ignored_payloads: usize,
    candidate_payloads: usize,
    parse_errors: usize,
    total_payload_bytes: usize,
    last_payload_bytes: usize,
    assembler: &ScreenshotFrameAssembler,
    last_error: Option<&str>,
) -> String {
    let mut stats = format!(
        "payloads_seen={payloads_seen}, ignored_payloads={ignored_payloads}, candidate_payloads={candidate_payloads}, parse_errors={parse_errors}, total_payload_bytes={total_payload_bytes}, last_payload_bytes={last_payload_bytes}, frame_state={}",
        assembler.state_label()
    );
    if let Some(error) = last_error {
        stats.push_str(", last_error=");
        stats.push_str(error);
    }
    stats
}

impl ScreenshotFrameAssembler {
    fn state_label(&self) -> &'static str {
        match (
            self.active_frame_id.is_some(),
            self.pending_artifact.is_some(),
        ) {
            (false, false) => "idle",
            (true, false) => "frame_started",
            (true, true) => "keyframe_received",
            (false, true) => "keyframe_without_active_frame",
        }
    }

    fn handle_payload(&mut self, payload: &[u8]) -> Result<Option<ScreenshotArtifact>> {
        match decode_display_record(payload).context("decode screenshot display record")? {
            DisplayRecord::FrameBegin {
                frame_id,
                width,
                height,
            } => {
                self.active_frame_id = Some(frame_id);
                self.active_width = width;
                self.active_height = height;
                self.pending_artifact = None;
                Ok(None)
            }
            DisplayRecord::Keyframe {
                frame_id,
                width,
                height,
                raw_len,
                payload,
            } => {
                if let Some(active_frame_id) = self.active_frame_id {
                    if active_frame_id != frame_id {
                        return Err(anyhow!(
                            "screenshot keyframe frameId {frame_id} did not match active frame {active_frame_id}"
                        ));
                    }
                } else {
                    self.active_frame_id = Some(frame_id);
                    self.active_width = width;
                    self.active_height = height;
                }
                if self.active_width != width || self.active_height != height {
                    return Err(anyhow!(
                        "screenshot keyframe dimensions changed inside frame {frame_id}"
                    ));
                }
                let artifact =
                    build_screenshot_artifact(frame_id, width, height, raw_len, &payload)?;
                self.active_frame_id = None;
                self.active_width = 0;
                self.active_height = 0;
                self.pending_artifact = None;
                Ok(Some(artifact))
            }
            DisplayRecord::FrameEnd { frame_id } => {
                let active_frame_id = self
                    .active_frame_id
                    .ok_or_else(|| anyhow!("screenshot frame end arrived before frame begin"))?;
                if active_frame_id != frame_id {
                    return Err(anyhow!(
                        "screenshot frame end frameId {frame_id} did not match active frame {active_frame_id}"
                    ));
                }
                let artifact = self
                    .pending_artifact
                    .take()
                    .ok_or_else(|| anyhow!("screenshot frame ended without a keyframe"))?;
                self.active_frame_id = None;
                self.active_width = 0;
                self.active_height = 0;
                Ok(Some(artifact))
            }
            _ => Ok(None),
        }
    }
}

fn build_screenshot_artifact(
    frame_id: u64,
    width: u32,
    height: u32,
    raw_len: u32,
    bgra: &[u8],
) -> Result<ScreenshotArtifact> {
    debug!(
        frame_id,
        width,
        height,
        raw_len,
        payload_bytes = bgra.len(),
        "AI runner building screenshot artifact"
    );
    let expected = screenshot_bgra_len(width, height)?;
    if raw_len as usize != expected || bgra.len() != expected {
        return Err(anyhow!(
            "screenshot keyframe payload length mismatch: rawLen={}, payload={}, expected={}",
            raw_len,
            bgra.len(),
            expected
        ));
    }
    let (base64_content, png_bytes) = encode_bgra_png_base64(width, height, bgra)?;
    debug!(
        frame_id,
        png_bytes,
        base64_chars = base64_content.len(),
        "AI runner encoded screenshot PNG"
    );
    if base64_content.len() > MAX_SCREENSHOT_ARTIFACT_BASE64_CHARS {
        return Err(anyhow!(
            "screenshot PNG artifact is too large for callback: {} base64 chars",
            base64_content.len()
        ));
    }
    Ok(ScreenshotArtifact {
        frame_id,
        width,
        height,
        payload_bytes: bgra.len(),
        png_bytes,
        base64_content,
    })
}

fn screenshot_bgra_len(width: u32, height: u32) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(anyhow!("screenshot dimensions are empty"));
    }
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow!("screenshot dimensions overflow"))
}

fn encode_bgra_png_base64(width: u32, height: u32, bgra: &[u8]) -> Result<(String, usize)> {
    let expected = screenshot_bgra_len(width, height)?;
    if bgra.len() != expected {
        return Err(anyhow!("screenshot BGRA length does not match dimensions"));
    }

    let mut rgba = Vec::with_capacity(expected);
    for pixel in bgra.chunks_exact(4) {
        rgba.push(pixel[2]);
        rgba.push(pixel[1]);
        rgba.push(pixel[0]);
        rgba.push(pixel[3]);
    }

    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .context("write screenshot PNG header")?;
        writer
            .write_image_data(&rgba)
            .context("write screenshot PNG data")?;
    }
    let png_len = png_bytes.len();
    Ok((BASE64_STANDARD.encode(png_bytes), png_len))
}

fn should_ignore_relay_payload(payload: &[u8]) -> bool {
    payload == b"hello-world" || payload == HEARTBEAT_PAYLOAD || is_rmmd_metadata_payload(payload)
}

fn is_rmmd_metadata_payload(payload: &[u8]) -> bool {
    if payload.len() < 8 || payload.get(0..4) != Some(b"RMMD") {
        return false;
    }
    let json_len = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
    payload.len() == 8 + json_len
}

fn shell_reference_variable_name(shell_reference: &str) -> Result<&str> {
    let reference = shell_reference.trim();
    let variable = reference
        .strip_prefix('$')
        .ok_or_else(|| anyhow!("generated shell reference must start with $"))?;
    if variable.is_empty()
        || !variable
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(anyhow!(
            "generated shell reference is not a safe variable name"
        ));
    }
    Ok(variable)
}

fn platform_is_windows(platform: Option<&str>) -> bool {
    platform
        .unwrap_or_default()
        .to_ascii_lowercase()
        .contains("windows")
}

fn build_shell_secret_materialization_setup(
    shell_reference: &str,
    platform: Option<&str>,
) -> Result<String> {
    let variable = shell_reference_variable_name(shell_reference)?;
    if platform_is_windows(platform) {
        Ok(format!("${variable} = Read-Host -AsSecureString\r"))
    } else {
        Ok(format!(
            "stty -echo; IFS= read -r {variable}; stty echo; printf '\\n'\r"
        ))
    }
}

#[derive(Debug, Clone)]
struct ShellCommandMarkers {
    start: String,
    end: String,
}

#[derive(Debug, Clone)]
struct ShellCommandExecution {
    output: String,
    exit_code: Option<i32>,
}

struct ActiveShellCommand {
    before_len: usize,
    markers: ShellCommandMarkers,
    started_at: tokio::time::Instant,
    last_streamed_terminal_len: usize,
}

#[derive(Debug, Clone)]
struct ShellCommandCheckpoint {
    output: String,
    elapsed_ms: u64,
    remaining_ms: u64,
}

#[derive(Debug, Clone)]
enum ShellCommandWaitOutcome {
    Completed(ShellCommandExecution),
    Checkpoint(ShellCommandCheckpoint),
}

struct ShellCommandOutputSink<'a> {
    state: &'a AppState,
    callback_base: &'a str,
    request: &'a StartJobRequest,
    approval_id: &'a str,
    turn_index: u32,
    redacted_secret_values: &'a [String],
    sequence: u64,
    output_offset: usize,
}

impl<'a> ShellCommandOutputSink<'a> {
    async fn publish(&mut self, text: &str, terminal: bool) {
        let redacted = redact_generated_secrets(text, self.redacted_secret_values);
        if redacted.is_empty() && !terminal {
            return;
        }
        let sequence = self.sequence;
        self.sequence = self.sequence.saturating_add(1);
        let offset = self.output_offset;
        self.output_offset = self.output_offset.saturating_add(redacted.chars().count());
        if let Err(error) = post_command_output_delta(
            self.state,
            self.callback_base,
            self.request,
            self.approval_id,
            self.turn_index,
            sequence,
            offset,
            &redacted,
            terminal,
        )
        .await
        {
            warn!(
                job_id = %self.request.job_id,
                approval_id = %self.approval_id,
                sequence,
                error = %error,
                "AI runner command output delta callback failed"
            );
        }
    }
}

fn safe_marker_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "command".to_string()
    } else {
        out
    }
}

fn shell_command_markers(approval_id: &str, turn_index: u32) -> ShellCommandMarkers {
    let id = format!(
        "{}_{}_{}",
        turn_index,
        now_unix_ms(),
        safe_marker_component(approval_id)
    );
    ShellCommandMarkers {
        start: format!("__TALOS_CMD_START_{id}__"),
        end: format!("__TALOS_CMD_END_{id}__"),
    }
}

fn build_marked_shell_command(
    command: &str,
    markers: &ShellCommandMarkers,
    platform: Option<&str>,
) -> String {
    if platform_is_windows(platform) {
        format!(
            "$__talosExit = 0\r\n$global:LASTEXITCODE = $null\r\nWrite-Output \"{start}\"\r\ntry {{\r\n{command}\r\nif (-not $?) {{ $__talosExit = if ($global:LASTEXITCODE -ne $null) {{ [int]$global:LASTEXITCODE }} else {{ 1 }} }} elseif ($global:LASTEXITCODE -ne $null) {{ $__talosExit = [int]$global:LASTEXITCODE }} else {{ $__talosExit = 0 }}\r\n}} catch {{\r\nWrite-Error $_\r\n$__talosExit = 1\r\n}}\r\nWrite-Output \"{end}:$__talosExit\"\r\n",
            start = markers.start,
            command = command.trim_end(),
            end = markers.end,
        )
    } else {
        format!(
            "printf '\\n%s\\n' '{start}'\n{command}\n__talos_exit=$?\nprintf '\\n%s:%s\\n' '{end}' \"$__talos_exit\"\n",
            start = markers.start,
            command = command.trim_end(),
            end = markers.end,
        )
    }
}

fn line_is_shell_runner_internal(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("__talos_cmd_")
        || lower.contains("__talos_exit")
        || lower.contains("__talosexit")
}

fn clean_shell_runner_transcript(value: &str) -> String {
    strip_shell_control_sequences(value)
        .lines()
        .filter(|line| !line.is_empty() && !line_is_shell_runner_internal(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn clean_shell_command_output(raw: &str, markers: &ShellCommandMarkers) -> String {
    let cleaned = strip_shell_control_sequences(raw);
    let mut inside = false;
    let mut output = Vec::new();
    for line in cleaned.lines() {
        if line.contains(&markers.start) {
            inside = true;
            continue;
        }
        if line.contains(&markers.end) {
            break;
        }
        if !inside
            || line.is_empty()
            || line_is_shell_runner_internal(line)
            || line_is_shell_runner_wrapper_echo(line)
        {
            continue;
        }
        output.push(line);
    }
    output.join("\n")
}

fn shell_prompt_echo_body(line: &str) -> (&str, bool) {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    if trimmed.starts_with(">>") {
        return (trimmed.trim_start_matches('>').trim_start(), true);
    }
    if lower.starts_with("ps ") {
        if let Some(index) = trimmed.find('>') {
            return (trimmed[index + 1..].trim_start(), true);
        }
    }
    (trimmed, false)
}

fn line_is_shell_runner_wrapper_echo(line: &str) -> bool {
    let (body, prompt_echo) = shell_prompt_echo_body(line);
    let lower = body.to_ascii_lowercase();
    if prompt_echo && body.is_empty() {
        return true;
    }
    if lower.contains("__talos_cmd_")
        || lower.contains("__talos_exit")
        || lower.contains("__talosexit")
        || lower.contains("$__talosexit")
    {
        return true;
    }
    if lower == "$global:lastexitcode = $null" {
        return true;
    }
    if prompt_echo
        && (lower == "write-error $_" || lower == "try {" || lower == "} catch {" || lower == "}")
    {
        return true;
    }
    false
}

fn terminal_shell_command_output(raw: &str, markers: &ShellCommandMarkers) -> String {
    let mut inside = false;
    let mut output = String::new();
    for segment in raw.split_inclusive('\n') {
        let cleaned_segment = strip_shell_control_sequences(segment);
        if cleaned_segment.contains(&markers.start) {
            inside = true;
            continue;
        }
        if cleaned_segment.contains(&markers.end) {
            break;
        }
        if !inside {
            continue;
        }
        if line_is_shell_runner_internal(&cleaned_segment)
            || line_is_shell_runner_wrapper_echo(&cleaned_segment)
        {
            continue;
        }
        output.push_str(segment);
    }
    output
}

fn shell_command_exit_code(raw: &str, markers: &ShellCommandMarkers) -> Option<i32> {
    let cleaned = strip_shell_control_sequences(raw);
    let marker = format!("{}:", markers.end);
    let index = cleaned.rfind(&marker)?;
    let after = &cleaned[index + marker.len()..];
    let digits = after
        .trim_start()
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
        .collect::<String>();
    if digits.is_empty() {
        None
    } else {
        digits.parse::<i32>().ok()
    }
}

fn shell_command_has_end_marker(raw: &str, markers: &ShellCommandMarkers) -> bool {
    strip_shell_control_sequences(raw).contains(&format!("{}:", markers.end))
}

struct ShellRelaySession {
    write_tx: mpsc::UnboundedSender<Vec<u8>>,
    prompt_transcript: Arc<Mutex<String>>,
    full_transcript: Arc<Mutex<String>>,
    notify: Arc<Notify>,
    exit_code: Arc<Mutex<Option<u32>>>,
    reader_task: tokio::task::JoinHandle<()>,
    writer_task: tokio::task::JoinHandle<()>,
}

impl ShellRelaySession {
    async fn connect(
        state: &AppState,
        session_id: &str,
        token: &str,
        capabilities: &SessionCapabilities,
    ) -> Result<Self> {
        let relay_url = capabilities
            .relay_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("shell capabilities did not include relayUrl"))?;
        let e2e_key = capabilities
            .e2e_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("shell capabilities did not include e2eKey"))?;
        let relay_target = parse_relay_target(relay_url).context("parse shell relay target")?;
        let addr = format!("{}:{}", relay_target.host, relay_target.port);
        let connect_timeout = Duration::from_secs(LIVE_RELAY_CONNECT_TIMEOUT_SECS);
        let tcp_stream = timeout(connect_timeout, TcpStream::connect(&addr))
            .await
            .context("connect shell relay tcp timed out")?
            .with_context(|| format!("connect shell relay tcp {addr}"))?;
        tcp_stream
            .set_nodelay(true)
            .context("set shell relay tcp nodelay")?;

        let tls_config = build_relay_client_tls_config(
            state.config.relay_ca_path.as_deref(),
            state.config.relay_verify_hostname.as_deref(),
        )
        .context("build shell relay TLS config")?;
        let connector = TlsConnector::from(Arc::new(tls_config));
        let server_name = ServerName::try_from(relay_target.host.clone())
            .context("build shell relay server name")?;
        let mut stream = timeout(connect_timeout, connector.connect(server_name, tcp_stream))
            .await
            .context("shell relay tls connect timed out")?
            .context("shell relay tls connect")?;

        let request = format!(
            "GET /relay/{session_id} HTTP/1.1\r\nHost: {host}\r\n\r\n",
            host = relay_target.host
        );
        stream
            .write_all(request.as_bytes())
            .await
            .context("write shell relay request")?;
        timeout(connect_timeout, read_http_response(&mut stream))
            .await
            .context("read shell relay response timed out")?
            .context("read shell relay response")?;

        let key_bytes = BASE64_STANDARD
            .decode(e2e_key)
            .or_else(|_| BASE64_URL_SAFE_NO_PAD.decode(e2e_key))
            .context("decode shell relay e2e key")?;
        let cipher = build_e2e_cipher(&key_bytes).context("build shell relay e2e cipher")?;
        let mut send_counter = 0u64;
        write_e2e_frame(&mut stream, &cipher, &mut send_counter, b"hello-world")
            .await
            .context("send shell relay hello")?;
        stream.flush().await.context("flush shell relay hello")?;

        let (reader, writer) = split(stream);
        let writer_cipher = build_e2e_cipher(&key_bytes).context("build shell writer cipher")?;
        let (write_tx, write_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let prompt_transcript = Arc::new(Mutex::new(String::new()));
        let full_transcript = Arc::new(Mutex::new(String::new()));
        let notify = Arc::new(Notify::new());
        let exit_code = Arc::new(Mutex::new(None));
        let reader_task = tokio::spawn(run_shell_relay_reader(
            session_id.to_string(),
            reader,
            cipher,
            Arc::clone(&prompt_transcript),
            Arc::clone(&full_transcript),
            Arc::clone(&notify),
            Arc::clone(&exit_code),
        ));
        let writer_task = tokio::spawn(run_live_relay_writer(
            session_id.to_string(),
            writer,
            writer_cipher,
            send_counter + 1,
            write_rx,
        ));
        let auth_frame = build_shell_frame(SHELL_MSG_AUTH, token.as_bytes())
            .map_err(|error| anyhow!("{error}"))?;
        write_tx
            .send(auth_frame)
            .map_err(|_| anyhow!("shell relay writer closed before auth"))?;
        Ok(Self {
            write_tx,
            prompt_transcript,
            full_transcript,
            notify,
            exit_code,
            reader_task,
            writer_task,
        })
    }

    async fn transcript(&self) -> String {
        let transcript = self.prompt_transcript.lock().await.clone();
        truncate_from_end(
            &clean_shell_runner_transcript(&transcript),
            SHELL_TRANSCRIPT_MAX_CHARS,
        )
    }

    async fn full_transcript(&self) -> String {
        let transcript = self.full_transcript.lock().await.clone();
        clean_shell_runner_transcript(&transcript)
    }

    fn send_input(&self, input: &str) -> Result<()> {
        let frame = build_shell_frame(SHELL_MSG_INPUT, input.as_bytes())
            .map_err(|error| anyhow!("{error}"))?;
        self.write_tx
            .send(frame)
            .map_err(|_| anyhow!("shell relay writer closed"))
    }

    async fn start_command(
        &self,
        command: &str,
        platform: Option<&str>,
        output_sink: &mut ShellCommandOutputSink<'_>,
    ) -> Result<ActiveShellCommand> {
        let before_len = self.full_transcript.lock().await.len();
        let markers = shell_command_markers(output_sink.approval_id, output_sink.turn_index);
        let command_text = build_marked_shell_command(command, &markers, platform);
        self.send_input(&command_text)?;
        Ok(ActiveShellCommand {
            before_len,
            markers,
            started_at: tokio::time::Instant::now(),
            last_streamed_terminal_len: 0,
        })
    }

    async fn send_interrupt(&self) -> Result<()> {
        self.send_input("\x03")
    }

    async fn materialize_generated_secret(
        &self,
        shell_reference: &str,
        secret: &str,
        platform: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let setup = build_shell_secret_materialization_setup(shell_reference, platform)?;
        let before_len = self.full_transcript.lock().await.len();
        self.send_input(&setup)?;
        cancellable_sleep(Duration::from_millis(250), cancel).await?;
        self.send_input(&format!("{secret}\r"))?;
        let _ = self.wait_for_output_idle(before_len, cancel).await?;
        Ok(())
    }

    async fn wait_for_output_idle(&self, before_len: usize, cancel: &AtomicBool) -> Result<String> {
        let started_at = tokio::time::Instant::now();
        let mut last_len = self.full_transcript.lock().await.len();
        let mut last_changed_at = tokio::time::Instant::now();
        loop {
            ensure_not_cancelled(cancel)?;
            if self.exit_code.lock().await.is_some() {
                break;
            }
            let notified = timeout(Duration::from_millis(250), self.notify.notified()).await;
            let current_len = self.full_transcript.lock().await.len();
            if current_len != last_len {
                last_len = current_len;
                last_changed_at = tokio::time::Instant::now();
            }
            if current_len > before_len && last_changed_at.elapsed() >= Duration::from_millis(1_200)
            {
                break;
            }
            if started_at.elapsed() >= Duration::from_secs(60) {
                break;
            }
            if notified.is_err() {
                continue;
            }
        }
        let transcript = self.full_transcript.lock().await.clone();
        let delta = transcript.get(before_len..).unwrap_or_default();
        Ok(truncate_from_end(
            &strip_shell_control_sequences(delta).trim().to_string(),
            4_000,
        ))
    }

    async fn wait_for_command_checkpoint(
        &self,
        active: &mut ActiveShellCommand,
        wait_ms: u64,
        max_wait_secs: u64,
        cancel: &AtomicBool,
        output_sink: &mut ShellCommandOutputSink<'_>,
    ) -> Result<ShellCommandWaitOutcome> {
        let timeout_duration = Duration::from_secs(max_wait_secs.max(1));
        let wait_duration = Duration::from_millis(clamp_shell_command_wait_ms(wait_ms));
        let wait_deadline = tokio::time::Instant::now() + wait_duration;
        loop {
            ensure_not_cancelled(cancel)?;
            let transcript = self.full_transcript.lock().await.clone();
            let raw_delta = transcript.get(active.before_len..).unwrap_or_default();
            let visible_output = clean_shell_command_output(raw_delta, &active.markers);
            let terminal_output = terminal_shell_command_output(raw_delta, &active.markers);
            if terminal_output.len() > active.last_streamed_terminal_len {
                let pending = &terminal_output[active.last_streamed_terminal_len..];
                for chunk in chunk_text_by_chars(pending, SHELL_COMMAND_OUTPUT_CHUNK_CHARS) {
                    output_sink.publish(&chunk, false).await;
                }
                active.last_streamed_terminal_len = terminal_output.len();
            }
            if shell_command_has_end_marker(raw_delta, &active.markers) {
                let exit_code = shell_command_exit_code(raw_delta, &active.markers);
                output_sink.publish("", true).await;
                return Ok(ShellCommandWaitOutcome::Completed(ShellCommandExecution {
                    output: truncate_from_end(visible_output.trim(), 4_000),
                    exit_code,
                }));
            }
            if let Some(code) = *self.exit_code.lock().await {
                output_sink.publish("", true).await;
                return Err(anyhow!(
                    "shell session exited with code {code} before command completion marker"
                ));
            }
            let elapsed = active.started_at.elapsed();
            if elapsed >= timeout_duration {
                output_sink.publish("", true).await;
                return Err(anyhow!(
                    "shell command timed out after {} seconds",
                    timeout_duration.as_secs()
                ));
            }
            if tokio::time::Instant::now() >= wait_deadline {
                let elapsed_ms = duration_ms_u64(elapsed);
                let timeout_ms = duration_ms_u64(timeout_duration);
                return Ok(ShellCommandWaitOutcome::Checkpoint(
                    ShellCommandCheckpoint {
                        output: truncate_from_end(visible_output.trim(), 4_000),
                        elapsed_ms,
                        remaining_ms: timeout_ms.saturating_sub(elapsed_ms),
                    },
                ));
            }
            let wait_remaining =
                wait_deadline.saturating_duration_since(tokio::time::Instant::now());
            let flush_wait =
                wait_remaining.min(Duration::from_millis(SHELL_COMMAND_OUTPUT_FLUSH_MS));
            let _ = timeout(flush_wait, self.notify.notified()).await;
        }
    }

    async fn shutdown(self, session_id: &str) {
        self.reader_task.abort();
        self.writer_task.abort();
        info!(session_id, "AI runner shell relay shutdown requested");
    }
}

async fn run_shell_relay_reader<R>(
    session_id: String,
    mut reader: R,
    cipher: chacha20poly1305::ChaCha20Poly1305,
    prompt_transcript: Arc<Mutex<String>>,
    full_transcript: Arc<Mutex<String>>,
    notify: Arc<Notify>,
    exit_code: Arc<Mutex<Option<u32>>>,
) where
    R: AsyncRead + Unpin,
{
    loop {
        let payload = match read_e2e_frame_from(&mut reader, &cipher).await {
            Ok(payload) => payload,
            Err(error) => {
                warn!(session_id = %session_id, error = %error, "AI runner shell relay read failed");
                break;
            }
        };
        if should_ignore_relay_payload(&payload) {
            continue;
        }
        let Ok((message_type, frame_payload)) = parse_shell_wire_frame(&payload) else {
            warn!(session_id = %session_id, "AI runner ignored invalid shell frame");
            continue;
        };
        match message_type {
            SHELL_MSG_OUTPUT => {
                let text = String::from_utf8_lossy(frame_payload);
                {
                    let mut guard = full_transcript.lock().await;
                    guard.push_str(&text);
                }
                {
                    let mut guard = prompt_transcript.lock().await;
                    guard.push_str(&text);
                    if guard.len() > SHELL_TRANSCRIPT_MAX_CHARS * 2 {
                        let truncated =
                            truncate_from_end(guard.as_str(), SHELL_TRANSCRIPT_MAX_CHARS);
                        *guard = truncated;
                    }
                }
                notify.notify_waiters();
            }
            SHELL_MSG_EXIT => {
                let code = parse_shell_exit_payload(frame_payload).unwrap_or(0);
                *exit_code.lock().await = Some(code);
                notify.notify_waiters();
                break;
            }
            SHELL_MSG_ERROR => {
                let text = String::from_utf8_lossy(frame_payload);
                let error_text = format!("\n[shell error] {text}");
                {
                    let mut guard = full_transcript.lock().await;
                    guard.push_str(&error_text);
                }
                {
                    let mut guard = prompt_transcript.lock().await;
                    guard.push_str(&error_text);
                    if guard.len() > SHELL_TRANSCRIPT_MAX_CHARS * 2 {
                        let truncated =
                            truncate_from_end(guard.as_str(), SHELL_TRANSCRIPT_MAX_CHARS);
                        *guard = truncated;
                    }
                }
                notify.notify_waiters();
                break;
            }
            _ => {}
        }
    }
}

impl Drop for ShellRelaySession {
    fn drop(&mut self) {
        self.reader_task.abort();
        self.writer_task.abort();
    }
}

async fn post_shell_transcript_artifacts(
    state: &AppState,
    callback_base: &str,
    request: &StartJobRequest,
    session_id: &str,
    shell_session: &ShellRelaySession,
    redacted_secret_values: &[String],
) -> Result<usize> {
    let transcript = shell_session.full_transcript().await;
    let redacted = redact_generated_secrets(&transcript, redacted_secret_values);
    if redacted.trim().is_empty() {
        return Ok(0);
    }

    let chunks = chunk_text_by_chars(&redacted, SHELL_TRANSCRIPT_CHUNK_CHARS);
    let total_chunks = chunks.len();
    let transcript_chars = redacted.chars().count();
    for (index, chunk) in chunks.into_iter().enumerate() {
        let sequence = index + 1;
        let name = if total_chunks == 1 {
            format!("shell-transcript-{}.txt", request.job_id)
        } else {
            format!(
                "shell-transcript-{}-part-{}-of-{}.txt",
                request.job_id, sequence, total_chunks
            )
        };
        post_artifact(
            state,
            callback_base,
            &request.job_id,
            SHELL_TRANSCRIPT_ARTIFACT_TYPE,
            &name,
            "text/plain; charset=utf-8",
            BASE64_STANDARD.encode(chunk.as_bytes()),
            json!({
                "jobId": request.job_id,
                "agentId": request.agent_id,
                "sessionId": session_id,
                "source": "shell_relay_stream",
                "sequence": sequence,
                "totalChunks": total_chunks,
                "redacted": true,
                "controlSequencesStripped": true,
                "transcriptChars": transcript_chars,
                "chunkChars": chunk.chars().count(),
                "createdAtUnixMs": now_unix_ms(),
            }),
            false,
            None,
            None,
        )
        .await?;
    }

    Ok(total_chunks)
}

fn parse_shell_wire_frame(frame: &[u8]) -> Result<(u8, &[u8])> {
    if frame.len() < 3 {
        return Err(anyhow!("shell frame too short"));
    }
    let message_type = frame[0];
    let payload_len = u16::from_be_bytes([frame[1], frame[2]]) as usize;
    if frame.len() != 3 + payload_len {
        return Err(anyhow!("shell frame length mismatch"));
    }
    Ok((message_type, &frame[3..]))
}

fn strip_shell_control_sequences(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.peek() == Some(&'[') {
                let _ = chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        if ch == '\r' {
            output.push('\n');
        } else if ch != '\u{0007}' {
            output.push(ch);
        }
    }
    output
}

fn truncate_from_end(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut chars = value.chars().rev().take(max_chars).collect::<Vec<_>>();
    chars.reverse();
    chars.into_iter().collect()
}

fn chunk_text_by_chars(value: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut count = 0usize;
    for (index, _) in value.char_indices() {
        if count == max_chars {
            chunks.push(value[start..index].to_string());
            start = index;
            count = 0;
        }
        count += 1;
    }
    if start < value.len() {
        chunks.push(value[start..].to_string());
    }
    chunks
}

struct LiveRelaySession {
    control_tx: mpsc::UnboundedSender<Vec<u8>>,
    latest_frame: Arc<Mutex<Option<LiveFrame>>>,
    notify: Arc<Notify>,
    reader_task: tokio::task::JoinHandle<()>,
    writer_task: tokio::task::JoinHandle<()>,
}

#[derive(Debug, Clone)]
struct LiveFrameObservation {
    frame: LiveFrame,
    updated: bool,
}

impl LiveRelaySession {
    async fn connect(
        state: &AppState,
        session_id: &str,
        capabilities: &SessionCapabilities,
    ) -> Result<Self> {
        let relay_url = capabilities
            .relay_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("session capabilities did not include relayUrl"))?;
        let e2e_key = capabilities
            .e2e_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("session capabilities did not include e2eKey"))?;
        let relay_target = parse_relay_target(relay_url).context("parse relay target")?;
        let addr = format!("{}:{}", relay_target.host, relay_target.port);
        let connect_timeout = Duration::from_secs(LIVE_RELAY_CONNECT_TIMEOUT_SECS);
        debug!(
            session_id,
            relay_host = %relay_target.host,
            relay_port = relay_target.port,
            "AI runner connecting live relay"
        );
        let tcp_stream = timeout(connect_timeout, TcpStream::connect(&addr))
            .await
            .context("connect live relay tcp timed out")?
            .with_context(|| format!("connect live relay tcp {addr}"))?;
        tcp_stream
            .set_nodelay(true)
            .context("set live relay tcp nodelay")?;

        let tls_config = build_relay_client_tls_config(
            state.config.relay_ca_path.as_deref(),
            state.config.relay_verify_hostname.as_deref(),
        )
        .context("build live relay TLS config")?;
        let connector = TlsConnector::from(Arc::new(tls_config));
        let server_name = ServerName::try_from(relay_target.host.clone())
            .context("build live relay server name")?;
        let mut stream = timeout(connect_timeout, connector.connect(server_name, tcp_stream))
            .await
            .context("live relay tls connect timed out")?
            .context("live relay tls connect")?;

        let request = format!(
            "GET /relay/{session_id} HTTP/1.1\r\nHost: {host}\r\n\r\n",
            host = relay_target.host
        );
        stream
            .write_all(request.as_bytes())
            .await
            .context("write live relay request")?;
        timeout(connect_timeout, read_http_response(&mut stream))
            .await
            .context("read live relay response timed out")?
            .context("read live relay response")?;

        let key_bytes = BASE64_STANDARD
            .decode(e2e_key)
            .or_else(|_| BASE64_URL_SAFE_NO_PAD.decode(e2e_key))
            .context("decode live relay e2e key")?;
        let cipher = build_e2e_cipher(&key_bytes).context("build live relay e2e cipher")?;
        let mut send_counter = 0u64;
        write_e2e_frame(&mut stream, &cipher, &mut send_counter, b"hello-world")
            .await
            .context("send live relay hello")?;
        stream.flush().await.context("flush live relay hello")?;

        let (reader, writer) = split(stream);
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let latest_frame = Arc::new(Mutex::new(None));
        let notify = Arc::new(Notify::new());
        let reader_task = tokio::spawn(run_live_relay_reader(
            session_id.to_string(),
            reader,
            cipher.clone(),
            Arc::clone(&latest_frame),
            Arc::clone(&notify),
        ));
        let writer_task = tokio::spawn(run_live_relay_writer(
            session_id.to_string(),
            writer,
            cipher,
            send_counter,
            control_rx,
        ));
        Ok(Self {
            control_tx,
            latest_frame,
            notify,
            reader_task,
            writer_task,
        })
    }

    async fn wait_for_frame_after(
        &self,
        min_seq: u64,
        read_timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<LiveFrame> {
        timeout(read_timeout, async {
            loop {
                ensure_not_cancelled(cancel)?;
                let notified = self.notify.notified();
                if let Some(frame) = self.latest_frame.lock().await.clone() {
                    if frame.seq > min_seq {
                        return Ok(frame);
                    }
                }
                tokio::select! {
                    _ = notified => {}
                    _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                }
            }
        })
        .await
        .map_err(|_| {
            anyhow!(
                "timed out waiting for live desktop frame after {}s",
                read_timeout.as_secs()
            )
        })?
    }

    async fn wait_for_frame_after_or_latest(
        &self,
        min_seq: u64,
        read_timeout: Duration,
        cancel: &AtomicBool,
    ) -> Result<LiveFrameObservation> {
        match timeout(read_timeout, async {
            loop {
                ensure_not_cancelled(cancel)?;
                let notified = self.notify.notified();
                if let Some(frame) = self.latest_frame.lock().await.clone() {
                    if frame.seq > min_seq {
                        return Ok(frame);
                    }
                }
                tokio::select! {
                    _ = notified => {}
                    _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                }
            }
        })
        .await
        {
            Ok(frame) => frame.map(|frame| LiveFrameObservation {
                frame,
                updated: true,
            }),
            Err(_) => {
                ensure_not_cancelled(cancel)?;
                let frame = self.latest_frame.lock().await.clone().ok_or_else(|| {
                    anyhow!("no live desktop frame is available after unchanged wait")
                })?;
                Ok(LiveFrameObservation {
                    updated: frame.seq > min_seq,
                    frame,
                })
            }
        }
    }

    async fn send_control_frame(&self, frame: Vec<u8>) -> Result<()> {
        self.control_tx
            .send(frame)
            .map_err(|_| anyhow!("live relay control writer is closed"))
    }

    async fn shutdown(&self, session_id: &str) {
        match build_control_frame(CONTROL_TYPE_STOP_CAPTURE, &[]) {
            Ok(frame) => {
                if let Err(error) = self.send_control_frame(frame).await {
                    warn!(
                        session_id,
                        error = %error,
                        "AI runner failed to queue live relay stop_capture"
                    );
                }
            }
            Err(error) => {
                warn!(session_id, error = %error, "AI runner failed to build live relay stop_capture");
            }
        }
        tokio::time::sleep(Duration::from_millis(RELAY_STOP_CAPTURE_GRACE_MS)).await;
        self.reader_task.abort();
        self.writer_task.abort();
        info!(session_id, "AI runner live relay shutdown requested");
    }
}

async fn run_live_relay_writer<W>(
    session_id: String,
    mut writer: W,
    cipher: chacha20poly1305::ChaCha20Poly1305,
    mut send_counter: u64,
    mut control_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) where
    W: AsyncWrite + Unpin,
{
    let mut heartbeat = interval(Duration::from_secs(LIVE_RELAY_HEARTBEAT_SECS));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if let Err(error) = write_e2e_frame(&mut writer, &cipher, &mut send_counter, HEARTBEAT_PAYLOAD).await {
                    warn!(session_id = %session_id, error = %error, "AI runner live relay heartbeat write failed");
                    break;
                }
                if let Err(error) = writer.flush().await {
                    warn!(session_id = %session_id, error = %error, "AI runner live relay heartbeat flush failed");
                    break;
                }
            }
            Some(frame) = control_rx.recv() => {
                if let Err(error) = write_e2e_frame(&mut writer, &cipher, &mut send_counter, &frame).await {
                    warn!(session_id = %session_id, error = %error, "AI runner live relay control write failed");
                    break;
                }
                if let Err(error) = writer.flush().await {
                    warn!(session_id = %session_id, error = %error, "AI runner live relay control flush failed");
                    break;
                }
            }
            else => break,
        }
    }
}

async fn run_live_relay_reader<R>(
    session_id: String,
    mut reader: R,
    cipher: chacha20poly1305::ChaCha20Poly1305,
    latest_frame: Arc<Mutex<Option<LiveFrame>>>,
    notify: Arc<Notify>,
) where
    R: AsyncRead + Unpin,
{
    let mut decoder = LiveVp8StreamDecoder::default();
    loop {
        let payload = match read_e2e_frame_from(&mut reader, &cipher).await {
            Ok(payload) => payload,
            Err(error) => {
                warn!(session_id = %session_id, error = %error, "AI runner live relay read failed");
                break;
            }
        };
        if should_ignore_relay_payload(&payload) {
            continue;
        }
        match decoder.handle_payload(&payload) {
            Ok(Some(frame)) => {
                debug!(
                    session_id = %session_id,
                    frame_seq = frame.seq,
                    width = frame.width,
                    height = frame.height,
                    "AI runner live relay decoded frame"
                );
                *latest_frame.lock().await = Some(frame);
                notify.notify_waiters();
            }
            Ok(None) => {}
            Err(error) => {
                warn!(
                    session_id = %session_id,
                    payload_bytes = payload.len(),
                    error = %error,
                    "AI runner live relay payload decode failed"
                );
            }
        }
    }
}

#[derive(Default)]
struct LiveVp8StreamDecoder {
    decoder: Option<Vp8Decoder>,
    next_seq: u64,
    width: u32,
    height: u32,
}

impl LiveVp8StreamDecoder {
    fn handle_payload(&mut self, payload: &[u8]) -> Result<Option<LiveFrame>> {
        if payload.len() >= 32 && payload.get(0..4) == Some(b"DKIF") {
            let (width, height, _fps) =
                parse_ivf_header(payload).ok_or_else(|| anyhow!("invalid IVF header"))?;
            self.decoder = Some(Vp8Decoder::new().map_err(|error| anyhow!("{error}"))?);
            self.width = width;
            self.height = height;
            debug!(width, height, "AI runner live relay IVF header received");
            return Ok(None);
        }
        if payload.len() < 12 {
            return Ok(None);
        }
        let frame_len =
            u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
        if frame_len > MAX_LEGACY_VP8_PAYLOAD_LEN {
            return Err(anyhow!(
                "VP8 payload length {} exceeds maximum {}",
                frame_len,
                MAX_LEGACY_VP8_PAYLOAD_LEN
            ));
        }
        let frame_end = 12usize
            .checked_add(frame_len)
            .ok_or_else(|| anyhow!("VP8 frame length overflow"))?;
        if payload.len() < frame_end {
            return Err(anyhow!(
                "VP8 relay chunk too short: chunk={}, declared_frame={}",
                payload.len(),
                frame_len
            ));
        }
        let decoder = self
            .decoder
            .as_mut()
            .ok_or_else(|| anyhow!("VP8 frame arrived before IVF header"))?;
        let Some(decoded) = decoder
            .decode(&payload[12..frame_end])
            .map_err(|error| anyhow!("{error}"))?
        else {
            return Ok(None);
        };
        self.next_seq = self.next_seq.saturating_add(1);
        self.width = decoded.width;
        self.height = decoded.height;
        Ok(Some(LiveFrame {
            seq: self.next_seq,
            width: decoded.width,
            height: decoded.height,
            bgra: decoded.bgra,
        }))
    }
}

struct DecodedVp8Frame {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

struct Vp8Decoder {
    ctx: vpx_codec_ctx,
    iter: vpx_codec_iter_t,
}

unsafe impl Send for Vp8Decoder {}

impl Vp8Decoder {
    fn new() -> Result<Self, String> {
        let mut ctx = std::mem::MaybeUninit::uninit();
        let cfg = std::mem::MaybeUninit::zeroed();
        let ret = unsafe {
            vpx_codec_dec_init_ver(
                ctx.as_mut_ptr(),
                vpx_codec_vp8_dx(),
                cfg.as_ptr(),
                0,
                VPX_DECODER_ABI_VERSION as i32,
            )
        };
        if ret != vpx_codec_err_t::VPX_CODEC_OK {
            return Err("VP8 decoder init failed".to_string());
        }
        Ok(Self {
            ctx: unsafe { ctx.assume_init() },
            iter: ptr::null(),
        })
    }

    fn decode(&mut self, payload: &[u8]) -> Result<Option<DecodedVp8Frame>, String> {
        let ret = unsafe {
            vpx_codec_decode(
                &mut self.ctx,
                payload.as_ptr(),
                payload.len() as u32,
                ptr::null_mut(),
                0,
            )
        };
        self.iter = ptr::null();
        if ret != vpx_codec_err_t::VPX_CODEC_OK {
            return Err(vpx_error_to_str(&mut self.ctx));
        }
        let img_ptr = unsafe { vpx_codec_get_frame(&mut self.ctx, &mut self.iter) };
        if img_ptr.is_null() {
            return Ok(None);
        }
        let img = unsafe { *img_ptr };
        if img.fmt != vpx_img_fmt::VPX_IMG_FMT_I420 {
            return Err("unsupported VP8 pixel format".to_string());
        }
        let width = img.d_w as u32;
        let height = img.d_h as u32;
        let y_stride = img.stride[0] as usize;
        let u_stride = img.stride[1] as usize;
        let v_stride = img.stride[2] as usize;
        let y_len = y_stride * height as usize;
        let uv_height = height.div_ceil(2) as usize;
        let u_len = u_stride * uv_height;
        let v_len = v_stride * uv_height;
        let y = unsafe { slice::from_raw_parts(img.planes[0] as *const u8, y_len) };
        let u = unsafe { slice::from_raw_parts(img.planes[1] as *const u8, u_len) };
        let v = unsafe { slice::from_raw_parts(img.planes[2] as *const u8, v_len) };
        Ok(Some(DecodedVp8Frame {
            width,
            height,
            bgra: i420_to_bgra(y, u, v, y_stride, u_stride, v_stride, width, height),
        }))
    }
}

impl Drop for Vp8Decoder {
    fn drop(&mut self) {
        unsafe { vpx_codec_destroy(&mut self.ctx) };
    }
}

fn vpx_error_to_str(ctx: &mut vpx_codec_ctx) -> String {
    unsafe {
        let c_str = vpx_codec_error(ctx);
        if c_str.is_null() {
            "libvpx error".to_string()
        } else {
            CStr::from_ptr(c_str).to_string_lossy().into_owned()
        }
    }
}

fn parse_ivf_header(header: &[u8]) -> Option<(u32, u32, u32)> {
    if header.len() < 24 || header.get(0..4) != Some(b"DKIF") {
        return None;
    }
    let width = u16::from_le_bytes([header[12], header[13]]) as u32;
    let height = u16::from_le_bytes([header[14], header[15]]) as u32;
    let fps_num = u32::from_le_bytes([header[16], header[17], header[18], header[19]]);
    let fps_den = u32::from_le_bytes([header[20], header[21], header[22], header[23]]);
    let fps = if fps_num == 0 {
        0
    } else if fps_den <= 1 {
        fps_num
    } else {
        (fps_num / fps_den).max(1)
    };
    Some((width, height, fps))
}

fn i420_to_bgra(
    y: &[u8],
    u: &[u8],
    v: &[u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0u8; w * h * 4];
    for row in 0..h {
        let y_off = row * y_stride;
        let u_off = (row / 2) * u_stride;
        let v_off = (row / 2) * v_stride;
        for col in 0..w {
            let y_val = y[y_off + col] as i32;
            let u_val = u[u_off + (col / 2)] as i32;
            let v_val = v[v_off + (col / 2)] as i32;
            let c = y_val - 16;
            let d = u_val - 128;
            let e = v_val - 128;
            let r = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
            let g = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
            let b = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
            let out_index = (row * w + col) * 4;
            out[out_index] = b;
            out[out_index + 1] = g;
            out[out_index + 2] = r;
            out[out_index + 3] = 255;
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActionExecutionSummary {
    executed_actions: usize,
    settle_ms: u64,
}

async fn execute_desktop_action_batch(
    state: &AppState,
    callback_base: &str,
    request: &StartJobRequest,
    relay_session: &LiveRelaySession,
    actions: &[AiDesktopAction],
    frame: &LiveFrame,
    pointer_state: &mut PointerState,
    cancel: &AtomicBool,
) -> Result<ActionExecutionSummary> {
    ensure_not_cancelled(cancel)?;
    let settle_ms = ai_assist_post_action_settle_ms(actions);
    let immediate_len = ai_assist_immediate_action_len(actions)
        .min(parse_usize_env("RMM_AI_ASSIST_MAX_ACTIONS_PER_STEP", 6));
    for action in &actions[..immediate_len] {
        ensure_not_cancelled(cancel)?;
        execute_desktop_action(
            state,
            callback_base,
            request,
            relay_session,
            action,
            frame.width,
            frame.height,
            cancel,
        )
        .await?;
        pointer_state.apply_action(action, frame.width, frame.height);
    }
    Ok(ActionExecutionSummary {
        executed_actions: immediate_len,
        settle_ms,
    })
}

fn ai_assist_post_action_settle_ms(actions: &[AiDesktopAction]) -> u64 {
    match actions.last() {
        Some(AiDesktopAction::Wait { ms }) => clamp_ai_assist_wait_ms(*ms),
        _ => AI_ASSIST_DEFAULT_SETTLE_MS,
    }
}

fn ai_assist_immediate_action_len(actions: &[AiDesktopAction]) -> usize {
    match actions.last() {
        Some(AiDesktopAction::Wait { .. }) => actions.len().saturating_sub(1),
        _ => actions.len(),
    }
}

fn clamp_ai_assist_wait_ms(ms: u64) -> u64 {
    ms.min(AI_ASSIST_MAX_SETTLE_MS)
}

async fn execute_desktop_action(
    state: &AppState,
    callback_base: &str,
    request: &StartJobRequest,
    relay_session: &LiveRelaySession,
    action: &AiDesktopAction,
    width: u32,
    height: u32,
    cancel: &AtomicBool,
) -> Result<()> {
    ensure_not_cancelled(cancel)?;
    match action {
        AiDesktopAction::Move { x, y, keys } => {
            let modifiers = press_mouse_modifiers(relay_session, keys).await?;
            let result = async {
                ensure_not_cancelled(cancel)?;
                send_mouse_move(relay_session, *x, *y, width, height).await?;
                cancellable_sleep(Duration::from_millis(40), cancel).await?;
                Ok(())
            }
            .await;
            release_mouse_modifiers(relay_session, &modifiers).await?;
            result
        }
        AiDesktopAction::Click { x, y, button, keys } => {
            let modifiers = press_mouse_modifiers(relay_session, keys).await?;
            let result = async {
                ensure_not_cancelled(cancel)?;
                send_mouse_move(relay_session, *x, *y, width, height).await?;
                cancellable_sleep(Duration::from_millis(40), cancel).await?;
                send_mouse_button(relay_session, button, true, *x, *y, width, height).await?;
                tokio::time::sleep(Duration::from_millis(25)).await;
                send_mouse_button(relay_session, button, false, *x, *y, width, height).await?;
                cancellable_sleep(Duration::from_millis(70), cancel).await?;
                Ok(())
            }
            .await;
            release_mouse_modifiers(relay_session, &modifiers).await?;
            result
        }
        AiDesktopAction::DoubleClick { x, y, button, keys } => {
            let modifiers = press_mouse_modifiers(relay_session, keys).await?;
            let result = async {
                ensure_not_cancelled(cancel)?;
                send_mouse_move(relay_session, *x, *y, width, height).await?;
                cancellable_sleep(Duration::from_millis(40), cancel).await?;
                send_mouse_double_click(relay_session, button, *x, *y, width, height).await?;
                Ok(())
            }
            .await;
            release_mouse_modifiers(relay_session, &modifiers).await?;
            result
        }
        AiDesktopAction::Drag { button, path, keys } => {
            if path.len() < 2 {
                return Err(anyhow!("AI drag action requires at least 2 path points"));
            }
            let modifiers = press_mouse_modifiers(relay_session, keys).await?;
            let result = async {
                ensure_not_cancelled(cancel)?;
                let first = path[0];
                let mut current = first;
                let mut button_down = false;
                send_mouse_move(relay_session, first.x, first.y, width, height).await?;
                cancellable_sleep(Duration::from_millis(40), cancel).await?;
                let drag_result = async {
                    ensure_not_cancelled(cancel)?;
                    send_mouse_button(relay_session, button, true, first.x, first.y, width, height)
                        .await?;
                    button_down = true;
                    cancellable_sleep(Duration::from_millis(25), cancel).await?;
                    for point in &path[1..] {
                        ensure_not_cancelled(cancel)?;
                        current = *point;
                        send_mouse_move(relay_session, point.x, point.y, width, height).await?;
                        cancellable_sleep(Duration::from_millis(30), cancel).await?;
                    }
                    Ok(())
                }
                .await;
                if button_down {
                    send_mouse_button(
                        relay_session,
                        button,
                        false,
                        current.x,
                        current.y,
                        width,
                        height,
                    )
                    .await?;
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                drag_result
            }
            .await;
            release_mouse_modifiers(relay_session, &modifiers).await?;
            result
        }
        AiDesktopAction::Scroll {
            x,
            y,
            scroll_x,
            scroll_y,
            keys,
        } => {
            let vertical_delta = if *scroll_y != 0 { *scroll_y } else { *scroll_x };
            if vertical_delta == 0 {
                return Ok(());
            }
            let wheel_notches = (vertical_delta.unsigned_abs().div_ceil(120)).clamp(1, 20);
            let delta = if vertical_delta > 0 { 120 } else { -120 };
            let modifiers = press_mouse_modifiers(relay_session, keys).await?;
            let result = async {
                ensure_not_cancelled(cancel)?;
                send_mouse_move(relay_session, *x, *y, width, height).await?;
                cancellable_sleep(Duration::from_millis(40), cancel).await?;
                for _ in 0..wheel_notches {
                    ensure_not_cancelled(cancel)?;
                    send_mouse_wheel(relay_session, *x, *y, delta, width, height).await?;
                    cancellable_sleep(Duration::from_millis(45), cancel).await?;
                }
                Ok(())
            }
            .await;
            release_mouse_modifiers(relay_session, &modifiers).await?;
            result
        }
        AiDesktopAction::Type { text } => {
            ensure_not_cancelled(cancel)?;
            send_typed_input(relay_session, text).await?;
            cancellable_sleep(Duration::from_millis(50), cancel).await?;
            Ok(())
        }
        AiDesktopAction::InjectSecret { secret_handle } => {
            ensure_not_cancelled(cancel)?;
            let resolved =
                resolve_generated_secret_for_runner(state, callback_base, request, secret_handle)
                    .await?;
            debug!(
                job_id = %request.job_id,
                secret_handle = %secret_handle,
                secure_note_url = %resolved.secure_note_url,
                desktop_reference = ?resolved.desktop_reference,
                "AI runner resolved generated desktop secret"
            );
            send_typed_input(relay_session, &resolved.secret).await?;
            cancellable_sleep(Duration::from_millis(50), cancel).await?;
            Ok(())
        }
        AiDesktopAction::Keypress { keys } => send_keypress(relay_session, keys, cancel).await,
        AiDesktopAction::Wait { ms } => {
            cancellable_sleep(Duration::from_millis(clamp_ai_assist_wait_ms(*ms)), cancel).await?;
            Ok(())
        }
    }
}

fn normalize_ai_key_name(raw: &str) -> String {
    raw.trim()
        .to_uppercase()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

fn split_ai_keys(keys: &[String]) -> Vec<String> {
    keys.iter()
        .flat_map(|key| key.split('+'))
        .map(normalize_ai_key_name)
        .filter(|key| !key.is_empty())
        .collect()
}

fn ai_modifier_bit(key: &str) -> u8 {
    match normalize_ai_key_name(key).as_str() {
        "CTRL" | "CONTROL" => CONTROL_MOD_CTRL,
        "SHIFT" => CONTROL_MOD_SHIFT,
        "ALT" | "OPTION" => CONTROL_MOD_ALT,
        "WIN" | "META" | "CMD" | "COMMAND" => CONTROL_MOD_WIN,
        _ => 0,
    }
}

fn ai_key_to_vkey(raw: &str) -> Option<u16> {
    let key = normalize_ai_key_name(raw);
    match key.as_str() {
        "ENTER" | "RETURN" => Some(0x0d),
        "ESC" | "ESCAPE" => Some(0x1b),
        "TAB" => Some(0x09),
        "SPACE" => Some(0x20),
        "BACKSPACE" => Some(0x08),
        "DELETE" | "DEL" => Some(0x2e),
        "HOME" => Some(0x24),
        "END" => Some(0x23),
        "PAGEUP" => Some(0x21),
        "PAGEDOWN" => Some(0x22),
        "UP" | "ARROWUP" => Some(0x26),
        "DOWN" | "ARROWDOWN" => Some(0x28),
        "LEFT" | "ARROWLEFT" => Some(0x25),
        "RIGHT" | "ARROWRIGHT" => Some(0x27),
        "INSERT" => Some(0x2d),
        "CTRL" | "CONTROL" => Some(0x11),
        "SHIFT" => Some(0x10),
        "ALT" | "OPTION" => Some(0x12),
        "WIN" | "META" | "CMD" | "COMMAND" => Some(0x5b),
        _ if key.len() == 1 => {
            let ch = key.as_bytes()[0];
            if ch.is_ascii_uppercase() || ch.is_ascii_digit() {
                Some(ch as u16)
            } else {
                None
            }
        }
        _ if key.starts_with('F') => key[1..]
            .parse::<u16>()
            .ok()
            .filter(|value| (1..=12).contains(value))
            .map(|value| 0x70 + value - 1),
        _ => None,
    }
}

async fn press_mouse_modifiers(
    relay_session: &LiveRelaySession,
    keys: &[String],
) -> Result<Vec<String>> {
    let modifier_keys: Vec<String> = split_ai_keys(keys)
        .into_iter()
        .filter(|key| ai_modifier_bit(key) != 0)
        .collect();
    for key in &modifier_keys {
        send_modifier_key(relay_session, key, true).await?;
    }
    Ok(modifier_keys)
}

async fn release_mouse_modifiers(
    relay_session: &LiveRelaySession,
    modifier_keys: &[String],
) -> Result<()> {
    for key in modifier_keys.iter().rev() {
        send_modifier_key(relay_session, key, false).await?;
    }
    Ok(())
}

async fn send_modifier_key(relay_session: &LiveRelaySession, key: &str, down: bool) -> Result<()> {
    let vkey = ai_key_to_vkey(key).ok_or_else(|| anyhow!("Unsupported modifier key: {key}"))?;
    send_key_event(relay_session, down, vkey, 0, 0).await?;
    tokio::time::sleep(Duration::from_millis(20)).await;
    Ok(())
}

async fn send_keypress(
    relay_session: &LiveRelaySession,
    keys: &[String],
    cancel: &AtomicBool,
) -> Result<()> {
    ensure_not_cancelled(cancel)?;
    let expanded_keys = split_ai_keys(keys);
    let modifier_mask = expanded_keys
        .iter()
        .fold(0u8, |mask, key| mask | ai_modifier_bit(key));
    let non_modifier_keys: Vec<&String> = expanded_keys
        .iter()
        .filter(|key| ai_modifier_bit(key) == 0)
        .collect();
    if non_modifier_keys.is_empty() {
        for key in &expanded_keys {
            send_modifier_key(relay_session, key, true).await?;
        }
        for key in expanded_keys.iter().rev() {
            send_modifier_key(relay_session, key, false).await?;
        }
        return Ok(());
    }
    for key in non_modifier_keys {
        ensure_not_cancelled(cancel)?;
        let vkey = ai_key_to_vkey(key).ok_or_else(|| anyhow!("Unsupported keypress key: {key}"))?;
        send_key_event(relay_session, true, vkey, 0, modifier_mask).await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        send_key_event(relay_session, false, vkey, 0, modifier_mask).await?;
        cancellable_sleep(Duration::from_millis(35), cancel).await?;
    }
    Ok(())
}

async fn send_key_event(
    relay_session: &LiveRelaySession,
    down: bool,
    vkey: u16,
    scan: u16,
    modifiers: u8,
) -> Result<()> {
    let frame = build_key_control_frame(
        if down {
            CONTROL_TYPE_KEY_DOWN
        } else {
            CONTROL_TYPE_KEY_UP
        },
        vkey,
        scan,
        modifiers,
    )?;
    relay_session.send_control_frame(frame).await
}

async fn send_mouse_move(
    relay_session: &LiveRelaySession,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<()> {
    relay_session
        .send_control_frame(build_mouse_move_frame(x, y, width, height)?)
        .await
}

async fn send_mouse_button(
    relay_session: &LiveRelaySession,
    button: &str,
    down: bool,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<()> {
    relay_session
        .send_control_frame(build_mouse_button_frame(button, down, x, y, width, height)?)
        .await
}

async fn send_mouse_double_click(
    relay_session: &LiveRelaySession,
    button: &str,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<()> {
    relay_session
        .send_control_frame(build_mouse_double_click_frame(button, x, y, width, height)?)
        .await
}

async fn send_mouse_wheel(
    relay_session: &LiveRelaySession,
    x: u32,
    y: u32,
    delta: i32,
    width: u32,
    height: u32,
) -> Result<()> {
    relay_session
        .send_control_frame(build_mouse_wheel_frame(x, y, delta, width, height)?)
        .await
}

async fn send_typed_input(relay_session: &LiveRelaySession, text: &str) -> Result<()> {
    relay_session
        .send_control_frame(build_text_control_frame(CONTROL_TYPE_TYPED_INPUT, text)?)
        .await
}

fn build_mouse_move_frame(x: u32, y: u32, width: u32, height: u32) -> Result<Vec<u8>> {
    let (nx, ny) = normalize_coords(x, y, width, height, Some((width, height)));
    let mut payload = Vec::with_capacity(CONTROL_PAYLOAD_MOUSE_MOVE_LEN);
    payload.extend_from_slice(&nx.to_be_bytes());
    payload.extend_from_slice(&ny.to_be_bytes());
    build_control_frame(CONTROL_TYPE_MOUSE_MOVE, &payload)
        .map_err(|error| anyhow!("build mouse move control frame: {error}"))
}

fn build_mouse_button_frame(
    button: &str,
    down: bool,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let mapped_button = map_mouse_button(button);
    let (nx, ny) = normalize_coords(x, y, width, height, Some((width, height)));
    let mut payload = Vec::with_capacity(CONTROL_PAYLOAD_MOUSE_BUTTON_LEN);
    payload.push(mapped_button);
    payload.push(if down { 1 } else { 0 });
    payload.extend_from_slice(&nx.to_be_bytes());
    payload.extend_from_slice(&ny.to_be_bytes());
    build_control_frame(CONTROL_TYPE_MOUSE_BUTTON, &payload)
        .map_err(|error| anyhow!("build mouse button control frame: {error}"))
}

fn build_mouse_double_click_frame(
    button: &str,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let mapped_button = map_mouse_button(button);
    let (nx, ny) = normalize_coords(x, y, width, height, Some((width, height)));
    let mut payload = Vec::with_capacity(CONTROL_PAYLOAD_MOUSE_DOUBLE_CLICK_LEN);
    payload.push(mapped_button);
    payload.extend_from_slice(&nx.to_be_bytes());
    payload.extend_from_slice(&ny.to_be_bytes());
    build_control_frame(CONTROL_TYPE_MOUSE_DOUBLE_CLICK, &payload)
        .map_err(|error| anyhow!("build mouse double-click control frame: {error}"))
}

fn map_mouse_button(button: &str) -> u8 {
    match button.trim().to_lowercase().as_str() {
        "right" => 1,
        "middle" => 2,
        _ => 0,
    }
}

fn build_mouse_wheel_frame(x: u32, y: u32, delta: i32, width: u32, height: u32) -> Result<Vec<u8>> {
    let delta = delta.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    let (nx, ny) = normalize_coords(x, y, width, height, Some((width, height)));
    let mut payload = Vec::with_capacity(CONTROL_PAYLOAD_MOUSE_WHEEL_LEN);
    payload.extend_from_slice(&delta.to_be_bytes());
    payload.extend_from_slice(&nx.to_be_bytes());
    payload.extend_from_slice(&ny.to_be_bytes());
    build_control_frame(CONTROL_TYPE_MOUSE_WHEEL, &payload)
        .map_err(|error| anyhow!("build mouse wheel control frame: {error}"))
}

fn build_key_control_frame(
    message_type: u8,
    vkey: u16,
    scan: u16,
    modifiers: u8,
) -> Result<Vec<u8>> {
    let mut payload = Vec::with_capacity(CONTROL_PAYLOAD_KEY_LEN);
    payload.extend_from_slice(&vkey.to_be_bytes());
    payload.extend_from_slice(&scan.to_be_bytes());
    payload.push(modifiers);
    build_control_frame(message_type, &payload)
        .map_err(|error| anyhow!("build key control frame: {error}"))
}

fn build_text_control_frame(message_type: u8, text: &str) -> Result<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() > u16::MAX as usize {
        return Err(anyhow!("control text payload too large"));
    }
    let mut payload = Vec::with_capacity(2 + bytes.len());
    payload.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    payload.extend_from_slice(bytes);
    build_control_frame(message_type, &payload)
        .map_err(|error| anyhow!("build text control frame: {error}"))
}

fn normalize_coords(
    x: u32,
    y: u32,
    element_width: u32,
    element_height: u32,
    stream_size: Option<(u32, u32)>,
) -> (u32, u32) {
    if element_width == 0 || element_height == 0 {
        return (0, 0);
    }
    let element_w = element_width as f64;
    let element_h = element_height as f64;
    let (frame_w, frame_h, offset_x, offset_y) = match stream_size {
        Some((stream_w, stream_h)) if stream_w > 0 && stream_h > 0 => {
            let stream_w = stream_w as f64;
            let stream_h = stream_h as f64;
            let stream_aspect = stream_w / stream_h;
            let element_aspect = element_w / element_h;
            if stream_aspect > element_aspect {
                let frame_w = element_w;
                let frame_h = (element_w / stream_aspect).max(1.0);
                (frame_w, frame_h, 0.0, (element_h - frame_h) / 2.0)
            } else {
                let frame_h = element_h;
                let frame_w = (element_h * stream_aspect).max(1.0);
                (frame_w, frame_h, (element_w - frame_w) / 2.0, 0.0)
            }
        }
        _ => (element_w.max(1.0), element_h.max(1.0), 0.0, 0.0),
    };
    let local_x = (x as f64 - offset_x).clamp(0.0, frame_w);
    let local_y = (y as f64 - offset_y).clamp(0.0, frame_h);
    let nx = ((local_x / frame_w) * 65535.0).round().clamp(0.0, 65535.0) as u32;
    let ny = ((local_y / frame_h) * 65535.0).round().clamp(0.0, 65535.0) as u32;
    (nx, ny)
}

async fn post_status(
    state: &AppState,
    callback_base: &str,
    job_id: &str,
    status: &str,
    message: Option<String>,
    result: Option<Value>,
    error: Option<String>,
) -> Result<()> {
    let lease_id = job_lease_id(state, job_id).await;
    let event_key = status_event_key(status, message.as_deref(), result.as_ref());
    debug!(
        job_id = %job_id,
        status,
        has_message = message.is_some(),
        has_result = result.is_some(),
        has_error = error.is_some(),
        "AI runner posting job status callback"
    );
    let url = format!(
        "{}/command-center/internal/ai-runner/jobs/{}/status",
        callback_base,
        encode_path_segment(job_id)
    );
    post_json(
        state,
        &url,
        json!({
            "status": status,
            "runnerId": state.config.runner_id,
            "leaseId": lease_id,
            "eventKey": event_key,
            "message": message,
            "result": result.unwrap_or_else(|| json!({})),
            "error": error,
        }),
    )
    .await
}

fn event_key_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn status_event_key(status: &str, message: Option<&str>, result: Option<&Value>) -> String {
    let result_record = result.and_then(Value::as_object);
    let phase = result_record
        .and_then(|record| {
            record
                .get("phase")
                .or_else(|| record.get("approvalMode"))
                .and_then(Value::as_str)
        })
        .or(message)
        .unwrap_or("state");
    let mut parts = vec![format!("status:{status}"), event_key_component(phase)];
    if let Some(record) = result_record {
        for key in ["turnIndex", "approvalId", "checkpointCount", "waitMs"] {
            if let Some(value) = record.get(key) {
                if let Some(text) = value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .or_else(|| value.as_u64().map(|number| number.to_string()))
                    .or_else(|| value.as_i64().map(|number| number.to_string()))
                {
                    parts.push(event_key_component(&format!("{key}_{text}")));
                }
            }
        }
    }
    parts.join(":")
}

fn artifact_event_key(artifact_type: &str, name: &str, metadata: &Value) -> String {
    let artifact_frame_id = metadata
        .get("frameId")
        .or_else(|| metadata.get("frame_id"))
        .or_else(|| metadata.get("frameSeq"))
        .or_else(|| metadata.get("frame_seq"))
        .and_then(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .or_else(|| value.as_u64().map(|number| number.to_string()))
        })
        .unwrap_or_else(|| name.to_string());
    format!(
        "artifact:{}:{}",
        event_key_component(artifact_type),
        event_key_component(&artifact_frame_id)
    )
}

async fn post_artifact(
    state: &AppState,
    callback_base: &str,
    job_id: &str,
    artifact_type: &str,
    name: &str,
    mime_type: &str,
    content_base64: String,
    metadata: Value,
    append_to_chat: bool,
    message_content: Option<&str>,
    chat_presentation: Option<&str>,
) -> Result<()> {
    let lease_id = job_lease_id(state, job_id).await;
    let event_key = artifact_event_key(artifact_type, name, &metadata);
    debug!(
        job_id = %job_id,
        artifact_type,
        name,
        mime_type,
        content_base64_chars = content_base64.len(),
        "AI runner posting artifact callback"
    );
    let url = format!(
        "{}/command-center/internal/ai-runner/jobs/{}/artifacts",
        callback_base,
        encode_path_segment(job_id)
    );
    post_json(
        state,
        &url,
        json!({
            "runnerId": state.config.runner_id,
            "leaseId": lease_id,
            "eventKey": event_key,
            "artifactType": artifact_type,
            "name": name,
            "mimeType": mime_type,
            "contentBase64": content_base64,
            "metadata": metadata,
            "appendToChat": append_to_chat,
            "messageContent": message_content,
            "chatPresentation": chat_presentation,
        }),
    )
    .await
}

async fn post_runner_event(
    state: &AppState,
    callback_base: &str,
    job_id: &str,
    event_type: &str,
    event_key: String,
    payload: Value,
) -> Result<()> {
    let lease_id = job_lease_id(state, job_id).await;
    let url = format!(
        "{}/command-center/internal/ai-runner/jobs/{}/events",
        callback_base,
        encode_path_segment(job_id)
    );
    post_json(
        state,
        &url,
        json!({
            "eventKey": event_key,
            "eventType": event_type,
            "runnerId": state.config.runner_id,
            "leaseId": lease_id,
            "payload": payload,
        }),
    )
    .await
}

async fn post_json(state: &AppState, url: &str, body: Value) -> Result<()> {
    debug!(url, "AI runner callback request sending");
    let response = state
        .client
        .post(url)
        .header("x-service-key", &state.config.service_key)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("callback request failed: {url}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        warn!(url, status = %status, body = %text, "AI runner callback request failed");
        return Err(anyhow!("callback {url} returned {status}: {text}"));
    }
    debug!(url, "AI runner callback request succeeded");
    Ok(())
}

fn encode_path_segment(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

fn encode_query_component(value: &str) -> String {
    encode_path_segment(value)
}

fn extract_query_param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    for part in query.split('&') {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        if key == name {
            return percent_decode(value);
        }
    }
    None
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hi = hex_value(bytes[index + 1])?;
                let lo = hex_value(bytes[index + 2])?;
                out.push((hi << 4) | lo);
                index += 3;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/internal/jobs", post(start_job))
        .route("/internal/jobs/:job_id", get(get_job))
        .route("/internal/jobs/:job_id/stop", post(stop_job))
        .with_state(state)
}

async fn shutdown_active_jobs(state: AppState) {
    let active = {
        let active_jobs = state.active_jobs.lock().await;
        active_jobs
            .iter()
            .map(|(job_id, cancel)| (job_id.clone(), Arc::clone(cancel)))
            .collect::<Vec<_>>()
    };
    for (job_id, cancel) in &active {
        cancel.store(true, Ordering::SeqCst);
        info!(job_id = %job_id, "AI runner shutdown requested active job cancellation");
    }

    let cleanup_slots = {
        let cleanups = state.session_cleanups.lock().await;
        active
            .iter()
            .filter_map(|(job_id, _)| {
                cleanups
                    .get(job_id)
                    .cloned()
                    .map(|slot| (job_id.clone(), slot))
            })
            .collect::<Vec<_>>()
    };
    let leases = {
        let jobs = state.jobs.lock().await;
        active
            .iter()
            .filter_map(|(job_id, _)| {
                jobs.get(job_id).and_then(|job| {
                    job.lease_id.as_ref().map(|lease_id| {
                        (job_id.clone(), lease_id.clone(), job.callback_base.clone())
                    })
                })
            })
            .collect::<Vec<_>>()
    };
    for (job_id, cleanup_slot) in cleanup_slots {
        let callback_base = {
            let jobs = state.jobs.lock().await;
            jobs.get(&job_id)
                .map(|job| job.callback_base.clone())
                .unwrap_or_else(|| state.config.api_callback_base_url.clone())
        };
        end_registered_headless_session(&state, &job_id, &callback_base, cleanup_slot).await;
    }
    for (job_id, lease_id, callback_base) in leases {
        match post_status(
            &state,
            &callback_base,
            &job_id,
            "stopped",
            Some("AI runner shutting down".to_string()),
            None,
            None,
        )
        .await
        {
            Ok(_) => {
                update_job_record(&state, &job_id, JobStatus::Stopped, None, None).await;
                release_job_lease(&state, &callback_base, &job_id, &lease_id).await;
            }
            Err(error) => {
                warn!(
                    job_id = %job_id,
                    lease_id = %lease_id,
                    error = %error,
                    "AI runner shutdown status update failed; leaving lease to expire"
                );
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    install_rustls_crypto_provider()?;
    let log_filter =
        env::var("RUST_LOG").unwrap_or_else(|_| "info,talos_ai_runner=debug".to_string());
    tracing_subscriber::fmt()
        .with_env_filter(log_filter)
        .with_target(false)
        .init();

    let config = Arc::new(load_config()?);
    debug!(
        runner_id = %config.runner_id,
        bind_addr = %config.bind_addr,
        rmm_server_url = %config.rmm_server_url,
        api_callback_base_url = %config.api_callback_base_url,
        max_concurrent_jobs = config.max_concurrent_jobs,
        job_timeout_secs = config.job_timeout_secs,
        screenshot_read_timeout_secs = config.screenshot_read_timeout_secs,
        shell_command_max_wait_secs = config.shell_command_max_wait_secs,
        shell_command_checkpoint_ms = config.shell_command_checkpoint_ms,
        relay_ca_path = ?config.relay_ca_path,
        relay_verify_hostname = ?config.relay_verify_hostname,
        "AI runner config loaded"
    );
    let bind_addr: SocketAddr = config
        .bind_addr
        .parse()
        .with_context(|| format!("invalid TALOS_AI_RUNNER_BIND_ADDR {}", config.bind_addr))?;
    let state = AppState {
        config: Arc::clone(&config),
        jobs: Arc::new(Mutex::new(HashMap::new())),
        active_jobs: Arc::new(Mutex::new(HashMap::new())),
        session_cleanups: Arc::new(Mutex::new(HashMap::new())),
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(DEFAULT_HTTP_TIMEOUT_SECS))
            .build()
            .context("build HTTP client")?,
    };

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("bind {}", config.bind_addr))?;
    info!(addr = %config.bind_addr, runner_id = %config.runner_id, "Talos AI runner listening");
    let shutdown_state = state.clone();
    axum::serve(listener, build_router(state))
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                error!(error = %error, "failed to listen for ctrl_c");
            } else {
                info!("Talos AI runner shutdown signal received");
                shutdown_active_jobs(shutdown_state).await;
            }
        })
        .await
        .context("serve Talos AI runner")?;
    info!("Talos AI runner stopped");
    Ok(())
}

fn install_rustls_crypto_provider() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|error| anyhow!("install rustls CryptoProvider: {:?}", error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_native_default_bind_is_loopback_only() {
        let address = DEFAULT_BIND_ADDR
            .parse::<SocketAddr>()
            .expect("default AI runner address must parse");

        assert!(address.ip().is_loopback());
    }

    #[test]
    fn start_job_request_accepts_device_context() {
        let request: StartJobRequest = serde_json::from_value(json!({
            "jobId": "job-1",
            "organizationId": "org-1",
            "agentId": "agent-1",
            "jobType": "shell_goal",
            "goal": "Check updates",
            "deviceContext": {
                "agentId": "agent-1",
                "platform": { "family": "windows" }
            },
            "generatedSecrets": [{
                "secretHandle": "sec_a1b2c3d4e5f6g7h8",
                "shellReference": "$__talos_secret_f6g7h8",
                "secureNoteUrl": "/SN/a1b2c3d4",
                "expiresAt": "2026-06-18T10:00:00.000Z"
            }]
        }))
        .expect("deserialize start job request");

        assert_eq!(
            request.device_context,
            Some(json!({
                "agentId": "agent-1",
                "platform": { "family": "windows" }
            }))
        );
        assert_eq!(request.generated_secrets.len(), 1);
        assert_eq!(
            request.generated_secrets[0].secret_handle,
            "sec_a1b2c3d4e5f6g7h8"
        );
    }

    #[test]
    fn start_job_response_serializes_lease_rejection_reason() {
        let response = StartJobResponse {
            accepted: false,
            job_id: "job-1".to_string(),
            runner_id: "runner-a".to_string(),
            lease_id: None,
            lease_expires_at: Some("2026-06-15T10:00:00.000Z".to_string()),
            reason: Some("lease_active".to_string()),
        };

        let value = serde_json::to_value(response).expect("serialize response");
        assert_eq!(value["accepted"], false);
        assert_eq!(value["reason"], "lease_active");
        assert_eq!(value["leaseExpiresAt"], "2026-06-15T10:00:00.000Z");
    }

    #[test]
    fn callback_event_keys_are_stable() {
        assert_eq!(
            status_event_key(
                "running",
                Some("Planning shell command"),
                Some(&json!({ "phase": "planning_shell" })),
            ),
            "status:running:planning_shell"
        );
        assert_eq!(
            status_event_key(
                "running",
                Some("Continuing approved command"),
                Some(&json!({
                    "phase": "executing_command",
                    "turnIndex": 9,
                    "approvalId": "approval-1",
                    "checkpointCount": 2,
                    "waitMs": 10_000
                })),
            ),
            "status:running:executing_command:turnIndex_9:approvalId_approval_1:checkpointCount_2:waitMs_10000"
        );
        assert_eq!(
            artifact_event_key(
                "runner-screenshot",
                "desktop-frame.png",
                &json!({ "frameSeq": 7 }),
            ),
            "artifact:runner_screenshot:7"
        );
    }

    #[test]
    fn chat_approval_error_frame_classifies_no_interactive_user() {
        let body = serde_json::to_vec(&ChatWireErrorPayload {
            code: OperationErrorCode::NoInteractiveUser,
            message: NO_INTERACTIVE_USER_APPROVAL_MESSAGE.to_string(),
            retryable: true,
        })
        .expect("serialize chat error");

        let error = handle_chat_approval_frame(CHAT_MSG_ERROR, &body, "approval-1")
            .expect_err("no interactive user should fail approval wait");
        let (status_text, status, message, result) = classify_job_error(&error);

        assert_eq!(status_text, "failed");
        assert!(matches!(status, JobStatus::Failed));
        assert_eq!(message, NO_INTERACTIVE_USER_APPROVAL_MESSAGE);
        let result = result.expect("approval unavailable result");
        assert_eq!(result["phase"], "approval_unavailable");
        assert_eq!(result["reason"], NO_INTERACTIVE_USER_APPROVAL_REASON);
        assert_eq!(result["summary"], NO_INTERACTIVE_USER_APPROVAL_MESSAGE);
    }

    #[test]
    fn shell_transcript_chunks_preserve_utf8_and_redaction() {
        let cleaned = strip_shell_control_sequences("alpha\r\n\x1b[31msecret\x1b[0m βeta");
        let redacted = redact_generated_secrets(&cleaned, &["secret".to_string()]);
        assert_eq!(redacted, "alpha\n\n[generated secret redacted] βeta");

        let chunks = chunk_text_by_chars(&redacted, 8);
        assert_eq!(chunks.concat(), redacted);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 8));
    }

    #[test]
    fn shell_command_wrappers_include_platform_markers() {
        let markers = ShellCommandMarkers {
            start: "__TALOS_CMD_START_test__".to_string(),
            end: "__TALOS_CMD_END_test__".to_string(),
        };
        let unix = build_marked_shell_command("sleep 1", &markers, Some("linux"));
        assert!(unix.contains("printf '\\n%s\\n' '__TALOS_CMD_START_test__'"));
        assert!(unix.contains("__talos_exit=$?"));
        assert!(unix.contains("__TALOS_CMD_END_test__"));

        let windows =
            build_marked_shell_command("Start-Sleep -Seconds 1", &markers, Some("windows"));
        assert!(windows.contains("Write-Output \"__TALOS_CMD_START_test__\""));
        assert!(windows.contains("$__talosExit"));
        assert!(windows.contains("Write-Output \"__TALOS_CMD_END_test__:$__talosExit\""));
    }

    #[test]
    fn shell_command_output_parser_hides_markers_and_reads_exit_code() {
        let markers = ShellCommandMarkers {
            start: "__TALOS_CMD_START_test__".to_string(),
            end: "__TALOS_CMD_END_test__".to_string(),
        };
        let raw = concat!(
            "printf '\\n%s\\n' '__TALOS_CMD_START_test__'\r\n",
            "__TALOS_CMD_START_test__\r\n",
            "brew install example\r\n",
            "Downloading...\r\n",
            "__talos_exit=$?\r\n",
            "printf '\\n%s:%s\\n' '__TALOS_CMD_END_test__' \"$__talos_exit\"\r\n",
            "__TALOS_CMD_END_test__:0\r\n",
        );
        assert!(shell_command_has_end_marker(raw, &markers));
        assert_eq!(shell_command_exit_code(raw, &markers), Some(0));
        assert_eq!(
            clean_shell_command_output(raw, &markers),
            "brew install example\nDownloading..."
        );
        assert_eq!(
            clean_shell_runner_transcript(raw),
            "brew install example\nDownloading..."
        );
    }

    #[test]
    fn shell_command_terminal_output_preserves_progress_and_ansi() {
        let markers = ShellCommandMarkers {
            start: "__TALOS_CMD_START_test__".to_string(),
            end: "__TALOS_CMD_END_test__".to_string(),
        };
        let raw = concat!(
            "PS C:\\Windows\\system32> Write-Output \"__TALOS_CMD_START_test__\"\r\n",
            "__TALOS_CMD_START_test__\r\n",
            ">>\r\n",
            "Downloading 10%\r\x1b[32mDownloading 90%\x1b[0m\rDownloading 100%\r\n",
            "PS C:\\Windows\\system32> $__talosExit = 0\r\n",
            "__TALOS_CMD_END_test__:0\r\n",
        );

        assert_eq!(
            terminal_shell_command_output(raw, &markers),
            "Downloading 10%\r\x1b[32mDownloading 90%\x1b[0m\rDownloading 100%\r\n"
        );
        assert_eq!(
            clean_shell_command_output(raw, &markers),
            "Downloading 10%\nDownloading 90%\nDownloading 100%"
        );
    }

    #[test]
    fn shell_command_wait_ms_is_clamped() {
        assert_eq!(clamp_shell_command_wait_ms(1), SHELL_COMMAND_WAIT_MIN_MS);
        assert_eq!(clamp_shell_command_wait_ms(10_000), 10_000);
        assert_eq!(
            clamp_shell_command_wait_ms(120_000),
            SHELL_COMMAND_WAIT_MAX_MS
        );
    }

    #[test]
    fn shell_assist_action_parser_accepts_interrupt() {
        assert_eq!(
            parse_shell_assist_action(" interrupt ").expect("parse interrupt"),
            ShellAssistAction::Interrupt
        );
    }

    #[test]
    fn shell_assist_action_rejects_running_actions_without_active_command() {
        let interrupt_error =
            ensure_shell_action_allowed_without_active_command(ShellAssistAction::Interrupt)
                .expect_err("interrupt without active command should fail");
        assert!(interrupt_error
            .to_string()
            .contains("shell assist requested interrupt but no command is running"));

        let wait_error =
            ensure_shell_action_allowed_without_active_command(ShellAssistAction::Wait)
                .expect_err("wait without active command should fail");
        assert!(wait_error
            .to_string()
            .contains("shell assist requested wait but no command is running"));
    }

    #[test]
    fn heartbeat_cancellation_sets_cancel_flag() {
        let cancel = AtomicBool::new(false);
        let lease = LeaseRecord {
            accepted: true,
            reason: None,
            lease_id: Some("lease-1".to_string()),
            lease_expires_at: None,
            cancel_requested_at: Some("2026-06-15T10:00:00.000Z".to_string()),
        };
        assert!(apply_heartbeat_lease(&lease, &cancel));
        assert!(cancel.load(Ordering::SeqCst));

        let cancel = AtomicBool::new(false);
        let lost = LeaseRecord {
            accepted: false,
            reason: Some("lease_lost".to_string()),
            lease_id: None,
            lease_expires_at: None,
            cancel_requested_at: None,
        };
        assert!(!apply_heartbeat_lease(&lost, &cancel));
        assert!(cancel.load(Ordering::SeqCst));
    }

    #[test]
    fn assist_request_bodies_forward_device_context() {
        let device_context = json!({
            "agentId": "agent-1",
            "hostname": "win-ops-1",
            "platform": { "family": "windows" }
        });
        let connect_response = ConnectResponse {
            url: "wss://relay.example.test/session".to_string(),
            session_id: "session-1".to_string(),
        };
        let capabilities = SessionCapabilities {
            relay_url: Some("wss://relay.example.test".to_string()),
            e2e_key: Some("key".to_string()),
            selected_display_profile: Some(REMOTE_DESKTOP_PROFILE_MODERN_CPU.to_string()),
            display_profiles: vec![],
            platform: Some("windows".to_string()),
        };
        let artifact = ScreenshotArtifact {
            frame_id: 1,
            width: 1024,
            height: 768,
            payload_bytes: 4,
            png_bytes: 8,
            base64_content: "abcd".to_string(),
        };
        let history = vec![AiShellAssistHistoryEntry {
            command: "Get-ComputerInfo".to_string(),
            approved: true,
            output: Some("ok".to_string()),
            response_id: Some("resp-1".to_string()),
        }];
        let request = StartJobRequest {
            job_id: "job-1".to_string(),
            organization_id: "org-1".to_string(),
            user_id: Some("user-1".to_string()),
            conversation_id: Some("conversation-1".to_string()),
            agent_id: "agent-1".to_string(),
            job_type: "shell_goal".to_string(),
            goal: Some("Check updates".to_string()),
            device_context: Some(device_context.clone()),
            generated_secrets: vec![],
            callback_base_url: None,
            approval_mode: None,
            approval: None,
        };

        let start_body = desktop_task_start_body(
            &request,
            "Check updates",
            &connect_response,
            "token-1",
            "https://rmm.example.test",
            &capabilities,
            &artifact,
            Some(&device_context),
        );
        let continue_body = desktop_task_continue_body(
            &request,
            &connect_response,
            "token-1",
            "https://rmm.example.test",
            &capabilities,
            &artifact,
            "Clicked Settings.",
            Some(&device_context),
        );
        let shell_body = shell_assist_proposal_body(
            &request,
            "Check updates",
            "PS>",
            &history,
            Some(&AiShellAssistActiveCommand {
                command: "Start-Sleep -Seconds 20".to_string(),
                approval_id: "approval-1".to_string(),
                turn_index: 2,
                elapsed_ms: 10_000,
                checkpoint_count: 1,
                recent_output: "Downloading...".to_string(),
                remaining_ms: 7_190_000,
            }),
            "shell-session-1",
            "token-2",
            "https://rmm.example.test",
            Some("windows"),
            Some(&device_context),
        );

        assert_eq!(start_body["deviceContext"], device_context);
        assert_eq!(continue_body["deviceContext"], device_context);
        assert_eq!(shell_body["deviceContext"], device_context);
        assert_eq!(start_body["jobId"], "job-1");
        assert_eq!(start_body["organizationId"], "org-1");
        assert_eq!(start_body["userId"], "user-1");
        assert_eq!(continue_body["conversationId"], "conversation-1");
        assert_eq!(shell_body["agentId"], "agent-1");
        assert_eq!(shell_body["history"][0]["command"], "Get-ComputerInfo");
        assert_eq!(shell_body["activeCommand"]["approvalId"], "approval-1");
        assert_eq!(
            shell_body["activeCommand"]["recentOutput"],
            "Downloading..."
        );
    }

    #[test]
    fn shell_secret_materialization_builders_match_platform() {
        assert_eq!(
            build_shell_secret_materialization_setup("$__talos_secret_ab12cd", Some("windows"))
                .expect("powershell setup"),
            "$__talos_secret_ab12cd = Read-Host -AsSecureString\r"
        );
        assert_eq!(
            build_shell_secret_materialization_setup("$__talos_secret_ab12cd", Some("linux"))
                .expect("posix setup"),
            "stty -echo; IFS= read -r __talos_secret_ab12cd; stty echo; printf '\\n'\r"
        );
        assert!(build_shell_secret_materialization_setup("not_safe", Some("linux")).is_err());
    }

    #[test]
    fn shell_secret_reference_scanner_finds_unique_talos_variables() {
        let references = talos_shell_secret_references(
            "Set-ADAccountPassword -NewPassword $__talos_secret_ab12cd; echo $__talos_secret_ab12cd; echo $__talos_secret_ef34gh",
        );
        assert_eq!(
            references,
            vec![
                "$__talos_secret_ab12cd".to_string(),
                "$__talos_secret_ef34gh".to_string(),
            ]
        );
        assert!(talos_shell_secret_references("echo $__talos_secret_").is_empty());
    }

    #[test]
    fn windows_secret_command_contract_rejects_plaintext_conversion() {
        let error = validate_shell_generated_secret_command_contract(
            "$sec = ConvertTo-SecureString $__talos_secret_ab12cd -AsPlainText -Force; Set-ADAccountPassword -NewPassword $sec",
            Some("windows"),
        )
        .expect_err("ConvertTo-SecureString should be rejected for generated SecureString refs");

        assert!(error
            .to_string()
            .contains("already PowerShell SecureString variables"));
        validate_shell_generated_secret_command_contract(
            "Set-ADAccountPassword -NewPassword $__talos_secret_ab12cd",
            Some("windows"),
        )
        .expect("direct SecureString use should be accepted");
    }

    #[test]
    fn screenshot_assembler_converts_complete_bgra_frame_to_png_artifact() {
        let bgra = vec![
            0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255,
        ];
        let records = [
            talos_protocol::build_display_frame_begin(42, 2, 2),
            talos_protocol::build_display_keyframe(42, 2, 2, bgra.len() as u32, &bgra),
            talos_protocol::build_display_frame_end(42),
        ];
        let mut assembler = ScreenshotFrameAssembler::default();

        assert!(assembler.handle_payload(&records[0]).unwrap().is_none());
        let artifact = assembler
            .handle_payload(&records[1])
            .unwrap()
            .expect("screenshot artifact");

        assert_eq!(artifact.frame_id, 42);
        assert_eq!(artifact.width, 2);
        assert_eq!(artifact.height, 2);
        assert_eq!(artifact.payload_bytes, bgra.len());
        let png = BASE64_STANDARD
            .decode(artifact.base64_content.as_bytes())
            .expect("decode png");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(artifact.png_bytes, png.len());
    }

    #[test]
    fn screenshot_artifact_rejects_bad_lengths() {
        let error = build_screenshot_artifact(1, 2, 2, 4, &[0, 0, 0, 255])
            .expect_err("short payload should fail");
        assert!(error
            .to_string()
            .contains("screenshot keyframe payload length mismatch"));
    }

    #[test]
    fn relay_payload_filter_skips_hello_heartbeat_and_rmmd_metadata() {
        let metadata = {
            let json = br#"{"display_stream":{"mode":"screenshot_only"}}"#;
            let mut payload = Vec::new();
            payload.extend_from_slice(b"RMMD");
            payload.extend_from_slice(&(json.len() as u32).to_le_bytes());
            payload.extend_from_slice(json);
            payload
        };

        assert!(should_ignore_relay_payload(b"hello-world"));
        assert!(should_ignore_relay_payload(HEARTBEAT_PAYLOAD));
        assert!(should_ignore_relay_payload(&metadata));
        assert!(!should_ignore_relay_payload(
            &talos_protocol::build_display_frame_end(1)
        ));
    }

    #[test]
    fn control_mouse_move_normalizes_coordinates() {
        let frame = build_mouse_move_frame(50, 25, 100, 50).expect("mouse move frame");
        let parsed = talos_protocol::parse_control_frame(&frame).expect("parse control");

        assert_eq!(parsed.message_type, CONTROL_TYPE_MOUSE_MOVE);
        assert_eq!(parsed.payload.len(), CONTROL_PAYLOAD_MOUSE_MOVE_LEN);
        let nx = u32::from_be_bytes([
            parsed.payload[0],
            parsed.payload[1],
            parsed.payload[2],
            parsed.payload[3],
        ]);
        let ny = u32::from_be_bytes([
            parsed.payload[4],
            parsed.payload[5],
            parsed.payload[6],
            parsed.payload[7],
        ]);
        assert_eq!(nx, 32768);
        assert_eq!(ny, 32768);
    }

    #[test]
    fn control_button_and_wheel_payloads_match_viewer_shape() {
        let button =
            build_mouse_button_frame("right", true, 100, 50, 200, 100).expect("button frame");
        let parsed_button =
            talos_protocol::parse_control_frame(&button).expect("parse button frame");
        assert_eq!(parsed_button.message_type, CONTROL_TYPE_MOUSE_BUTTON);
        assert_eq!(parsed_button.payload[0], 1);
        assert_eq!(parsed_button.payload[1], 1);

        let double_click =
            build_mouse_double_click_frame("left", 100, 50, 200, 100).expect("double-click frame");
        let parsed_double_click =
            talos_protocol::parse_control_frame(&double_click).expect("parse double-click frame");
        assert_eq!(
            parsed_double_click.message_type,
            CONTROL_TYPE_MOUSE_DOUBLE_CLICK
        );
        assert_eq!(
            parsed_double_click.payload.len(),
            CONTROL_PAYLOAD_MOUSE_DOUBLE_CLICK_LEN
        );
        assert_eq!(parsed_double_click.payload[0], 0);

        let wheel = build_mouse_wheel_frame(100, 50, -120, 200, 100).expect("wheel frame");
        let parsed_wheel = talos_protocol::parse_control_frame(&wheel).expect("parse wheel frame");
        assert_eq!(parsed_wheel.message_type, CONTROL_TYPE_MOUSE_WHEEL);
        assert_eq!(
            i16::from_be_bytes([parsed_wheel.payload[0], parsed_wheel.payload[1]]),
            -120
        );
    }

    #[test]
    fn control_text_key_and_wait_helpers_match_viewer_rules() {
        assert_eq!(ai_modifier_bit("command"), CONTROL_MOD_WIN);
        assert_eq!(ai_modifier_bit("control"), CONTROL_MOD_CTRL);
        assert_eq!(ai_key_to_vkey("Enter"), Some(0x0d));
        assert_eq!(ai_key_to_vkey("F12"), Some(0x7b));

        let typed = build_text_control_frame(CONTROL_TYPE_TYPED_INPUT, "hello").expect("typed");
        let parsed = talos_protocol::parse_control_frame(&typed).expect("parse typed");
        assert_eq!(parsed.message_type, CONTROL_TYPE_TYPED_INPUT);
        assert_eq!(
            u16::from_be_bytes([parsed.payload[0], parsed.payload[1]]),
            5
        );
        assert_eq!(&parsed.payload[2..], b"hello");

        let actions = vec![AiDesktopAction::Wait { ms: 99_999 }];
        assert_eq!(ai_assist_immediate_action_len(&actions), 0);
        assert_eq!(
            ai_assist_post_action_settle_ms(&actions),
            AI_ASSIST_MAX_SETTLE_MS
        );
    }

    #[test]
    fn pointer_state_tracks_pointer_actions_and_clamps_to_frame() {
        let mut pointer = PointerState::default();
        assert!(!pointer.visible);

        pointer.apply_action(
            &AiDesktopAction::Move {
                x: 50,
                y: 25,
                keys: vec![],
            },
            100,
            50,
        );
        assert_eq!(
            pointer,
            PointerState {
                visible: true,
                x: 50,
                y: 25,
            }
        );

        pointer.apply_action(
            &AiDesktopAction::Drag {
                button: "left".to_string(),
                path: vec![
                    AiDesktopPoint { x: 5, y: 5 },
                    AiDesktopPoint { x: 140, y: 80 },
                ],
                keys: vec![],
            },
            100,
            50,
        );
        assert_eq!(
            pointer,
            PointerState {
                visible: true,
                x: 99,
                y: 49,
            }
        );
    }

    #[test]
    fn pointer_state_preserves_position_for_keyboard_and_wait_actions() {
        let mut pointer = PointerState {
            visible: true,
            x: 12,
            y: 34,
        };
        pointer.apply_action(
            &AiDesktopAction::Type {
                text: "hello".to_string(),
            },
            100,
            50,
        );
        pointer.apply_action(
            &AiDesktopAction::Keypress {
                keys: vec!["Escape".to_string()],
            },
            100,
            50,
        );
        pointer.apply_action(&AiDesktopAction::Wait { ms: 500 }, 100, 50);

        assert_eq!(
            pointer.metadata(100, 50),
            json!({
                "visible": true,
                "x": 12,
                "y": 34,
                "width": 100,
                "height": 50,
            })
        );
    }

    #[test]
    fn desktop_action_narration_uses_human_readable_actions() {
        let actions = vec![
            AiDesktopAction::Click {
                x: 12,
                y: 8,
                button: "left".to_string(),
                keys: vec![],
            },
            AiDesktopAction::Keypress {
                keys: vec!["Escape".to_string()],
            },
        ];

        let narration = desktop_action_batch_narration(&actions, 100, 80);

        assert_eq!(
            narration,
            "Click in the upper-left of the screen then press Escape"
        );
        assert!(!narration.contains("12"));
        assert!(!narration.contains("8"));
    }

    #[test]
    fn desktop_action_narration_describes_drag_scroll_type_and_wait() {
        let actions = vec![
            AiDesktopAction::Drag {
                button: "left".to_string(),
                path: vec![
                    AiDesktopPoint { x: 10, y: 10 },
                    AiDesktopPoint { x: 90, y: 70 },
                ],
                keys: vec!["Shift".to_string()],
            },
            AiDesktopAction::Scroll {
                x: 50,
                y: 40,
                scroll_x: 0,
                scroll_y: -240,
                keys: vec![],
            },
            AiDesktopAction::Type {
                text: "hello\nworld".to_string(),
            },
            AiDesktopAction::Wait { ms: 500 },
        ];

        let narration = desktop_action_batch_narration(&actions, 100, 80);

        assert!(narration.contains("Drag from the upper-left of the screen to the lower-right of the screen while holding Shift"));
        assert!(narration.contains("scroll up near the center of the screen"));
        assert!(narration.contains("type \"hello world\""));
        assert!(!narration.contains("10"));
        assert!(!narration.contains("90"));
    }

    #[test]
    fn live_frame_message_marks_unchanged_screen() {
        let response = AiDesktopTaskStepResponse {
            task_id: "task-1".to_string(),
            status: "running".to_string(),
            plan: vec![],
            assistant_message: "Waiting for the installer window.".to_string(),
            actions: vec![],
            response_id: None,
            step_index: 3,
            max_steps: 12,
            generated_secrets: vec![],
        };

        let message = live_frame_action_message(&response, "", true, 10);

        assert!(message.contains("Waiting for the installer window."));
        assert!(message.contains("No newer desktop frame arrived within 10 seconds"));
        assert!(message.contains("choose the next action"));
        assert_eq!(
            live_frame_action_message(&response, "Click in the center", false, 10),
            "Click in the center"
        );
    }

    #[test]
    fn ivf_header_parser_reads_dimensions_and_fps() {
        let mut header = [0u8; 32];
        header[0..4].copy_from_slice(b"DKIF");
        header[12..14].copy_from_slice(&1920u16.to_le_bytes());
        header[14..16].copy_from_slice(&1080u16.to_le_bytes());
        header[16..20].copy_from_slice(&30u32.to_le_bytes());
        header[20..24].copy_from_slice(&1u32.to_le_bytes());

        assert_eq!(parse_ivf_header(&header), Some((1920, 1080, 30)));
    }

    #[test]
    fn extracts_percent_encoded_query_params() {
        let token =
            extract_query_param("rmm://connect?session=s&token=a%2Bb%20c", "token").expect("token");
        assert_eq!(token, "a+b c");
    }
}
